use std::collections::HashMap;
use std::path::PathBuf;

use a3s_use_core::{
    CatalogAvailability, CatalogSurface, PluginCatalogRecord, PluginPackageLockHost,
    PluginReleaseChannel, PluginSurfaceKind,
};
use sha2::{Digest, Sha256};

use super::test_support::{
    package_directory_archive, TestRepository, TestServer, TestTarget, EXPIRED, FUTURE,
};
use super::*;

const COMPLETE_CATALOG: &[u8] =
    include_bytes!("../../core/fixtures/plugins/complete-package-catalog-v1.json");
const OKF_CATALOG_V3: &[u8] =
    include_bytes!("../../core/fixtures/plugins/catalog-record-okf-v3.json");
const FIXTURE_ROOT: &[u8] = include_bytes!("../fixtures/registry/plugin-v3/metadata/root.json");
const FIXTURE_TARGETS: &[u8] =
    include_bytes!("../fixtures/registry/plugin-v3/metadata/targets.json");
const FIXTURE_SNAPSHOT: &[u8] =
    include_bytes!("../fixtures/registry/plugin-v3/metadata/snapshot.json");
const FIXTURE_TIMESTAMP: &[u8] =
    include_bytes!("../fixtures/registry/plugin-v3/metadata/timestamp.json");
const FIXTURE_ROOT_SHA256: &str =
    include_str!("../fixtures/registry/plugin-v3/root.sha256").trim_ascii_end();

#[tokio::test]
async fn complete_signed_fixture_is_searchable_and_inspectable_without_archive_download() {
    let routes = HashMap::from([
        (
            "/metadata/root.json".to_owned(),
            canonical_fixture(FIXTURE_ROOT).to_vec(),
        ),
        (
            "/metadata/targets.json".to_owned(),
            canonical_fixture(FIXTURE_TARGETS).to_vec(),
        ),
        (
            "/metadata/snapshot.json".to_owned(),
            canonical_fixture(FIXTURE_SNAPSHOT).to_vec(),
        ),
        (
            "/metadata/timestamp.json".to_owned(),
            canonical_fixture(FIXTURE_TIMESTAMP).to_vec(),
        ),
    ]);
    let server = TestServer::start(routes);
    let temp = tempfile::tempdir().unwrap();
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        FIXTURE_ROOT_SHA256,
        None,
        temp.path().join("tuf"),
    )
    .unwrap();
    let host = PluginCatalogHost::new("linux-x86_64", "0.3.0").unwrap();
    let search = catalog_search("literature", 20);

    let page = search_remote_plugins(&trusted, &host, &search)
        .await
        .unwrap();

    assert_eq!(page.snapshot.source, PluginCatalogSnapshotSource::Refreshed);
    assert_eq!(page.snapshot.metadata.targets_version, 7);
    assert_eq!(page.snapshot.catalog_records, 1);
    assert!(page.snapshot.snapshot_digest.starts_with("sha256:"));
    assert_eq!(page.total_matches, 1);
    assert_eq!(page.plugins[0].record.package_id, "acme/research");
    assert_eq!(page.plugins[0].record.surfaces.len(), 5);
    assert_eq!(
        page.plugins[0].provenance.catalog_record_digest,
        page.plugins[0].record.descriptor_digest().unwrap()
    );
    let inspection = inspect_remote_plugin(
        &trusted,
        &host,
        "acme/research",
        Some("2.0.0"),
        Some(PluginReleaseChannel::Stable),
    )
    .await
    .unwrap();
    assert_eq!(
        inspection.plugin.record.permission_ceiling.surfaces.len(),
        4
    );
    assert_eq!(
        inspection.plugin.provenance.root_sha256,
        FIXTURE_ROOT_SHA256
    );
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn package_graph_resolution_and_download_replay_the_exact_verified_lock() {
    let package_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    let archive = package_directory_archive(&package_root);
    let mut catalog: serde_json::Value = serde_json::from_slice(OKF_CATALOG_V3).unwrap();
    let target = host_target().unwrap();
    catalog["target"] = serde_json::json!(target);
    catalog["requiresUse"] = serde_json::json!(">=0.3.0, <0.4.0");
    let original_target_name = catalog["archive"]["targetName"]
        .as_str()
        .unwrap()
        .to_string();
    catalog["archive"]["targetName"] =
        serde_json::json!(original_target_name.replace("linux-x86_64", &target));
    let target_name = catalog["archive"]["targetName"]
        .as_str()
        .unwrap()
        .to_string();
    let repository = TestRepository::with_target_metadata(archive, target_name, catalog, 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/knowledge",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(lock.root_package_id, "acme/knowledge");
    assert_eq!(lock.packages.len(), 1);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));

    server.clear_requests();
    let downloads = download_locked_remote_packages(&lock, &[trusted])
        .await
        .unwrap();
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].resolved().package_id, "acme/knowledge");
    assert!(downloads[0].path().is_file());
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("/targets/"))
            .count(),
        1
    );
}

