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

mod cache_policy;
mod catalog;
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
pub use download::{DownloadedRemotePackage, PreparedRemotePackage};
pub use network::RegistryNetworkPolicy;
pub use package_graph::{
    download_locked_cached_remote_packages, download_locked_remote_packages,
    download_selected_locked_cached_remote_packages, download_selected_locked_remote_packages,
    resolve_cached_remote_package_lock, resolve_remote_package_lock,
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
    pub fn new(
        name: impl Into<String>,
        base_url: impl AsRef<str>,
        root_sha256: impl AsRef<str>,
        trusted_root_path: Option<PathBuf>,
        datastore: PathBuf,
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
pub async fn prepare_remote_package(
    registry: &TrustedRegistry,
    package_id: &str,
    requested_version: Option<&str>,
    channel: &str,
    expected_plan_digest: Option<&str>,
) -> UseResult<PreparedRemotePackage> {
    prepare_remote_package_with_access(
        registry,
        package_id,
        requested_version,
        channel,
        expected_plan_digest,
        RemoteRegistryAccess::Refreshed,
    )
    .await
}

/// Select one exact package only from the last verified, unexpired local TUF
/// snapshot. This function never constructs a network transport.
pub async fn prepare_cached_remote_package(
    registry: &TrustedRegistry,
    package_id: &str,
    requested_version: Option<&str>,
    channel: &str,
    expected_plan_digest: Option<&str>,
) -> UseResult<PreparedRemotePackage> {
    prepare_remote_package_with_access(
        registry,
        package_id,
        requested_version,
        channel,
        expected_plan_digest,
        RemoteRegistryAccess::Cached,
    )
    .await
}

async fn prepare_remote_package_with_access(
    registry: &TrustedRegistry,
    package_id: &str,
    requested_version: Option<&str>,
    channel: &str,
    expected_plan_digest: Option<&str>,
    access: RemoteRegistryAccess,
) -> UseResult<PreparedRemotePackage> {
    if !super::valid_package_id(package_id) {
        return Err(UseError::new(
            "use.extension.id_invalid",
            "Extension IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    let requested_version = requested_version
        .map(|version| {
            Version::parse(version).map_err(|error| {
                UseError::new(
                    "use.extension.version_invalid",
                    format!("Invalid requested extension version: {error}"),
                )
            })
        })
        .transpose()?;
    validate_channel(channel)?;
    let repository = match access {
        RemoteRegistryAccess::Refreshed => load_repository(registry).await?,
        RemoteRegistryAccess::Cached => catalog::load_verified_cached_repository(registry).await?,
    };

    let host_target = host_target()?;
    let host_use_version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        UseError::new(
            "use.extension.registry_target_invalid",
            format!("The A3S Use package version is invalid: {error}"),
        )
    })?;
    let mut candidates = Vec::new();
    let mut identities = BTreeSet::new();
    let mut incompatible = false;
    for (target_name, target) in repository.all_targets() {
        let Some(metadata) = target.custom.get(REGISTRY_METADATA_KEY) else {
            continue;
        };
        let metadata = decode_registry_target_metadata(target_name, metadata)?;
        validate_target_metadata(target_name, target, &metadata)?;
        let identity = (
            metadata.package_id().to_owned(),
            metadata.version().to_owned(),
            metadata.channel().to_owned(),
            metadata.target().to_owned(),
        );
        if !identities.insert(identity) {
            return Err(UseError::new(
                "use.extension.registry_target_invalid",
                "The TUF repository contains duplicate A3S package targets.",
            ));
        }
        if metadata.package_id() != package_id || metadata.channel() != channel {
            continue;
        }
        let version = Version::parse(metadata.version()).map_err(|error| {
            UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "TUF target '{}' declares an invalid version: {error}",
                    target_name.raw()
                ),
            )
        })?;
        if requested_version
            .as_ref()
            .is_some_and(|requested| requested != &version)
        {
            continue;
        }
        let target_compatible = metadata.target() == host_target || metadata.target() == "any";
        let use_compatible = VersionReq::parse(&metadata.catalog_record().requires_use)
            .map(|requirement| requirement.matches(&host_use_version))
            .unwrap_or(false);
        if !target_compatible || !use_compatible {
            incompatible = true;
            continue;
        }
        candidates.push((version, metadata, target_name.clone(), target.clone()));
    }
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| (left.1.target() == host_target).cmp(&(right.1.target() == host_target)))
            .then_with(|| left.2.raw().cmp(right.2.raw()))
    });
    let Some((version, metadata, target_name, target)) = candidates.pop() else {
        if incompatible {
            return Err(UseError::new(
                "use.extension.registry_package_incompatible",
                format!(
                    "Registry '{}' has no '{}' package compatible with A3S Use {} on '{}'.",
                    registry.name, package_id, host_use_version, host_target
                ),
            ));
        }
        return Err(UseError::new(
            "use.extension.registry_package_missing",
            format!(
                "Registry '{}' has no '{}' package for channel '{}' and target '{}'.",
                registry.name, package_id, channel, host_target
            ),
        ));
    };
    if candidates.last().is_some_and(|candidate| {
        candidate.0 == version
            && (candidate.1.target() == host_target) == (metadata.target() == host_target)
    }) {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            "The TUF repository resolves the same package version to multiple targets.",
        ));
    }
    let verified_catalog =
        verified_catalog_record(registry, &repository, metadata.catalog_record().clone())?;
    let resolved = resolved_remote_package(registry, &repository, &metadata, &target_name, &target);
    resolved.verify_expected_plan(expected_plan_digest)?;
    Ok(PreparedRemotePackage::new(
        repository,
        target_name,
        resolved,
        verified_catalog,
        registry.clone(),
        access,
    ))
}

