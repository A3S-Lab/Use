use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_use_core::{
    PlanPackageRole, PlannedPackageState, PlannedPackageTransition, PluginCatalogRecord,
    PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult, VerifiedPluginCatalogRecord,
};
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::digest::package_sha256;
use super::package::{
    copy_package, io_error, lock_is_contended, owned_package_path, read_manifest, sha256,
    unique_suffix, unix_timestamp, validate_surface_files, write_receipt, RegistryLock,
};
use super::registry_io::{read_registry_snapshot, write_registry_snapshot};
use super::remote::{prepare_remote_package, ResolvedRemotePackage, TrustedRegistry};
use super::route_lock::{acquire_drain_lock, deadline_after, open_route_lock};
use super::source::prepare_package_source;
use super::{ExtensionManifest, ExtensionPaths, McpTransport};

mod lifecycle;

pub use lifecycle::{
    ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
    ExtensionLifecycleResult, ExtensionLifecycleRollbackResult,
};

const RECEIPT_SCHEMA_VERSION_V1: u32 = 1;
const RECEIPT_SCHEMA_VERSION_V2: u32 = 2;
const RECEIPT_SCHEMA_VERSION_V3: u32 = 3;
pub(super) const REGISTRY_SCHEMA_VERSION: u32 = 1;
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const WATCH_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionTrust {
    LocalExplicit,
    ReleaseBundle,
    RegistryTuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionReceipt {
    pub schema_version: u32,
    pub package_id: String,
    pub component_id: String,
    pub route: String,
    pub version: String,
    pub package_root: PathBuf,
    pub manifest_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_sha256: Option<String>,
    pub trust: ExtensionTrust,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<ResolvedRemotePackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_catalog: Option<VerifiedPluginCatalogRecord>,
    pub installed_at_unix: u64,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_generation: Option<u64>,
}

impl ExtensionReceipt {
    /// Canonical identity of the complete installed ownership and provenance
    /// record. Secret values are not part of extension receipts.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!("Failed to encode the canonical extension receipt: {error}"),
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtension {
    pub receipt: ExtensionReceipt,
    pub manifest: ExtensionManifest,
}

impl InstalledExtension {
    pub fn surfaces(&self) -> Vec<&'static str> {
        self.manifest.surface_kinds()
    }

    pub fn cli_executable(&self) -> Option<PathBuf> {
        self.manifest
            .cli
            .as_ref()
            .map(|surface| self.receipt.package_root.join(&surface.executable))
    }

    pub fn mcp_executable(&self) -> Option<PathBuf> {
        self.manifest
            .mcp
            .as_ref()
            .map(|surface| self.receipt.package_root.join(&surface.executable))
    }

    pub fn mcp_args(&self) -> Option<&[String]> {
        self.manifest
            .mcp
            .as_ref()
            .map(|surface| surface.args.as_slice())
    }

    pub fn mcp_transport(&self) -> Option<McpTransport> {
        self.manifest.mcp.as_ref().map(|surface| surface.transport)
    }

    pub fn skill_path(&self) -> Option<PathBuf> {
        self.manifest
            .skill
            .as_ref()
            .map(|surface| self.receipt.package_root.join(&surface.path))
    }

    pub fn enabled(&self) -> bool {
        self.receipt.enabled
    }

    pub fn supports_use_version(&self, version: &str) -> bool {
        self.manifest.supports_use_version(version).unwrap_or(false)
    }

    /// Return the verified package-planning evidence retained by this
    /// installed package after checking its internal receipt bindings.
    pub fn plan_ready_catalog(&self) -> UseResult<&VerifiedPluginCatalogRecord> {
        let catalog = self.receipt.verified_catalog.as_ref().ok_or_else(|| {
            plan_evidence_error(
                "The installed extension does not retain verified package-planning evidence.",
            )
        })?;
        if !matches!(
            self.receipt.schema_version,
            RECEIPT_SCHEMA_VERSION_V2 | RECEIPT_SCHEMA_VERSION_V3
        ) || self.receipt.trust != ExtensionTrust::RegistryTuf
        {
            return Err(plan_evidence_error(
                "The installed extension receipt is not plan-ready registry state.",
            ));
        }
        let package_digest = self.receipt.package_sha256.as_deref().ok_or_else(|| {
            plan_evidence_error("The installed extension receipt omitted its package digest.")
        })?;
        validate_catalog_binding(
            catalog,
            self.receipt.registry.as_ref(),
            &self.manifest,
            &self.receipt.manifest_sha256,
            package_digest,
        )?;
        Ok(catalog)
    }

    /// Resolve the exact installed package state using active surfaces
    /// observed by the capability snapshot.
    pub fn planned_state(
        &self,
        active_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageState> {
        self.plan_ready_catalog()?.selected_state(active_surfaces)
    }

    pub fn remove_transition(
        &self,
        role: PlanPackageRole,
        active_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        self.plan_ready_catalog()?
            .remove_transition(role, active_surfaces)
    }

    pub fn replace_transition(
        &self,
        candidate: &VerifiedPluginCatalogRecord,
        role: PlanPackageRole,
        active_surfaces: &[PluginSurfaceRef],
        requested_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        candidate.replace_transition(
            self.plan_ready_catalog()?,
            role,
            active_surfaces,
            requested_surfaces,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRouteBinding {
    pub package_id: String,
    pub component_id: String,
    pub route: String,
    pub version: String,
    #[serde(default)]
    pub package_root: PathBuf,
    pub manifest_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_generation: Option<u64>,
    pub enabled: bool,
    pub surfaces: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRegistrySnapshot {
    pub schema_version: u32,
    pub generation: u64,
    pub routes: Vec<ExtensionRouteBinding>,
}

impl Default for ExtensionRegistrySnapshot {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation: 0,
            routes: Vec::new(),
        }
    }
}

impl ExtensionRegistrySnapshot {
    /// Canonical digest of the exact capability projection selected by one
    /// Registry generation.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(UseError::new(
                "use.extension.registry_incompatible",
                format!(
                    "Extension registry schema {} is not supported.",
                    self.schema_version
                ),
            ));
        }
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            UseError::new(
                "use.extension.registry_invalid",
                format!("Failed to encode the canonical extension Registry snapshot: {error}"),
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationResult {
    pub package_id: String,
    pub changed: bool,
    pub enabled: bool,
    pub generation: u64,
}

pub struct ExtensionRouteLease {
    extension: InstalledExtension,
    file: File,
}

impl ExtensionRouteLease {
    pub fn extension(&self) -> &InstalledExtension {
        &self.extension
    }
}

impl Drop for ExtensionRouteLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub force: bool,
    pub allow_unsigned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub changed: bool,
    pub extension: InstalledExtension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub package_id: String,
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct ExtensionRegistry {
    paths: ExtensionPaths,
}

impl ExtensionRegistry {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(ExtensionPaths::from_env()?))
    }

    pub fn new(paths: ExtensionPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &ExtensionPaths {
        &self.paths
    }

    /// Return the immutable route projection currently visible to consumers.
    ///
    /// The published projection is compared with ownership-validated receipts
    /// without blocking lifecycle writers. A mismatch is rebuilt under the
    /// registry lock, repairing a crash between receipt activation and
    /// generation publication without requiring a resident daemon.
    pub async fn snapshot(&self) -> UseResult<ExtensionRegistrySnapshot> {
        // The common read path is lock-free with respect to lifecycle writers.
        // Only a real receipt/publication mismatch needs the registry lock for
        // crash reconciliation.
        let path = self.paths.registry_snapshot_path();
        let published = read_registry_snapshot(&path).await?;
        match self.list().await {
            Ok(installed) if published.routes == route_bindings(&installed) => {
                return Ok(published)
            }
            // A lifecycle writer may remove a receipt between the optimistic
            // directory scan and receipt read. Re-check under the lock below;
            // if that writer still owns it, the last complete publication is
            // the only coherent snapshot to return.
            Ok(_) | Err(_) => {}
        }
        let _lock = match RegistryLock::acquire(&self.paths.registry_lock_path()) {
            Ok(lock) => lock,
            Err(error) if error.code == "use.extension.busy" => {
                return read_registry_snapshot(&path).await;
            }
            Err(error) => return Err(error),
        };
        let installed = self.list().await?;
        let published = read_registry_snapshot(&path).await?;
        let routes = route_bindings(&installed);
        if published.routes == routes {
            return Ok(published);
        }
        // Receipt writes belonging to a schema-v3 graph publication are not a
        // multi-file transaction. The immutable snapshot is therefore the
        // visibility commit point. Never infer a new active generation from
        // partially written receipts after a crash; the durable lifecycle
        // journal must replay the exact reviewed batch instead.
        if active_lifecycle_bindings(&published.routes) != active_lifecycle_bindings(&routes) {
            return Ok(published);
        }
        self.publish_snapshot_locked(&installed).await
    }

    /// Wait until a newer registry generation is published.
    ///
    /// Consumers such as A3S Code can keep their process alive and refresh CLI,
    /// MCP, and Skill surfaces when this returns a snapshot.
    pub async fn wait_for_change(
        &self,
        after_generation: u64,
        timeout: Duration,
    ) -> UseResult<Option<ExtensionRegistrySnapshot>> {
        // Reconcile once when the subscription starts. Polling after this
        // point reads only immutable publications so watchers never become a
        // periodic source of write-lock contention for lifecycle operations.
        // Start the caller's wait budget only after this one-time subscription
        // setup so filesystem scheduling cannot consume the entire timeout
        // before the watcher begins polling.
        let initial = self.snapshot().await?;
        if initial.generation > after_generation {
            return Ok(Some(initial));
        }
        let deadline = deadline_after(timeout)?;
        loop {
            // Lifecycle mutations publish the immutable projection before
            // draining old calls. Reading it directly keeps watchers live even
            // while the mutation deliberately holds the registry write lock.
            let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
            if published.generation > after_generation {
                return Ok(Some(published));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(WATCH_INTERVAL.min(deadline.saturating_duration_since(now))).await;
        }
    }

    pub async fn list(&self) -> UseResult<Vec<InstalledExtension>> {
        let root = self.paths.receipts_root();
        let mut publishers = match fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error("read extension receipts", &root, error)),
        };
        let mut receipt_paths = Vec::new();
        while let Some(publisher) = publishers
            .next_entry()
            .await
            .map_err(|error| io_error("read extension receipt directory", &root, error))?
        {
            let metadata = publisher
                .file_type()
                .await
                .map_err(|error| io_error("inspect receipt publisher", &publisher.path(), error))?;
            if !metadata.is_dir() || metadata.is_symlink() {
                continue;
            }
            let mut entries = fs::read_dir(publisher.path())
                .await
                .map_err(|error| io_error("read publisher receipts", &publisher.path(), error))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| io_error("read publisher receipt", &publisher.path(), error))?
            {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) == Some("json") {
                    receipt_paths.push(path);
                }
            }
        }
        receipt_paths.sort();
        let mut installed = Vec::with_capacity(receipt_paths.len());
        for path in receipt_paths {
            installed.push(self.load_receipt(&path).await?);
        }
        installed.sort_by(|left, right| left.receipt.package_id.cmp(&right.receipt.package_id));
        ensure_unique_routes(&installed)?;
        Ok(installed)
    }

    pub async fn get(&self, package_id: &str) -> UseResult<Option<InstalledExtension>> {
        let package_id = normalize_package_id(package_id)?;
        let path = self.paths.receipt_path(&package_id);
        match fs::metadata(&path).await {
            Ok(_) => self.load_receipt(&path).await.map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("inspect extension receipt", &path, error)),
        }
    }

    /// Resolve the exact receipt selected by one immutable Registry snapshot
    /// binding. During blue-green preparation this may be a retained prior
    /// generation rather than the primary candidate receipt.
    pub async fn get_snapshot_binding(
        &self,
        binding: &ExtensionRouteBinding,
    ) -> UseResult<Option<InstalledExtension>> {
        let extension = if let Some(generation) = binding.lifecycle_generation {
            let package_sha256 = binding.package_sha256.as_deref().ok_or_else(|| {
                UseError::new(
                    "use.extension.lifecycle_binding_invalid",
                    "A lifecycle snapshot binding omitted its package digest.",
                )
            })?;
            let identity = ExtensionLifecycleIdentity::new(
                &binding.package_id,
                format!("sha256:{package_sha256}"),
                format!("sha256:{}", binding.manifest_sha256),
                generation,
            )?;
            self.get_lifecycle_generation(&identity).await?
        } else {
            self.get(&binding.package_id).await?
        };
        Ok(extension.filter(|extension| published_binding_matches_extension(binding, extension)))
    }

    /// Return installed packages whose admitted manifests directly require
    /// `package_id`. The sorted result is suitable for uninstall review and
    /// is recomputed from authoritative receipts instead of a mutable index.
    pub async fn dependent_packages(&self, package_id: &str) -> UseResult<Vec<String>> {
        let package_id = normalize_package_id(package_id)?;
        let installed = self.list().await?;
        Ok(installed_dependents(&installed, &package_id))
    }

    pub async fn find_route(&self, route: &str) -> UseResult<Option<InstalledExtension>> {
        self.find_route_for_host_version(route, env!("CARGO_PKG_VERSION"))
            .await
    }

    async fn find_route_for_host_version(
        &self,
        route: &str,
        host_version: &str,
    ) -> UseResult<Option<InstalledExtension>> {
        let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        let Some(binding) = published
            .routes
            .iter()
            .find(|binding| binding.enabled && binding.route == route)
        else {
            return Ok(None);
        };
        let Some(extension) = self.get_snapshot_binding(binding).await? else {
            return Ok(None);
        };
        Ok((extension.receipt.enabled
            && extension.supports_use_version(host_version)
            && published_binding_matches_extension(binding, &extension))
        .then_some(extension))
    }

    /// Pin an active route generation for the lifetime of one delegated call.
    /// Disable and uninstall operations acquire the matching exclusive lock
    /// before deleting package files, so an accepted invocation cannot lose its
    /// executable halfway through dispatch.
    pub async fn acquire_route(&self, route: &str) -> UseResult<Option<ExtensionRouteLease>> {
        let Some(candidate) = self.find_route(route).await? else {
            return Ok(None);
        };
        self.acquire_extension_lease(candidate, Some(route)).await
    }

    pub async fn acquire_extension(
        &self,
        package_id: &str,
    ) -> UseResult<Option<ExtensionRouteLease>> {
        let package_id = normalize_package_id(package_id)?;
        let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        let Some(binding) = published
            .routes
            .iter()
            .find(|binding| binding.enabled && binding.package_id == package_id)
        else {
            return Ok(None);
        };
        let Some(candidate) = self.get_snapshot_binding(binding).await? else {
            return Ok(None);
        };
        if !candidate.receipt.enabled || !candidate.supports_use_version(env!("CARGO_PKG_VERSION"))
        {
            return Ok(None);
        }
        self.acquire_extension_lease(candidate, None).await
    }

    pub async fn install_local(
        &self,
        expected_package_id: &str,
        source: &Path,
        options: InstallOptions,
    ) -> UseResult<InstallResult> {
        let expected_package_id = normalize_package_id(expected_package_id)?;
        if !options.allow_unsigned {
            return Err(UseError::new(
                "use.extension.trust_required",
                "Unsigned local extensions require explicit trust approval.",
            )
            .with_suggestion("Rerun the explicit install with --allow-unsigned."));
        }

        let source = prepare_package_source(source).await?;
        self.install_prepared(
            &expected_package_id,
            source.root(),
            options.force,
            ExtensionTrust::LocalExplicit,
            None,
            None,
        )
        .await
    }

    /// Install a package carried by the same verified A3S Use release.
    ///
    /// The caller supplies the digest shown in the reviewed umbrella plan.
    /// This method resolves and validates the release-owned directory again,
    /// refusing activation if any byte changed between review and apply.
    pub async fn install_release_bundle(
        &self,
        expected_package_id: &str,
        source: &Path,
        expected_package_sha256: &str,
        force: bool,
    ) -> UseResult<InstallResult> {
        let expected_package_id = normalize_package_id(expected_package_id)?;
        let bundle = super::inspect_release_bundle(source).await?;
        if bundle.package_id != expected_package_id {
            return Err(UseError::new(
                "use.extension.identity_mismatch",
                format!(
                    "Requested extension '{}' but the release bundle declares '{}'.",
                    expected_package_id, bundle.package_id
                ),
            ));
        }
        if bundle.package_sha256 != expected_package_sha256 {
            return Err(UseError::new(
                "use.extension.release_bundle_changed",
                format!(
                    "Release bundle '{}' changed after its installation plan was reviewed.",
                    expected_package_id
                ),
            ));
        }
        self.install_prepared(
            &expected_package_id,
            source,
            force,
            ExtensionTrust::ReleaseBundle,
            None,
            None,
        )
        .await
    }

    /// Install an extension selected through a fully verified TUF repository.
    ///
    /// Metadata is resolved and the optional reviewed plan is checked before
    /// the target payload is downloaded. The package manifest must repeat the
    /// exact ID and version carried by the signed target metadata.
    pub async fn install_remote(
        &self,
        expected_package_id: &str,
        registry: &TrustedRegistry,
        requested_version: Option<&str>,
        channel: &str,
        expected_plan_digest: Option<&str>,
        force: bool,
    ) -> UseResult<InstallResult> {
        let expected_package_id = normalize_package_id(expected_package_id)?;
        let prepared = prepare_remote_package(
            registry,
            &expected_package_id,
            requested_version,
            channel,
            expected_plan_digest,
        )
        .await?;
        if !force {
            if let Some(result) = self
                .converged_remote_install(
                    &expected_package_id,
                    prepared.resolved(),
                    prepared
                        .verified_catalog()
                        .filter(|catalog| catalog.record.is_package_plan_ready()),
                )
                .await?
            {
                return Ok(result);
            }
        }
        let downloaded = prepared.download().await?;
        let provenance = downloaded.resolved().clone();
        let verified_catalog = downloaded
            .verified_catalog()
            .filter(|catalog| catalog.record.is_package_plan_ready())
            .cloned();
        let source = prepare_package_source(downloaded.path()).await?;
        self.install_prepared(
            &expected_package_id,
            source.root(),
            force,
            ExtensionTrust::RegistryTuf,
            Some(provenance),
            verified_catalog,
        )
        .await
    }

    async fn converged_remote_install(
        &self,
        expected_package_id: &str,
        resolved: &ResolvedRemotePackage,
        verified_catalog: Option<&VerifiedPluginCatalogRecord>,
    ) -> UseResult<Option<InstallResult>> {
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let Some(current) = self.get(expected_package_id).await? else {
            return Ok(None);
        };
        let same_target = current.receipt.trust == ExtensionTrust::RegistryTuf
            && current.receipt.version == resolved.version
            && registry_identity(current.receipt.registry.as_ref())
                == registry_identity(Some(resolved))
            && current.receipt.verified_catalog.as_ref() == verified_catalog;
        if !same_target {
            return Ok(None);
        }
        let installed = self.list().await?;
        self.publish_snapshot_locked(&installed).await?;
        Ok(Some(InstallResult {
            changed: false,
            extension: current,
        }))
    }

    async fn install_prepared(
        &self,
        expected_package_id: &str,
        source: &Path,
        force: bool,
        trust: ExtensionTrust,
        registry: Option<ResolvedRemotePackage>,
        verified_catalog: Option<VerifiedPluginCatalogRecord>,
    ) -> UseResult<InstallResult> {
        match (trust, registry.as_ref(), verified_catalog.as_ref()) {
            (ExtensionTrust::LocalExplicit | ExtensionTrust::ReleaseBundle, None, None)
            | (ExtensionTrust::RegistryTuf, Some(_), None)
            | (ExtensionTrust::RegistryTuf, Some(_), Some(_)) => {}
            _ => {
                return Err(UseError::new(
                    "use.extension.trust_invalid",
                    "Extension installation provenance is internally inconsistent.",
                ))
            }
        }

        let (manifest, manifest_bytes) = read_manifest(source).await?;
        if manifest.package_id != expected_package_id {
            return Err(UseError::new(
                "use.extension.identity_mismatch",
                format!(
                    "Requested extension '{}' but the package declares '{}'.",
                    expected_package_id, manifest.package_id
                ),
            ));
        }
        if manifest.schema_version == 3 {
            return Err(UseError::new(
                "use.extension.lifecycle_required",
                "Schema-v3 cognitive packages must be installed through the package lifecycle coordinator.",
            )
            .with_suggestion(
                "Use the cognitive-package install or apply flow so every declared surface is committed atomically.",
            ));
        }
        if !manifest.supports_use_version(env!("CARGO_PKG_VERSION"))? {
            return Err(UseError::new(
                "use.extension.host_incompatible",
                format!(
                    "Extension '{}' {} does not support A3S Use {}.",
                    manifest.package_id,
                    manifest.version,
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .with_detail("requiresUse", manifest.requires_use.clone())
            .with_detail("hostVersion", env!("CARGO_PKG_VERSION")));
        }
        if let Some(registry) = &registry {
            if registry.package_id != manifest.package_id || registry.version != manifest.version {
                return Err(UseError::new(
                    "use.extension.registry_identity_mismatch",
                    format!(
                        "Signed target '{}@{}' does not match package manifest '{}@{}'.",
                        registry.package_id,
                        registry.version,
                        manifest.package_id,
                        manifest.version
                    ),
                ));
            }
        }
        validate_surface_files(&manifest, source).await?;
        let package_digest = package_sha256(source).await?;
        validate_catalog_package(
            verified_catalog.as_ref(),
            registry.as_ref(),
            &manifest,
            &manifest_bytes,
            &package_digest,
        )?;

        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let installed = self.list().await?;
        if let Some(conflict) = installed.iter().find(|extension| {
            extension.receipt.package_id != expected_package_id
                && extension.receipt.route == manifest.route
        }) {
            return Err(UseError::new(
                "use.extension.route_conflict",
                format!(
                    "Route '{}' is already owned by extension '{}'.",
                    manifest.route, conflict.receipt.package_id
                ),
            ));
        }

        let digest = sha256(&manifest_bytes);
        if let Some(current) = installed
            .iter()
            .find(|extension| extension.receipt.package_id == expected_package_id)
        {
            let current_package_digest = match &current.receipt.package_sha256 {
                Some(digest) => digest.clone(),
                None => package_sha256(&current.receipt.package_root).await?,
            };
            let same_provenance = current.receipt.trust == trust
                && registry_identity(current.receipt.registry.as_ref())
                    == registry_identity(registry.as_ref())
                && current.receipt.verified_catalog.as_ref() == verified_catalog.as_ref();
            if !force
                && current.receipt.version == manifest.version
                && current_package_digest == package_digest
                && same_provenance
            {
                self.publish_snapshot_locked(&installed).await?;
                return Ok(InstallResult {
                    changed: false,
                    extension: current.clone(),
                });
            }
            if !force
                && current.receipt.version == manifest.version
                && current_package_digest != package_digest
            {
                return Err(UseError::new(
                    "use.extension.version_conflict",
                    format!(
                        "Extension '{}' version {} is already active with different content.",
                        expected_package_id, manifest.version
                    ),
                )
                .with_suggestion("Use a new version or rerun the explicit install with --force."));
            }
        }

        let package_parent = self.paths.package_parent(expected_package_id);
        fs::create_dir_all(&package_parent).await.map_err(|error| {
            io_error("create extension package directory", &package_parent, error)
        })?;
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&package_parent)
            .map_err(|error| {
                io_error("create extension staging directory", &package_parent, error)
            })?;
        copy_package(source, staging.path()).await?;
        let (staged_manifest, staged_bytes) = read_manifest(staging.path()).await?;
        if staged_manifest != manifest || sha256(&staged_bytes) != digest {
            return Err(UseError::new(
                "use.extension.package_changed",
                "The extension manifest changed while the package was staged.",
            ));
        }
        validate_surface_files(&staged_manifest, staging.path()).await?;
        if package_sha256(staging.path()).await? != package_digest {
            return Err(UseError::new(
                "use.extension.package_changed",
                "The extension package changed while it was staged.",
            ));
        }

        let activation = unique_suffix();
        let target = self
            .paths
            .package_root(expected_package_id, &manifest.version, &activation);
        let staging = staging.keep();
        if let Err(error) = fs::rename(&staging, &target).await {
            let _ = fs::remove_dir_all(&staging).await;
            return Err(io_error("activate extension package", &target, error));
        }

        let enabled = installed
            .iter()
            .find(|extension| extension.receipt.package_id == expected_package_id)
            .map(|extension| extension.receipt.enabled)
            .unwrap_or(true);

        let receipt = ExtensionReceipt {
            schema_version: if verified_catalog.is_some() {
                RECEIPT_SCHEMA_VERSION_V2
            } else {
                RECEIPT_SCHEMA_VERSION_V1
            },
            package_id: expected_package_id.to_string(),
            component_id: format!("use/{expected_package_id}"),
            route: manifest.route.clone(),
            version: manifest.version.clone(),
            package_root: target.clone(),
            manifest_sha256: digest,
            package_sha256: Some(package_digest),
            trust,
            registry,
            verified_catalog,
            installed_at_unix: unix_timestamp(),
            enabled,
            lifecycle_generation: None,
        };
        let receipt_path = self.paths.receipt_path(expected_package_id);
        if let Err(error) = write_receipt(&receipt_path, &receipt).await {
            let _ = fs::remove_dir_all(&target).await;
            return Err(error);
        }

        // Previous immutable package generations remain available while calls
        // that pinned them drain. Explicit uninstall and the future package GC
        // are the only operations allowed to remove these directories.
        let current = self.list().await?;
        self.publish_snapshot_locked(&current).await?;

        Ok(InstallResult {
            changed: true,
            extension: InstalledExtension { receipt, manifest },
        })
    }

    pub async fn enable(&self, package_id: &str) -> UseResult<ActivationResult> {
        let package_id = normalize_package_id(package_id)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let extension = self.get(&package_id).await?.ok_or_else(|| {
            UseError::new(
                "use.extension.not_installed",
                format!("Extension '{package_id}' is not installed."),
            )
        })?;
        reject_lifecycle_managed(&extension.receipt)?;
        let changed = !extension.receipt.enabled;
        if changed {
            let mut receipt = extension.receipt;
            receipt.enabled = true;
            write_receipt(&self.paths.receipt_path(&package_id), &receipt).await?;
        }
        let installed = self.list().await?;
        let snapshot = self.publish_snapshot_locked(&installed).await?;
        Ok(ActivationResult {
            package_id,
            changed,
            enabled: true,
            generation: snapshot.generation,
        })
    }

    pub async fn disable(&self, package_id: &str) -> UseResult<ActivationResult> {
        self.disable_with_timeout(package_id, DEFAULT_DRAIN_TIMEOUT)
            .await
    }

    pub async fn disable_with_timeout(
        &self,
        package_id: &str,
        timeout: Duration,
    ) -> UseResult<ActivationResult> {
        deadline_after(timeout)?;
        let package_id = normalize_package_id(package_id)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let extension = self.get(&package_id).await?.ok_or_else(|| {
            UseError::new(
                "use.extension.not_installed",
                format!("Extension '{package_id}' is not installed."),
            )
        })?;
        reject_lifecycle_managed(&extension.receipt)?;
        let changed = extension.receipt.enabled;
        if changed {
            let mut receipt = extension.receipt;
            receipt.enabled = false;
            write_receipt(&self.paths.receipt_path(&package_id), &receipt).await?;
        }
        let installed = self.list().await?;
        let snapshot = self.publish_snapshot_locked(&installed).await?;
        // Route visibility changes before draining. New calls fail closed while
        // accepted calls retain their shared generation lease. Keep the
        // registry lock for the drain so a concurrent enable cannot republish
        // the route before all accepted calls have released their leases.
        let _drain =
            acquire_drain_lock(&self.paths.package_lock_path(&package_id), timeout).await?;
        Ok(ActivationResult {
            package_id,
            changed,
            enabled: false,
            generation: snapshot.generation,
        })
    }

    pub async fn uninstall(&self, package_id: &str) -> UseResult<UninstallResult> {
        let package_id = normalize_package_id(package_id)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let Some(extension) = self.get(&package_id).await? else {
            // A previous uninstall may have committed receipt removal and then
            // stopped before deleting its immutable package generations. The
            // missing receipt already makes the route invisible; reconcile the
            // projection and finish the owned cleanup on retry.
            let installed = self.list().await?;
            self.publish_snapshot_locked(&installed).await?;
            let package_parent = self.paths.package_parent(&package_id);
            reject_lifecycle_orphan_cleanup(&package_parent).await?;
            let changed = remove_package_parent_if_present(&package_parent).await?;
            return Ok(UninstallResult {
                package_id,
                changed,
            });
        };
        reject_lifecycle_managed(&extension.receipt)?;
        let installed = self.list().await?;
        ensure_no_installed_dependents(&installed, &package_id)?;
        if extension.receipt.enabled {
            let mut receipt = extension.receipt.clone();
            receipt.enabled = false;
            write_receipt(&self.paths.receipt_path(&package_id), &receipt).await?;
            let installed = self.list().await?;
            self.publish_snapshot_locked(&installed).await?;
        }

        // Keep both locks until the receipt and every immutable package
        // generation are gone. An enable or install cannot interleave between
        // route removal and package deletion.
        let _drain = acquire_drain_lock(
            &self.paths.package_lock_path(&package_id),
            DEFAULT_DRAIN_TIMEOUT,
        )
        .await?;
        if !owned_package_path(&self.paths, &package_id, &extension.receipt.package_root) {
            return Err(UseError::new(
                "use.extension.ownership_invalid",
                "The extension receipt does not own its package directory.",
            )
            .with_detail("routeDisabled", true));
        }
        let receipt_path = self.paths.receipt_path(&package_id);
        fs::remove_file(&receipt_path)
            .await
            .map_err(|error| io_error("disable extension route", &receipt_path, error))?;
        // Publish receipt removal before best-effort storage cleanup. If the
        // latter is interrupted, a retry enters the no-receipt recovery path
        // above without re-exposing the route.
        let installed = self.list().await?;
        self.publish_snapshot_locked(&installed).await?;
        let package_parent = self.paths.package_parent(&package_id);
        remove_package_parent_if_present(&package_parent).await?;
        Ok(UninstallResult {
            package_id,
            changed: true,
        })
    }

    async fn acquire_extension_lease(
        &self,
        candidate: InstalledExtension,
        expected_route: Option<&str>,
    ) -> UseResult<Option<ExtensionRouteLease>> {
        self.acquire_extension_lease_for_host_version(
            candidate,
            expected_route,
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    async fn acquire_extension_lease_for_host_version(
        &self,
        candidate: InstalledExtension,
        expected_route: Option<&str>,
        host_version: &str,
    ) -> UseResult<Option<ExtensionRouteLease>> {
        let path = lifecycle_route_lock_path(&self.paths, &candidate.receipt)?;
        let file = open_route_lock(&path)?;
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {}
            Err(error) if lock_is_contended(&error) => return Ok(None),
            Err(error) => return Err(io_error("acquire extension route lease", &path, error)),
        }

        // Re-read after locking so a concurrent disable cannot admit a call
        // using stale route metadata.
        let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        let Some(binding) = published.routes.iter().find(|binding| {
            binding.enabled && published_binding_matches_extension(binding, &candidate)
        }) else {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        };
        let Some(extension) = self.get_snapshot_binding(binding).await? else {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        };
        if !extension.receipt.enabled
            || !extension.supports_use_version(host_version)
            || expected_route.is_some_and(|route| extension.receipt.route != route)
        {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        }
        verify_package_integrity(&extension).await?;
        Ok(Some(ExtensionRouteLease { extension, file }))
    }

    async fn publish_snapshot_locked(
        &self,
        installed: &[InstalledExtension],
    ) -> UseResult<ExtensionRegistrySnapshot> {
        let routes = route_bindings(installed);
        let path = self.paths.registry_snapshot_path();
        let current = read_registry_snapshot(&path).await?;
        if current.routes == routes {
            return Ok(current);
        }
        let snapshot = ExtensionRegistrySnapshot {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation: current.generation.checked_add(1).ok_or_else(|| {
                UseError::new(
                    "use.extension.generation_exhausted",
                    "The extension registry generation is exhausted.",
                )
            })?,
            routes,
        };
        write_registry_snapshot(&path, &snapshot).await?;
        Ok(snapshot)
    }

    async fn load_receipt(&self, receipt_path: &Path) -> UseResult<InstalledExtension> {
        let bytes = fs::read(receipt_path)
            .await
            .map_err(|error| io_error("read extension receipt", receipt_path, error))?;
        let receipt: ExtensionReceipt = serde_json::from_slice(&bytes).map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!(
                    "Invalid extension receipt '{}': {error}",
                    receipt_path.display()
                ),
            )
        })?;
        if !matches!(
            receipt.schema_version,
            RECEIPT_SCHEMA_VERSION_V1 | RECEIPT_SCHEMA_VERSION_V2 | RECEIPT_SCHEMA_VERSION_V3
        ) {
            return Err(UseError::new(
                "use.extension.receipt_incompatible",
                format!(
                    "Extension receipt schema {} is not supported.",
                    receipt.schema_version
                ),
            ));
        }
        match (
            receipt.schema_version,
            receipt.verified_catalog.as_ref(),
            receipt.package_sha256.as_ref(),
        ) {
            (RECEIPT_SCHEMA_VERSION_V1, None, _)
            | (RECEIPT_SCHEMA_VERSION_V2, Some(_), Some(_))
            | (RECEIPT_SCHEMA_VERSION_V3, _, Some(_)) => {}
            _ => {
                return Err(UseError::new(
                    "use.extension.receipt_invalid",
                    format!(
                        "Extension receipt for '{}' has inconsistent catalog evidence.",
                        receipt.package_id
                    ),
                ))
            }
        }
        match (receipt.schema_version, receipt.lifecycle_generation) {
            (RECEIPT_SCHEMA_VERSION_V1 | RECEIPT_SCHEMA_VERSION_V2, None)
            | (RECEIPT_SCHEMA_VERSION_V3, Some(1..)) => {}
            _ => {
                return Err(UseError::new(
                    "use.extension.lifecycle_receipt_invalid",
                    format!(
                        "Extension receipt for '{}' has an invalid lifecycle generation.",
                        receipt.package_id
                    ),
                ))
            }
        }
        if receipt.package_sha256.as_deref().is_some_and(|digest| {
            digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                format!(
                    "Extension receipt for '{}' has an invalid package digest.",
                    receipt.package_id
                ),
            ));
        }
        match (
            receipt.trust,
            receipt.registry.as_ref(),
            receipt.verified_catalog.as_ref(),
        ) {
            (ExtensionTrust::LocalExplicit | ExtensionTrust::ReleaseBundle, None, None) => {}
            (ExtensionTrust::RegistryTuf, Some(registry), catalog) => {
                registry.validate_provenance()?;
                if registry.package_id != receipt.package_id || registry.version != receipt.version
                {
                    return Err(UseError::new(
                        "use.extension.receipt_invalid",
                        format!(
                            "Registry provenance for '{}' does not match its receipt.",
                            receipt.package_id
                        ),
                    ));
                }
                if catalog.is_some_and(|catalog| !catalog.record.is_package_plan_ready()) {
                    return Err(UseError::new(
                        "use.extension.receipt_invalid",
                        format!(
                            "Extension receipt for '{}' contains non-plan-ready catalog evidence.",
                            receipt.package_id
                        ),
                    ));
                }
            }
            _ => {
                return Err(UseError::new(
                    "use.extension.receipt_invalid",
                    format!(
                        "Extension receipt for '{}' has inconsistent trust provenance.",
                        receipt.package_id
                    ),
                ))
            }
        }
        let package_id = normalize_package_id(&receipt.package_id)?;
        if receipt.component_id != format!("use/{package_id}")
            || !owned_package_path(&self.paths, &package_id, &receipt.package_root)
        {
            return Err(UseError::new(
                "use.extension.ownership_invalid",
                format!(
                    "Receipt for '{}' has invalid ownership metadata.",
                    package_id
                ),
            ));
        }
        let (manifest, manifest_bytes) = read_manifest(&receipt.package_root).await?;
        if manifest.package_id != receipt.package_id
            || manifest.version != receipt.version
            || manifest.route != receipt.route
            || sha256(&manifest_bytes) != receipt.manifest_sha256
        {
            return Err(UseError::new(
                "use.extension.receipt_mismatch",
                format!(
                    "Installed package '{}' does not match its receipt.",
                    package_id
                ),
            ));
        }
        if receipt.schema_version == RECEIPT_SCHEMA_VERSION_V3 {
            let generation = receipt.lifecycle_generation.ok_or_else(|| {
                UseError::new(
                    "use.extension.lifecycle_receipt_invalid",
                    "A lifecycle receipt omitted its exact package generation.",
                )
            })?;
            let package_sha256 = receipt.package_sha256.as_deref().ok_or_else(|| {
                UseError::new(
                    "use.extension.lifecycle_receipt_invalid",
                    "A lifecycle receipt omitted its exact package digest.",
                )
            })?;
            if manifest.schema_version != 3
                || package_sha256.bytes().any(|byte| byte.is_ascii_uppercase())
                || receipt.package_root
                    != self
                        .paths
                        .lifecycle_package_root(&package_id, generation, package_sha256)
            {
                return Err(UseError::new(
                    "use.extension.lifecycle_receipt_invalid",
                    format!(
                        "Lifecycle receipt for '{}' does not bind its immutable generation.",
                        package_id
                    ),
                ));
            }
        }
        validate_surface_files(&manifest, &receipt.package_root).await?;
        if let Some(catalog) = receipt.verified_catalog.as_ref() {
            let package_digest = package_sha256(&receipt.package_root).await?;
            if receipt.package_sha256.as_deref() != Some(package_digest.as_str()) {
                return Err(UseError::new(
                    "use.extension.package_digest_mismatch",
                    format!(
                        "Installed package '{}' no longer matches its recorded digest.",
                        receipt.package_id
                    ),
                )
                .with_suggestion("Reinstall the extension from its trusted source."));
            }
            validate_catalog_package(
                Some(catalog),
                receipt.registry.as_ref(),
                &manifest,
                &manifest_bytes,
                &package_digest,
            )?;
        }
        Ok(InstalledExtension { receipt, manifest })
    }
}

