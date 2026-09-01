use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{
    OkfBundleContract, OkfBundleFile, PluginCatalogRecord, UseError, UseResult,
    VerifiedPluginCatalogRecord,
};
use fs2::FileExt;

use super::{
    artifact_store_error, open_existing_lock_file, ArtifactStore, MUTATION_LOCK, REACHABILITY_LOCK,
};
use crate::digest::package_fingerprint;
use crate::package::{lock_is_contended, read_manifest, sha256, validate_surface_files};
use crate::registry::validate_catalog_manifest_binding;
use crate::surface_files::{
    inspect_skill_surface_file, inspect_ui_surface_files, load_okf_bundle_files,
    PluginSurfaceFileEvidence,
};
use crate::ExtensionManifest;

const READ_LOCK_WAIT: Duration = Duration::from_secs(2);
const READ_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// A verified, read-only lease over one exact content-addressed package.
///
/// The lease holds the global reachability boundary and the package mutation
/// lock in shared mode. Coordinated collection, quarantine, rehydration, and
/// package publication therefore cannot race the read. Local paths remain
/// private; owner adapters receive only the exact catalog-bound manifest and
/// path-free identity. Call [`Self::verify_unchanged`] after any bounded read
/// or launch handshake to detect uncoordinated filesystem tampering.
#[must_use = "dropping the package lease allows artifact mutation and collection to resume"]
pub struct VerifiedArtifactPackage {
    _lease: ArtifactPackageReadLease,
    store: ArtifactStore,
    root: PathBuf,
    catalog: PluginCatalogRecord,
    manifest: ExtensionManifest,
}

/// Path-free immutable bytes for one exact catalog-bound OKF surface.
///
/// The payload is created only while a [`VerifiedArtifactPackage`] lease is
/// held. Its bundle contract and byte snapshot can cross the Knowledge adapter
/// boundary without granting access to the package filesystem.
#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedOkfSurfacePayload {
    surface_id: String,
    bundle: OkfBundleContract,
    files: Vec<OkfBundleFile>,
}

impl VerifiedOkfSurfacePayload {
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    pub fn bundle(&self) -> &OkfBundleContract {
        &self.bundle
    }

    pub fn files(&self) -> &[OkfBundleFile] {
        &self.files
    }

    pub fn into_parts(self) -> (OkfBundleContract, Vec<OkfBundleFile>) {
        (self.bundle, self.files)
    }
}

impl fmt::Debug for VerifiedArtifactPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedArtifactPackage")
            .field("package_id", &self.package_id())
            .field("version", &self.version())
            .field("package_digest", &self.package_digest())
            .field("manifest_digest", &self.manifest_digest())
            .field("expanded_bytes", &self.expanded_bytes())
            .field("file_count", &self.file_count())
            .finish_non_exhaustive()
    }
}

impl VerifiedArtifactPackage {
    pub fn package_id(&self) -> &str {
        &self.catalog.package_id
    }

    pub fn version(&self) -> &str {
        &self.catalog.version
    }

    pub fn package_digest(&self) -> &str {
        self.catalog.package.sha256.as_deref().unwrap_or_default()
    }

    pub fn manifest_digest(&self) -> &str {
        self.catalog
            .package
            .manifest_sha256
            .as_deref()
            .unwrap_or_default()
    }

    pub fn expanded_bytes(&self) -> u64 {
        self.catalog.package.expanded_bytes
    }

    pub fn file_count(&self) -> u64 {
        self.catalog.package.file_count
    }

    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    /// Inspect one exact immutable Skill contribution while the verified
    /// package lease remains held.
    ///
    /// The package root never crosses this boundary. The complete package is
    /// reverified after the bounded surface read, so the returned digest is
    /// not based on a stale acquisition-time check.
    pub async fn inspect_skill_surface(
        &self,
        surface_id: &str,
    ) -> UseResult<PluginSurfaceFileEvidence> {
        let surface = self
            .manifest
            .skills
            .iter()
            .find(|surface| surface.id == surface_id)
            .ok_or_else(surface_missing)?;
        let evidence = inspect_skill_surface_file(surface, &self.root).await?;
        self.verify_unchanged().await?;
        Ok(evidence)
    }

