use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_use_core::{
    PlanPackageRole, PlannedPackageState, PlannedPackageTransition, PluginCatalogRecord,
    PluginPlanningBundle, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
    VerifiedPluginCatalogRecord,
};
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::digest::package_sha256;
use super::package::{
    io_error, lock_is_contended, owned_package_path, read_manifest, sha256, validate_surface_files,
    RegistryLock,
};
use super::registry_io::{read_registry_snapshot, write_registry_snapshot};
use super::remote::ResolvedRemotePackage;
use super::route_lock::{deadline_after, open_route_lock};
use super::{ExtensionManifest, ExtensionPaths};

mod cutover;
mod lifecycle;

pub use cutover::{
    ExtensionRegistryCutoverRecord, EXTENSION_REGISTRY_CUTOVER_SCHEMA,
    MAX_PENDING_REGISTRY_CUTOVERS,
};
pub use lifecycle::{
    ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
    ExtensionLifecycleResult, ExtensionLifecycleRollbackResult,
};

const RECEIPT_SCHEMA_VERSION_V3: u32 = 3;
pub(super) const REGISTRY_SCHEMA_VERSION: u32 = 1;
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
    pub package_sha256: Option<String>,
    pub trust: ExtensionTrust,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<ResolvedRemotePackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_catalog: Option<VerifiedPluginCatalogRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_bundle: Option<PluginPlanningBundle>,
    pub installed_at_unix: u64,
    pub enabled: bool,
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
        if self.receipt.schema_version != RECEIPT_SCHEMA_VERSION_V3
            || self.receipt.trust != ExtensionTrust::RegistryTuf
        {
            return Err(plan_evidence_error(
                "The installed extension receipt is not plan-ready registry state.",
            ));
        }
        validate_catalog_binding(
            catalog,
            self.receipt.registry.as_ref(),
            &self.manifest,
            &self.receipt.manifest_sha256,
            self.receipt.package_sha256.as_deref().ok_or_else(|| {
                plan_evidence_error("The cognitive-package receipt omitted its package digest.")
            })?,
        )?;
        Ok(catalog)
    }

    /// Return the signed executable-planning target retained at installation.
    ///
    /// Static packages legitimately return `None`. A package whose catalog
    /// declares executable planning must retain the exact validated bundle so
    /// enablement can be reviewed offline without consulting a mutable
    /// Registry again.
    pub fn plan_ready_planning_bundle(&self) -> UseResult<Option<&PluginPlanningBundle>> {
        let catalog = self.plan_ready_catalog()?;
        match (&catalog.record.planning, &self.receipt.planning_bundle) {
            (None, None) => Ok(None),
            (Some(_), Some(bundle)) => {
                bundle.validate_catalog_binding(catalog)?;
                Ok(Some(bundle))
            }
            _ => Err(plan_evidence_error(
                "The installed extension receipt does not retain its exact signed planning bundle.",
            )),
        }
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
    pub package_sha256: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_cutovers: Vec<ExtensionRegistryCutoverRecord>,
}

impl Default for ExtensionRegistrySnapshot {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation: 0,
            routes: Vec::new(),
            pending_cutovers: Vec::new(),
        }
    }
}