async fn verify_package_integrity(extension: &InstalledExtension) -> UseResult<()> {
    let Some(expected) = extension.receipt.package_sha256.as_deref() else {
        return Ok(());
    };
    let actual = package_sha256(&extension.receipt.package_root).await?;
    if actual != expected {
        return Err(UseError::new(
            "use.extension.package_digest_mismatch",
            format!(
                "Installed package '{}' no longer matches its recorded digest.",
                extension.receipt.package_id
            ),
        )
        .with_suggestion("Reinstall the extension from its trusted source."));
    }
    Ok(())
}

fn route_bindings(installed: &[InstalledExtension]) -> Vec<ExtensionRouteBinding> {
    installed
        .iter()
        .map(|extension| ExtensionRouteBinding {
            package_id: extension.receipt.package_id.clone(),
            component_id: extension.receipt.component_id.clone(),
            route: extension.receipt.route.clone(),
            version: extension.receipt.version.clone(),
            package_root: extension.receipt.package_root.clone(),
            manifest_sha256: extension.receipt.manifest_sha256.clone(),
            package_sha256: extension.receipt.package_sha256.clone(),
            lifecycle_generation: extension.receipt.lifecycle_generation,
            enabled: extension.receipt.enabled,
            surfaces: extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
        .collect()
}

fn active_lifecycle_bindings(
    routes: &[ExtensionRouteBinding],
) -> BTreeMap<&str, (u64, &str, Option<&str>, &str, &str)> {
    routes
        .iter()
        .filter_map(|binding| {
            let generation = binding.lifecycle_generation?;
            binding.enabled.then_some((
                binding.package_id.as_str(),
                (
                    generation,
                    binding.manifest_sha256.as_str(),
                    binding.package_sha256.as_deref(),
                    binding.version.as_str(),
                    binding.route.as_str(),
                ),
            ))
        })
        .collect()
}

fn published_binding_matches_extension(
    binding: &ExtensionRouteBinding,
    extension: &InstalledExtension,
) -> bool {
    binding == &route_bindings(std::slice::from_ref(extension))[0]
}

fn lifecycle_route_lock_path(
    paths: &ExtensionPaths,
    receipt: &ExtensionReceipt,
) -> UseResult<PathBuf> {
    match receipt.lifecycle_generation {
        Some(generation) if receipt.schema_version == RECEIPT_SCHEMA_VERSION_V3 => {
            Ok(paths.lifecycle_package_lock_path(&receipt.package_id, generation))
        }
        None if matches!(
            receipt.schema_version,
            RECEIPT_SCHEMA_VERSION_V1 | RECEIPT_SCHEMA_VERSION_V2
        ) =>
        {
            Ok(paths.package_lock_path(&receipt.package_id))
        }
        _ => Err(UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "An extension receipt has inconsistent route-lease generation evidence.",
        )),
    }
}

fn reject_lifecycle_managed(receipt: &ExtensionReceipt) -> UseResult<()> {
    if receipt.schema_version == RECEIPT_SCHEMA_VERSION_V3 || receipt.lifecycle_generation.is_some()
    {
        return Err(UseError::new(
            "use.extension.lifecycle_managed",
            format!(
                "Cognitive package '{}' is owned by the package lifecycle coordinator.",
                receipt.package_id
            ),
        )
        .with_suggestion(
            "Use the cognitive-package lifecycle operation instead of the legacy extension toggle.",
        ));
    }
    Ok(())
}

fn installed_dependents(installed: &[InstalledExtension], package_id: &str) -> Vec<String> {
    installed
        .iter()
        .filter(|extension| {
            extension.receipt.package_id != package_id
                && extension
                    .manifest
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.package_id == package_id)
        })
        .map(|extension| extension.receipt.package_id.clone())
        .collect()
}

