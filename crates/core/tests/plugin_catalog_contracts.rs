use a3s_use_core::{
    PluginCatalogRecord, PluginPermissionCeiling, PluginSurfaceKind, ToolWorkloadClass,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
};

const PERMISSION_CEILING: &[u8] = include_bytes!("../fixtures/plugins/permission-ceiling-v1.json");
const CATALOG_RECORD: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-v1.json");
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
