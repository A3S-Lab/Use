use std::path::Path;

use a3s_use_core::{PluginPlanningBundle, UseError, UseResult, VerifiedPluginCatalogRecord};
use tokio::fs;

use super::staging::{prepare_lifecycle_package_parent, LIFECYCLE_STAGING_PREFIX};
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
use crate::surface_files::validate_planning_bundle_package_binding;
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
        let verified_catalog = downloaded.verified_catalog().clone();
        let planning_bundle = downloaded.planning_bundle().cloned();
        let source = prepare_package_source(downloaded.path()).await?;
        Self::prepare(
            expected_package_id,
            source,
            ExtensionTrust::RegistryTuf,
            Some(registry),
            Some(verified_catalog),
            planning_bundle,
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
        planning_bundle: Option<PluginPlanningBundle>,
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
        match (verified_catalog.as_ref(), planning_bundle.as_ref()) {
            (Some(catalog), Some(bundle)) if catalog.record.planning.is_some() => {
                bundle.validate_catalog_binding(catalog)?;
                validate_planning_bundle_package_binding(bundle, &manifest, source.root()).await?;
            }
            (Some(catalog), None) if catalog.record.planning.is_none() => {}
            (Some(_), _) => {
                return Err(UseError::new(
                    "use.extension.planning_package_mismatch",
                    "The downloaded package and its signed catalog disagree about executable planning evidence.",
                ));
            }
            (None, None) => {}
            (None, Some(_)) => {
                return Err(UseError::new(
                    "use.extension.planning_package_mismatch",
                    "Unsigned package state cannot carry Registry planning evidence.",
                ));
            }
        }
        Ok(Self {
            source,
            manifest,
            package_digest: format!("sha256:{package_sha256}"),
            manifest_digest: format!("sha256:{}", sha256(&manifest_bytes)),
            trust,
            registry,
            verified_catalog,
            planning_bundle,
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
            && receipt.planning_bundle == self.planning_bundle
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
    data_root: &Path,
) -> UseResult<bool> {
    let parent = target.parent().ok_or_else(|| {
        lifecycle_state_error("The lifecycle package root has no owned parent directory.")
    })?;
    prepare_lifecycle_package_parent(data_root, parent).await?;
    match fs::symlink_metadata(target).await {
        Ok(_) => {
            validate_committed_root(candidate, target).await?;
            return Ok(false);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect lifecycle package", target, error)),
    }
    let staging = tempfile::Builder::new()
        .prefix(LIFECYCLE_STAGING_PREFIX)
        .tempdir_in(parent)
        .map_err(|error| io_error("create lifecycle package staging", parent, error))?;
    copy_package(candidate.source.root(), staging.path()).await?;
    validate_committed_root(candidate, staging.path()).await?;
    let staging = staging.keep();
    let rename_source = staging.clone();
    let rename_target = target.to_path_buf();
    let renamed = tokio::task::spawn_blocking(move || {
        crate::rename_path_with_windows_retry_blocking(&rename_source, &rename_target)
    })
    .await
    .map_err(|error| {
        lifecycle_state_error(format!(
            "Lifecycle package commit worker did not complete: {error}"
        ))
    })?;
    if let Err(error) = renamed {
        let _ = crate::package::remove_dir_all_with_windows_retry(
            staging,
            "remove failed lifecycle package staging",
        )
        .await;
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
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
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
        (ExtensionTrust::RegistryTuf, Some(registry), Some(catalog)) => {
            registry.validate_provenance()?;
            if !catalog.record.is_package_plan_ready() {
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
