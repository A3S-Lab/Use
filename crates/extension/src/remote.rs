//! TUF-backed remote extension registry resolution.
//!
//! The trusted root is pinned out of band by SHA-256. Tough then verifies the
//! complete root/timestamp/snapshot/targets chain, enforces expiration, and
//! persists metadata versions in its datastore to reject rollback attacks.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord};
use fs2::FileExt;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tough::schema::{Root, Signed};
use tough::{ExpirationEnforcement, Limits, Repository};
use tough::{RepositoryLoader, TargetName};
use url::Url;

use super::package::{activate_temporary_file, io_error, sync_parent_directory, unique_suffix};
use crate::ArtifactStore;

mod cache_policy;
mod catalog;
mod description_trust;
mod download;
mod network;
mod package_graph;
mod presentation;
mod resumable_http;
mod target;
mod target_cache;
mod target_cache_inventory;

pub use cache_policy::{
    VerifiedTargetCachePolicy, VerifiedTargetCachePruneResult, VerifiedTargetCacheUsage,
    DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES, DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES,
    DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES, VERIFIED_TARGET_CACHE_SCHEMA_VERSION,
};
pub use catalog::{
    inspect_cached_plugin, inspect_remote_plugin, list_remote_packages,
    plugin_catalog_host_input_schema, plugin_catalog_inspection_input_schema,
    plugin_catalog_search_input_schema, search_cached_plugins, search_remote_plugins,
    PluginCatalogAvailability, PluginCatalogHost, PluginCatalogInspection, PluginCatalogPage,
    PluginCatalogSearch, PluginCatalogSnapshot, PluginCatalogSnapshotSource,
    VerifiedRegistryCatalog, VerifiedRegistryMetadata, MAX_PLUGIN_CATALOG_PAGE_BYTES,
    MAX_PLUGIN_CATALOG_PAGE_SIZE,
};
pub use description_trust::{
    description_trust_store_custom, load_cached_capability_description_trust_store,
    load_capability_description_trust_store, VerifiedCapabilityDescriptionTrustStore,
    CAPABILITY_DESCRIPTION_TRUST_STORE_TARGET,
};
pub use download::{DownloadedRemotePackage, PreparedRemotePackage};
pub use network::RegistryNetworkPolicy;
pub use package_graph::{
    download_locked_cached_remote_packages, download_locked_remote_packages,
    download_selected_locked_cached_remote_packages, download_selected_locked_remote_packages,
    resolve_cached_remote_package_lock, resolve_cached_remote_package_lock_with_observer,
    resolve_remote_package_lock, resolve_remote_package_lock_with_observer,
    PackageRegistryResolutionObserver,
};
pub use presentation::{
    fetch_cached_cognitive_package_media, fetch_cognitive_package_media,
    inspect_cached_cognitive_package_presentation, inspect_cognitive_package_presentation,
    CognitivePackageFormFactor, CognitivePackageMediaKind, CognitivePackagePresentationIndexV1,
    CognitivePackagePresentationMediaV1, CognitivePackagePresentationRecordV1,
    CognitivePackagePresentationV1, VerifiedCognitivePackageMedia,
    VerifiedCognitivePackagePresentation, COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA,
    COGNITIVE_PACKAGE_PRESENTATION_SCHEMA, MAX_COGNITIVE_PACKAGE_MEDIA_BYTES,
    MAX_COGNITIVE_PACKAGE_PRESENTATION_MEDIA,
};
use target::{
    decode_registry_target_metadata, resolved_remote_package, validate_target_metadata,
    validate_target_name, RegistryTargetMetadata,
};
pub use target_cache::{VerifiedTargetObservation, VerifiedTargetObservationStatus};

const ROOT_NAME: &str = "root.json";
const ROOT_CACHE_NAME: &str = "bootstrap-root.json";
const REGISTRY_METADATA_KEY: &str = "a3s";
pub const MAX_BOOTSTRAP_ROOT_BYTES: u64 = 1024 * 1024;
const MAX_REMOTE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REGISTRY_PACKAGE_TARGETS: u64 = 10_000;
const MAX_ROOT_UPDATES: u64 = 64;

/// One configured registry whose TUF root is pinned out of band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedRegistry {
    name: String,
    base_url: Url,
    root_sha256: String,
    trusted_root_path: Option<PathBuf>,
    datastore: PathBuf,
    artifact_store: ArtifactStore,
    target_cache_policy: VerifiedTargetCachePolicy,
    network_policy: RegistryNetworkPolicy,
}

/// Exact evidence decoded from caller-pinned bootstrap-root bytes.
///
/// This identifies the out-of-band trust anchor only. It is not a verified
/// Registry snapshot; callers must still perform the ordinary TUF refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedBootstrapRoot {
    pub root_sha256: String,
    pub root_version: u64,
    pub size_bytes: u64,
}

