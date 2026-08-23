use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    CatalogAvailability, PluginPackageLock, PluginPackageLockHost, PluginPackageResolver,
    PluginReleaseChannel, UseError, UseResult, VerifiedPluginCatalogRecord,
    MAX_PLUGIN_RESOLUTION_CANDIDATES, PLUGIN_CATALOG_SCHEMA_V3,
};
use async_trait::async_trait;
use semver::{Version, VersionReq};

use super::catalog::{load_cached_plugin_candidates, load_refreshed_plugin_candidates};
use super::{
    prepare_cached_remote_package, prepare_remote_package, DownloadedRemotePackage,
    PreparedRemotePackage, ResolvedRemotePackage, TrustedRegistry,
};

#[derive(Clone, Copy)]
enum RegistrySnapshotAccess {
    Refreshed,
    Cached,
}

/// Durable-operation observer for the Registry/TUF phase that precedes an
/// exact package lock.
///
/// Implementations must persist only bounded, non-secret evidence. Returning
/// an error fails the resolution so a manager never silently loses required
/// operational evidence.
#[async_trait]
pub trait PackageRegistryResolutionObserver: Send + Sync {
    async fn registry_resolution_started(&self, registry_name: &str) -> UseResult<()>;

    async fn registry_resolution_verified(
        &self,
        metadata: &super::VerifiedRegistryMetadata,
    ) -> UseResult<()>;

    async fn registry_resolution_failed(
        &self,
        registry_name: &str,
        error_code: &str,
    ) -> UseResult<()>;
}

/// Resolve one exact schema-v3 root and its complete transitive dependency
/// closure from the host-selected set of replaceable named Registries.
///
/// Registry URLs remain host configuration. Signed manifests name only
/// package IDs and SemVer requirements; the resulting lock records the exact
/// Registry and TUF provenance chosen for every node.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_remote_package_lock(
    root_registry: &TrustedRegistry,
    dependency_registries: &[TrustedRegistry],
    root_package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
    host: PluginPackageLockHost,
) -> UseResult<PluginPackageLock> {
    resolve_remote_package_lock_with_access(
        root_registry,
        dependency_registries,
        root_package_id,
        requested_version,
        channel,
        host,
        RegistrySnapshotAccess::Refreshed,
        None,
    )
    .await
}

/// Resolve an exact dependency lock while durably observing each Registry/TUF
/// verification boundary before the lock exists.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_remote_package_lock_with_observer(
    root_registry: &TrustedRegistry,
    dependency_registries: &[TrustedRegistry],
    root_package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
    host: PluginPackageLockHost,
    observer: &dyn PackageRegistryResolutionObserver,
) -> UseResult<PluginPackageLock> {
    resolve_remote_package_lock_with_access(
        root_registry,
        dependency_registries,
        root_package_id,
        requested_version,
        channel,
        host,
        RegistrySnapshotAccess::Refreshed,
        Some(observer),
    )
    .await
}

/// Resolve an exact dependency closure only from locally cached, still-valid
/// TUF snapshots. No Registry network transport is constructed.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_cached_remote_package_lock(
    root_registry: &TrustedRegistry,
    dependency_registries: &[TrustedRegistry],
    root_package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
    host: PluginPackageLockHost,
) -> UseResult<PluginPackageLock> {
    resolve_remote_package_lock_with_access(
        root_registry,
        dependency_registries,
        root_package_id,
        requested_version,
        channel,
        host,
        RegistrySnapshotAccess::Cached,
        None,
    )
    .await
}

