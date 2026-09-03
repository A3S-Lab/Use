use a3s_use_core::PluginOperationAction;

use super::aggregate_tests::fixtures::operation_at;
use super::aggregate_tests::grant_fixtures::reviewed_grant_operation;
use super::model::ReviewedControlOperation;
use super::operation_admission::reviewed_cognitive_package_operation;
use crate::cognitive_package::{
    CognitivePackageAuthorizationEvidence, PlannedWorkspaceGrantOperation,
};

fn authorization(reviewed: &ReviewedControlOperation) -> CognitivePackageAuthorizationEvidence {
    CognitivePackageAuthorizationEvidence {
        operation_confirmation: reviewed.authorization.operation_confirmation.clone(),
        grant_confirmations: reviewed.authorization.grant_confirmations.clone(),
    }
}

fn grants(reviewed: &ReviewedControlOperation) -> PlannedWorkspaceGrantOperation {
    let transition = reviewed
        .authorization
        .grant_transition
        .as_ref()
        .expect("the fixture must carry reviewed Grant evidence");
    PlannedWorkspaceGrantOperation {
        snapshot: transition.snapshot.clone(),
        change_set: transition.change_set.clone(),
        ceilings: Vec::new(),
    }
}

#[test]
fn lifecycle_admission_derives_store_cursors_for_every_action() {
    let actions = [
        PluginOperationAction::Install,
        PluginOperationAction::Upgrade,
        PluginOperationAction::Enable,
        PluginOperationAction::Disable,
        PluginOperationAction::Uninstall,
    ];

    for (index, action) in actions.into_iter().enumerate() {
        let expected_generation = u64::try_from(index).unwrap();
        let expected_capability_generation = expected_generation + 10;
        let reviewed = operation_at(
            &format!("operation:lifecycle-admission-{index}"),
            action,
            expected_generation,
            expected_capability_generation,
        );

        let admitted = reviewed_cognitive_package_operation(
            &reviewed.envelope,
            &authorization(&reviewed),
            None,
            reviewed.reviewed_at_ms,
        )
        .unwrap();

        assert_eq!(admitted, reviewed);
    }
}

#[test]
fn lifecycle_admission_preserves_exact_reviewed_grant_evidence() {
    let reviewed = reviewed_grant_operation(
        "operation:lifecycle-grant-admission",
        PluginOperationAction::Install,
        None,
        None,
    );
    let planned_grants = grants(&reviewed);

    let admitted = reviewed_cognitive_package_operation(
        &reviewed.envelope,
        &authorization(&reviewed),
        Some(&planned_grants),
        reviewed.reviewed_at_ms,
    )
    .unwrap();

    assert_eq!(admitted, reviewed);
}

#[test]
fn lifecycle_admission_rejects_missing_or_unreviewed_grant_evidence() {
    let reviewed_grants = reviewed_grant_operation(
        "operation:lifecycle-grant-required",
        PluginOperationAction::Install,
        None,
        None,
    );
    let missing = reviewed_cognitive_package_operation(
        &reviewed_grants.envelope,
        &authorization(&reviewed_grants),
        None,
        reviewed_grants.reviewed_at_ms,
    )
    .unwrap_err();
    assert_eq!(missing.code, "use.control_store.input_invalid");

    let permission_free = operation_at(
        "operation:lifecycle-grant-unreviewed",
        PluginOperationAction::Install,
        0,
        0,
    );
    let unexpected = reviewed_cognitive_package_operation(
        &permission_free.envelope,
        &authorization(&permission_free),
        Some(&grants(&reviewed_grants)),
        permission_free.reviewed_at_ms,
    )
    .unwrap_err();
    assert_eq!(unexpected.code, "use.control_store.input_invalid");
}

#[test]
fn lifecycle_admission_rejects_a_plan_without_a_prior_state_revision() {
    let reviewed = operation_at(
        "operation:lifecycle-zero-state-revision",
        PluginOperationAction::Install,
        0,
        0,
    );
    let mut envelope = reviewed.envelope.clone();
    envelope.plan.state.state_revision = 0;

    let error = reviewed_cognitive_package_operation(
        &envelope,
        &authorization(&reviewed),
        None,
        reviewed.reviewed_at_ms,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.control_store.input_invalid");
}
