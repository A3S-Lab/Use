use std::path::PathBuf;

use a3s_use_core::{
    CatalogPlanningTarget, ExecutablePlanningSurface, PlanPackageRole, PlanningArtifactRef,
    PlanningSurfaceActivation, PluginCatalogRecord, PluginPlanningBundle, PluginSurfaceKind,
    PluginSurfaceRef, ToolReleaseDescriptor, PLUGIN_CATALOG_SCHEMA_V2, PLUGIN_CATALOG_SCHEMA_V3,
    PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use sha2::{Digest, Sha256};

use super::test_support::{
    extension_archive, find_subslice, TestRepository, TestServer, TestTarget, EXPIRED, FUTURE,
    PACKAGE_VERSION,
};
use super::*;
use crate::{ExtensionPaths, ExtensionRegistry, ExtensionTrust};

const COMPLETE_CATALOG: &[u8] =
    include_bytes!("../../core/fixtures/plugins/complete-package-catalog-v1.json");

#[tokio::test]
async fn tuf_refresh_verifies_metadata_without_downloading_targets() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let metadata = refresh_remote_registry(&trusted).await.unwrap();

    assert_eq!(metadata.registry_name, "fixture");
    assert_eq!(metadata.root_version, 1);
    assert_eq!(metadata.timestamp_version, 7);
    assert_eq!(metadata.snapshot_version, 7);
    assert_eq!(metadata.targets_version, 7);
    assert_eq!(metadata.package_targets, 1);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_catalog_lists_signed_packages_without_downloading_targets() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let catalog = list_remote_packages(&trusted).await.unwrap();

    assert_eq!(catalog.metadata.registry_name, "fixture");
    assert_eq!(catalog.metadata.package_targets, 1);
    assert_eq!(catalog.packages.len(), 1);
    assert_eq!(catalog.packages[0].package_id, "a3s/science");
    assert_eq!(catalog.packages[0].version, PACKAGE_VERSION);
    assert_eq!(catalog.packages[0].target, catalog.host_target);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_install_records_signed_provenance_and_converges() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive, 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().plan_digest().unwrap();
    drop(prepared);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));

    let paths = ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("extension-state"),
    );
    let registry = ExtensionRegistry::new(paths);
    let installed = registry
        .install_remote(
            "a3s/science",
            &trusted,
            None,
            "stable",
            Some(&digest),
            false,
        )
        .await
        .unwrap();
    assert!(installed.changed);
    assert_eq!(
        installed.extension.receipt.trust,
        ExtensionTrust::RegistryTuf
    );
    let provenance = installed.extension.receipt.registry.as_ref().unwrap();
    assert_eq!(provenance.package_id, "a3s/science");
    assert_eq!(provenance.version, PACKAGE_VERSION);
    assert_eq!(provenance.sha256, repository.target_sha256);
    assert_eq!(
        installed.extension.plan_ready_catalog().unwrap_err().code,
        "use.extension.plan_evidence_missing"
    );
    assert!(installed.extension.cli_executable().unwrap().is_file());
    let package_root = &installed.extension.receipt.package_root;
    assert!(package_root.join("web/activity.html").is_file());
    assert!(package_root.join("web/activity.css").is_file());
    assert!(package_root.join("web/activity.js").is_file());

    server.clear_requests();
    let second = registry
        .install_remote(
            "a3s/science",
            &trusted,
            None,
            "stable",
            Some(&digest),
            false,
        )
        .await
        .unwrap();
    assert!(!second.changed);
    assert_eq!(registry.list().await.unwrap().len(), 1);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn catalog_v3_loads_only_the_exact_signed_planning_target() {
    let (repository, expected, archive_target, planning_target) = planning_test_repository(false);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();
    let actual = prepared.load_planning_bundle().await.unwrap().unwrap();

    assert_eq!(actual, expected);
    assert!(server
        .requests()
        .iter()
        .any(|request| request == &format!("/targets/{planning_target}")));
    assert!(server
        .requests()
        .iter()
        .all(|request| request != &format!("/targets/{archive_target}")));
}

