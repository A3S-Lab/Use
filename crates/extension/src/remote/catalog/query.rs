use std::cmp::Ordering;
use std::collections::BTreeMap;

use a3s_use_core::{
    CatalogAvailability, PluginReleaseChannel, PluginSurfaceKind, UseError, UseResult,
    VerifiedPluginCatalogRecord,
};
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};

use super::{
    catalog_cursor_error, catalog_input_error, registry_target_error, CatalogEntry, LoadedCatalog,
    PluginCatalogAvailability, PluginCatalogHost, PluginCatalogInspection, PluginCatalogPage,
    PluginCatalogSearch, MAX_PLUGIN_CATALOG_PAGE_BYTES,
};

pub(super) fn search_catalog(
    catalog: LoadedCatalog,
    host: &PluginCatalogHost,
    search: &PluginCatalogSearch,
) -> UseResult<PluginCatalogPage> {
    let query_digest = search_digest(host, search);
    let snapshot_digest = catalog.snapshot.snapshot_digest.clone();
    let mut matches = compatible_entries(catalog.entries, host)?
        .into_iter()
        .filter_map(|entry| catalog_match_score(&entry.plugin, search).map(|score| (score, entry)))
        .collect::<Vec<_>>();
    matches.sort_by(compare_search_match);

    let start = match search.cursor.as_deref() {
        Some(cursor) => parse_cursor(cursor, &snapshot_digest, &query_digest, matches.len())?,
        None => 0,
    };
    let total_matches = u64::try_from(matches.len()).map_err(|error| {
        registry_target_error(format!("The catalog result count is invalid: {error}"))
    })?;
    let mut plugins = Vec::new();
    let mut end = start;
    while end < matches.len() && plugins.len() < usize::from(search.limit) {
        plugins.push(matches[end].1.plugin.clone());
        let provisional_end = end + 1;
        let next_cursor = (provisional_end < matches.len())
            .then(|| make_cursor(&snapshot_digest, &query_digest, provisional_end));
        let page = PluginCatalogPage {
            snapshot: catalog.snapshot.clone(),
            total_matches,
            plugins: plugins.clone(),
            next_cursor,
        };
        let bytes = encode_page(&page)?;
        if bytes.len() > MAX_PLUGIN_CATALOG_PAGE_BYTES {
            plugins.pop();
            if plugins.is_empty() {
                return Err(registry_target_error(
                    "One verified catalog record exceeds the response-size limit.",
                ));
            }
            break;
        }
        end = provisional_end;
    }

    let next_cursor =
        (end < matches.len()).then(|| make_cursor(&snapshot_digest, &query_digest, end));
    let page = PluginCatalogPage {
        snapshot: catalog.snapshot,
        total_matches,
        plugins,
        next_cursor,
    };
    if encode_page(&page)?.len() > MAX_PLUGIN_CATALOG_PAGE_BYTES {
        return Err(registry_target_error(
            "The verified catalog page exceeds its response-size limit.",
        ));
    }
    Ok(page)
}