/// Inspect caller-supplied bootstrap-root bytes without creating Registry
/// state or performing network I/O.
///
/// The result identifies only the out-of-band trust anchor. It does not verify
/// signatures, expiry, rollback state, or any later TUF metadata. A
/// `TrustedRegistry` refresh remains required before catalog evidence is
/// trusted.
pub fn inspect_bootstrap_root(bytes: &[u8]) -> UseResult<PinnedBootstrapRoot> {
    let (root_sha256, size_bytes) = bootstrap_root_identity(bytes)?;
    decode_bootstrap_root(bytes, root_sha256, size_bytes)
}

impl TrustedRegistry {
    /// Construct one trusted Registry source with an explicit shared Artifact Store.
    ///
    /// Obtain the store from the process-wide `UsePaths`. Registry metadata,
    /// source observations, and resumable partials remain in `datastore`; fully
    /// verified target bytes are committed to the injected global store.
    pub fn new(
        name: impl Into<String>,
        base_url: impl AsRef<str>,
        root_sha256: impl AsRef<str>,
        trusted_root_path: Option<PathBuf>,
        datastore: PathBuf,
        artifact_store: ArtifactStore,
    ) -> UseResult<Self> {
        let name = name.into();
        validate_registry_name(&name)?;
        let base_url = normalize_registry_url(base_url.as_ref())?;
        let root_sha256 = normalize_sha256(root_sha256.as_ref(), "registry trust root")?;
        if !datastore.is_absolute() {
            return Err(UseError::new(
                "use.extension.registry_path_invalid",
                "The TUF metadata datastore must be an absolute path.",
            ));
        }
        if !artifact_store.root().is_absolute() {
            return Err(UseError::new(
                "use.artifact_store.path_invalid",
                "The global Artifact Store path must be absolute.",
            ));
        }
        if trusted_root_path
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(UseError::new(
                "use.extension.registry_path_invalid",
                "The trusted TUF root path must be absolute.",
            ));
        }
        Ok(Self {
            name,
            base_url,
            root_sha256,
            trusted_root_path,
            datastore,
            artifact_store,
            target_cache_policy: VerifiedTargetCachePolicy::default(),
            network_policy: RegistryNetworkPolicy::default(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn root_sha256(&self) -> &str {
        &self.root_sha256
    }

    /// Stable digest of the complete replaceable Registry source identity.
    ///
    /// The digest binds name, canonical URL, and trust root without requiring
    /// diagnostics to expose the URL itself.
    pub fn source_identity(&self) -> String {
        registry_source_identity(&self.name, self.base_url.as_str(), &self.root_sha256)
    }

    /// Returns whether this trust configuration is the exact source recorded
    /// in verified catalog provenance.
    pub fn matches_provenance(&self, provenance: &VerifiedCatalogProvenance) -> bool {
        let provenance_root = provenance
            .root_sha256
            .strip_prefix("sha256:")
            .unwrap_or(&provenance.root_sha256);
        self.name == provenance.registry_name
            && self.base_url.as_str() == provenance.registry_url
            && self.root_sha256 == provenance_root
    }

    pub fn datastore(&self) -> &Path {
        &self.datastore
    }

    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.artifact_store
    }

    pub const fn target_cache_policy(&self) -> VerifiedTargetCachePolicy {
        self.target_cache_policy
    }

    pub fn with_target_cache_policy(mut self, policy: VerifiedTargetCachePolicy) -> Self {
        self.target_cache_policy = policy;
        self
    }

    pub const fn network_policy(&self) -> RegistryNetworkPolicy {
        self.network_policy
    }

    pub fn with_network_policy(mut self, policy: RegistryNetworkPolicy) -> Self {
        self.network_policy = policy;
        self
    }

    /// Pin caller-supplied bootstrap root bytes in this Registry's metadata
    /// datastore. The bytes must match the configured SHA-256 exactly and are
    /// immutable once admitted. A subsequent refresh still performs the full
    /// TUF chain, expiration, and rollback verification.
    pub async fn pin_trusted_root(&self, bytes: &[u8]) -> UseResult<PinnedBootstrapRoot> {
        if self.trusted_root_path.is_some() {
            return Err(UseError::new(
                "use.extension.registry_path_invalid",
                "A Registry with an explicit trusted-root path cannot pin separate root bytes.",
            ));
        }
        let evidence = pinned_bootstrap_root(self, bytes)?;
        ensure_metadata_directory(&self.datastore).await?;
        let _lock = acquire_metadata_lock(&self.datastore)?;
        let cache = self.datastore.join(ROOT_CACHE_NAME);
        match read_trusted_root_file(&cache).await? {
            Some(existing) => {
                if existing == bytes {
                    Ok(evidence)
                } else {
                    Err(UseError::new(
                        "use.extension.registry_root_conflict",
                        "The Registry metadata store already contains different bootstrap root bytes.",
                    ))
                }
            }
            None => {
                write_bootstrap_root(&cache, bytes).await?;
                Ok(evidence)
            }
        }
    }

    fn metadata_url(&self) -> UseResult<Url> {
        self.base_url.join("metadata/").map_err(|error| {
            UseError::new(
                "use.extension.registry_url_invalid",
                format!("Failed to resolve the registry metadata URL: {error}"),
            )
        })
    }

    pub(super) fn targets_url(&self) -> UseResult<Url> {
        self.base_url.join("targets/").map_err(|error| {
            UseError::new(
                "use.extension.registry_url_invalid",
                format!("Failed to resolve the registry targets URL: {error}"),
            )
        })
    }
}

pub(crate) fn registry_source_identity(
    name: &str,
    registry_url: &str,
    root_sha256: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.use.registry-source.v1\0");
    for value in [name, registry_url, root_sha256] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Exact signed target selected from a verified TUF repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRemotePackage {
    pub registry_name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub root_version: u64,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub package_id: String,
    pub version: String,
    pub channel: String,
    pub target: String,
    pub target_name: String,
    pub archive_name: String,
    pub length: u64,
    pub sha256: String,
}

impl ResolvedRemotePackage {
    /// Adapt a complete verified catalog record into exact target resolution
    /// consumed by the package planner and archive verifier.
    ///
    /// This is a metadata-only conversion. It preserves the same registry,
    /// TUF role, target, and digest evidence without fetching the archive.
    pub fn from_verified_catalog(plugin: &VerifiedPluginCatalogRecord) -> UseResult<Self> {
        plugin.validate()?;
        let record = &plugin.record;
        let provenance = &plugin.provenance;
        let archive_name = record
            .archive
            .target_name
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let resolved = Self {
            registry_name: provenance.registry_name.clone(),
            registry_url: provenance.registry_url.clone(),
            root_sha256: normalize_sha256(&provenance.root_sha256, "registry trust root")?,
            root_version: provenance.root_version,
            timestamp_version: provenance.timestamp_version,
            snapshot_version: provenance.snapshot_version,
            targets_version: provenance.targets_version,
            package_id: record.package_id.clone(),
            version: record.version.clone(),
            channel: record.channel.as_str().to_owned(),
            target: record.target.clone(),
            target_name: record.archive.target_name.clone(),
            archive_name,
            length: record.archive.length,
            sha256: normalize_sha256(&record.archive.sha256, "registry target")?,
        };
        resolved.validate_provenance()?;
        Ok(resolved)
    }

    pub fn plan_digest(&self) -> UseResult<String> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            UseError::new(
                "use.extension.registry_plan_invalid",
                format!("Failed to encode the resolved registry plan: {error}"),
            )
        })?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn verify_expected_plan(&self, expected: Option<&str>) -> UseResult<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let expected = normalize_sha256(expected, "expected registry plan")?;
        let actual = self.plan_digest()?;
        if expected == actual {
            return Ok(());
        }
        Err(UseError::new(
            "use.extension.registry_plan_mismatch",
            "The signed registry target changed after review.",
        )
        .with_detail("expected", expected)
        .with_detail("actual", actual))
    }