#[tokio::test]
async fn package_graph_downloads_the_complete_dependency_closure_in_install_order() {
    let package_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    let archive = package_directory_archive(&package_root);
    let target = host_target().unwrap();
    let mut base: serde_json::Value = serde_json::from_slice(OKF_CATALOG_V3).unwrap();
    base["packageId"] = serde_json::json!("acme/base");
    base["displayName"] = serde_json::json!("Base Knowledge");
    base["description"] = serde_json::json!("Dependency closure base fixture.");
    base["repository"] = serde_json::json!("https://github.com/acme/base");
    base["target"] = serde_json::json!(&target);
    base["requiresUse"] = serde_json::json!(">=0.3.0, <0.4.0");
    base["archive"]["targetName"] = serde_json::json!(format!(
        "extensions/acme/base/1.0.0/stable/{target}/acme-base-1.0.0-{target}.tar.gz"
    ));

    let mut root = base.clone();
    root["packageId"] = serde_json::json!("acme/root");
    root["displayName"] = serde_json::json!("Root Knowledge");
    root["description"] = serde_json::json!("Dependency closure root fixture.");
    root["repository"] = serde_json::json!("https://github.com/acme/root");
    root["dependencies"] = serde_json::json!([{
        "packageId": "acme/base",
        "versionRequirement": "^1.0.0"
    }]);
    root["archive"]["targetName"] = serde_json::json!(format!(
        "extensions/acme/root/1.0.0/stable/{target}/acme-root-1.0.0-{target}.tar.gz"
    ));
    let base_target = base["archive"]["targetName"].as_str().unwrap().to_string();
    let root_target = root["archive"]["targetName"].as_str().unwrap().to_string();
    let repository = TestRepository::with_targets(
        vec![
            TestTarget {
                archive: archive.clone(),
                target_name: root_target,
                custom: Some(root),
            },
            TestTarget {
                archive,
                target_name: base_target,
                custom: Some(base),
            },
        ],
        9,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/root",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        lock.install_order()
            .unwrap()
            .into_iter()
            .map(|package| package.package_id())
            .collect::<Vec<_>>(),
        ["acme/base", "acme/root"]
    );

    server.clear_requests();
    let downloads = download_locked_remote_packages(&lock, &[trusted])
        .await
        .unwrap();
    assert_eq!(
        downloads
            .iter()
            .map(|package| package.resolved().package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/base", "acme/root"]
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("/targets/"))
            .count(),
        2
    );
}

