use std::collections::BTreeMap;

use a3s_use_core::{
    PluginReleaseChannel, PluginSurfaceKind, UseError, UseResult, VerifiedPluginCatalogRecord,
};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use tough::{Repository, TargetName};

use super::{
    decode_registry_target_metadata, host_target, load_repository, resolved_remote_package,
    validate_target_metadata, verified_catalog_record, verified_registry_metadata,
    RegistryTargetMetadata, ResolvedRemotePackage, TrustedRegistry, MAX_REGISTRY_PACKAGE_TARGETS,
    REGISTRY_METADATA_KEY,
};

mod cache;
mod query;

use cache::{
    load_cached_repository, record_catalog_refresh as persist_catalog_refresh, unix_time_seconds,
    verify_stamp_metadata, CatalogCacheStamp,
};
use query::{inspect_catalog, search_catalog};

const MAX_CATALOG_QUERY_BYTES: usize = 256;
const MAX_CATALOG_CURSOR_BYTES: usize = 512;
const MAX_CATALOG_FILTER_BYTES: usize = 64;

pub const MAX_PLUGIN_CATALOG_PAGE_SIZE: u16 = 50;
pub const MAX_PLUGIN_CATALOG_PAGE_BYTES: usize = 1024 * 1024;

/// Signed metadata versions observed after a complete TUF refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedRegistryMetadata {
    pub registry_name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub root_version: u64,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub package_targets: u64,
}

/// Installable package targets discovered from one fully verified TUF
/// repository. The legacy catalog contains only targets compatible with the
/// current host and never downloads package payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedRegistryCatalog {
    pub metadata: VerifiedRegistryMetadata,
    pub host_target: String,
    pub packages: Vec<ResolvedRemotePackage>,
}

/// Compatibility context selected by the trusted manager, not by catalog
/// content or an autonomous plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogHost {
    pub target: String,
    pub use_version: String,
}

impl PluginCatalogHost {
    pub fn new(target: impl Into<String>, use_version: impl Into<String>) -> UseResult<Self> {
        let host = Self {
            target: target.into(),
            use_version: use_version.into(),
        };
        host.validate()?;
        Ok(host)
    }

    pub fn current() -> UseResult<Self> {
        Self::new(host_target()?, env!("CARGO_PKG_VERSION"))
    }

