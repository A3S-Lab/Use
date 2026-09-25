use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_use_core::{
    InstallationId, InstallationPackageSelection, PluginCatalogRecord, PluginPlanningBundle,
    PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult, VerifiedPluginCatalogRecord,
};
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::digest::package_sha256;
use super::generation_lease::{deadline_after, open_generation_lock};
use super::package::{
    io_error, lock_is_contended, read_manifest, sha256, validate_surface_files, RegistryLock,
};
use super::registry_io::{read_registry_snapshot, write_registry_snapshot};
use super::remote::ResolvedRemotePackage;
use super::state_maintenance::StateMaintenanceLock;
use super::{ExtensionManifest, ExtensionPaths};

mod artifact_reference;
mod cutover;
mod lifecycle;
mod receipt;
mod snapshot_lease;
mod watch;

pub use artifact_reference::ExtensionArtifactReference;
pub use cutover::{
    ExtensionRegistryCutoverRecord, EXTENSION_REGISTRY_CUTOVER_SCHEMA,
    MAX_PENDING_REGISTRY_CUTOVERS,
};
pub use lifecycle::{
    ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
    ExtensionLifecycleResult, ExtensionLifecycleRollbackResult,
};
pub use receipt::{
    ExtensionReceipt, ExtensionTrust, InstalledExtension, EXTENSION_RECEIPT_SCHEMA_VERSION,
    MAX_EXTENSION_RECEIPT_BYTES,
};
pub use snapshot_lease::{
    ExtensionSnapshotCursor, ExtensionSnapshotLease, ExtensionSnapshotPackage,
    EXTENSION_SNAPSHOT_CURSOR_SCHEMA,
};