    /// Inspect one exact immutable UI contribution while the verified
    /// package lease remains held.
    ///
    /// The returned evidence binds the entry point and every declared style
    /// and script. No local path is exposed to the caller.
    pub async fn inspect_ui_surface(
        &self,
        surface_id: &str,
    ) -> UseResult<PluginSurfaceFileEvidence> {
        let surface = self
            .manifest
            .ui
            .iter()
            .find(|surface| surface.id == surface_id)
            .ok_or_else(surface_missing)?;
        let evidence = inspect_ui_surface_files(surface, &self.root).await?;
        self.verify_unchanged().await?;
        Ok(evidence)
    }

    /// Read one exact immutable OKF contribution without exposing its package
    /// path to the effect owner or Knowledge adapter.
    pub async fn read_okf_surface(&self, surface_id: &str) -> UseResult<VerifiedOkfSurfacePayload> {
        let surface = self
            .manifest
            .okf
            .iter()
            .find(|surface| surface.id == surface_id)
            .ok_or_else(surface_missing)?;
        let files = load_okf_bundle_files(surface, &self.root).await?;
        self.verify_unchanged().await?;
        Ok(VerifiedOkfSurfacePayload {
            surface_id: surface.id.clone(),
            bundle: surface.bundle.clone(),
            files,
        })
    }

    /// Recompute the complete package identity while both coordinated read
    /// locks remain held. This detects uncoordinated local tampering without
    /// turning a prior verification into permanent authority.
    pub async fn verify_unchanged(&self) -> UseResult<()> {
        let sha256 = package_sha256(&self.catalog)?;
        self.store
            .validate_expanded_package_path(sha256, &self.root)
            .await?;
        let observed = verify_package(&self.root, &self.catalog).await?;
        if observed != self.manifest {
            return Err(package_mismatch(
                "The artifact manifest changed after its verified lease was acquired.",
            ));
        }
        Ok(())
    }
}

impl ArtifactStore {
    /// Acquire and fully verify one exact package against a complete verified
    /// catalog record. Selection authority remains outside the Artifact Store;
    /// effect owners must supply the catalog from committed Control context.
    /// Resolving an expanded-package path alone establishes no authority.
    pub async fn acquire_verified_package(
        &self,
        catalog: &VerifiedPluginCatalogRecord,
    ) -> UseResult<VerifiedArtifactPackage> {
        catalog.validate().map_err(|error| {
            artifact_store_error(
                "use.artifact_store.catalog_invalid",
                format!(
                    "Verified package catalog authority is invalid: {}",
                    error.message
                ),
            )
        })?;
        if !catalog.record.is_package_plan_ready() {
            return Err(artifact_store_error(
                "use.artifact_store.catalog_invalid",
                "Artifact reads require complete plan-ready catalog authority.",
            ));
        }
        let sha256 = package_sha256(&catalog.record)?;
        let root = self.expanded_package_path_from_sha256(sha256);
        let lease = ArtifactPackageReadLease::acquire(self, sha256, &root).await?;
        let manifest = verify_package(&root, &catalog.record).await?;
        self.validate_expanded_package_path(sha256, &root).await?;
        Ok(VerifiedArtifactPackage {
            _lease: lease,
            store: self.clone(),
            root,
            catalog: catalog.record.clone(),
            manifest,
        })
    }
}

struct ArtifactPackageReadLease {
    reachability: File,
    mutation: File,
}

