use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::digest::package_sha256;
use super::package::{
    copy_package, io_error, owned_package_path, read_manifest, sha256, unique_suffix,
    unix_timestamp, validate_surface_files, write_receipt, RegistryLock,
};
use super::registry_io::{read_registry_snapshot, write_registry_snapshot};
use super::remote::{prepare_remote_package, ResolvedRemotePackage, TrustedRegistry};
use super::route_lock::{acquire_drain_lock, deadline_after, open_route_lock};
use super::source::prepare_package_source;
use super::{ExtensionManifest, ExtensionPaths, McpTransport};

const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub(super) const REGISTRY_SCHEMA_VERSION: u32 = 1;
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const WATCH_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionTrust {
    LocalExplicit,
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
    pub installed_at_unix: u64,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
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
        let mut surfaces = Vec::with_capacity(3);
        if self.manifest.cli.is_some() {
            surfaces.push("cli");
        }
        if self.manifest.mcp.is_some() {
            surfaces.push("mcp");
        }
        if self.manifest.skill.is_some() {
            surfaces.push("skill");
        }
        surfaces
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
        let deadline = deadline_after(timeout)?;
        // Reconcile once when the subscription starts. Polling after this
        // point reads only immutable publications so watchers never become a
        // periodic source of write-lock contention for lifecycle operations.
        let initial = self.snapshot().await?;
        if initial.generation > after_generation {
            return Ok(Some(initial));
        }
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

    pub async fn find_route(&self, route: &str) -> UseResult<Option<InstalledExtension>> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .find(|extension| extension.receipt.enabled && extension.receipt.route == route))
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
        let Some(candidate) = self.get(package_id).await? else {
            return Ok(None);
        };
        if !candidate.receipt.enabled {
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
                .converged_remote_install(&expected_package_id, prepared.resolved())
                .await?
            {
                return Ok(result);
            }
        }
        let downloaded = prepared.download().await?;
        let provenance = downloaded.resolved().clone();
        let source = prepare_package_source(downloaded.path()).await?;
        self.install_prepared(
            &expected_package_id,
            source.root(),
            force,
            ExtensionTrust::RegistryTuf,
            Some(provenance),
        )
        .await
    }

    async fn converged_remote_install(
        &self,
        expected_package_id: &str,
        resolved: &ResolvedRemotePackage,
    ) -> UseResult<Option<InstallResult>> {
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let Some(current) = self.get(expected_package_id).await? else {
            return Ok(None);
        };
        let same_target = current.receipt.trust == ExtensionTrust::RegistryTuf
            && current.receipt.version == resolved.version
            && registry_identity(current.receipt.registry.as_ref())
                == registry_identity(Some(resolved));
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
    ) -> UseResult<InstallResult> {
        match (trust, registry.as_ref()) {
            (ExtensionTrust::LocalExplicit, None) | (ExtensionTrust::RegistryTuf, Some(_)) => {}
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
                    == registry_identity(registry.as_ref());
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
            schema_version: RECEIPT_SCHEMA_VERSION,
            package_id: expected_package_id.to_string(),
            component_id: format!("use/{expected_package_id}"),
            route: manifest.route.clone(),
            version: manifest.version.clone(),
            package_root: target.clone(),
            manifest_sha256: digest,
            package_sha256: Some(package_digest),
            trust,
            registry,
            installed_at_unix: unix_timestamp(),
            enabled,
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
            let changed =
                remove_package_parent_if_present(&self.paths.package_parent(&package_id)).await?;
            return Ok(UninstallResult {
                package_id,
                changed,
            });
        };
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
        let path = self.paths.package_lock_path(&candidate.receipt.package_id);
        let file = open_route_lock(&path)?;
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(io_error("acquire extension route lease", &path, error)),
        }

        // Re-read after locking so a concurrent disable cannot admit a call
        // using stale route metadata.
        let Some(extension) = self.get(&candidate.receipt.package_id).await? else {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        };
        if !extension.receipt.enabled
            || expected_route.is_some_and(|route| extension.receipt.route != route)
        {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        }
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
        if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(UseError::new(
                "use.extension.receipt_incompatible",
                format!(
                    "Extension receipt schema {} is not supported.",
                    receipt.schema_version
                ),
            ));
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
        match (receipt.trust, receipt.registry.as_ref()) {
            (ExtensionTrust::LocalExplicit, None) => {}
            (ExtensionTrust::RegistryTuf, Some(registry)) => {
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
        validate_surface_files(&manifest, &receipt.package_root).await?;
        Ok(InstalledExtension { receipt, manifest })
    }
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
            enabled: extension.receipt.enabled,
            surfaces: extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
        .collect()
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