/// Resolve only from verified cached metadata while observing each pre-lock
/// Registry boundary without constructing network transport.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_cached_remote_package_lock_with_observer(
    root_registry: &TrustedRegistry,
    dependency_registries: &[TrustedRegistry],
    root_package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
    host: PluginPackageLockHost,
    observer: &dyn PackageRegistryResolutionObserver,
) -> UseResult<PluginPackageLock> {
    resolve_remote_package_lock_with_access(
        root_registry,
        dependency_registries,
        root_package_id,
        requested_version,
        channel,
        host,
        RegistrySnapshotAccess::Cached,
        Some(observer),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resolve_remote_package_lock_with_access(
    root_registry: &TrustedRegistry,
    dependency_registries: &[TrustedRegistry],
    root_package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
    host: PluginPackageLockHost,
    access: RegistrySnapshotAccess,
    observer: Option<&dyn PackageRegistryResolutionObserver>,
) -> UseResult<PluginPackageLock> {
    host.validate()?;
    let registries = unique_registries(root_registry, dependency_registries)?;
    let mut candidates = Vec::new();
    let mut root_candidates = Vec::new();
    for registry in registries.values() {
        if let Some(observer) = observer {
            observer
                .registry_resolution_started(registry.name())
                .await?;
        }
        let loaded = match match access {
            RegistrySnapshotAccess::Refreshed => load_refreshed_plugin_candidates(registry).await,
            RegistrySnapshotAccess::Cached => load_cached_plugin_candidates(registry).await,
        } {
            Ok(loaded) => loaded,
            Err(error) => {
                if let Some(observer) = observer {
                    observer
                        .registry_resolution_failed(registry.name(), &error.code)
                        .await?;
                }
                return Err(error);
            }
        };
        if let Some(observer) = observer {
            observer
                .registry_resolution_verified(&loaded.metadata)
                .await?;
        }
        let records = loaded.records;
        if registry.name() == root_registry.name() {
            root_candidates.extend(records.iter().cloned());
        }
        candidates.extend(
            records
                .into_iter()
                .filter(|record| record.record.schema == PLUGIN_CATALOG_SCHEMA_V3),
        );
        if candidates.len() > MAX_PLUGIN_RESOLUTION_CANDIDATES {
            return Err(package_graph_error(
                "use.plugin.package_resolution_limit",
                "The enabled Registry candidate set exceeds the deterministic resolution bound.",
            ));
        }
    }

    let root = select_root(
        root_candidates,
        root_registry.name(),
        root_package_id,
        requested_version,
        channel,
        &host,
    )?;
    candidates.retain(|candidate| candidate.record.package_id != root_package_id);
    PluginPackageResolver::new(host).resolve(root, candidates)
}

/// Revalidate every locked Registry snapshot before downloading anything,
/// then fetch the complete closure in dependency-forward order.
///
/// A changed Registry URL, trust root, TUF role version, catalog record,
/// archive target, or digest fails before its payload is admitted.
pub async fn download_locked_remote_packages(
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
) -> UseResult<Vec<DownloadedRemotePackage>> {
    let selected = package_lock
        .packages
        .iter()
        .map(|package| package.package_id().to_string())
        .collect();
    download_selected_locked_remote_packages(package_lock, registries, &selected).await
}

/// Revalidate and stage a complete exact lock only from verified local target
/// caches, preserving dependency-forward order without network access.
pub async fn download_locked_cached_remote_packages(
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
) -> UseResult<Vec<DownloadedRemotePackage>> {
    let selected = package_lock
        .packages
        .iter()
        .map(|package| package.package_id().to_string())
        .collect();
    download_selected_locked_cached_remote_packages(package_lock, registries, &selected).await
}

/// Revalidate the complete lock before downloading only the selected package
/// payloads. Retained shared nodes still receive exact Registry/TUF metadata
/// verification, but their immutable archives are not fetched again.
pub async fn download_selected_locked_remote_packages(
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
    selected_package_ids: &BTreeSet<String>,
) -> UseResult<Vec<DownloadedRemotePackage>> {
    download_selected_locked_remote_packages_with_access(
        package_lock,
        registries,
        selected_package_ids,
        RegistrySnapshotAccess::Refreshed,
    )
    .await
}

/// Revalidate the complete lock against cached TUF metadata before staging the
/// selected immutable payloads from the verified content-addressed cache.
pub async fn download_selected_locked_cached_remote_packages(
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
    selected_package_ids: &BTreeSet<String>,
) -> UseResult<Vec<DownloadedRemotePackage>> {
    download_selected_locked_remote_packages_with_access(
        package_lock,
        registries,
        selected_package_ids,
        RegistrySnapshotAccess::Cached,
    )
    .await
}

async fn download_selected_locked_remote_packages_with_access(
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
    selected_package_ids: &BTreeSet<String>,
    access: RegistrySnapshotAccess,
) -> UseResult<Vec<DownloadedRemotePackage>> {
    package_lock.validate()?;
    if selected_package_ids.len() > package_lock.packages.len()
        || selected_package_ids
            .iter()
            .any(|package_id| package_lock.package(package_id).is_none())
    {
        return Err(package_graph_error(
            "use.plugin.package_download_selection_invalid",
            "The selected download set contains a package outside the exact dependency lock.",
        ));
    }
    let registries = registry_map(registries)?;
    let mut prepared = Vec::<PreparedRemotePackage>::with_capacity(selected_package_ids.len());
    for locked in package_lock.install_order()? {
        let provenance = &locked.catalog.provenance;
        let registry = registries
            .get(provenance.registry_name.as_str())
            .ok_or_else(|| {
                package_graph_error(
                    "use.plugin.package_registry_missing",
                    format!(
                        "Locked Registry '{}' is not enabled by this host.",
                        provenance.registry_name
                    ),
                )
            })?;
        verify_registry_binding(registry, &locked.catalog)?;
        let expected = ResolvedRemotePackage::from_verified_catalog(&locked.catalog)?;
        let expected_plan_digest = expected.plan_digest()?;
        let candidate = match access {
            RegistrySnapshotAccess::Refreshed => {
                prepare_remote_package(
                    registry,
                    locked.package_id(),
                    Some(locked.version()),
                    locked.catalog.record.channel.as_str(),
                    Some(&expected_plan_digest),
                )
                .await?
            }
            RegistrySnapshotAccess::Cached => {
                prepare_cached_remote_package(
                    registry,
                    locked.package_id(),
                    Some(locked.version()),
                    locked.catalog.record.channel.as_str(),
                    Some(&expected_plan_digest),
                )
                .await?
            }
        };
        if candidate.verified_catalog() != &locked.catalog || candidate.resolved() != &expected {
            return Err(package_graph_error(
                "use.plugin.package_lock_changed",
                format!(
                    "Locked cognitive package '{}' changed after dependency review.",
                    locked.package_id()
                ),
            ));
        }
        if selected_package_ids.contains(locked.package_id()) {
            prepared.push(candidate);
        }
    }

    let mut downloaded = Vec::with_capacity(prepared.len());
    for candidate in prepared {
        downloaded.push(candidate.download().await?);
    }
    Ok(downloaded)
}

fn select_root(
    candidates: Vec<VerifiedPluginCatalogRecord>,
    registry_name: &str,
    package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
    host: &PluginPackageLockHost,
) -> UseResult<VerifiedPluginCatalogRecord> {
    let requested_version = requested_version
        .map(parse_requested_root_version)
        .transpose()?;
    let host_version = Version::parse(&host.use_version).map_err(|_| {
        package_graph_error(
            "use.plugin.package_resolution_invalid",
            "The package-lock host version is invalid.",
        )
    })?;
    let mut compatible = candidates
        .into_iter()
        .filter(|candidate| {
            let record = &candidate.record;
            if record.schema != PLUGIN_CATALOG_SCHEMA_V3
                || record.package_id != package_id
                || record.channel != channel
                || matches!(record.availability, CatalogAvailability::Withdrawn { .. })
                || (record.target != "any" && record.target != host.target)
            {
                return false;
            }
            let Ok(version) = Version::parse(&record.version) else {
                return false;
            };
            if requested_version
                .as_ref()
                .is_some_and(|requested| !requested.matches(&version))
            {
                return false;
            }
            VersionReq::parse(&record.requires_use)
                .is_ok_and(|requirement| requirement.matches(&host_version))
        })
        .collect::<Vec<_>>();
    compatible.sort_by(|left, right| {
        Version::parse(&right.record.version)
            .ok()
            .cmp(&Version::parse(&left.record.version).ok())
            .then_with(|| (left.record.target == "any").cmp(&(right.record.target == "any")))
            .then_with(|| {
                left.provenance
                    .catalog_record_digest
                    .cmp(&right.provenance.catalog_record_digest)
            })
    });
    if compatible.len() > 1
        && compatible[0].record.version == compatible[1].record.version
        && (compatible[0].record.target == "any") == (compatible[1].record.target == "any")
    {
        return Err(package_graph_error(
            "use.plugin.package_root_ambiguous",
            format!(
                "Root Registry '{registry_name}' resolves '{package_id}' to more than one equivalent release."
            ),
        ));
    }
    compatible.into_iter().next().ok_or_else(|| {
        package_graph_error(
            "use.plugin.package_root_missing",
            format!(
                "Root Registry '{}' has no compatible '{}' release for the requested version and channel.",
                registry_name,
                package_id
            ),
        )
    })
}

enum RequestedRootVersion {
    Exact(Version),
    Requirement(VersionReq),
}

impl RequestedRootVersion {
    fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Exact(expected) => expected == version,
            Self::Requirement(requirement) => requirement.matches(version),
        }
    }
}