pub(super) const REGISTRY_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionPackageBinding {
    pub package_id: String,
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_alias: Option<String>,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionRegistrySnapshot {
    pub schema_version: u32,
    pub installation: InstallationId,
    pub generation: u64,
    pub packages: Vec<ExtensionPackageBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_cutovers: Vec<ExtensionRegistryCutoverRecord>,
}

impl ExtensionRegistrySnapshot {
    pub fn empty(installation: InstallationId) -> UseResult<Self> {
        installation.validate()?;
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            installation,
            generation: 0,
            packages: Vec::new(),
            pending_cutovers: Vec::new(),
        }
        .validated()
    }

    fn validated(self) -> UseResult<Self> {
        self.validate()?;
        Ok(self)
    }

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
            installation: &'a InstallationId,
            generation: u64,
            packages: &'a [ExtensionPackageBinding],
        }
        let projection = CapabilityProjection {
            schema_version: self.schema_version,
            installation: &self.installation,
            generation: self.generation,
            packages: &self.packages,
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

pub struct ExtensionGenerationLease {
    extension: InstalledExtension,
    file: File,
}

impl ExtensionGenerationLease {
    pub fn extension(&self) -> &InstalledExtension {
        &self.extension
    }

    /// Revalidate the immutable package bytes while retaining the existing
    /// generation lease. Lifecycle cutover may publish a newer generation while a
    /// caller is draining this lease, but package or manifest drift must
    /// still fail closed before a host returns cited content.
    pub async fn verify_integrity(&self) -> UseResult<()> {
        verify_package_integrity(&self.extension).await
    }
}

impl Drop for ExtensionGenerationLease {
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
    pub fn from_env(installation: InstallationId) -> UseResult<Self> {
        Ok(Self::new(ExtensionPaths::from_env(installation)?))
    }

    pub fn new(paths: ExtensionPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &ExtensionPaths {
        &self.paths
    }

    pub fn installation(&self) -> &InstallationId {
        self.paths.installation()
    }

    /// Read the last durably published capability projection without
    /// reconciling receipts or writing Registry state.
    ///
    /// Operational diagnostics use this boundary so observation can never
    /// become lifecycle recovery authority. Call [`Self::snapshot`] when the
    /// caller explicitly wants ordinary crash reconciliation semantics.
    pub async fn published_snapshot(&self) -> UseResult<ExtensionRegistrySnapshot> {
        read_registry_snapshot(&self.paths).await
    }

    /// Return the immutable package-generation projection visible to consumers.
    ///
    /// The published projection is compared with ownership-validated receipts
    /// without blocking lifecycle writers. A mismatch is rebuilt under the
    /// registry lock, repairing a crash between receipt activation and
    /// generation publication without requiring a resident daemon.
    pub async fn snapshot(&self) -> UseResult<ExtensionRegistrySnapshot> {
        // The common read path is lock-free with respect to lifecycle writers.
        // Only a real receipt/publication mismatch needs the registry lock for
        // crash reconciliation.
        let published = read_registry_snapshot(&self.paths).await?;
        match self.list().await {
            Ok(installed) if published.packages == package_bindings(&installed) => {
                return Ok(published)
            }
            // A lifecycle writer may remove a receipt between the optimistic
            // directory scan and receipt read. Re-check under the lock below;
            // if that writer still owns it, the last complete publication is
            // the only coherent snapshot to return.
            Ok(_) | Err(_) => {}
        }
        let _maintenance = match StateMaintenanceLock::new(self.paths.state_root())
            .try_acquire_shared()
            .await
        {
            Ok(Some(guard)) => guard,
            Ok(None) => return Ok(published),
            Err(error) if error.code == "use.state.maintenance_restore_active" => {
                return Ok(published)
            }
            Err(error) => return Err(error),
        };
        let _lock = match RegistryLock::acquire(&self.paths.registry_lock_path()) {
            Ok(lock) => lock,
            Err(error) if error.code == "use.extension.busy" => {
                return read_registry_snapshot(&self.paths).await;
            }
            Err(error) => return Err(error),
        };
        let installed = self.list().await?;
        let published = read_registry_snapshot(&self.paths).await?;
        let packages = package_bindings(&installed);
        if published.packages == packages {
            return Ok(published);
        }
        // Receipt writes belonging to a schema-v3 graph publication are not a
        // multi-file transaction. The immutable snapshot is therefore the
        // visibility commit point for both staged and active lifecycle state.
        // Never let an observer publish an installed-disabled candidate or
        // infer another lifecycle generation from partially written receipts;
        // the durable lifecycle journal must replay the exact reviewed batch.
        if lifecycle_bindings(&published.packages) != lifecycle_bindings(&packages) {
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
        if Instant::now() >= deadline {
            return Ok(None);
        }
        let target = self.paths.registry_snapshot_path();
        loop {
            let Some(mut watcher) = watch::RegistryChangeWatcher::start(
                target.clone(),
                self.paths.use_paths().state_root().to_path_buf(),
                deadline,
            )
            .await?
            else {
                return Ok(None);
            };

            // Close every read-to-subscribe race. A cutover that committed
            // before the platform watcher was ready is observed by this read
            // even when no event remains queued.
            let published = read_registry_snapshot(&self.paths).await?;
            if published.generation > after_generation {
                return Ok(Some(published));
            }
            if !watcher.changed(deadline).await? {
                return Ok(None);
            }
            // Lifecycle mutations atomically replace the immutable projection
            // before draining old calls. A notification only wakes the reader;
            // the validated publication remains the source of truth.
            let published = read_registry_snapshot(&self.paths).await?;
            if published.generation > after_generation {
                return Ok(Some(published));
            }
            // When the installation state root did not exist at subscription
            // time, the watcher observes only the next ancestor creation. Drop
            // it and subscribe again so the watched directory advances toward
            // the exact Registry commit point without ever recursively
            // observing an unrelated ancestor tree.
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
            let publisher_path = publisher.path();
            let metadata = fs::symlink_metadata(&publisher_path)
                .await
                .map_err(|error| io_error("inspect receipt publisher", &publisher_path, error))?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                continue;
            }
            let mut entries = fs::read_dir(&publisher_path)
                .await
                .map_err(|error| io_error("read publisher receipts", &publisher_path, error))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| io_error("read publisher receipt", &publisher_path, error))?
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
        Ok(installed)
    }

    pub async fn get(&self, package_id: &str) -> UseResult<Option<InstalledExtension>> {
        let package_id = normalize_package_id(package_id)?;
        let path = self.paths.receipt_path(&package_id);
        match fs::symlink_metadata(&path).await {
            Ok(_) => self.load_receipt(&path).await.map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("inspect extension receipt", &path, error)),
        }
    }

    /// Load one package from Control/installation selection evidence and the
    /// Artifact Store without reading legacy `extensions/` receipts.
    pub async fn load_control_package_selection(
        &self,
        selection: &InstallationPackageSelection,
    ) -> UseResult<InstalledExtension> {
        selection.validate().map_err(|error| {
            UseError::new(
                "use.extension.control_selection_invalid",
                format!("A Control package selection is invalid: {}", error.message),
            )
        })?;
        let catalog = &selection.package.catalog;
        let record = &catalog.record;
        let package_digest = record.package.sha256.clone().ok_or_else(|| {
            UseError::new(
                "use.extension.control_selection_invalid",
                "A Control package selection omits its expanded package digest.",
            )
        })?;
        let package_sha256_hex = package_digest.strip_prefix("sha256:").ok_or_else(|| {
            UseError::new(
                "use.extension.control_selection_invalid",
                "A Control package digest must use the 'sha256:' prefix.",
            )
        })?;
        let artifact_store = self.paths.artifact_store();
        let package_root = artifact_store.expanded_package_path(&package_digest)?;
        artifact_store
            .validate_expanded_package_path(package_sha256_hex, &package_root)
            .await?;
        let (manifest, manifest_bytes) = read_manifest(&package_root).await?;
        if manifest.package_id != record.package_id || manifest.version != record.version {
            return Err(UseError::new(
                "use.extension.control_selection_mismatch",
                "The Artifact Store package does not match its Control catalog identity.",
            ));
        }
        validate_surface_selection(&manifest, Some(catalog), &selection.selected_surfaces)?;
        validate_surface_files(&manifest, &package_root).await?;
        let actual_digest = package_sha256(&package_root).await?;
        if actual_digest != package_sha256_hex {
            return Err(UseError::new(
                "use.extension.package_digest_mismatch",
                format!(
                    "Control package '{}' no longer matches its recorded digest.",
                    record.package_id
                ),
            ));
        }
        let manifest_digest = sha256(&manifest_bytes);
        if let Some(expected) = &record.package.manifest_sha256 {
            let expected = expected
                .strip_prefix("sha256:")
                .unwrap_or(expected.as_str());
            if expected != manifest_digest {
                return Err(UseError::new(
                    "use.extension.control_selection_mismatch",
                    "The package manifest digest disagrees with Control catalog evidence.",
                ));
            }
        }
        let resolved = ResolvedRemotePackage::from_verified_catalog(catalog).map_err(|error| {
            UseError::new(
                "use.extension.control_selection_invalid",
                format!(
                    "Control catalog evidence cannot reconstruct its registry target: {}",
                    error.message
                ),
            )
        })?;
        validate_catalog_package(
            Some(catalog),
            Some(&resolved),
            &manifest,
            &manifest_bytes,
            &actual_digest,
        )?;
        let planning_bundle = match &record.planning {
            Some(planning) => {
                let planning_path = artifact_store.blob_path(&planning.sha256)?;
                let bytes = fs::read(&planning_path).await.map_err(|error| {
                    io_error("read Control planning bundle", &planning_path, error)
                })?;
                if bytes.len() as u64 != planning.length {
                    return Err(UseError::new(
                        "use.extension.planning_package_mismatch",
                        "The planning bundle length disagrees with Control catalog evidence.",
                    ));
                }
                let bundle = PluginPlanningBundle::from_json(&bytes)?;
                bundle.validate_catalog_binding(catalog)?;
                crate::surface_files::validate_planning_bundle_package_binding(
                    &bundle,
                    &manifest,
                    &package_root,
                )
                .await?;
                Some(bundle)
            }
            None => None,
        };
        let selected_surfaces = selection.selected_surfaces.clone();
        Ok(InstalledExtension {
            receipt: ExtensionReceipt {
                schema_version: EXTENSION_RECEIPT_SCHEMA_VERSION,
                installation: self.installation().clone(),
                package_id: manifest.package_id.clone(),
                component_id: format!("use/{}", manifest.package_id),
                route_alias: manifest.route_alias.clone(),
                version: manifest.version.clone(),
                package_root,
                manifest_sha256: manifest_digest,
                package_sha256: Some(actual_digest),
                trust: ExtensionTrust::RegistryTuf,
                registry: Some(resolved),
                verified_catalog: Some(catalog.clone()),
                planning_bundle,
                selected_surfaces,
                installed_at_unix: 0,
                enabled: selection.enabled,
                lifecycle_generation: Some(selection.state_generation),
            },
            manifest,
        })
    }

    /// Resolve the exact receipt selected by one immutable Registry snapshot
    /// binding. During blue-green preparation this may be a retained prior
    /// generation rather than the primary candidate receipt.
    pub async fn get_snapshot_binding(
        &self,
        binding: &ExtensionPackageBinding,
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

    /// Resolve a human-facing alias and pin its exact published package
    /// generation. Duplicate aliases fail with an explicit ambiguity error;
    /// callers that already hold canonical identity must use
    /// [`Self::acquire_published_lifecycle_generation`].
    pub async fn acquire_published_alias(
        &self,
        alias: &str,
    ) -> UseResult<Option<ExtensionGenerationLease>> {
        let Some(candidate) = self
            .resolve_alias_for_host_version(alias, env!("CARGO_PKG_VERSION"))
            .await?
        else {
            return Ok(None);
        };
        self.acquire_extension_generation_for_host_version(
            candidate,
            Some(alias),
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    /// Pin one exact currently published cognitive-package generation.
    ///
    /// Managed hosts use this form when a capability projection already
    /// carries the reviewed package digest, manifest digest, and lifecycle
    /// generation. The lease participates in the same lifecycle drain as a
    /// alias dispatch. It returns `None` when the exact generation is missing,
    /// no longer published, incompatible with this host, or already draining.
    pub async fn acquire_published_lifecycle_generation(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<Option<ExtensionGenerationLease>> {
        let Some(candidate) = self.get_lifecycle_generation(identity).await? else {
            return Ok(None);
        };
        self.acquire_extension_generation_for_host_version(
            candidate,
            None,
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    /// Pin one exact Control-selected package generation without reading
    /// `registry.json` publication state.
    ///
    /// The caller supplies the committed Control selection. The lease locks
    /// the same generation drain file as lifecycle publication so concurrent
    /// Control cutover still fails closed for in-flight Knowledge sessions.
    pub async fn acquire_control_lifecycle_generation(
        &self,
        selection: &InstallationPackageSelection,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<Option<ExtensionGenerationLease>> {
        let candidate = self.load_control_package_selection(selection).await?;
        if !control_extension_matches_identity(&candidate, identity) {
            return Ok(None);
        }
        if !candidate.receipt.enabled || !candidate.supports_use_version(env!("CARGO_PKG_VERSION"))
        {
            return Ok(None);
        }
        let path = lifecycle_generation_lock_path(&self.paths, &candidate.receipt)?;
        let file = open_generation_lock(&path)?;
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {}
            Err(error) if lock_is_contended(&error) => return Ok(None),
            Err(error) => return Err(io_error("acquire Control generation lease", &path, error)),
        }
        // Re-load after locking so concurrent Control disable cannot admit a
        // caller with a stale selection.
        let extension = match self.load_control_package_selection(selection).await {
            Ok(extension) => extension,
            Err(_) => {
                let _ = FileExt::unlock(&file);
                return Ok(None);
            }
        };
        if !control_extension_matches_identity(&extension, identity)
            || !extension.receipt.enabled
            || !extension.supports_use_version(env!("CARGO_PKG_VERSION"))
        {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        }
        if let Err(error) = verify_package_integrity(&extension).await {
            let _ = FileExt::unlock(&file);
            return Err(error);
        }
        Ok(Some(ExtensionGenerationLease { extension, file }))
    }

    /// Resolve the exact currently published cognitive-package generation
    /// without acquiring a dispatch lease.
    pub async fn resolve_published_alias(
        &self,
        alias: &str,
    ) -> UseResult<Option<InstalledExtension>> {
        self.resolve_alias_for_host_version(alias, env!("CARGO_PKG_VERSION"))
            .await
    }

    async fn resolve_alias_for_host_version(
        &self,
        alias: &str,
        host_version: &str,
    ) -> UseResult<Option<InstalledExtension>> {
        let published = read_registry_snapshot(&self.paths).await?;
        let Some(binding) = unique_published_alias_binding(&published, alias)? else {
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

    async fn acquire_extension_generation_for_host_version(
        &self,
        candidate: InstalledExtension,
        expected_alias: Option<&str>,
        host_version: &str,
    ) -> UseResult<Option<ExtensionGenerationLease>> {
        let path = lifecycle_generation_lock_path(&self.paths, &candidate.receipt)?;
        let file = open_generation_lock(&path)?;
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {}
            Err(error) if lock_is_contended(&error) => return Ok(None),
            Err(error) => return Err(io_error("acquire extension generation lease", &path, error)),
        }

        // Re-read after locking so a concurrent disable cannot admit a call
        // using stale publication metadata.
        let published = read_registry_snapshot(&self.paths).await?;
        let binding = match expected_alias {
            Some(alias) => unique_published_alias_binding(&published, alias)?
                .filter(|binding| published_binding_matches_generation(binding, &candidate)),
            None => published.packages.iter().find(|binding| {
                binding.enabled && published_binding_matches_generation(binding, &candidate)
            }),
        };
        let Some(binding) = binding else {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        };
        let Some(extension) = self.get_snapshot_binding(binding).await? else {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        };
        if !extension.receipt.enabled || !extension.supports_use_version(host_version) {
            let _ = FileExt::unlock(&file);
            return Ok(None);
        }
        verify_package_integrity(&extension).await?;
        Ok(Some(ExtensionGenerationLease { extension, file }))
    }

    async fn publish_snapshot_locked(
        &self,
        installed: &[InstalledExtension],
    ) -> UseResult<ExtensionRegistrySnapshot> {
        let packages = package_bindings(installed);
        let current = read_registry_snapshot(&self.paths).await?;
        if current.packages == packages {
            return Ok(current);
        }
        let snapshot = ExtensionRegistrySnapshot {
            schema_version: REGISTRY_SCHEMA_VERSION,
            installation: self.installation().clone(),
            generation: current.generation.checked_add(1).ok_or_else(|| {
                UseError::new(
                    "use.extension.generation_exhausted",
                    "The extension registry generation is exhausted.",
                )
            })?,
            packages,
            pending_cutovers: current.pending_cutovers,
        };
        write_registry_snapshot(&self.paths, &snapshot).await?;
        Ok(snapshot)
    }

    async fn load_receipt(&self, receipt_path: &Path) -> UseResult<InstalledExtension> {
        let bytes = artifact_reference::read_extension_receipt_bytes(receipt_path).await?;
        let receipt: ExtensionReceipt = serde_json::from_slice(&bytes).map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!(
                    "Invalid extension receipt '{}': {error}",
                    receipt_path.display()
                ),
            )
        })?;
        let artifact_reference = receipt.artifact_reference(&self.paths.artifact_store())?;
        if receipt.installation != *self.installation() {
            return Err(UseError::new(
                "use.extension.receipt_scope_mismatch",
                "The extension receipt belongs to a different installation.",
            ));
        }
        let expected_package_sha256 = artifact_reference
            .digest
            .strip_prefix("sha256:")
            .ok_or_else(|| {
                UseError::new(
                    "use.extension.lifecycle_receipt_invalid",
                    "A cognitive-package receipt has an invalid package digest.",
                )
            })?;
        let package_id = normalize_package_id(&receipt.package_id)?;
        self.paths
            .artifact_store()
            .validate_expanded_package_path(expected_package_sha256, &receipt.package_root)
            .await?;
        let (manifest, manifest_bytes) = read_manifest(&receipt.package_root).await?;
        if manifest.package_id != receipt.package_id
            || manifest.version != receipt.version
            || manifest.route_alias != receipt.route_alias
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
        validate_surface_selection(
            &manifest,
            receipt.verified_catalog.as_ref(),
            &receipt.selected_surfaces,
        )?;
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

include!("registry_bindings.rs");

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