/// Inspect one Registry's verified target cache without refreshing metadata or
/// constructing a network transport.
pub async fn inspect_verified_target_cache(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedTargetCacheUsage> {
    target_cache::inspect_registry_target_cache(registry).await
}

/// Remove stale writes, resumable partials, and the oldest verified targets
/// until the configured per-Registry byte, entry, and free-space bounds are
/// satisfied.
pub async fn prune_verified_target_cache(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedTargetCachePruneResult> {
    target_cache::prune_registry_target_cache(registry).await
}

fn verified_catalog_record(
    registry: &TrustedRegistry,
    repository: &Repository,
    record: a3s_use_core::PluginCatalogRecord,
) -> UseResult<VerifiedPluginCatalogRecord> {
    let provenance = VerifiedCatalogProvenance {
        registry_name: registry.name().to_owned(),
        registry_url: registry.base_url().to_string(),
        root_sha256: format!("sha256:{}", registry.root_sha256()),
        root_version: repository.root().signed.version.get(),
        timestamp_version: repository.timestamp().signed.version.get(),
        snapshot_version: repository.snapshot().signed.version.get(),
        targets_version: repository.targets().signed.version.get(),
        catalog_record_digest: record.descriptor_digest()?,
    };
    VerifiedPluginCatalogRecord::new(record, provenance).map_err(|error| {
        UseError::new(
            "use.extension.registry_target_invalid",
            format!(
                "A TUF catalog record has invalid verified provenance: {}",
                error.message
            ),
        )
    })
}

/// Refresh and fully verify a registry without downloading any package target.
pub async fn refresh_remote_registry(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedRegistryMetadata> {
    let repository = load_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    catalog::record_catalog_refresh(registry, &repository, &metadata).await?;
    Ok(metadata)
}

fn verified_registry_metadata(
    registry: &TrustedRegistry,
    repository: &Repository,
) -> UseResult<VerifiedRegistryMetadata> {
    let mut identities = BTreeSet::new();
    let mut package_targets = 0_u64;
    for (target_name, target) in repository.all_targets() {
        let Some(metadata) = target.custom.get(REGISTRY_METADATA_KEY) else {
            continue;
        };
        let metadata = decode_registry_target_metadata(target_name, metadata)?;
        validate_target_metadata(target_name, target, &metadata)?;
        let identity = (
            metadata.package_id().to_owned(),
            metadata.version().to_owned(),
            metadata.channel().to_owned(),
            metadata.target().to_owned(),
        );
        if !identities.insert(identity) {
            return Err(UseError::new(
                "use.extension.registry_target_invalid",
                "The TUF repository contains duplicate A3S package targets.",
            ));
        }
        package_targets = package_targets.checked_add(1).ok_or_else(|| {
            UseError::new(
                "use.extension.registry_target_invalid",
                "The TUF repository contains too many package targets.",
            )
        })?;
        if package_targets > MAX_REGISTRY_PACKAGE_TARGETS {
            return Err(UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "The TUF repository exceeds the {MAX_REGISTRY_PACKAGE_TARGETS}-package target limit."
                ),
            ));
        }
    }
    Ok(VerifiedRegistryMetadata {
        registry_name: registry.name.clone(),
        registry_url: registry.base_url.to_string(),
        root_sha256: registry.root_sha256.clone(),
        root_version: repository.root().signed.version.get(),
        timestamp_version: repository.timestamp().signed.version.get(),
        snapshot_version: repository.snapshot().signed.version.get(),
        targets_version: repository.targets().signed.version.get(),
        package_targets,
    })
}

async fn load_repository(registry: &TrustedRegistry) -> UseResult<Repository> {
    ensure_metadata_directory(&registry.datastore).await?;
    let lock = acquire_metadata_lock(&registry.datastore)?;
    let root = load_trusted_root(registry).await?;
    let metadata_url = registry.metadata_url()?;
    let targets_url = registry.targets_url()?;
    let transport = network::RegistryTransport::new(registry.network_policy());
    let repository = RepositoryLoader::new(&root, metadata_url, targets_url)
        .transport(transport)
        .datastore(&registry.datastore)
        .limits(Limits {
            max_root_size: MAX_BOOTSTRAP_ROOT_BYTES,
            max_targets_size: 10 * 1024 * 1024,
            max_timestamp_size: 1024 * 1024,
            max_snapshot_size: 1024 * 1024,
            max_root_updates: MAX_ROOT_UPDATES,
        })
        .expiration_enforcement(ExpirationEnforcement::Safe)
        .load()
        .await
        .map_err(|error| {
            UseError::new(
                "use.extension.registry_untrusted",
                format!(
                    "TUF verification failed for registry '{}': {error}",
                    registry.name
                ),
            )
        })?;
    drop(lock);
    Ok(repository)
}

fn validate_channel(channel: &str) -> UseResult<()> {
    if matches!(channel, "stable" | "beta" | "nightly") {
        Ok(())
    } else {
        Err(UseError::new(
            "use.extension.registry_channel_invalid",
            format!("Unsupported extension release channel '{channel}'."),
        ))
    }
}

fn host_target() -> UseResult<String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64".to_string()),
        ("macos", "x86_64") => Ok("darwin-x86_64".to_string()),
        ("linux", "aarch64") => Ok("linux-arm64".to_string()),
        ("linux", "x86_64") => Ok("linux-x86_64".to_string()),
        ("windows", "x86_64") => Ok("windows-x86_64".to_string()),
        (os, arch) => Err(UseError::new(
            "use.extension.registry_target_unsupported",
            format!("Remote extension packages are unavailable for {os}-{arch}."),
        )),
    }
}