fn parse_requested_root_version(value: &str) -> UseResult<RequestedRootVersion> {
    if let Ok(version) = Version::parse(value) {
        if version.to_string() == value {
            return Ok(RequestedRootVersion::Exact(version));
        }
        return Err(package_graph_error(
            "use.plugin.package_version_invalid",
            "The requested root package version must use canonical semantic versioning.",
        ));
    }
    VersionReq::parse(value)
        .map(RequestedRootVersion::Requirement)
        .map_err(|_| {
            package_graph_error(
                "use.plugin.package_version_invalid",
                "The requested root package version or version requirement is invalid.",
            )
        })
}

fn unique_registries<'a>(
    root: &'a TrustedRegistry,
    dependencies: &'a [TrustedRegistry],
) -> UseResult<BTreeMap<String, &'a TrustedRegistry>> {
    let mut registries = BTreeMap::new();
    insert_registry(&mut registries, root)?;
    for registry in dependencies {
        insert_registry(&mut registries, registry)?;
    }
    Ok(registries)
}

fn registry_map(registries: &[TrustedRegistry]) -> UseResult<BTreeMap<String, &TrustedRegistry>> {
    let mut result = BTreeMap::new();
    for registry in registries {
        insert_registry(&mut result, registry)?;
    }
    Ok(result)
}

