use a3s_use_core::{
    PlanPackageChangeKind, PlannedPackageTransition, PluginOperationPlan,
    PluginOperationPlanBinding, PluginOperationPlanDraft,
};

const INSTALL_PLAN: &[u8] = include_bytes!("../fixtures/plugins/operation-plan-install-v1.json");

fn install_plan() -> PluginOperationPlan {
    PluginOperationPlan::from_json(INSTALL_PLAN).unwrap()
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