async fn ensure_metadata_directory(path: &Path) -> UseResult<()> {
    fs::create_dir_all(path)
        .await
        .map_err(|error| io_error("create TUF metadata datastore", path, error))?;
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect TUF metadata datastore", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.registry_path_invalid",
            format!(
                "The TUF metadata datastore '{}' must be a real directory.",
                path.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|error| io_error("secure TUF metadata datastore", path, error))?;
    }
    Ok(())
}

fn acquire_metadata_lock(datastore: &Path) -> UseResult<MetadataLock> {
    let path = datastore.join(".metadata.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error("open TUF metadata lock", &path, error))?;
    file.try_lock_exclusive().map_err(|error| {
        UseError::new(
            "use.extension.registry_busy",
            format!(
                "Another process is updating registry metadata '{}': {error}",
                datastore.display()
            ),
        )
    })?;
    Ok(MetadataLock(file))
}

async fn load_trusted_root(registry: &TrustedRegistry) -> UseResult<Vec<u8>> {
    let explicit = registry.trusted_root_path.as_deref();
    let cache = registry.datastore.join(ROOT_CACHE_NAME);
    let path = explicit.unwrap_or(&cache);
    let bytes = match read_trusted_root_file(path).await? {
        Some(bytes) => bytes,
        None if explicit.is_none() => {
            let metadata_url = registry.metadata_url()?;
            let root_url = metadata_url.join(ROOT_NAME).map_err(|error| {
                UseError::new(
                    "use.extension.registry_url_invalid",
                    format!("Failed to resolve the bootstrap root URL: {error}"),
                )
            })?;
            let bytes = download_bootstrap_root(registry, &root_url).await?;
            verify_root_bytes(registry, &bytes)?;
            write_bootstrap_root(&cache, &bytes).await?;
            bytes
        }
        None => {
            return Err(UseError::new(
                "use.extension.registry_path_invalid",
                format!("The trusted TUF root '{}' does not exist.", path.display()),
            ))
        }
    };
    verify_root_bytes(registry, &bytes)?;
    Ok(bytes)
}

fn verify_root_bytes(registry: &TrustedRegistry, bytes: &[u8]) -> UseResult<()> {
    let (actual, _) = bootstrap_root_identity(bytes)?;
    verify_root_digest(registry, &actual)
}

fn pinned_bootstrap_root(
    registry: &TrustedRegistry,
    bytes: &[u8],
) -> UseResult<PinnedBootstrapRoot> {
    let (root_sha256, size_bytes) = bootstrap_root_identity(bytes)?;
    verify_root_digest(registry, &root_sha256)?;
    decode_bootstrap_root(bytes, root_sha256, size_bytes)
}

fn bootstrap_root_identity(bytes: &[u8]) -> UseResult<(String, u64)> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_BOOTSTRAP_ROOT_BYTES {
        return Err(UseError::new(
            "use.extension.registry_root_invalid",
            "The trusted TUF root must contain at most one MiB.",
        ));
    }
    Ok((format!("{:x}", Sha256::digest(bytes)), bytes.len() as u64))
}