fn insert_registry<'a>(
    registries: &mut BTreeMap<String, &'a TrustedRegistry>,
    registry: &'a TrustedRegistry,
) -> UseResult<()> {
    if let Some(existing) = registries.get(registry.name()) {
        if existing.base_url() != registry.base_url()
            || existing.root_sha256() != registry.root_sha256()
        {
            return Err(package_graph_error(
                "use.plugin.package_registry_ambiguous",
                format!(
                    "Registry name '{}' resolves to more than one configured trust identity.",
                    registry.name()
                ),
            ));
        }
        return Ok(());
    }
    registries.insert(registry.name().to_string(), registry);
    Ok(())
}

fn verify_registry_binding(
    registry: &TrustedRegistry,
    locked: &VerifiedPluginCatalogRecord,
) -> UseResult<()> {
    let provenance = &locked.provenance;
    if !registry.matches_provenance(provenance) {
        return Err(package_graph_error(
            "use.plugin.package_registry_changed",
            format!(
                "Registry configuration for '{}' changed after package-lock review.",
                provenance.registry_name
            ),
        ));
    }
    Ok(())
}

fn package_graph_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests {
    use a3s_use_core::{
        PluginCatalogRecord, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
    };

    use super::*;

    const CATALOG: &[u8] =
        include_bytes!("../../../core/fixtures/plugins/catalog-record-okf-v3.json");

    #[test]
    fn exact_root_version_remains_exact() {
        let selected = select_root(
            vec![candidate("1.2.4"), candidate("1.2.3")],
            "fixture",
            "acme/knowledge",
            Some("1.2.3"),
            PluginReleaseChannel::Stable,
            &host(),
        )
        .unwrap();

        assert_eq!(selected.record.version, "1.2.3");
    }

    #[test]
    fn root_version_requirement_selects_the_highest_match() {
        let selected = select_root(
            vec![candidate("1.2.0"), candidate("1.9.9"), candidate("2.0.0")],
            "fixture",
            "acme/knowledge",
            Some("^1.2"),
            PluginReleaseChannel::Stable,
            &host(),
        )
        .unwrap();

        assert_eq!(selected.record.version, "1.9.9");
    }

    #[test]
    fn invalid_root_version_requirement_fails_closed() {
        let error = select_root(
            vec![candidate("1.2.3")],
            "fixture",
            "acme/knowledge",
            Some("latest"),
            PluginReleaseChannel::Stable,
            &host(),
        )
        .unwrap_err();

        assert_eq!(error.code, "use.plugin.package_version_invalid");
    }

    fn candidate(version: &str) -> VerifiedPluginCatalogRecord {
        let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
        record.archive.target_name = record.archive.target_name.replace("1.0.0", version);
        record.version = version.to_owned();
        let catalog_record_digest = record.descriptor_digest().unwrap();
        VerifiedPluginCatalogRecord::new(
            record,
            VerifiedCatalogProvenance {
                registry_name: "fixture".to_owned(),
                registry_url: "https://registry.example.com/".to_owned(),
                root_sha256: format!("sha256:{}", "a".repeat(64)),
                root_version: 1,
                timestamp_version: 1,
                snapshot_version: 1,
                targets_version: 1,
                catalog_record_digest,
            },
        )
        .unwrap()
    }

    fn host() -> PluginPackageLockHost {
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap()
    }
}
