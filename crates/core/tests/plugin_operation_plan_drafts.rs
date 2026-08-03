use a3s_use_core::{
    CatalogSurface, OkfBundleContract, PlanPackageChangeKind, PlannedPackageTransition,
    PluginOperationPlan, PluginOperationPlanBinding, PluginOperationPlanDraft,
    PluginPermissionCeiling, PluginPlanSource, PluginSurfaceKind,
    PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2, PLUGIN_OPERATION_PLAN_SCHEMA_V2,
    PLUGIN_PERMISSION_SCHEMA,
};

const INSTALL_PLAN: &[u8] = include_bytes!("../fixtures/plugins/operation-plan-install-v1.json");
const OKF_INSTALL_PLAN: &[u8] =
    include_bytes!("../fixtures/plugins/operation-plan-install-okf-v2.json");
const OKF_INSTALL_PLAN_DIGEST: &str =
    include_str!("../fixtures/plugins/operation-plan-install-okf-v2.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn install_plan() -> PluginOperationPlan {
    PluginOperationPlan::from_json(INSTALL_PLAN).unwrap()
}

fn okf_install_plan() -> PluginOperationPlan {
    let expected = install_plan();
    let mut after = expected.packages[0].after.clone().unwrap();
    let bundle =
        OkfBundleContract::from_json(include_bytes!("../fixtures/okf/bundle-contract-v1.json"))
            .unwrap();
    after.release.package_id = "acme/knowledge".to_owned();
    after.release.version = "1.0.0".to_owned();
    after.release.package_sha256 =
        "sha256:e030e6e8c3afa6383536b4973395e12b05257d2b7b10bf042f3bfaf71b420fe2".to_owned();
    after.release.manifest_sha256 =
        "sha256:16d065341edebaf45e0d633e46614155a60b853738290bbac8c5b62d9230bce2".to_owned();
    after.release.surfaces = vec![CatalogSurface {
        kind: PluginSurfaceKind::Okf,
        id: "domain-knowledge".to_owned(),
        optional: false,
        workload: None,
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: Some(bundle),
        requires: Vec::new(),
    }];
    after.permissions = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_owned(),
        surfaces: Vec::new(),
    };
    after.release.permission_ceiling_digest = after.permissions.descriptor_digest().unwrap();
    let package_digest = after.release.package_sha256.clone();
    let transition =
        PlannedPackageTransition::resolved(
            "acme/knowledge",
            expected.packages[0].role,
            PlanPackageChangeKind::Add,
            None,
            Some(after),
            Some(PluginPlanSource::ReleaseBundle {
                bundle_digest:
                    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                        .to_owned(),
                package_digest,
            }),
        )
        .unwrap();
    let mut impact = expected.impact;
    impact.download_bytes = 3_366;
    impact.installed_bytes_after = 3_366;
    let draft = PluginOperationPlanDraft::new(
        expected.action,
        "acme/knowledge",
        "knowledge:local",
        vec![transition],
        Vec::new(),
        expected.workspace_impacts,
        impact,
        expected.state,
    )
    .unwrap();
    assert_eq!(draft.schema, PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2);
    draft
        .bind(PluginOperationPlanBinding {
            operation_id: "install:acme-knowledge:0001".to_owned(),
            created_at_ms: expected.created_at_ms,
            expires_at_ms: expected.expires_at_ms,
            scope: expected.scope,
            authority: expected.authority,
        })
        .unwrap()
}

#[test]
fn draft_omits_host_identity_scope_and_authority_then_binds_exactly() {
    let expected = install_plan();
    let draft = PluginOperationPlanDraft::new(
        expected.action,
        expected.package_id.clone(),
        expected.component_id.clone(),
        expected.packages.clone(),
        expected.providers.clone(),
        expected.workspace_impacts.clone(),
        expected.impact.clone(),
        expected.state.clone(),
    )
    .unwrap();
    let value = serde_json::to_value(&draft).unwrap();

    assert!(value.get("operationId").is_none());
    assert!(value.get("createdAtMs").is_none());
    assert!(value.get("expiresAtMs").is_none());
    assert!(value.get("scope").is_none());
    assert!(value.get("authority").is_none());
    assert!(value.get("secretChanges").is_none());

    let bound = draft
        .bind(PluginOperationPlanBinding {
            operation_id: expected.operation_id.clone(),
            created_at_ms: expected.created_at_ms,
            expires_at_ms: expected.expires_at_ms,
            scope: expected.scope.clone(),
            authority: expected.authority.clone(),
        })
        .unwrap();

    assert_eq!(bound, expected);
}

#[test]
fn draft_json_rejects_delegated_host_authority() {
    let expected = install_plan();
    let draft = PluginOperationPlanDraft::new(
        expected.action,
        expected.package_id,
        expected.component_id,
        expected.packages,
        expected.providers,
        expected.workspace_impacts,
        expected.impact,
        expected.state,
    )
    .unwrap();
    let mut value = serde_json::to_value(draft).unwrap();
    value["authority"] = serde_json::json!({
        "actor": "agent",
        "decision": "allow",
        "policyDigest": format!("sha256:{}", "a".repeat(64)),
        "confirmationRequired": false,
    });

    assert!(PluginOperationPlanDraft::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn resolved_transition_derives_the_exact_surface_delta() {
    let expected = install_plan().packages.remove(0);
    let resolved = PlannedPackageTransition::resolved(
        expected.package_id.clone(),
        expected.role,
        PlanPackageChangeKind::Add,
        None,
        expected.after.clone(),
        expected.source.clone(),
    )
    .unwrap();

    assert_eq!(resolved, expected);
}

#[test]
fn resolved_retained_dependency_has_no_surface_delta() {
    let package = install_plan().packages.remove(0);
    let package_id = package.package_id;
    let state = package.after.unwrap();
    let resolved = PlannedPackageTransition::resolved(
        package_id,
        a3s_use_core::PlanPackageRole::Dependency,
        PlanPackageChangeKind::Retain,
        Some(state.clone()),
        Some(state),
        None,
    )
    .unwrap();

    assert!(resolved.surfaces.is_empty());
}

#[test]
fn draft_rejects_missing_explicit_runtime_provider_evidence() {
    let expected = install_plan();
    let result = PluginOperationPlanDraft::new(
        expected.action,
        expected.package_id,
        expected.component_id,
        expected.packages,
        Vec::new(),
        expected.workspace_impacts,
        expected.impact,
        expected.state,
    );

    assert!(result.is_err());
}

#[test]
fn okf_draft_derives_exact_bundle_impact_without_a_runtime_provider() {
    let mut plan = okf_install_plan();
    assert_eq!(plan.schema, PLUGIN_OPERATION_PLAN_SCHEMA_V2);
    assert_eq!(plan.impact.okf_changes.len(), 1);
    assert_eq!(
        plan.impact.okf_changes[0]
            .after
            .as_ref()
            .unwrap()
            .content_digest,
        "sha256:bd85b0b63adb32bdf616384a619286af4c32401542655dd09e00450902ab478d"
    );
    assert!(plan.providers.is_empty());
    plan.validate().unwrap();
    assert_eq!(
        plan.canonical_bytes().unwrap(),
        canonical_fixture(OKF_INSTALL_PLAN)
    );
    assert_eq!(plan.descriptor_digest().unwrap(), OKF_INSTALL_PLAN_DIGEST);
    assert_eq!(
        PluginOperationPlan::from_json(OKF_INSTALL_PLAN).unwrap(),
        plan
    );

    plan.schema = a3s_use_core::PLUGIN_OPERATION_PLAN_SCHEMA.to_owned();
    assert!(plan.validate().is_err());
}