    fn validate(&self) -> UseResult<()> {
        let valid_target = self.target != "any"
            && !self.target.is_empty()
            && self.target.len() <= MAX_CATALOG_FILTER_BYTES
            && matches!(self.target.as_bytes().first(), Some(b'a'..=b'z'))
            && self.target.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            });
        let valid_version = Version::parse(&self.use_version)
            .map(|version| version.to_string() == self.use_version)
            .unwrap_or(false);
        if valid_target && valid_version {
            Ok(())
        } else {
            Err(catalog_input_error(
                "The catalog host target or A3S Use version is invalid.",
            ))
        }
    }

    fn parsed_use_version(&self) -> UseResult<Version> {
        self.validate()?;
        Version::parse(&self.use_version).map_err(|error| {
            catalog_input_error(format!("The catalog host version is invalid: {error}"))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCatalogAvailability {
    Available,
    Deprecated,
    Withdrawn,
}

/// Bounded local query over already verified signed catalog records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogSearch {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<PluginSurfaceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<PluginReleaseChannel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<PluginCatalogAvailability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: u16,
}

impl PluginCatalogSearch {
    fn validate(&self) -> UseResult<()> {
        if self.query.len() > MAX_CATALOG_QUERY_BYTES
            || self.query.trim() != self.query
            || self.query.chars().any(char::is_control)
            || self.limit == 0
            || self.limit > MAX_PLUGIN_CATALOG_PAGE_SIZE
            || self
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CATALOG_CURSOR_BYTES)
            || self
                .publisher
                .as_deref()
                .is_some_and(|publisher| !valid_segment(publisher))
            || self
                .category
                .as_deref()
                .is_some_and(|category| !valid_tag(category))
        {
            return Err(catalog_input_error(
                "The catalog query, filters, cursor, or page limit is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCatalogSnapshotSource {
    Refreshed,
    Cached,
}

/// Exact verified metadata snapshot used by one search or inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogSnapshot {
    pub metadata: VerifiedRegistryMetadata,
    pub source: PluginCatalogSnapshotSource,
    pub host_target: String,
    pub use_version: String,
    pub catalog_records: u64,
    pub verified_at_unix_seconds: u64,
    pub age_seconds: u64,
    pub snapshot_digest: String,
}

/// One deterministic bounded page of verified records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogPage {
    pub snapshot: PluginCatalogSnapshot,
    pub total_matches: u64,
    pub plugins: Vec<VerifiedPluginCatalogRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Full review metadata for one exact compatible release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogInspection {
    pub snapshot: PluginCatalogSnapshot,
    pub plugin: VerifiedPluginCatalogRecord,
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    plugin: VerifiedPluginCatalogRecord,
    version: Version,
}

#[derive(Debug)]
struct LoadedCatalog {
    snapshot: PluginCatalogSnapshot,
    entries: Vec<CatalogEntry>,
}

/// Refresh and verify a registry, then enumerate current-host-compatible
/// signed package targets without downloading any archive.
pub async fn list_remote_packages(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedRegistryCatalog> {
    let repository = load_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    let host = PluginCatalogHost::current()?;
    let mut selected = BTreeMap::<
        (String, Version, String),
        (RegistryTargetMetadata, TargetName, tough::schema::Target),
    >::new();

    for (target_name, target) in repository.all_targets() {
        let Some(custom) = target.custom.get(REGISTRY_METADATA_KEY) else {
            continue;
        };
        let package = decode_registry_target_metadata(target_name, custom)?;
        validate_target_metadata(target_name, target, &package)?;
        if !metadata_is_compatible(&package, &host)? {
            continue;
        }
        let version = Version::parse(package.version()).map_err(|error| {
            registry_target_error(format!(
                "TUF target '{}' declares an invalid version: {error}",
                target_name.raw()
            ))
        })?;
        let key = (
            package.package_id().to_owned(),
            version,
            package.channel().to_owned(),
        );
        match selected.get(&key) {
            None => {
                selected.insert(key, (package, target_name.clone(), target.clone()));
            }
            Some((current, _, _))
                if current.target() == "any" && package.target() == host.target =>
            {
                selected.insert(key, (package, target_name.clone(), target.clone()));
            }
            Some((current, _, _))
                if current.target() == host.target && package.target() == "any" => {}
            Some(_) => {
                return Err(registry_target_error(
                    "The TUF repository resolves the same package version to multiple targets.",
                ));
            }
        }
    }

    let packages = selected
        .into_values()
        .map(|(package, target_name, target)| {
            resolved_remote_package(registry, &repository, &package, &target_name, &target)
        })
        .collect();
    persist_catalog_refresh(registry, &repository, &metadata).await?;
    Ok(VerifiedRegistryCatalog {
        metadata,
        host_target: host.target,
        packages,
    })
}

/// Refresh TUF metadata and search its signed catalog records locally.
pub async fn search_remote_plugins(
    registry: &TrustedRegistry,
    host: &PluginCatalogHost,
    search: &PluginCatalogSearch,
) -> UseResult<PluginCatalogPage> {
    search.validate()?;
    let catalog = load_refreshed_catalog(registry, host).await?;
    search_catalog(catalog, host, search)
}

/// Search only the last online-verified TUF snapshot. This function has no
/// network transport and continues to enforce TUF signatures and expiration.
pub async fn search_cached_plugins(
    registry: &TrustedRegistry,
    host: &PluginCatalogHost,
    search: &PluginCatalogSearch,
) -> UseResult<PluginCatalogPage> {
    search.validate()?;
    let catalog = load_cached_catalog(registry, host).await?;
    search_catalog(catalog, host, search)
}

/// Refresh TUF metadata and inspect one signed compatible release without
/// downloading its package target.
pub async fn inspect_remote_plugin(
    registry: &TrustedRegistry,
    host: &PluginCatalogHost,
    package_id: &str,
    version: Option<&str>,
    channel: Option<PluginReleaseChannel>,
) -> UseResult<PluginCatalogInspection> {
    let catalog = load_refreshed_catalog(registry, host).await?;
    inspect_catalog(catalog, host, package_id, version, channel)
}

/// Inspect one release from only the last online-verified TUF snapshot.
pub async fn inspect_cached_plugin(
    registry: &TrustedRegistry,
    host: &PluginCatalogHost,
    package_id: &str,
    version: Option<&str>,
    channel: Option<PluginReleaseChannel>,
) -> UseResult<PluginCatalogInspection> {
    let catalog = load_cached_catalog(registry, host).await?;
    inspect_catalog(catalog, host, package_id, version, channel)
}

async fn load_refreshed_catalog(
    registry: &TrustedRegistry,
    host: &PluginCatalogHost,
) -> UseResult<LoadedCatalog> {
    host.validate()?;
    let repository = load_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    let entries = collect_catalog_entries(registry, &repository)?;
    let stamp = persist_catalog_refresh(registry, &repository, &metadata).await?;
    loaded_catalog(
        metadata,
        PluginCatalogSnapshotSource::Refreshed,
        host,
        entries,
        stamp,
    )
}

pub(super) async fn load_refreshed_plugin_candidates(
    registry: &TrustedRegistry,
) -> UseResult<Vec<VerifiedPluginCatalogRecord>> {
    let repository = load_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    let entries = collect_catalog_entries(registry, &repository)?;
    persist_catalog_refresh(registry, &repository, &metadata).await?;
    Ok(entries.into_iter().map(|entry| entry.plugin).collect())
}

async fn load_cached_catalog(
    registry: &TrustedRegistry,
    host: &PluginCatalogHost,
) -> UseResult<LoadedCatalog> {
    host.validate()?;
    let (repository, stamp) = load_cached_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    verify_stamp_metadata(&stamp, &metadata)?;
    let entries = collect_catalog_entries(registry, &repository)?;
    loaded_catalog(
        metadata,
        PluginCatalogSnapshotSource::Cached,
        host,
        entries,
        stamp,
    )
}

fn loaded_catalog(
    metadata: VerifiedRegistryMetadata,
    source: PluginCatalogSnapshotSource,
    host: &PluginCatalogHost,
    entries: Vec<CatalogEntry>,
    stamp: CatalogCacheStamp,
) -> UseResult<LoadedCatalog> {
    let now = unix_time_seconds()?;
    let age_seconds = now
        .checked_sub(stamp.verified_at_unix_seconds)
        .ok_or_else(|| {
            catalog_cache_error(
                "use.extension.catalog_cache_invalid",
                "The catalog cache verification time is in the future.",
            )
        })?;
    let catalog_records = u64::try_from(entries.len()).map_err(|error| {
        registry_target_error(format!("The catalog record count is invalid: {error}"))
    })?;
    let snapshot = PluginCatalogSnapshot {
        metadata,
        source,
        host_target: host.target.clone(),
        use_version: host.use_version.clone(),
        catalog_records,
        verified_at_unix_seconds: stamp.verified_at_unix_seconds,
        age_seconds,
        snapshot_digest: stamp.snapshot_digest(),
    };
    Ok(LoadedCatalog { snapshot, entries })
}

pub(super) async fn record_catalog_refresh(
    registry: &TrustedRegistry,
    repository: &Repository,
    metadata: &VerifiedRegistryMetadata,
) -> UseResult<()> {
    persist_catalog_refresh(registry, repository, metadata)
        .await
        .map(drop)
}

fn collect_catalog_entries(
    registry: &TrustedRegistry,
    repository: &Repository,
) -> UseResult<Vec<CatalogEntry>> {
    let mut entries = Vec::new();
    for (target_name, target) in repository.all_targets() {
        let Some(custom) = target.custom.get(REGISTRY_METADATA_KEY) else {
            continue;
        };
        let metadata = decode_registry_target_metadata(target_name, custom)?;
        validate_target_metadata(target_name, target, &metadata)?;
        let RegistryTargetMetadata::Catalog(record) = metadata else {
            continue;
        };
        let record = *record;
        let version = Version::parse(&record.version).map_err(|error| {
            registry_target_error(format!(
                "TUF target '{}' declares an invalid catalog version: {error}",
                target_name.raw()
            ))
        })?;
        let plugin = verified_catalog_record(registry, repository, record)?;
        entries.push(CatalogEntry { plugin, version });
        if entries.len() as u64 > MAX_REGISTRY_PACKAGE_TARGETS {
            return Err(registry_target_error(format!(
                "The TUF repository exceeds the {MAX_REGISTRY_PACKAGE_TARGETS}-record catalog limit."
            )));
        }
    }
    Ok(entries)
}

fn metadata_is_compatible(
    metadata: &RegistryTargetMetadata,
    host: &PluginCatalogHost,
) -> UseResult<bool> {
    if metadata.target() != host.target && metadata.target() != "any" {
        return Ok(false);
    }
    let Some(record) = metadata.catalog_record() else {
        return Ok(true);
    };
    let requirement = VersionReq::parse(&record.requires_use).map_err(|error| {
        registry_target_error(format!(
            "Catalog record '{}' has an invalid A3S Use requirement: {error}",
            record.package_id
        ))
    })?;
    Ok(requirement.matches(&host.parsed_use_version()?))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_tag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CATALOG_FILTER_BYTES
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
}

fn registry_target_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_target_invalid", message)
}

fn catalog_input_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.catalog_query_invalid", message)
}

fn catalog_cursor_error(code: &'static str, message: &'static str) -> UseError {
    UseError::new(code, message)
}

fn catalog_cache_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