fn ensure_no_installed_dependents(
    installed: &[InstalledExtension],
    package_id: &str,
) -> UseResult<()> {
    let required_by = installed_dependents(installed, package_id);
    if required_by.is_empty() {
        return Ok(());
    }
    Err(UseError::new(
        "use.extension.package_required",
        format!("Cognitive package '{package_id}' is still required by another installed package."),
    )
    .with_detail("packageId", package_id.to_string())
    .with_detail("requiredBy", required_by)
    .with_suggestion(
        "Review and apply a cascade uninstall plan that removes dependents before dependencies.",
    ))
}

async fn reject_lifecycle_orphan_cleanup(path: &Path) -> UseResult<()> {
    let mut entries = match fs::read_dir(path).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("inspect extension package", path, error)),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("inspect extension package", path, error))?
    {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("lifecycle-"))
        {
            return Err(UseError::new(
                "use.extension.lifecycle_managed",
                "A lifecycle-managed package generation requires exact coordinator-owned cleanup.",
            ));
        }
    }
    Ok(())
}

async fn remove_package_parent_if_present(path: &Path) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error("inspect extension package", path, error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.ownership_invalid",
            format!(
                "Refusing to remove invalid extension package directory '{}'.",
                path.display()
            ),
        )
        .with_detail("routeDisabled", true));
    }
    fs::remove_dir_all(path).await.map_err(|error| {
        io_error("remove extension package", path, error).with_detail("routeDisabled", true)
    })?;
    Ok(true)
}

