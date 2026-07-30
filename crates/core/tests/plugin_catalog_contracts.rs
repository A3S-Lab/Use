use a3s_use_core::{
    PlanPackageRole, PluginCatalogRecord, PluginPermissionCeiling, PluginPlanSource,
    PluginSurfaceKind, PluginSurfaceRef, ToolWorkloadClass, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V2,
};

const PERMISSION_CEILING: &[u8] = include_bytes!("../fixtures/plugins/permission-ceiling-v1.json");
const CATALOG_RECORD: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-v1.json");
const COMPLETE_PACKAGE_CATALOG: &[u8] =
    include_bytes!("../fixtures/plugins/complete-package-catalog-v1.json");
const COMPLETE_PACKAGE_CATALOG_DIGEST: &str =
    include_str!("../fixtures/plugins/complete-package-catalog-v1.sha256").trim_ascii_end();
const PERMISSION_DIGEST: &str =
    include_str!("../fixtures/plugins/permission-ceiling-v1.sha256").trim_ascii_end();
const CATALOG_DIGEST: &str =
    include_str!("../fixtures/plugins/catalog-record-v1.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[test]
fn canonical_plugin_contract_fixtures_have_cross_sdk_digests() {
    let ceiling = PluginPermissionCeiling::from_json(PERMISSION_CEILING).unwrap();
    let catalog = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();

    assert_eq!(
        ceiling.canonical_bytes().unwrap(),
        canonical_fixture(PERMISSION_CEILING)
    );
    assert_eq!(
        catalog.canonical_bytes().unwrap(),
        canonical_fixture(CATALOG_RECORD)
    );
    assert_eq!(ceiling.descriptor_digest().unwrap(), PERMISSION_DIGEST);
    assert_eq!(catalog.descriptor_digest().unwrap(), CATALOG_DIGEST);

    assert_eq!(ceiling.surfaces.len(), 4);
    assert_eq!(ceiling.surfaces[0].surface.kind, PluginSurfaceKind::Mcp);
    assert!(ceiling.surfaces[1].native_execution);
    assert_eq!(ceiling.surfaces[3].ui_http[0].tool_id, "index");
    assert_eq!(catalog.package_id, "acme/research");
    assert_eq!(catalog.surfaces.len(), 5);
    assert_eq!(catalog.surfaces[2].workload, Some(ToolWorkloadClass::Task));
    assert_eq!(
        catalog.permission_ceiling_digest,
        catalog.permission_ceiling.descriptor_digest().unwrap()
    );

    let reordered = serde_json::to_vec_pretty(
        &serde_json::from_slice::<serde_json::Value>(CATALOG_RECORD).unwrap(),
    )
    .unwrap();
    assert_eq!(
        PluginCatalogRecord::from_json(&reordered)
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        CATALOG_DIGEST
    );
}

#[test]
fn permission_ceiling_rejects_ambient_or_unscoped_authority() {
    let mut unsafe_path: serde_json::Value = serde_json::from_slice(PERMISSION_CEILING).unwrap();
    unsafe_path["surfaces"][1]["filesystem"][0]["path"] = serde_json::json!("/etc/passwd");
    assert_eq!(
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&unsafe_path).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.permission_invalid"
    );

    let mut ambient_ui: serde_json::Value = serde_json::from_slice(PERMISSION_CEILING).unwrap();
    ambient_ui["surfaces"][3]["nativeExecution"] = serde_json::json!(true);
    assert_eq!(
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&ambient_ui).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.permission_invalid"
    );

    let mut missing_resources: serde_json::Value =
        serde_json::from_slice(PERMISSION_CEILING).unwrap();
    missing_resources["surfaces"][1]
        .as_object_mut()
        .unwrap()
        .remove("resources");
    assert_eq!(
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&missing_resources).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.permission_invalid"
    );
}

#[test]
fn catalog_record_binds_permissions_surfaces_and_archive() {
    let mut changed: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    changed["permissionCeiling"]["surfaces"][1]["nativeExecution"] = serde_json::json!(false);
    assert_eq!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&changed).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.catalog_invalid"
    );

    let mut unsorted: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    unsorted["surfaces"].as_array_mut().unwrap().swap(0, 1);
    assert_eq!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&unsorted).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.catalog_invalid"
    );

    let mut public_service: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    public_service["permissionCeiling"]["surfaces"][2]["privateService"] = serde_json::json!(false);
    let permissions: PluginPermissionCeiling =
        serde_json::from_value(public_service["permissionCeiling"].clone()).unwrap();
    public_service["permissionCeilingDigest"] =
        serde_json::json!(permissions.descriptor_digest().unwrap());
    assert_eq!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&public_service).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.catalog_invalid"
    );
}

#[test]
fn catalog_v2_binds_manifest_and_resolves_only_the_surface_dependency_closure() {
    let mut value: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    value["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    value["package"]["manifestSha256"] = serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    for surface in value["surfaces"].as_array_mut().unwrap() {
        surface["optional"] = serde_json::json!(true);
    }
    value["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    value["surfaces"][4]["requires"] = serde_json::json!([
        {"kind": "skill", "id": "review"},
        {"kind": "tool", "id": "index"}
    ]);
    let catalog = PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        catalog.descriptor_digest().unwrap(),
        "sha256:3b2bbd9a4dbd0c1e16468cf4a5c971ee83fabc721d116439e76e5ab759df90ef"
    );
    let resolved = catalog
        .resolve_surfaces(&[PluginSurfaceRef {
            kind: PluginSurfaceKind::Ui,
            id: "review".to_string(),
        }])
        .unwrap();

    assert_eq!(
        resolved
            .iter()
            .map(|surface| surface.reference())
            .collect::<Vec<_>>(),
        vec![
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Skill,
                id: "review".to_string(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "index".to_string(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: "review".to_string(),
            },
        ]
    );
    let requested = PluginSurfaceRef {
        kind: PluginSurfaceKind::Ui,
        id: "review".to_string(),
    };
    assert!(catalog
        .resolve_surfaces(&[requested.clone(), requested])
        .is_err());
    assert!(catalog
        .resolve_surfaces(&[PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "missing".to_string(),
        }])
        .is_err());
}