#[tokio::test]
async fn catalog_v3_static_package_has_no_planning_target_download() {
    let archive = extension_archive(PACKAGE_VERSION);
    let target = host_target().unwrap();
    let archive_target = format!(
        "extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/a3s-use-science-{PACKAGE_VERSION}-{target}.tar.gz"
    );
    let mut catalog = PluginCatalogRecord::from_json(COMPLETE_CATALOG).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    catalog.package_id = "a3s/science".to_owned();
    catalog.display_name = "A3S Science".to_owned();
    catalog.description = "Static scientific guidance for A3S agents.".to_owned();
    catalog.publisher = "a3s".to_owned();
    catalog.version = PACKAGE_VERSION.to_owned();
    catalog.requires_use = ">=0.3.0, <0.4.0".to_owned();
    catalog.target = target;
    catalog
        .surfaces
        .retain(|surface| surface.kind == PluginSurfaceKind::Skill && surface.id == "review");
    catalog.permission_ceiling.surfaces.clear();
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    catalog.planning = None;
    catalog.archive.target_name = archive_target.clone();
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.manifest_sha256 = Some(format!("sha256:{}", "c".repeat(64)));
    catalog.repository = "https://github.com/A3S-Lab/Science".to_owned();
    catalog.validate().unwrap();
    let repository = TestRepository::with_target_metadata(
        archive,
        archive_target,
        serde_json::to_value(catalog).unwrap(),
        13,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();
    assert!(prepared.load_planning_bundle().await.unwrap().is_none());
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn catalog_v3_rejects_planning_target_metadata_drift_before_download() {
    let (repository, _, archive_target, planning_target) = planning_test_repository(true);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();
    let error = prepared.load_planning_bundle().await.unwrap_err();

    assert_eq!(error.code, "use.extension.registry_planning_target_invalid");
    assert!(server.requests().iter().all(|request| {
        request != &format!("/targets/{planning_target}")
            && request != &format!("/targets/{archive_target}")
    }));
}

#[tokio::test]
async fn tuf_catalog_v2_install_persists_and_revalidates_plan_ready_evidence() {
    let archive = extension_archive(PACKAGE_VERSION);
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("science.tar.gz");
    tokio::fs::write(&archive_path, &archive).await.unwrap();
    let source = crate::source::prepare_package_source(&archive_path)
        .await
        .unwrap();
    let fingerprint = crate::digest::package_fingerprint(source.root())
        .await
        .unwrap();
    let manifest_bytes = tokio::fs::read(source.root().join("a3s-use-extension.acl"))
        .await
        .unwrap();
    let target = host_target().unwrap();
    let target_name = format!(
        "extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/a3s-use-science-{PACKAGE_VERSION}-{target}.tar.gz"
    );
    let mut catalog = PluginCatalogRecord::from_json(COMPLETE_CATALOG).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V2.to_owned();
    catalog.package_id = "a3s/science".to_owned();
    catalog.display_name = "A3S Science".to_owned();
    catalog.description = "Scientific research capabilities for A3S agents.".to_owned();
    catalog.publisher = "a3s".to_owned();
    catalog.version = PACKAGE_VERSION.to_owned();
    catalog.requires_use = ">=0.2.0, <0.4.0".to_owned();
    catalog.target = target;
    catalog.archive.target_name = target_name.clone();
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.byte_count;
    catalog.package.file_count = fingerprint.file_count;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.sha256));
    catalog.package.manifest_sha256 = Some(format!("sha256:{:x}", Sha256::digest(&manifest_bytes)));
    catalog.repository = "https://github.com/A3S-Lab/Science".to_owned();
    catalog
        .surfaces
        .iter_mut()
        .find(|surface| surface.kind == PluginSurfaceKind::Ui && surface.id == "review")
        .unwrap()
        .requires = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "index".to_owned(),
    }];
    catalog.validate().unwrap();

    let repository = TestRepository::with_target_metadata(
        archive,
        target_name,
        serde_json::to_value(&catalog).unwrap(),
        9,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let registry = ExtensionRegistry::new(ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("extension-state"),
    ));

    let installed = registry
        .install_remote("a3s/science", &trusted, None, "stable", None, false)
        .await
        .unwrap();

    assert_eq!(installed.extension.receipt.schema_version, 2);
    let verified = installed
        .extension
        .receipt
        .verified_catalog
        .as_ref()
        .unwrap();
    assert_eq!(verified.record, catalog);
    assert_eq!(
        installed.extension.receipt.registry.as_ref(),
        Some(&ResolvedRemotePackage::from_verified_catalog(verified).unwrap())
    );
    assert!(registry.get("a3s/science").await.unwrap().is_some());
    let removal = installed
        .extension
        .remove_transition(
            PlanPackageRole::Root,
            &[PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: "review".to_owned(),
            }],
        )
        .unwrap();
    assert_eq!(removal.before.as_ref().unwrap().release.surfaces.len(), 5);
    assert_eq!(removal.surfaces.len(), 5);
    assert!(removal.after.is_none());

    tokio::fs::write(
        installed.extension.cli_executable().unwrap(),
        b"tampered executable",
    )
    .await
    .unwrap();
    let error = registry.get("a3s/science").await.unwrap_err();
    assert_eq!(error.code, "use.extension.package_digest_mismatch");
}