fn decode_bootstrap_root(
    bytes: &[u8],
    root_sha256: String,
    size_bytes: u64,
) -> UseResult<PinnedBootstrapRoot> {
    let root = serde_json::from_slice::<Signed<Root>>(bytes).map_err(|error| {
        UseError::new(
            "use.extension.registry_root_invalid",
            format!("The pinned bootstrap TUF root is invalid: {error}"),
        )
    })?;
    Ok(PinnedBootstrapRoot {
        root_sha256,
        root_version: root.signed.version.get(),
        size_bytes,
    })
}

async fn read_trusted_root_file(path: &Path) -> UseResult<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("inspect trusted TUF root", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(UseError::new(
            "use.extension.registry_path_invalid",
            format!(
                "The trusted TUF root '{}' must be a regular file.",
                path.display()
            ),
        ));
    }
    fs::read(path)
        .await
        .map(Some)
        .map_err(|error| io_error("read trusted TUF root", path, error))
}

async fn download_bootstrap_root(registry: &TrustedRegistry, url: &Url) -> UseResult<Vec<u8>> {
    validate_download_url(url)?;
    let client = match registry.network_policy() {
        RegistryNetworkPolicy::Standard => reqwest::Client::builder()
            .user_agent("a3s-use-extension/0.3")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|error| {
                UseError::new(
                    "use.extension.registry_download_failed",
                    format!("Failed to build the registry client: {error}"),
                )
            })?,
        RegistryNetworkPolicy::PublicInternet => {
            network::public_internet_client(url, Duration::from_secs(15), Duration::from_secs(30))
                .await?
        }
    };
    let mut response = client.get(url.clone()).send().await.map_err(|error| {
        UseError::new(
            "use.extension.registry_download_failed",
            format!("Failed to download the bootstrap TUF root: {error}"),
        )
    })?;
    validate_download_url(response.url())?;
    if !response.status().is_success() {
        return Err(UseError::new(
            "use.extension.registry_download_failed",
            format!(
                "Bootstrap TUF root download returned HTTP {}.",
                response.status()
            ),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BOOTSTRAP_ROOT_BYTES)
    {
        return Err(UseError::new(
            "use.extension.registry_root_invalid",
            "The bootstrap TUF root exceeds the one MiB limit.",
        ));
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(MAX_BOOTSTRAP_ROOT_BYTES) as usize,
    );
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        UseError::new(
            "use.extension.registry_download_failed",
            format!("Failed to read the bootstrap TUF root: {error}"),
        )
    })? {
        if bytes.len().saturating_add(chunk.len()) as u64 > MAX_BOOTSTRAP_ROOT_BYTES {
            return Err(UseError::new(
                "use.extension.registry_root_invalid",
                "The bootstrap TUF root exceeds the one MiB limit.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn verify_root_digest(registry: &TrustedRegistry, actual: &str) -> UseResult<()> {
    if actual == registry.root_sha256 {
        return Ok(());
    }
    Err(UseError::new(
        "use.extension.registry_root_mismatch",
        format!(
            "Registry '{}' bootstrap root does not match its pinned SHA-256.",
            registry.name
        ),
    )
    .with_detail("expected", registry.root_sha256.clone())
    .with_detail("actual", actual.to_owned()))
}

async fn write_bootstrap_root(path: &Path, bytes: &[u8]) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.extension.registry_path_invalid",
            "The bootstrap TUF root cache has no parent directory.",
        )
    })?;
    let temporary = parent.join(format!(".root-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| io_error("create bootstrap TUF root cache", &temporary, error))?;
    if let Err(error) = file.write_all(bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write bootstrap TUF root cache",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error("sync bootstrap TUF root cache", &temporary, error));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path.to_path_buf(),
        "activate bootstrap TUF root cache",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(parent, "TUF metadata").await
}

pub(crate) fn normalize_registry_url(value: &str) -> UseResult<Url> {
    let mut url = Url::parse(value).map_err(|error| {
        UseError::new(
            "use.extension.registry_url_invalid",
            format!("Invalid registry URL: {error}"),
        )
    })?;
    validate_download_url(&url)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(UseError::new(
            "use.extension.registry_url_invalid",
            "Registry URLs must not contain credentials, query parameters, or fragments.",
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn validate_download_url(url: &Url) -> UseResult<()> {
    let https = url.scheme() == "https";
    let loopback_http = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    if https || loopback_http {
        Ok(())
    } else {
        Err(UseError::new(
            "use.extension.registry_url_invalid",
            "Registry downloads require HTTPS; HTTP is accepted only on loopback for local testing.",
        ))
    }
}

pub(crate) fn validate_registry_name(name: &str) -> UseResult<()> {
    let mut characters = name.chars();
    if characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        Ok(())
    } else {
        Err(UseError::new(
            "use.extension.registry_name_invalid",
            "Registry names use lowercase letters, digits, and hyphens and start with a letter.",
        ))
    }
}

pub(crate) fn normalize_sha256(value: &str, label: &str) -> UseResult<String> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(value.to_string())
    } else {
        Err(UseError::new(
            "use.extension.registry_digest_invalid",
            format!("The {label} must be exactly 64 lowercase hexadecimal characters."),
        ))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
#[path = "tuf_test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "remote_resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "remote_catalog_tests.rs"]
mod catalog_tests;
