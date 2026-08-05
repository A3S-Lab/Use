use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    CatalogAvailability, PluginPackageLock, PluginPackageLockHost, PluginPackageResolver,
    PluginReleaseChannel, UseError, UseResult, VerifiedPluginCatalogRecord,
    MAX_PLUGIN_RESOLUTION_CANDIDATES, PLUGIN_CATALOG_SCHEMA_V3,
};
use semver::{Version, VersionReq};

use super::catalog::load_refreshed_plugin_candidates;
use super::{
    prepare_remote_package, DownloadedRemotePackage, PreparedRemotePackage, ResolvedRemotePackage,
    TrustedRegistry,
};

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
    host.validate()?;
    let registries = unique_registries(root_registry, dependency_registries)?;
    let mut candidates = Vec::new();
    let mut root_candidates = Vec::new();
    for registry in registries.values() {
        let records = load_refreshed_plugin_candidates(registry).await?;
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

/// Revalidate the complete lock before downloading only the selected package
/// payloads. Retained shared nodes still receive exact Registry/TUF metadata
/// verification, but their immutable archives are not fetched again.
pub async fn download_selected_locked_remote_packages(
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
    selected_package_ids: &BTreeSet<String>,
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
        let candidate = prepare_remote_package(
            registry,
            locked.package_id(),
            Some(locked.version()),
            locked.catalog.record.channel.as_str(),
            Some(&expected_plan_digest),
        )
        .await?;
        if candidate.verified_catalog() != Some(&locked.catalog)
            || candidate.resolved() != &expected
        {
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
        .map(|value| {
            Version::parse(value)
                .map_err(|_| {
                    package_graph_error(
                        "use.plugin.package_version_invalid",
                        "The requested root package version is invalid semantic versioning.",
                    )
                })
                .and_then(|version| {
                    if version.to_string() == value {
                        Ok(version)
                    } else {
                        Err(package_graph_error(
                            "use.plugin.package_version_invalid",
                            "The requested root package version must be canonical semantic versioning.",
                        ))
                    }
                })
        })
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
                .is_some_and(|requested| requested != &version)
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
    let locked_root = provenance
        .root_sha256
        .strip_prefix("sha256:")
        .unwrap_or(&provenance.root_sha256);
    if registry.name() != provenance.registry_name
        || registry.base_url().as_str() != provenance.registry_url
        || registry.root_sha256() != locked_root
    {
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