    pub(crate) fn validate_provenance(&self) -> UseResult<()> {
        validate_registry_name(&self.registry_name)?;
        let normalized_url = normalize_registry_url(&self.registry_url)?;
        if normalized_url.as_str() != self.registry_url {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                "The registry URL in the extension receipt is not canonical.",
            ));
        }
        normalize_sha256(&self.root_sha256, "registry trust root")?;
        normalize_sha256(&self.sha256, "registry target")?;
        if self.root_version == 0
            || self.timestamp_version == 0
            || self.snapshot_version == 0
            || self.targets_version == 0
            || self.length == 0
            || self.length > MAX_REMOTE_ARCHIVE_BYTES
            || !super::valid_package_id(&self.package_id)
            || Version::parse(&self.version).is_err()
        {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                "The registry provenance in the extension receipt is invalid.",
            ));
        }
        validate_channel(&self.channel)?;
        let host = host_target()?;
        if self.target != host && self.target != "any" {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                "The installed registry target does not match this platform.",
            ));
        }
        let target_name = TargetName::new(self.target_name.clone()).map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!("The registry target name in the receipt is invalid: {error}"),
            )
        })?;
        validate_target_name(&target_name, self)?;
        if target_name.raw().rsplit('/').next() != Some(self.archive_name.as_str()) {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                "The registry archive name does not match its signed target path.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteRegistryAccess {
    Refreshed,
    Cached,
}

struct MetadataLock(File);

impl Drop for MetadataLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// Load and verify a TUF repository, then select one exact extension target.

include!("remote_prepare.rs");

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "remote_resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "remote_catalog_tests.rs"]
mod catalog_tests;