#[test]
fn catalog_versions_fail_closed_across_new_evidence_fields() {
    let mut missing_manifest: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    missing_manifest["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    assert!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&missing_manifest).unwrap()).is_err()
    );

    let mut v1_dependency: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    v1_dependency["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&v1_dependency).unwrap()).is_err());

    let mut forbidden_back_edge: serde_json::Value =
        serde_json::from_slice(CATALOG_RECORD).unwrap();
    forbidden_back_edge["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    forbidden_back_edge["package"]["manifestSha256"] =
        serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    forbidden_back_edge["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    forbidden_back_edge["surfaces"][2]["requires"] = serde_json::json!([
        {"kind": "skill", "id": "review"}
    ]);
    assert!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&forbidden_back_edge).unwrap()).is_err()
    );

    let mut missing_package_digest: serde_json::Value =
        serde_json::from_slice(CATALOG_RECORD).unwrap();
    missing_package_digest["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    missing_package_digest["package"]["manifestSha256"] =
        serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    missing_package_digest["package"]
        .as_object_mut()
        .unwrap()
        .remove("sha256");
    assert!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&missing_package_digest).unwrap())
            .is_err()
    );
}

#[test]
fn verified_catalog_v2_derives_a_plan_ready_selected_install_transition() {
    let mut value: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    value["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    value["package"]["manifestSha256"] = serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    for surface in value["surfaces"].as_array_mut().unwrap() {
        surface["optional"] = serde_json::json!(true);
    }
    value["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    value["surfaces"][4]["requires"] = serde_json::json!([
        {"kind": "skill", "id": "review"},
        {"kind": "tool", "id": "index"}
    ]);
    let record = PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    let transition = verified
        .install_transition(
            PlanPackageRole::Root,
            &[PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: "review".to_string(),
            }],
        )
        .unwrap();
    let after = transition.after.as_ref().unwrap();

    assert_eq!(after.release.surfaces.len(), 4);
    assert_eq!(after.permissions.surfaces.len(), 3);
    assert!(after
        .release
        .surfaces
        .iter()
        .all(|surface| surface.id != "library"));
    assert_eq!(
        after.release.permission_ceiling_digest,
        after.permissions.descriptor_digest().unwrap()
    );
    assert!(matches!(
        transition.source,
        Some(PluginPlanSource::Registry { .. })
    ));
}

#[test]
fn catalog_v1_remains_searchable_but_cannot_claim_plan_ready_evidence() {
    let record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    let error = verified
        .install_transition(PlanPackageRole::Root, &[])
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.catalog_plan_evidence_missing");
}

#[test]
fn privilege_bearing_unknown_fields_fail_closed_without_echo() {
    let secret_marker = "do-not-echo-super-secret";
    let mut permissions: serde_json::Value = serde_json::from_slice(PERMISSION_CEILING).unwrap();
    permissions["surfaces"][1]["environment"] = serde_json::json!({"TOKEN": secret_marker});
    let error =
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&permissions).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.permission_invalid");
    assert!(!error.message.contains(secret_marker));

    let mut catalog: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    catalog["endpointUrl"] = serde_json::json!("https://public.example");
    let error = PluginCatalogRecord::from_json(&serde_json::to_vec(&catalog).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.catalog_invalid");
}

#[test]
fn verified_catalog_provenance_binds_outer_tuf_evidence_to_the_record() {
    let record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: CATALOG_DIGEST.to_owned(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    verified.validate().unwrap();

    let mut drift = verified;
    drift.provenance.catalog_record_digest =
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned();
    assert!(drift.validate().is_err());
}

#[test]
fn verified_catalog_provenance_accepts_only_secure_or_loopback_registries() {
    let record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "fixture".to_owned(),
        registry_url: "http://127.0.0.1:43210/".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: CATALOG_DIGEST.to_owned(),
    };
    VerifiedPluginCatalogRecord::new(record.clone(), provenance).unwrap();

    let insecure = VerifiedCatalogProvenance {
        registry_name: "fixture".to_owned(),
        registry_url: "http://plugins.example/".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: CATALOG_DIGEST.to_owned(),
    };
    assert!(VerifiedPluginCatalogRecord::new(record, insecure).is_err());
}

#[test]
fn complete_package_catalog_fixture_is_canonical() {
    let catalog = PluginCatalogRecord::from_json(COMPLETE_PACKAGE_CATALOG).unwrap();
    assert_eq!(
        catalog.canonical_bytes().unwrap(),
        canonical_fixture(COMPLETE_PACKAGE_CATALOG)
    );
    assert_eq!(
        catalog.descriptor_digest().unwrap(),
        COMPLETE_PACKAGE_CATALOG_DIGEST
    );
}