impl ArtifactPackageReadLease {
    async fn acquire(store: &ArtifactStore, sha256: &str, root: &Path) -> UseResult<Self> {
        let deadline = tokio::time::Instant::now() + READ_LOCK_WAIT;
        let reachability = acquire_shared_lock(
            &store.root().join(REACHABILITY_LOCK),
            "global artifact reachability",
            deadline,
        )
        .await?;
        super::garbage_collection::ensure_reference_admission_allowed(store.root()).await?;
        store.validate_expanded_package_path(sha256, root).await?;
        let container = root.parent().ok_or_else(|| {
            artifact_store_error(
                "use.artifact_store.ownership_invalid",
                "An expanded package has no owned Artifact Store container.",
            )
        })?;
        let mutation = acquire_shared_lock(
            &container.join(MUTATION_LOCK),
            "expanded-package artifact mutation",
            deadline,
        )
        .await?;
        store.validate_expanded_package_path(sha256, root).await?;
        Ok(Self {
            reachability,
            mutation,
        })
    }
}

impl Drop for ArtifactPackageReadLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.mutation);
        let _ = FileExt::unlock(&self.reachability);
    }
}

async fn acquire_shared_lock(
    path: &Path,
    label: &str,
    deadline: tokio::time::Instant,
) -> UseResult<File> {
    let file = open_existing_lock_file(path, label).map_err(|error| {
        if error.code == "use.extension.io" {
            artifact_store_error(
                "use.artifact_store.content_missing",
                format!("The required {label} is missing or unreadable."),
            )
        } else {
            error
        }
    })?;
    loop {
        match FileExt::try_lock_shared(&file) {
            Ok(()) => return Ok(file),
            Err(error) if lock_is_contended(&error) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(artifact_store_error(
                        "use.artifact_store.busy",
                        format!("Another process owns the {label} boundary."),
                    ));
                }
                tokio::time::sleep(
                    READ_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(error) => {
                return Err(UseError::new(
                    "use.artifact_store.io",
                    format!("Failed to acquire {label}: {error}"),
                ))
            }
        }
    }
}

async fn verify_package(
    root: &Path,
    catalog: &PluginCatalogRecord,
) -> UseResult<ExtensionManifest> {
    let (manifest, manifest_bytes) = read_manifest(root).await?;
    validate_catalog_manifest_binding(catalog, &manifest).map_err(|error| {
        package_mismatch(format!(
            "The artifact manifest differs from committed catalog authority: {}",
            error.message
        ))
    })?;
    validate_surface_files(&manifest, root).await?;
    let fingerprint = package_fingerprint(root).await?;
    let actual_package_digest = format!("sha256:{}", fingerprint.sha256);
    let actual_manifest_digest = format!("sha256:{}", sha256(&manifest_bytes));
    if catalog.package.sha256.as_deref() != Some(actual_package_digest.as_str())
        || catalog.package.manifest_sha256.as_deref() != Some(actual_manifest_digest.as_str())
        || catalog.package.expanded_bytes != fingerprint.byte_count
        || catalog.package.file_count != fingerprint.file_count
    {
        return Err(package_mismatch(
            "The expanded package differs from its catalog digest or exact measurements.",
        ));
    }
    Ok(manifest)
}

fn package_sha256(catalog: &PluginCatalogRecord) -> UseResult<&str> {
    let digest = catalog.package.sha256.as_deref().ok_or_else(|| {
        artifact_store_error(
            "use.artifact_store.catalog_invalid",
            "Verified package catalog authority omitted its package digest.",
        )
    })?;
    digest.strip_prefix("sha256:").ok_or_else(|| {
        artifact_store_error(
            "use.artifact_store.catalog_invalid",
            "Verified package catalog authority has a non-canonical package digest.",
        )
    })
}

fn package_mismatch(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.package_mismatch", message)
}

fn surface_missing() -> UseError {
    artifact_store_error(
        "use.artifact_store.surface_missing",
        "The requested static surface is absent from the verified package manifest.",
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VerifiedArtifactPackage>();
    assert_send_sync::<VerifiedOkfSurfacePayload>();
};
