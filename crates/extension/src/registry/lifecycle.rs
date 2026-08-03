use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult, VerifiedPluginCatalogRecord};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs;

use super::{
    normalize_package_id, verify_package_integrity, ExtensionReceipt, ExtensionRegistry,
    ExtensionTrust, InstalledExtension, UninstallResult, RECEIPT_SCHEMA_VERSION_V3,
};
use crate::package::{
    copy_package, io_error, read_manifest, sha256, unix_timestamp, validate_surface_files,
    write_receipt, RegistryLock,
};
use crate::remote::{DownloadedRemotePackage, ResolvedRemotePackage};
use crate::source::{prepare_package_source, PreparedPackageSource};
use crate::{ExtensionManifest, ExtensionPaths};

/// Exact package identity owned by one schema-v3 lifecycle operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecycleIdentity {
    package_id: String,
    package_digest: String,
    manifest_digest: String,
    generation: u64,
    #[serde(skip)]
    package_sha256: String,
    #[serde(skip)]
    manifest_sha256: String,
}

impl ExtensionLifecycleIdentity {
    pub fn new(
        package_id: impl AsRef<str>,
        package_digest: impl Into<String>,
        manifest_digest: impl Into<String>,
        generation: u64,
    ) -> UseResult<Self> {
        let package_id = normalize_package_id(package_id.as_ref())?;
        let package_digest = canonical_sha256(package_digest.into(), "package")?;
        let manifest_digest = canonical_sha256(manifest_digest.into(), "manifest")?;
        let package_sha256 = package_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| lifecycle_identity_error("The package digest prefix is invalid."))?
            .to_string();
        let manifest_sha256 = manifest_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| lifecycle_identity_error("The manifest digest prefix is invalid."))?
            .to_string();
        if generation == 0 {
            return Err(lifecycle_identity_error(
                "A lifecycle package generation must be positive.",
            ));
        }
        Ok(Self {
            package_id,
            package_digest,
            manifest_digest,
            generation,
            package_sha256,
            manifest_sha256,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            lifecycle_identity_error(format!(
                "Failed to encode the lifecycle package identity: {error}"
            ))
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    fn package_sha256(&self) -> &str {
        &self.package_sha256
    }

    fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
}

/// Validated schema-v3 package bytes retained until immutable commit.
///
/// Constructors preserve the same trust boundaries as the legacy installer:
/// local packages require explicit approval, release bundles recheck their
/// reviewed digest, and remote packages can only originate from a verified
/// TUF download object.
#[derive(Debug)]
pub struct ExtensionLifecyclePackage {
    source: PreparedPackageSource,
    manifest: ExtensionManifest,
    package_digest: String,
    manifest_digest: String,
    trust: ExtensionTrust,
    registry: Option<ResolvedRemotePackage>,
    verified_catalog: Option<VerifiedPluginCatalogRecord>,
}

impl ExtensionLifecyclePackage {
    pub async fn prepare_local(
        expected_package_id: &str,
        source: &Path,
        allow_unsigned: bool,
    ) -> UseResult<Self> {
        Self::prepare_local_for_host(
            expected_package_id,
            source,
            allow_unsigned,
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    async fn prepare_local_for_host(
        expected_package_id: &str,
        source: &Path,
        allow_unsigned: bool,
        host_version: &str,
    ) -> UseResult<Self> {
        if !allow_unsigned {
            return Err(UseError::new(
                "use.extension.trust_required",
                "Unsigned local cognitive packages require explicit trust approval.",
            )
            .with_suggestion("Rerun the explicit install with --allow-unsigned."));
        }
        let source = prepare_package_source(source).await?;
        Self::prepare(
            expected_package_id,
            source,
            ExtensionTrust::LocalExplicit,
            None,
            None,
            host_version,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn prepare_local_for_host_version(
        expected_package_id: &str,
        source: &Path,
        allow_unsigned: bool,
        host_version: &str,
    ) -> UseResult<Self> {
        Self::prepare_local_for_host(expected_package_id, source, allow_unsigned, host_version)
            .await
    }

    pub async fn prepare_release_bundle(
        expected_package_id: &str,
        source: &Path,
        expected_package_sha256: &str,
    ) -> UseResult<Self> {
        let expected_package_id = normalize_package_id(expected_package_id)?;
        let bundle = crate::inspect_release_bundle(source).await?;
        if bundle.package_id != expected_package_id
            || bundle.package_sha256 != expected_package_sha256
        {
            return Err(UseError::new(
                "use.extension.release_bundle_changed",
                format!(
                    "Release bundle '{}' changed after its lifecycle plan was reviewed.",
                    expected_package_id
                ),
            ));
        }
        let source = prepare_package_source(source).await?;
        Self::prepare(
            &expected_package_id,
            source,
            ExtensionTrust::ReleaseBundle,
            None,
            None,
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    pub async fn prepare_remote(
        expected_package_id: &str,
        downloaded: DownloadedRemotePackage,
    ) -> UseResult<Self> {
        let registry = downloaded.resolved().clone();
        let verified_catalog = downloaded
            .verified_catalog()
            .filter(|catalog| catalog.record.is_package_plan_ready())
            .cloned();
        let source = prepare_package_source(downloaded.path()).await?;
        Self::prepare(
            expected_package_id,
            source,
            ExtensionTrust::RegistryTuf,
            Some(registry),
            verified_catalog,
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    async fn prepare(
        expected_package_id: &str,
        source: PreparedPackageSource,
        trust: ExtensionTrust,
        registry: Option<ResolvedRemotePackage>,
        verified_catalog: Option<VerifiedPluginCatalogRecord>,
        host_version: &str,
    ) -> UseResult<Self> {
        let expected_package_id = normalize_package_id(expected_package_id)?;
        validate_provenance(trust, registry.as_ref(), verified_catalog.as_ref())?;
        let (manifest, manifest_bytes) = read_manifest(source.root()).await?;
        if manifest.package_id != expected_package_id {
            return Err(UseError::new(
                "use.extension.identity_mismatch",
                format!(
                    "Requested cognitive package '{}' but the package declares '{}'.",
                    expected_package_id, manifest.package_id
                ),
            ));
        }
        if manifest.schema_version != 3 {
            return Err(UseError::new(
                "use.extension.lifecycle_required",
                "Only schema-v3 cognitive packages use the package lifecycle coordinator.",
            ));
        }
        if !manifest.supports_use_version(host_version)? {
            return Err(UseError::new(
                "use.extension.host_incompatible",
                format!(
                    "Cognitive package '{}' {} does not support A3S Use {}.",
                    manifest.package_id, manifest.version, host_version
                ),
            )
            .with_detail("requiresUse", manifest.requires_use.clone())
            .with_detail("hostVersion", host_version));
        }
        if let Some(registry) = &registry {
            if registry.package_id != manifest.package_id || registry.version != manifest.version {
                return Err(UseError::new(
                    "use.extension.registry_identity_mismatch",
                    "The signed registry target does not match the cognitive package manifest.",
                ));
            }
        }
        validate_surface_files(&manifest, source.root()).await?;
        let package_sha256 = crate::digest::package_sha256(source.root()).await?;
        super::validate_catalog_package(
            verified_catalog.as_ref(),
            registry.as_ref(),
            &manifest,
            &manifest_bytes,
            &package_sha256,
        )?;
        Ok(Self {
            source,
            manifest,
            package_digest: format!("sha256:{package_sha256}"),
            manifest_digest: format!("sha256:{}", sha256(&manifest_bytes)),
            trust,
            registry,
            verified_catalog,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.manifest.package_id
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    fn validate_identity(&self, identity: &ExtensionLifecycleIdentity) -> UseResult<()> {
        if self.package_id() != identity.package_id
            || self.package_digest != identity.package_digest
            || self.manifest_digest != identity.manifest_digest
        {
            return Err(lifecycle_identity_error(
                "The prepared cognitive package does not match the lifecycle identity.",
            ));
        }
        Ok(())
    }

    fn matches_provenance(&self, receipt: &ExtensionReceipt) -> bool {
        receipt.trust == self.trust
            && receipt.registry == self.registry
            && receipt.verified_catalog == self.verified_catalog
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecycleResult {
    pub changed: bool,
    pub extension: InstalledExtension,
    pub registry_generation: u64,
}

impl ExtensionRegistry {
    pub fn lifecycle_package_root(&self, identity: &ExtensionLifecycleIdentity) -> PathBuf {
        lifecycle_root(&self.paths, identity)
    }

    /// Commit one exact immutable cognitive-package generation as
    /// installed-disabled. This is the only schema-v3 package commit path.
    pub async fn commit_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        candidate: &ExtensionLifecyclePackage,
    ) -> UseResult<ExtensionLifecycleResult> {
        candidate.validate_identity(identity)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        if let Some(current) = self.get(identity.package_id()).await? {
            if current.receipt.schema_version == RECEIPT_SCHEMA_VERSION_V3 {
                if exact_receipt(identity, &current.receipt).is_ok()
                    && candidate.matches_provenance(&current.receipt)
                {
                    if current.receipt.enabled {
                        return Err(lifecycle_state_error(
                            "The exact lifecycle generation is already published while package commit is being replayed.",
                        ));
                    }
                    verify_package_integrity(&current).await?;
                    let installed = self.list().await?;
                    let snapshot = self.publish_snapshot_locked(&installed).await?;
                    return Ok(ExtensionLifecycleResult {
                        changed: false,
                        extension: current,
                        registry_generation: snapshot.generation,
                    });
                }
                return Err(UseError::new(
                    "use.extension.lifecycle_generation_retirement_required",
                    "A different cognitive-package generation is retained and must be retired before replacement.",
                ));
            }
            return Err(UseError::new(
                "use.extension.lifecycle_legacy_conflict",
                "A legacy extension receipt already owns this cognitive package ID.",
            ));
        }

        let installed = self.list().await?;
        if let Some(conflict) = installed.iter().find(|extension| {
            extension.receipt.package_id != identity.package_id
                && extension.receipt.route == candidate.manifest.route
        }) {
            return Err(UseError::new(
                "use.extension.route_conflict",
                format!(
                    "Route '{}' is already owned by extension '{}'.",
                    candidate.manifest.route, conflict.receipt.package_id
                ),
            ));
        }

        validate_candidate_source(candidate).await?;
        let target = self.lifecycle_package_root(identity);
        let target_created = commit_candidate_root(candidate, &target).await?;
        let receipt = ExtensionReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION_V3,
            package_id: identity.package_id.clone(),
            component_id: format!("use/{}", identity.package_id),
            route: candidate.manifest.route.clone(),
            version: candidate.manifest.version.clone(),
            package_root: target.clone(),
            manifest_sha256: identity.manifest_sha256().to_string(),
            package_sha256: Some(identity.package_sha256().to_string()),
            trust: candidate.trust,
            registry: candidate.registry.clone(),
            verified_catalog: candidate.verified_catalog.clone(),
            installed_at_unix: unix_timestamp(),
            enabled: false,
            lifecycle_generation: Some(identity.generation),
        };
        let receipt_path = self.paths.receipt_path(identity.package_id());
        if let Err(error) = write_receipt(&receipt_path, &receipt).await {
            let committed = self
                .get(identity.package_id())
                .await
                .ok()
                .flatten()
                .is_some_and(|extension| extension.receipt == receipt);
            if !committed {
                if target_created {
                    let _ = remove_exact_root(&target).await;
                }
                return Err(error);
            }
        }

        let current = self.list().await?;
        let snapshot = self.publish_snapshot_locked(&current).await?;
        Ok(ExtensionLifecycleResult {
            changed: true,
            extension: InstalledExtension {
                receipt,
                manifest: candidate.manifest.clone(),
            },
            registry_generation: snapshot.generation,
        })
    }

    pub async fn publish_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, true, env!("CARGO_PKG_VERSION"))
            .await
    }

    pub async fn hide_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, false, env!("CARGO_PKG_VERSION"))
            .await
    }

    pub async fn drain_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        timeout: Duration,
    ) -> UseResult<ExtensionLifecycleResult> {
        crate::route_lock::deadline_after(timeout)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let extension = self.exact_lifecycle_extension(identity).await?;
        if extension.receipt.enabled {
            return Err(lifecycle_state_error(
                "The cognitive package must be hidden before accepted calls can drain.",
            ));
        }
        let installed = self.list().await?;
        let snapshot = self.publish_snapshot_locked(&installed).await?;
        let _drain = crate::route_lock::acquire_drain_lock(
            &self.paths.package_lock_path(identity.package_id()),
            timeout,
        )
        .await?;
        Ok(ExtensionLifecycleResult {
            changed: false,
            extension,
            registry_generation: snapshot.generation,
        })
    }

    pub async fn remove_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        timeout: Duration,
    ) -> UseResult<UninstallResult> {
        crate::route_lock::deadline_after(timeout)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let target = self.lifecycle_package_root(identity);
        let Some(extension) = self.get(identity.package_id()).await? else {
            let installed = self.list().await?;
            self.publish_snapshot_locked(&installed).await?;
            let _drain = crate::route_lock::acquire_drain_lock(
                &self.paths.package_lock_path(identity.package_id()),
                timeout,
            )
            .await?;
            let changed = remove_exact_root(&target).await?;
            return Ok(UninstallResult {
                package_id: identity.package_id.clone(),
                changed,
            });
        };
        exact_receipt(identity, &extension.receipt)?;
        verify_package_integrity(&extension).await?;
        if extension.receipt.enabled {
            return Err(lifecycle_state_error(
                "The cognitive package must be hidden before its immutable generation is removed.",
            ));
        }
        let _drain = crate::route_lock::acquire_drain_lock(
            &self.paths.package_lock_path(identity.package_id()),
            timeout,
        )
        .await?;
        let receipt_path = self.paths.receipt_path(identity.package_id());
        fs::remove_file(&receipt_path)
            .await
            .map_err(|error| io_error("remove lifecycle package receipt", &receipt_path, error))?;
        let installed = self.list().await?;
        self.publish_snapshot_locked(&installed).await?;
        remove_exact_root(&target).await?;
        Ok(UninstallResult {
            package_id: identity.package_id.clone(),
            changed: true,
        })
    }

    async fn set_lifecycle_visibility(
        &self,
        identity: &ExtensionLifecycleIdentity,
        enabled: bool,
        host_version: &str,
    ) -> UseResult<ExtensionLifecycleResult> {
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let mut extension = self.exact_lifecycle_extension(identity).await?;
        if enabled && !extension.supports_use_version(host_version) {
            return Err(UseError::new(
                "use.extension.host_incompatible",
                format!(
                    "Cognitive package '{}' is not compatible with this A3S Use host.",
                    identity.package_id
                ),
            ));
        }
        let changed = extension.receipt.enabled != enabled;
        if changed {
            extension.receipt.enabled = enabled;
            write_receipt(
                &self.paths.receipt_path(identity.package_id()),
                &extension.receipt,
            )
            .await?;
        }
        let installed = self.list().await?;
        let snapshot = self.publish_snapshot_locked(&installed).await?;
        Ok(ExtensionLifecycleResult {
            changed,
            extension,
            registry_generation: snapshot.generation,
        })
    }

    #[cfg(test)]
    pub(crate) async fn publish_lifecycle_package_for_host_version(
        &self,
        identity: &ExtensionLifecycleIdentity,
        host_version: &str,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, true, host_version)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn acquire_lifecycle_route_for_host_version(
        &self,
        route: &str,
        host_version: &str,
    ) -> UseResult<Option<super::ExtensionRouteLease>> {
        let Some(candidate) = self
            .find_route_for_host_version(route, host_version)
            .await?
        else {
            return Ok(None);
        };
        self.acquire_extension_lease_for_host_version(candidate, Some(route), host_version)
            .await
    }

    async fn exact_lifecycle_extension(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<InstalledExtension> {
        let extension = self.get(identity.package_id()).await?.ok_or_else(|| {
            UseError::new(
                "use.extension.not_installed",
                format!(
                    "Cognitive package '{}' is not installed.",
                    identity.package_id
                ),
            )
        })?;
        exact_receipt(identity, &extension.receipt)?;
        verify_package_integrity(&extension).await?;
        Ok(extension)
    }
}

fn lifecycle_root(paths: &ExtensionPaths, identity: &ExtensionLifecycleIdentity) -> PathBuf {
    paths.lifecycle_package_root(
        identity.package_id(),
        identity.generation(),
        identity.package_sha256(),
    )
}

fn exact_receipt(
    identity: &ExtensionLifecycleIdentity,
    receipt: &ExtensionReceipt,
) -> UseResult<()> {
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION_V3
        || receipt.package_id != identity.package_id
        || receipt.lifecycle_generation != Some(identity.generation)
        || receipt.package_sha256.as_deref() != Some(identity.package_sha256())
        || receipt.manifest_sha256 != identity.manifest_sha256()
    {
        return Err(lifecycle_identity_error(
            "The installed cognitive package does not match the exact lifecycle generation.",
        ));
    }
    Ok(())
}

async fn validate_candidate_source(candidate: &ExtensionLifecyclePackage) -> UseResult<()> {
    let (manifest, manifest_bytes) = read_manifest(candidate.source.root()).await?;
    validate_surface_files(&manifest, candidate.source.root()).await?;
    let package_sha256 = crate::digest::package_sha256(candidate.source.root()).await?;
    if manifest != candidate.manifest
        || format!("sha256:{}", sha256(&manifest_bytes)) != candidate.manifest_digest
        || format!("sha256:{package_sha256}") != candidate.package_digest
    {
        return Err(UseError::new(
            "use.extension.package_changed",
            "The cognitive package changed after lifecycle preparation.",
        ));
    }
    Ok(())
}

async fn commit_candidate_root(
    candidate: &ExtensionLifecyclePackage,
    target: &Path,
) -> UseResult<bool> {
    match fs::symlink_metadata(target).await {
        Ok(_) => {
            validate_committed_root(candidate, target).await?;
            return Ok(false);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect lifecycle package", target, error)),
    }
    let parent = target.parent().ok_or_else(|| {
        lifecycle_state_error("The lifecycle package root has no owned parent directory.")
    })?;
    fs::create_dir_all(parent)
        .await
        .map_err(|error| io_error("create lifecycle package directory", parent, error))?;
    let staging = tempfile::Builder::new()
        .prefix(".lifecycle-staging-")
        .tempdir_in(parent)
        .map_err(|error| io_error("create lifecycle package staging", parent, error))?;
    copy_package(candidate.source.root(), staging.path()).await?;
    validate_committed_root(candidate, staging.path()).await?;
    let staging = staging.keep();
    if let Err(error) = fs::rename(&staging, target).await {
        let _ = fs::remove_dir_all(&staging).await;
        return Err(io_error(
            "commit lifecycle package generation",
            target,
            error,
        ));
    }
    Ok(true)
}

async fn validate_committed_root(
    candidate: &ExtensionLifecyclePackage,
    root: &Path,
) -> UseResult<()> {
    let metadata = fs::symlink_metadata(root)
        .await
        .map_err(|error| io_error("inspect lifecycle package", root, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.ownership_invalid",
            "The lifecycle package root must be an owned directory.",
        ));
    }
    let (manifest, manifest_bytes) = read_manifest(root).await?;
    validate_surface_files(&manifest, root).await?;
    let package_sha256 = crate::digest::package_sha256(root).await?;
    if manifest != candidate.manifest
        || format!("sha256:{}", sha256(&manifest_bytes)) != candidate.manifest_digest
        || format!("sha256:{package_sha256}") != candidate.package_digest
    {
        return Err(UseError::new(
            "use.extension.package_changed",
            "The committed lifecycle package does not match its prepared bytes.",
        ));
    }
    Ok(())
}

async fn remove_exact_root(path: &Path) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error("inspect lifecycle package", path, error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.ownership_invalid",
            format!(
                "Refusing to remove invalid lifecycle package root '{}'.",
                path.display()
            ),
        ));
    }
    fs::remove_dir_all(path)
        .await
        .map_err(|error| io_error("remove lifecycle package generation", path, error))?;
    Ok(true)
}

fn validate_provenance(
    trust: ExtensionTrust,
    registry: Option<&ResolvedRemotePackage>,
    verified_catalog: Option<&VerifiedPluginCatalogRecord>,
) -> UseResult<()> {
    match (trust, registry, verified_catalog) {
        (ExtensionTrust::LocalExplicit | ExtensionTrust::ReleaseBundle, None, None) => Ok(()),
        (ExtensionTrust::RegistryTuf, Some(registry), catalog) => {
            registry.validate_provenance()?;
            if catalog.is_some_and(|catalog| !catalog.record.is_package_plan_ready()) {
                return Err(UseError::new(
                    "use.extension.trust_invalid",
                    "Lifecycle registry evidence is not package-plan ready.",
                ));
            }
            Ok(())
        }
        _ => Err(UseError::new(
            "use.extension.trust_invalid",
            "Cognitive-package lifecycle provenance is internally inconsistent.",
        )),
    }
}

fn canonical_sha256(value: String, label: &str) -> UseResult<String> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    if !valid {
        return Err(lifecycle_identity_error(format!(
            "The lifecycle {label} digest must be canonical SHA-256."
        )));
    }
    Ok(value)
}

fn lifecycle_identity_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.lifecycle_identity_mismatch", message)
}

fn lifecycle_state_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.lifecycle_state_invalid", message)
}