fn normalize_package_id(value: &str) -> UseResult<String> {
    let value = value.strip_prefix("use/").unwrap_or(value);
    if !super::valid_package_id(value) {
        return Err(UseError::new(
            "use.extension.id_invalid",
            "Extension IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    Ok(value.to_string())
}

fn registry_identity(registry: Option<&ResolvedRemotePackage>) -> Option<(&str, &str, &str, &str)> {
    registry.map(|registry| {
        (
            registry.registry_name.as_str(),
            registry.registry_url.as_str(),
            registry.root_sha256.as_str(),
            registry.sha256.as_str(),
        )
    })
}

fn validate_catalog_package(
    catalog: Option<&VerifiedPluginCatalogRecord>,
    registry: Option<&ResolvedRemotePackage>,
    manifest: &ExtensionManifest,
    manifest_bytes: &[u8],
    package_digest: &str,
) -> UseResult<()> {
    let Some(catalog) = catalog else {
        return Ok(());
    };
    let manifest_digest = sha256(manifest_bytes);
    validate_catalog_binding(
        catalog,
        registry,
        manifest,
        &manifest_digest,
        package_digest,
    )
}

fn validate_catalog_binding(
    catalog: &VerifiedPluginCatalogRecord,
    registry: Option<&ResolvedRemotePackage>,
    manifest: &ExtensionManifest,
    manifest_digest: &str,
    package_digest: &str,
) -> UseResult<()> {
    catalog.validate().map_err(|error| {
        catalog_package_error(format!(
            "The verified catalog evidence is invalid: {}",
            error.message
        ))
    })?;
    if !catalog.record.is_package_plan_ready() {
        return Err(catalog_package_error(
            "Only complete catalog evidence can be persisted as plan-ready installation state.",
        ));
    }
    let resolved = ResolvedRemotePackage::from_verified_catalog(catalog).map_err(|error| {
        catalog_package_error(format!(
            "The verified catalog cannot reconstruct its registry target: {}",
            error.message
        ))
    })?;
    if registry != Some(&resolved) {
        return Err(catalog_package_error(
            "The verified catalog does not match the selected registry target.",
        ));
    }
    let record = &catalog.record;
    validate_catalog_manifest_binding(record, manifest)?;
    let expected_package_digest = record
        .package
        .sha256
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    let expected_manifest_digest = record
        .package
        .manifest_sha256
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    if expected_package_digest != Some(package_digest)
        || expected_manifest_digest != Some(manifest_digest)
    {
        return Err(catalog_package_error(
            "The verified catalog does not match the installed package, manifest, or dependency graph.",
        ));
    }
    Ok(())
}

/// Validate the manifest fields that drive lifecycle side effects against one
/// signed catalog record. This is intentionally independent of package bytes
/// so durable operation journals can reject a changed replay manifest before
/// touching a retained generation.
pub fn validate_catalog_manifest_binding(
    record: &PluginCatalogRecord,
    manifest: &ExtensionManifest,
) -> UseResult<()> {
    record.validate().map_err(|error| {
        catalog_package_error(format!(
            "The catalog record is invalid during manifest binding: {}",
            error.message
        ))
    })?;
    if record.package_id != manifest.package_id
        || record.version != manifest.version
        || record.dependencies != manifest.dependencies
    {
        return Err(catalog_package_error(
            "The catalog does not match the manifest package, version, or dependency graph.",
        ));
    }
    if manifest.schema_version == 3 {
        validate_surface_catalog_binding(record, manifest)?;
    }
    Ok(())
}

fn validate_surface_catalog_binding(
    record: &a3s_use_core::PluginCatalogRecord,
    manifest: &ExtensionManifest,
) -> UseResult<()> {
    let manifest_surfaces = manifest.plugin_surfaces()?;
    if record.surfaces.len() != manifest_surfaces.len() {
        return Err(catalog_package_error(
            "The verified catalog surface inventory does not match the installed manifest.",
        ));
    }
    for surface in &manifest_surfaces {
        let Some(catalog) = record
            .surfaces
            .iter()
            .find(|catalog| catalog.reference() == surface.surface)
        else {
            return Err(catalog_package_error(
                "The verified catalog omitted a manifest-declared surface.",
            ));
        };
        if catalog.optional != surface.optional || catalog.requires != surface.dependencies {
            return Err(catalog_package_error(
                "The verified catalog surface dependency graph does not match the installed manifest.",
            ));
        }
    }
    for surface in &manifest.okf {
        let Some(catalog) = record
            .surfaces
            .iter()
            .find(|catalog| catalog.kind == PluginSurfaceKind::Okf && catalog.id == surface.id)
        else {
            return Err(catalog_package_error(
                "The verified catalog omitted a manifest-declared OKF surface.",
            ));
        };
        if catalog.okf_bundle.as_ref() != Some(&surface.bundle) {
            return Err(catalog_package_error(
                "The verified catalog OKF contract does not match the installed manifest.",
            ));
        }
    }
    Ok(())
}

fn catalog_package_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.catalog_package_mismatch", message)
}

fn plan_evidence_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.plan_evidence_missing", message)
}

fn ensure_unique_routes(installed: &[InstalledExtension]) -> UseResult<()> {
    for (index, extension) in installed.iter().enumerate() {
        if let Some(conflict) = installed[index + 1..]
            .iter()
            .find(|candidate| candidate.receipt.route == extension.receipt.route)
        {
            return Err(UseError::new(
                "use.extension.route_conflict",
                format!(
                    "Route '{}' is claimed by '{}' and '{}'.",
                    extension.receipt.route,
                    extension.receipt.package_id,
                    conflict.receipt.package_id
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