impl ExtensionRegistrySnapshot {
    /// Canonical digest of the exact capability projection selected by one
    /// Registry generation. Pending operation metadata is deliberately
    /// excluded, so acknowledging a durable cutover cannot change capability
    /// identity or advance its generation.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CapabilityProjection<'a> {
            schema_version: u32,
            generation: u64,
            routes: &'a [ExtensionRouteBinding],
        }
        let projection = CapabilityProjection {
            schema_version: self.schema_version,
            generation: self.generation,
            routes: &self.routes,
        };
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        projection.serialize(&mut serializer).map_err(|error| {
            UseError::new(
                "use.extension.registry_invalid",
                format!("Failed to encode the canonical extension Registry snapshot: {error}"),
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
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
        let package_sha256 = binding.package_sha256.as_deref().ok_or_else(|| {
            UseError::new(
                "use.extension.lifecycle_binding_invalid",
                "A cognitive-package snapshot binding omitted its package digest.",
            )
        })?;
        let generation = binding.lifecycle_generation.ok_or_else(|| {
            UseError::new(
                "use.extension.lifecycle_binding_invalid",
                "A cognitive-package snapshot binding omitted its generation.",
            )
        })?;
        let identity = ExtensionLifecycleIdentity::new(
            &binding.package_id,
            format!("sha256:{package_sha256}"),
            format!("sha256:{}", binding.manifest_sha256),
            generation,
        )?;
        let extension = self.get_lifecycle_generation(&identity).await?;
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

    /// Pin the exact currently published cognitive-package generation for a
    /// host-owned dispatch. The lease participates in lifecycle drain and
    /// never launches package-owned processes outside the surface lifecycle.
    pub async fn acquire_published_route(
        &self,
        route: &str,
    ) -> UseResult<Option<ExtensionRouteLease>> {
        let Some(candidate) = self
            .find_route_for_host_version(route, env!("CARGO_PKG_VERSION"))
            .await?
        else {
            return Ok(None);
        };
        self.acquire_extension_lease_for_host_version(
            candidate,
            Some(route),
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    /// Pin one exact currently published cognitive-package generation.
    ///
    /// Managed hosts use this form when a capability projection already
    /// carries the reviewed package digest, manifest digest, and lifecycle
    /// generation. The lease participates in the same lifecycle drain as a
    /// route dispatch. It returns `None` when the exact generation is missing,
    /// no longer published, incompatible with this host, or already draining.
    pub async fn acquire_published_lifecycle_generation(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<Option<ExtensionRouteLease>> {
        let Some(candidate) = self.get_lifecycle_generation(identity).await? else {
            return Ok(None);
        };
        self.acquire_extension_lease_for_host_version(candidate, None, env!("CARGO_PKG_VERSION"))
            .await
    }

    /// Resolve the exact currently published cognitive-package generation
    /// without acquiring a dispatch lease.
    pub async fn find_published_route(&self, route: &str) -> UseResult<Option<InstalledExtension>> {
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
            pending_cutovers: current.pending_cutovers,
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
        if receipt.schema_version != RECEIPT_SCHEMA_VERSION_V3 {
            return Err(UseError::new(
                "use.extension.receipt_incompatible",
                format!(
                    "Extension receipt schema {} is obsolete; remove the pre-release state and reinstall the package.",
                    receipt.schema_version
                ),
            ));
        }
        let generation = receipt.lifecycle_generation.ok_or_else(|| {
            UseError::new(
                "use.extension.lifecycle_receipt_invalid",
                "A cognitive-package receipt omitted its generation.",
            )
        })?;
        let expected_package_sha256 = receipt.package_sha256.as_deref().ok_or_else(|| {
            UseError::new(
                "use.extension.lifecycle_receipt_invalid",
                "A cognitive-package receipt omitted its package digest.",
            )
        })?;
        if generation == 0
            || expected_package_sha256.len() != 64
            || !expected_package_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(UseError::new(
                "use.extension.lifecycle_receipt_invalid",
                format!(
                    "Extension receipt for '{}' has an invalid generation or package digest.",
                    receipt.package_id
                ),
            ));
        }
        match (
            receipt.trust,
            receipt.registry.as_ref(),
            receipt.verified_catalog.as_ref(),
            receipt.planning_bundle.as_ref(),
        ) {
            (ExtensionTrust::LocalExplicit | ExtensionTrust::ReleaseBundle, None, None, None) => {}
            (ExtensionTrust::RegistryTuf, Some(registry), Some(catalog), planning_bundle) => {
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
                if !catalog.record.is_package_plan_ready() {
                    return Err(UseError::new(
                        "use.extension.receipt_invalid",
                        format!(
                            "Extension receipt for '{}' contains non-plan-ready catalog evidence.",
                            receipt.package_id
                        ),
                    ));
                }
                if catalog.record.planning.is_some() != planning_bundle.is_some() {
                    return Err(UseError::new(
                        "use.extension.receipt_invalid",
                        format!(
                            "Extension receipt for '{}' does not retain its signed planning target.",
                            receipt.package_id
                        ),
                    )
                    .with_suggestion("Reinstall the cognitive package from its trusted Registry."));
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
        if receipt.package_root
            != self
                .paths
                .lifecycle_package_root(&package_id, generation, expected_package_sha256)
        {
            return Err(UseError::new(
                "use.extension.lifecycle_receipt_invalid",
                format!(
                    "Lifecycle receipt for '{}' does not bind its immutable generation.",
                    package_id
                ),
            ));
        }
        validate_surface_files(&manifest, &receipt.package_root).await?;
        let package_digest = package_sha256(&receipt.package_root).await?;
        if expected_package_sha256 != package_digest {
            return Err(UseError::new(
                "use.extension.package_digest_mismatch",
                format!(
                    "Installed package '{}' no longer matches its recorded digest.",
                    receipt.package_id
                ),
            )
            .with_suggestion("Reinstall the extension from its trusted source."));
        }
        if let Some(catalog) = receipt.verified_catalog.as_ref() {
            validate_catalog_package(
                Some(catalog),
                receipt.registry.as_ref(),
                &manifest,
                &manifest_bytes,
                &package_digest,
            )?;
            match receipt.planning_bundle.as_ref() {
                Some(bundle) => {
                    bundle.validate_catalog_binding(catalog)?;
                    crate::surface_files::validate_planning_bundle_package_binding(
                        bundle,
                        &manifest,
                        &receipt.package_root,
                    )
                    .await?;
                }
                None if catalog.record.planning.is_none() => {}
                None => {
                    return Err(UseError::new(
                        "use.extension.receipt_invalid",
                        format!(
                            "Extension receipt for '{}' omitted executable planning evidence.",
                            receipt.package_id
                        ),
                    ))
                }
            }
        }
        Ok(InstalledExtension { receipt, manifest })
    }
}

async fn verify_package_integrity(extension: &InstalledExtension) -> UseResult<()> {
    let actual = package_sha256(&extension.receipt.package_root).await?;
    if extension.receipt.package_sha256.as_deref() != Some(actual.as_str()) {
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
) -> BTreeMap<&str, (u64, &str, &str, &str, &str)> {
    routes
        .iter()
        .filter_map(|binding| {
            let generation = binding.lifecycle_generation?;
            let package_sha256 = binding.package_sha256.as_deref()?;
            binding.enabled.then_some((
                binding.package_id.as_str(),
                (
                    generation,
                    binding.manifest_sha256.as_str(),
                    package_sha256,
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
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION_V3 {
        return Err(UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "An extension receipt has inconsistent route-lease generation evidence.",
        ));
    }
    let generation = receipt.lifecycle_generation.ok_or_else(|| {
        UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "A cognitive-package receipt omitted its route-lease generation.",
        )
    })?;
    Ok(paths.lifecycle_package_lock_path(&receipt.package_id, generation))
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