#[tokio::test]
async fn reviewed_registry_plan_fails_before_target_download() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let error = prepare_remote_package(
        &trusted,
        "a3s/science",
        None,
        "stable",
        Some(&"0".repeat(64)),
    )
    .await
    .unwrap_err();

    assert_eq!(error.code, "use.extension.registry_plan_mismatch");
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_rejects_wrong_root_and_tampered_target() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive, 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let wrong = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        "f".repeat(64),
        None,
        temp.path().join("wrong-root"),
    )
    .unwrap();
    let error = prepare_remote_package(&wrong, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_root_mismatch");

    let mut routes = repository.routes.clone();
    routes.insert(
        format!("/targets/{}", repository.target_name),
        b"tampered archive".to_vec(),
    );
    let tampered_server = TestServer::start(routes);
    let trusted = trusted_registry(
        &tampered_server,
        &repository,
        temp.path().join("tampered-target"),
    );
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let error = prepared.download().await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_download_failed");
}

#[tokio::test]
async fn tuf_rejects_metadata_tampering_expiration_and_rollback() {
    let archive = extension_archive(PACKAGE_VERSION);
    let version_two = TestRepository::new(archive.clone(), 2, FUTURE);
    let server_two = TestServer::start(version_two.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("rollback-state");
    let trusted_two = trusted_registry(&server_two, &version_two, datastore.clone());
    prepare_remote_package(&trusted_two, "a3s/science", None, "stable", None)
        .await
        .unwrap();

    let version_one = TestRepository::new(archive.clone(), 1, FUTURE);
    assert_eq!(version_one.root_sha256, version_two.root_sha256);
    let server_one = TestServer::start(version_one.routes.clone());
    let trusted_one = trusted_registry(&server_one, &version_one, datastore);
    let rollback = prepare_remote_package(&trusted_one, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(rollback.code, "use.extension.registry_untrusted");

    let expired = TestRepository::new(archive.clone(), 1, EXPIRED);
    let expired_server = TestServer::start(expired.routes.clone());
    let expired_registry =
        trusted_registry(&expired_server, &expired, temp.path().join("expired-state"));
    let error = prepare_remote_package(&expired_registry, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_untrusted");

    let mut tampered_routes = version_one.routes.clone();
    let targets = tampered_routes.get_mut("/metadata/targets.json").unwrap();
    let position = find_subslice(targets, b"stable").unwrap();
    targets[position..position + 6].copy_from_slice(b"nightl");
    let tampered_server = TestServer::start(tampered_routes);
    let tampered_registry = trusted_registry(
        &tampered_server,
        &version_one,
        temp.path().join("tampered-metadata"),
    );
    let error = prepare_remote_package(&tampered_registry, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_untrusted");
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

fn planning_test_repository(
    mismatched_catalog_digest: bool,
) -> (TestRepository, PluginPlanningBundle, String, String) {
    let archive = extension_archive(PACKAGE_VERSION);
    let target = host_target().unwrap();
    let archive_target = format!(
        "extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/a3s-use-science-{PACKAGE_VERSION}-{target}.tar.gz"
    );
    let planning_target =
        format!("extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/planning-v1.json");
    let mut catalog = PluginCatalogRecord::from_json(COMPLETE_CATALOG).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    catalog.package_id = "a3s/science".to_owned();
    catalog.display_name = "A3S Science".to_owned();
    catalog.description = "Scientific research capabilities for A3S agents.".to_owned();
    catalog.publisher = "a3s".to_owned();
    catalog.version = PACKAGE_VERSION.to_owned();
    catalog.requires_use = ">=0.3.0, <0.4.0".to_owned();
    catalog.target = target;
    catalog.archive.target_name = archive_target.clone();
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.manifest_sha256 = Some(format!("sha256:{}", "c".repeat(64)));
    catalog.repository = "https://github.com/A3S-Lab/Science".to_owned();
    catalog.surfaces = vec![catalog
        .surfaces
        .iter()
        .find(|surface| surface.kind == PluginSurfaceKind::Tool && surface.id == "index")
        .unwrap()
        .clone()];
    catalog.permission_ceiling.surfaces = vec![catalog
        .permission_ceiling
        .surfaces
        .iter()
        .find(|permission| {
            permission.surface.kind == PluginSurfaceKind::Tool && permission.surface.id == "index"
        })
        .unwrap()
        .clone()];
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();

    let descriptor = ToolReleaseDescriptor::from_json(include_bytes!(
        "../../core/fixtures/releases/tool-service-release-v1.json"
    ))
    .unwrap();
    let bundle = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: catalog.package_id.clone(),
        version: catalog.version.clone(),
        channel: catalog.channel,
        target: catalog.target.clone(),
        archive_sha256: catalog.archive.sha256.clone(),
        package_sha256: catalog.package.sha256.clone().unwrap(),
        manifest_sha256: catalog.package.manifest_sha256.clone().unwrap(),
        permission_ceiling_digest: catalog.permission_ceiling_digest.clone(),
        surfaces: vec![ExecutablePlanningSurface::ToolService {
            id: "index".to_owned(),
            activation: PlanningSurfaceActivation::Eager,
            base_path: "/api".to_owned(),
            artifact: PlanningArtifactRef {
                uri: format!(
                    "oci://registry.example/a3s/science-index@{}",
                    descriptor.artifact.digest
                ),
                digest: descriptor.artifact.digest.clone(),
                media_type: descriptor.artifact.media_type.clone(),
            },
            descriptor,
        }],
    };
    let planning_bytes = bundle.canonical_bytes().unwrap();
    catalog.planning = Some(CatalogPlanningTarget {
        target_name: planning_target.clone(),
        length: planning_bytes.len() as u64,
        sha256: if mismatched_catalog_digest {
            format!("sha256:{}", "e".repeat(64))
        } else {
            format!("sha256:{:x}", Sha256::digest(&planning_bytes))
        },
    });
    catalog.validate().unwrap();

    let repository = TestRepository::with_targets(
        vec![
            TestTarget {
                archive,
                target_name: archive_target.clone(),
                custom: Some(serde_json::to_value(catalog).unwrap()),
            },
            TestTarget {
                archive: planning_bytes,
                target_name: planning_target.clone(),
                custom: None,
            },
        ],
        11,
        FUTURE,
    );
    (repository, bundle, archive_target, planning_target)
}