pub(super) fn inspect_catalog(
    catalog: LoadedCatalog,
    host: &PluginCatalogHost,
    package_id: &str,
    requested_version: Option<&str>,
    requested_channel: Option<PluginReleaseChannel>,
) -> UseResult<PluginCatalogInspection> {
    if !super::super::super::valid_package_id(package_id) {
        return Err(catalog_input_error(
            "Plugin package IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    let requested_version = requested_version
        .map(|value| {
            Version::parse(value)
                .map_err(|error| catalog_input_error(format!("Invalid plugin version: {error}")))
                .and_then(|version| {
                    if version.to_string() == value {
                        Ok(version)
                    } else {
                        Err(catalog_input_error(
                            "Plugin versions must use canonical semantic version syntax.",
                        ))
                    }
                })
        })
        .transpose()?;

    let matching = catalog
        .entries
        .into_iter()
        .filter(|entry| {
            entry.plugin.record.package_id == package_id
                && requested_version
                    .as_ref()
                    .is_none_or(|version| version == &entry.version)
                && requested_channel.is_none_or(|channel| channel == entry.plugin.record.channel)
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Err(UseError::new(
            "use.extension.catalog_package_missing",
            format!("The verified catalog has no matching '{package_id}' release."),
        ));
    }

    let mut compatible = compatible_entries(matching, host)?;
    if compatible.is_empty() {
        return Err(UseError::new(
            "use.extension.catalog_package_incompatible",
            format!(
                "The verified catalog has no '{package_id}' release compatible with A3S Use {} on '{}'.",
                host.use_version, host.target
            ),
        ));
    }
    compatible.sort_by(compare_inspection_candidate);
    let plugin = compatible
        .pop()
        .ok_or_else(|| registry_target_error("The compatible catalog selection is empty."))?
        .plugin;
    let inspection = PluginCatalogInspection {
        snapshot: catalog.snapshot,
        plugin,
    };
    let size = serde_json::to_vec(&inspection)
        .map_err(|error| registry_target_error(format!("Failed to encode inspection: {error}")))?
        .len();
    if size > MAX_PLUGIN_CATALOG_PAGE_BYTES {
        return Err(registry_target_error(
            "The verified plugin inspection exceeds its response-size limit.",
        ));
    }
    Ok(inspection)
}

fn compatible_entries(
    entries: Vec<CatalogEntry>,
    host: &PluginCatalogHost,
) -> UseResult<Vec<CatalogEntry>> {
    let host_version = host.parsed_use_version()?;
    let mut selected = BTreeMap::<(String, Version, PluginReleaseChannel), CatalogEntry>::new();
    for entry in entries {
        let record = &entry.plugin.record;
        let requirement = VersionReq::parse(&record.requires_use).map_err(|error| {
            registry_target_error(format!(
                "Catalog record '{}' has an invalid A3S Use requirement: {error}",
                record.package_id
            ))
        })?;
        if (record.target != host.target && record.target != "any")
            || !requirement.matches(&host_version)
        {
            continue;
        }
        let key = (
            record.package_id.clone(),
            entry.version.clone(),
            record.channel,
        );
        match selected.get(&key) {
            None => {
                selected.insert(key, entry);
            }
            Some(current)
                if current.plugin.record.target == "any" && record.target == host.target =>
            {
                selected.insert(key, entry);
            }
            Some(current)
                if current.plugin.record.target == host.target && record.target == "any" => {}
            Some(_) => {
                return Err(registry_target_error(
                    "The signed catalog resolves the same plugin release to multiple targets.",
                ));
            }
        }
    }
    Ok(selected.into_values().collect())
}

fn catalog_match_score(
    plugin: &VerifiedPluginCatalogRecord,
    search: &PluginCatalogSearch,
) -> Option<u8> {
    let record = &plugin.record;
    if search
        .kind
        .is_some_and(|kind| !record.surfaces.iter().any(|surface| surface.kind == kind))
        || search
            .channel
            .is_some_and(|channel| record.channel != channel)
        || search
            .publisher
            .as_deref()
            .is_some_and(|publisher| record.publisher != publisher)
        || search
            .category
            .as_deref()
            .is_some_and(|category| !record.categories.iter().any(|value| value == category))
        || search
            .availability
            .is_some_and(|availability| availability_kind(&record.availability) != availability)
    {
        return None;
    }

    let query = search.query.to_lowercase();
    let package_id = record.package_id.to_lowercase();
    let display_name = record.display_name.to_lowercase();
    if package_id == query {
        return Some(0);
    }
    if display_name == query {
        return Some(1);
    }
    if package_id.starts_with(&query) {
        return Some(2);
    }
    if display_name.starts_with(&query) {
        return Some(3);
    }
    let searchable = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        package_id,
        display_name,
        record.description.to_lowercase(),
        record.publisher,
        record.keywords.join("\n"),
        record.categories.join("\n")
    );
    query
        .split_whitespace()
        .all(|token| searchable.contains(token))
        .then_some(4)
}

fn compare_search_match(left: &(u8, CatalogEntry), right: &(u8, CatalogEntry)) -> Ordering {
    left.0
        .cmp(&right.0)
        .then_with(|| {
            left.1
                .plugin
                .record
                .package_id
                .cmp(&right.1.plugin.record.package_id)
        })
        .then_with(|| right.1.version.cmp(&left.1.version))
        .then_with(|| {
            channel_rank(left.1.plugin.record.channel)
                .cmp(&channel_rank(right.1.plugin.record.channel))
        })
        .then_with(|| {
            left.1
                .plugin
                .record
                .target
                .cmp(&right.1.plugin.record.target)
        })
}

fn compare_inspection_candidate(left: &CatalogEntry, right: &CatalogEntry) -> Ordering {
    left.version
        .cmp(&right.version)
        .then_with(|| {
            channel_rank(right.plugin.record.channel).cmp(&channel_rank(left.plugin.record.channel))
        })
        .then_with(|| left.plugin.record.target.cmp(&right.plugin.record.target))
}

const fn channel_rank(channel: PluginReleaseChannel) -> u8 {
    match channel {
        PluginReleaseChannel::Stable => 0,
        PluginReleaseChannel::Beta => 1,
        PluginReleaseChannel::Nightly => 2,
    }
}

const fn availability_kind(availability: &CatalogAvailability) -> PluginCatalogAvailability {
    match availability {
        CatalogAvailability::Available => PluginCatalogAvailability::Available,
        CatalogAvailability::Deprecated { .. } => PluginCatalogAvailability::Deprecated,
        CatalogAvailability::Withdrawn { .. } => PluginCatalogAvailability::Withdrawn,
    }
}

fn search_digest(host: &PluginCatalogHost, search: &PluginCatalogSearch) -> String {
    let mut hasher = Sha256::new();
    update_digest_field(&mut hasher, &host.target);
    update_digest_field(&mut hasher, &host.use_version);
    update_digest_field(&mut hasher, &search.query.to_lowercase());
    update_digest_field(
        &mut hasher,
        search.kind.map(surface_kind_name).unwrap_or_default(),
    );
    update_digest_field(
        &mut hasher,
        search
            .channel
            .map(PluginReleaseChannel::as_str)
            .unwrap_or_default(),
    );
    update_digest_field(&mut hasher, search.publisher.as_deref().unwrap_or_default());
    update_digest_field(&mut hasher, search.category.as_deref().unwrap_or_default());
    update_digest_field(
        &mut hasher,
        search
            .availability
            .map(availability_name)
            .unwrap_or_default(),
    );
    format!("{:x}", hasher.finalize())
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

const fn availability_name(availability: PluginCatalogAvailability) -> &'static str {
    match availability {
        PluginCatalogAvailability::Available => "available",
        PluginCatalogAvailability::Deprecated => "deprecated",
        PluginCatalogAvailability::Withdrawn => "withdrawn",
    }
}

fn update_digest_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn make_cursor(snapshot_digest: &str, query_digest: &str, offset: usize) -> String {
    let snapshot_digest = snapshot_digest
        .strip_prefix("sha256:")
        .unwrap_or(snapshot_digest);
    format!("v1.{snapshot_digest}.{query_digest}.{offset}")
}

fn parse_cursor(
    cursor: &str,
    snapshot_digest: &str,
    query_digest: &str,
    result_count: usize,
) -> UseResult<usize> {
    let mut parts = cursor.split('.');
    let version = parts.next();
    let cursor_snapshot = parts.next();
    let cursor_query = parts.next();
    let offset = parts.next();
    if version != Some("v1")
        || parts.next().is_some()
        || cursor_snapshot.is_none_or(|value| !valid_raw_sha256(value))
        || cursor_query.is_none_or(|value| !valid_raw_sha256(value))
    {
        return Err(catalog_cursor_error(
            "use.extension.catalog_cursor_invalid",
            "The catalog cursor is malformed.",
        ));
    }
    let expected_snapshot = snapshot_digest
        .strip_prefix("sha256:")
        .unwrap_or(snapshot_digest);
    if cursor_snapshot != Some(expected_snapshot) {
        return Err(catalog_cursor_error(
            "use.extension.catalog_cursor_stale",
            "The verified catalog snapshot changed; restart the search.",
        ));
    }
    if cursor_query != Some(query_digest) {
        return Err(catalog_cursor_error(
            "use.extension.catalog_cursor_invalid",
            "The catalog cursor belongs to a different query.",
        ));
    }
    let offset = offset
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value <= result_count)
        .ok_or_else(|| {
            catalog_cursor_error(
                "use.extension.catalog_cursor_invalid",
                "The catalog cursor offset is invalid.",
            )
        })?;
    Ok(offset)
}

fn valid_raw_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn encode_page(page: &PluginCatalogPage) -> UseResult<Vec<u8>> {
    serde_json::to_vec(page).map_err(|error| {
        registry_target_error(format!(
            "Failed to encode the verified catalog page: {error}"
        ))
    })
}
