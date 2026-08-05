use std::path::Path;

use a3s_use_core::{UseError, UseResult, VerifiedPluginCatalogRecord};
use tokio::fs;

use super::{
    lifecycle_identity_error, lifecycle_state_error, ExtensionLifecycleIdentity,
    ExtensionLifecyclePackage,
};
use crate::package::{copy_package, io_error, read_manifest, sha256, validate_surface_files};
use crate::registry::{
    normalize_package_id, validate_catalog_package, ExtensionReceipt, ExtensionTrust,
};
use crate::remote::{DownloadedRemotePackage, ResolvedRemotePackage};
use crate::source::{prepare_package_source, PreparedPackageSource};
use crate::ExtensionManifest;

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
        validate_catalog_package(
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

    pub(super) fn validate_identity(&self, identity: &ExtensionLifecycleIdentity) -> UseResult<()> {
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

    pub(super) fn matches_provenance(&self, receipt: &ExtensionReceipt) -> bool {
        receipt.trust == self.trust
            && receipt.registry == self.registry
            && receipt.verified_catalog == self.verified_catalog
    }
}

pub(super) async fn validate_candidate_source(
    candidate: &ExtensionLifecyclePackage,
) -> UseResult<()> {
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

pub(super) async fn commit_candidate_root(
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