#[tokio::test]
async fn catalog_search_filters_sorts_and_paginates_deterministically() {
    let target = host_target().unwrap();
    let repository = catalog_repository(
        &target,
        &[
            CatalogSpec::stable("acme/alpha", "1.0.0", "Alpha Research"),
            CatalogSpec::stable("acme/beta", "2.0.0", "Beta Research"),
            CatalogSpec {
                package_id: "acme/gamma",
                version: "3.0.0",
                display_name: "Gamma Research",
                channel: PluginReleaseChannel::Beta,
                availability: CatalogAvailability::Deprecated {
                    message: "Use acme/beta for new work.".to_owned(),
                    replacement: Some("acme/beta".to_owned()),
                },
                requires_use: ">=0.3.0, <0.4.0",
            },
        ],
        7,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let host = PluginCatalogHost::new(&target, "0.3.0").unwrap();

    let first = search_remote_plugins(&trusted, &host, &catalog_search("research", 1))
        .await
        .unwrap();
    assert_eq!(first.total_matches, 3);
    assert_eq!(first.plugins[0].record.package_id, "acme/alpha");
    assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_PLUGIN_CATALOG_PAGE_BYTES);
    let resolved = ResolvedRemotePackage::from_verified_catalog(&first.plugins[0]).unwrap();
    assert_eq!(resolved.package_id, "acme/alpha");
    assert_eq!(
        resolved.target_name,
        first.plugins[0].record.archive.target_name
    );
    assert_eq!(
        format!("sha256:{}", resolved.sha256),
        first.plugins[0].record.archive.sha256
    );
    assert_eq!(resolved.plan_digest().unwrap().len(), 64);
    let cursor = first.next_cursor.clone().unwrap();

    let mut second_search = catalog_search("research", 2);
    second_search.cursor = Some(cursor);
    let second = search_remote_plugins(&trusted, &host, &second_search)
        .await
        .unwrap();
    assert_eq!(
        second
            .plugins
            .iter()
            .map(|plugin| plugin.record.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/beta", "acme/gamma"]
    );
    assert!(second.next_cursor.is_none());

    let mut filtered = catalog_search("research", 20);
    filtered.kind = Some(PluginSurfaceKind::Tool);
    filtered.channel = Some(PluginReleaseChannel::Beta);
    filtered.publisher = Some("acme".to_owned());
    filtered.category = Some("science".to_owned());
    filtered.availability = Some(PluginCatalogAvailability::Deprecated);
    let filtered = search_remote_plugins(&trusted, &host, &filtered)
        .await
        .unwrap();
    assert_eq!(filtered.total_matches, 1);
    assert_eq!(filtered.plugins[0].record.package_id, "acme/gamma");

    let browse = search_remote_plugins(&trusted, &host, &catalog_search("", 2))
        .await
        .unwrap();
    assert_eq!(browse.total_matches, 3);
    assert_eq!(
        browse
            .plugins
            .iter()
            .map(|plugin| plugin.record.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/alpha", "acme/beta"]
    );
    assert!(browse.next_cursor.is_some());

    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn cached_search_reverifies_the_last_snapshot_without_network() {
    let target = host_target().unwrap();
    let repository = catalog_repository(
        &target,
        &[CatalogSpec::stable(
            "acme/research",
            "2.0.0",
            "Research Toolkit",
        )],
        9,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());
    let host = PluginCatalogHost::new(&target, "0.3.0").unwrap();
    let search = catalog_search("research", 20);

    let refreshed = search_remote_plugins(&trusted, &host, &search)
        .await
        .unwrap();
    server.clear_requests();
    let cached = search_cached_plugins(&trusted, &host, &search)
        .await
        .unwrap();

    assert_eq!(cached.snapshot.source, PluginCatalogSnapshotSource::Cached);
    assert_eq!(
        cached.snapshot.snapshot_digest,
        refreshed.snapshot.snapshot_digest
    );
    assert_eq!(
        cached.snapshot.verified_at_unix_seconds,
        refreshed.snapshot.verified_at_unix_seconds
    );
    assert_eq!(cached.plugins, refreshed.plugins);
    assert!(server.requests().is_empty());

    let targets_path = datastore.join("catalog-metadata").join("targets.json");
    let mut targets = std::fs::read(&targets_path).unwrap();
    let position = super::test_support::find_subslice(&targets, b"Research Toolkit").unwrap();
    targets[position] = b'X';
    std::fs::write(&targets_path, targets).unwrap();
    let error = search_cached_plugins(&trusted, &host, &search)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_cache_changed");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn incompatible_expired_and_archive_drift_catalogs_fail_closed() {
    let target = host_target().unwrap();
    let incompatible = catalog_repository(
        &target,
        &[CatalogSpec {
            package_id: "acme/future",
            version: "4.0.0",
            display_name: "Future Research",
            channel: PluginReleaseChannel::Stable,
            availability: CatalogAvailability::Available,
            requires_use: ">=0.4.0, <0.5.0",
        }],
        1,
        FUTURE,
    );
    let server = TestServer::start(incompatible.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &incompatible, temp.path().join("incompatible"));
    let host = PluginCatalogHost::new(&target, "0.3.0").unwrap();
    let page = search_remote_plugins(&trusted, &host, &catalog_search("future", 20))
        .await
        .unwrap();
    assert!(page.plugins.is_empty());
    let error = inspect_remote_plugin(&trusted, &host, "acme/future", None, None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_package_incompatible");

    let expired = catalog_repository(
        &target,
        &[CatalogSpec::stable(
            "acme/expired",
            "1.0.0",
            "Expired Research",
        )],
        1,
        EXPIRED,
    );
    let expired_server = TestServer::start(expired.routes.clone());
    let expired_registry = trusted_registry(&expired_server, &expired, temp.path().join("expired"));
    let error = search_remote_plugins(&expired_registry, &host, &catalog_search("expired", 20))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_untrusted");

    let archive = b"signed target".to_vec();
    let mut record = catalog_record(
        &target,
        CatalogSpec::stable("acme/drift", "1.0.0", "Drift Research"),
        &archive,
    );
    record.archive.sha256 =
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned();
    record.validate().unwrap();
    let drift = TestRepository::with_target_metadata(
        archive,
        record.archive.target_name.clone(),
        serde_json::to_value(record).unwrap(),
        1,
        FUTURE,
    );
    let drift_server = TestServer::start(drift.routes.clone());
    let drift_registry = trusted_registry(&drift_server, &drift, temp.path().join("drift"));
    let error = search_remote_plugins(&drift_registry, &host, &catalog_search("drift", 20))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_target_invalid");
    assert!(drift_server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn catalog_query_and_cursor_bounds_are_enforced_before_payload_download() {
    let target = host_target().unwrap();
    let first_repository = catalog_repository(
        &target,
        &[
            CatalogSpec::stable("acme/alpha", "1.0.0", "Alpha Research"),
            CatalogSpec::stable("acme/beta", "1.0.0", "Beta Research"),
        ],
        1,
        FUTURE,
    );
    let server = TestServer::start(first_repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &first_repository, datastore);
    let host = PluginCatalogHost::new(&target, "0.3.0").unwrap();

    let invalid = catalog_search(&"x".repeat(257), 20);
    let error = search_remote_plugins(&trusted, &host, &invalid)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_query_invalid");
    assert!(server.requests().is_empty());

    let first = search_remote_plugins(&trusted, &host, &catalog_search("research", 1))
        .await
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    let second_repository = catalog_repository(
        &target,
        &[
            CatalogSpec::stable("acme/alpha", "1.0.0", "Alpha Research"),
            CatalogSpec::stable("acme/beta", "1.0.0", "Beta Research"),
        ],
        2,
        FUTURE,
    );
    assert_eq!(first_repository.root_sha256, second_repository.root_sha256);
    server.replace_routes(second_repository.routes);
    let mut stale = catalog_search("research", 1);
    stale.cursor = Some(cursor);
    let error = search_remote_plugins(&trusted, &host, &stale)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_cursor_stale");
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn catalog_page_stays_below_the_serialized_output_bound() {
    let target = host_target().unwrap();
    let mut targets = Vec::new();
    for index in 0..50 {
        let archive = format!("padded-{index:02}").into_bytes();
        let mut record = catalog_record(
            &target,
            CatalogSpec::stable("acme/padded", "1.0.0", "Padded Research"),
            &archive,
        );
        record.package_id = format!("acme/padded-{index:02}");
        record.display_name = format!("Padded Research {index:02}");
        record.repository = format!("https://github.com/acme/padded-{index:02}");
        record.archive.target_name = format!(
            "extensions/{}/1.0.0/stable/{}/padded-{index:02}-1.0.0-{target}.tar.gz",
            record.package_id, target
        );
        for surface_index in 0..240 {
            record.surfaces.push(CatalogSurface {
                kind: PluginSurfaceKind::Skill,
                id: format!("padding-{surface_index:03}-{}", "x".repeat(51)),
                optional: true,
                workload: None,
                mcp_transport: None,
                mcp_tool_count: None,
                okf_bundle: None,
                requires: Vec::new(),
            });
        }
        record
            .surfaces
            .sort_by(|left, right| (left.kind, &left.id).cmp(&(right.kind, &right.id)));
        record.validate().unwrap();
        targets.push(TestTarget {
            target_name: record.archive.target_name.clone(),
            custom: Some(serde_json::to_value(record).unwrap()),
            archive,
        });
    }
    let repository = TestRepository::with_targets(targets, 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let host = PluginCatalogHost::new(&target, "0.3.0").unwrap();

    let page = search_remote_plugins(&trusted, &host, &catalog_search("padded", 50))
        .await
        .unwrap();

    assert_eq!(page.total_matches, 50);
    assert!(page.plugins.len() < 50);
    assert!(!page.plugins.is_empty());
    assert!(page.next_cursor.is_some());
    assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PLUGIN_CATALOG_PAGE_BYTES);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[derive(Clone)]
struct CatalogSpec {
    package_id: &'static str,
    version: &'static str,
    display_name: &'static str,
    channel: PluginReleaseChannel,
    availability: CatalogAvailability,
    requires_use: &'static str,
}

impl CatalogSpec {
    fn stable(package_id: &'static str, version: &'static str, display_name: &'static str) -> Self {
        Self {
            package_id,
            version,
            display_name,
            channel: PluginReleaseChannel::Stable,
            availability: CatalogAvailability::Available,
            requires_use: ">=0.3.0, <0.4.0",
        }
    }
}

fn catalog_repository(
    target: &str,
    specs: &[CatalogSpec],
    metadata_version: u64,
    expires: &str,
) -> TestRepository {
    let targets = specs
        .iter()
        .cloned()
        .map(|spec| {
            let archive = format!("{}-{}", spec.package_id, spec.version).into_bytes();
            let record = catalog_record(target, spec, &archive);
            TestTarget {
                target_name: record.archive.target_name.clone(),
                custom: Some(serde_json::to_value(record).unwrap()),
                archive,
            }
        })
        .collect();
    TestRepository::with_targets(targets, metadata_version, expires)
}

fn catalog_record(target: &str, spec: CatalogSpec, archive: &[u8]) -> PluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(COMPLETE_CATALOG).unwrap();
    let name = spec.package_id.split('/').nth(1).unwrap();
    record.package_id = spec.package_id.to_owned();
    record.display_name = spec.display_name.to_owned();
    record.description = format!(
        "{} provides deterministic research capabilities.",
        spec.display_name
    );
    record.publisher = spec.package_id.split('/').next().unwrap().to_owned();
    record.keywords = vec!["research".to_owned()];
    record.categories = vec!["science".to_owned()];
    record.version = spec.version.to_owned();
    record.channel = spec.channel;
    record.requires_use = spec.requires_use.to_owned();
    record.target = target.to_owned();
    record.archive.target_name = format!(
        "extensions/{}/{}/{}/{}/{}-{}-{}.tar.gz",
        spec.package_id,
        spec.version,
        spec.channel.as_str(),
        target,
        name,
        spec.version,
        target
    );
    record.archive.length = archive.len() as u64;
    record.archive.sha256 = format!("sha256:{:x}", Sha256::digest(archive));
    record.repository = format!("https://github.com/acme/{name}");
    record.availability = spec.availability;
    record.validate().unwrap();
    record
}

fn catalog_search(query: &str, limit: u16) -> PluginCatalogSearch {
    PluginCatalogSearch {
        query: query.to_owned(),
        kind: None,
        channel: None,
        publisher: None,
        category: None,
        availability: None,
        cursor: None,
        limit,
    }
}

fn trusted_registry(
    server: &TestServer,
    repository: &TestRepository,
    datastore: PathBuf,
) -> TrustedRegistry {
    TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        datastore,
    )
    .unwrap()
}

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
