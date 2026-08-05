#[path = "support/plugin_workspace_grant_fixtures.rs"]
mod fixtures;

use a3s_use_core::{
    PlannedWorkspaceGrantChange, PluginWorkspaceGrantChangeSet, PluginWorkspaceGrantSnapshot,
};
use fixtures::{
    confirmations, multi_package_install, multi_package_uninstall, operation_confirmation,
    DIGEST_C, DIGEST_E,
};
const GRANT_SNAPSHOT: &[u8] =
    include_bytes!("../fixtures/plugins/workspace-grant-snapshot-v1.json");
const GRANT_SNAPSHOT_DIGEST: &str =
    include_str!("../fixtures/plugins/workspace-grant-snapshot-v1.sha256").trim_ascii_end();
const GRANT_CHANGES: &[u8] = include_bytes!("../fixtures/plugins/workspace-grant-changes-v1.json");
const GRANT_CHANGES_DIGEST: &str =
    include_str!("../fixtures/plugins/workspace-grant-changes-v1.sha256").trim_ascii_end();

#[test]
fn multi_package_install_finalizes_every_planned_grant_in_order() {
    let (plan, changes) = multi_package_install();
    changes.validate_against_plan(&plan, None).unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let confirmations = confirmations(&changes, &plan_digest);
    let operation_confirmation = operation_confirmation(&plan);
    assert_eq!(
        changes
            .finalize_against_plan(&plan, None, None, &confirmations, plan.created_at_ms + 200,)
            .unwrap_err()
            .code,
        "use.plugin.plan_confirmation_required"
    );
    let resolved = changes
        .finalize_against_plan(
            &plan,
            None,
            Some(&operation_confirmation),
            &confirmations,
            plan.created_at_ms + 200,
        )
        .unwrap();

    assert_eq!(resolved.scope_id, plan.scope.id);
    assert_eq!(resolved.operation_id, plan.operation_id);
    assert_eq!(resolved.plan_digest, plan.descriptor_digest().unwrap());
    assert_eq!(
        resolved.change_set_digest,
        changes.descriptor_digest().unwrap()
    );
    assert_eq!(resolved.state_revision_before, plan.state.state_revision);
    assert_eq!(resolved.revision, plan.state.state_revision + 1);
    assert_eq!(
        resolved.capability_generation_after,
        resolved.capability_generation_before + 1
    );
    assert_eq!(resolved.before_snapshot_digest, None);
    assert_eq!(resolved.transitioned_at_ms, plan.created_at_ms + 200);
    assert!(resolved.revocation_authority.confirmation_digest.is_some());
    assert!(resolved.revocations.is_empty());
    assert_eq!(resolved.grants.len(), 2);
    assert_eq!(
        resolved
            .grants
            .iter()
            .map(|grant| grant.grant.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["acme/helper", "acme/research"]
    );
    assert!(resolved.grants.iter().all(|grant| grant
        .grant
        .authority
        .confirmation_digest
        .is_some()));
}

#[test]
fn first_capability_cutover_resolves_generation_zero_to_one() {
    let (mut plan, changes) = multi_package_install();
    plan.state.capability_generation = 0;
    let plan_digest = plan.descriptor_digest().unwrap();
    let resolved = changes
        .finalize_against_plan(
            &plan,
            None,
            Some(&operation_confirmation(&plan)),
            &confirmations(&changes, &plan_digest),
            plan.created_at_ms + 200,
        )
        .unwrap();
    assert_eq!(resolved.capability_generation_before, 0);
    assert_eq!(resolved.capability_generation_after, 1);
}

#[test]
fn package_lock_bound_plan_finalizes_against_its_exact_digest() {
    let (mut plan, changes) = multi_package_install();
    plan.package_lock_digest = Some(DIGEST_C.to_string());
    plan.validate().unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let resolved = changes
        .finalize_against_plan(
            &plan,
            None,
            Some(&operation_confirmation(&plan)),
            &confirmations(&changes, &plan_digest),
            plan.created_at_ms + 200,
        )
        .unwrap();

    assert_eq!(resolved.plan_digest, plan_digest);
}

#[test]
fn change_set_rejects_plan_drift_extra_packages_and_missing_confirmation() {
    let (plan, changes) = multi_package_install();
    let mut drifted = changes.clone();
    drifted.changes[0].after.as_mut().unwrap().package_digest = DIGEST_E.to_string();
    assert_eq!(
        drifted.validate_against_plan(&plan, None).unwrap_err().code,
        "use.plugin.grant_changes_plan_mismatch"
    );

    let mut extra = changes.clone();
    let mut extra_proposal = extra.changes[0].after.clone().unwrap();
    extra_proposal.package_id = "acme/injected".to_string();
    extra.changes.insert(
        1,
        PlannedWorkspaceGrantChange {
            package_id: "acme/injected".to_string(),
            before: None,
            after: Some(extra_proposal),
        },
    );
    let mut rebound_plan = plan.clone();
    rebound_plan.workspace_impacts[0].grant_after_digest = Some(extra.descriptor_digest().unwrap());
    assert_eq!(
        extra
            .validate_against_plan(&rebound_plan, None)
            .unwrap_err()
            .code,
        "use.plugin.grant_changes_plan_mismatch"
    );

    let plan_digest = plan.descriptor_digest().unwrap();
    let operation_confirmation = operation_confirmation(&plan);
    let only_one_confirmation = vec![confirmations(&changes, &plan_digest).remove(0)];
    assert_eq!(
        changes
            .finalize_against_plan(
                &plan,
                None,
                Some(&operation_confirmation),
                &only_one_confirmation,
                plan.created_at_ms + 200,
            )
            .unwrap_err()
            .code,
        "use.plugin.grant_confirmation_required"
    );

    let mut duplicated = confirmations(&changes, &plan_digest);
    duplicated.push(duplicated[0].clone());
    assert_eq!(
        changes
            .finalize_against_plan(
                &plan,
                None,
                Some(&operation_confirmation),
                &duplicated,
                plan.created_at_ms + 200,
            )
            .unwrap_err()
            .code,
        "use.plugin.grant_changes_confirmation_mismatch"
    );

    let mut different_confirmation_event = confirmations(&changes, &plan_digest);
    different_confirmation_event[0].confirmed_at_ms += 1;
    assert_eq!(
        changes
            .finalize_against_plan(
                &plan,
                None,
                Some(&operation_confirmation),
                &different_confirmation_event,
                plan.created_at_ms + 200,
            )
            .unwrap_err()
            .code,
        "use.plugin.grant_changes_confirmation_mismatch"
    );

    let mut exhausted = plan;
    exhausted.state.capability_generation = u64::MAX;
    let exhausted_plan_digest = exhausted.descriptor_digest().unwrap();
    assert_eq!(
        changes
            .finalize_against_plan(
                &exhausted,
                None,
                Some(&fixtures::operation_confirmation(&exhausted)),
                &confirmations(&changes, &exhausted_plan_digest),
                exhausted.created_at_ms + 200,
            )
            .unwrap_err()
            .code,
        "use.plugin.grant_changes_generation_exhausted"
    );
}

#[test]
fn uninstall_binds_the_before_snapshot_and_resolves_exact_revocations() {
    let (plan, changes, snapshot) = multi_package_uninstall();
    assert_eq!(
        snapshot.canonical_bytes().unwrap(),
        canonical_fixture(GRANT_SNAPSHOT)
    );
    assert_eq!(
        PluginWorkspaceGrantSnapshot::from_json(GRANT_SNAPSHOT).unwrap(),
        snapshot
    );
    assert_eq!(snapshot.descriptor_digest().unwrap(), GRANT_SNAPSHOT_DIGEST);
    assert_eq!(
        changes.canonical_bytes().unwrap(),
        canonical_fixture(GRANT_CHANGES)
    );
    assert_eq!(
        PluginWorkspaceGrantChangeSet::from_json(GRANT_CHANGES).unwrap(),
        changes
    );
    assert_eq!(changes.descriptor_digest().unwrap(), GRANT_CHANGES_DIGEST);
    changes
        .validate_against_plan(&plan, Some(&snapshot))
        .unwrap();
    let operation_confirmation = operation_confirmation(&plan);
    let resolved = changes
        .finalize_against_plan(
            &plan,
            Some(&snapshot),
            Some(&operation_confirmation),
            &[],
            plan.created_at_ms + 200,
        )
        .unwrap();

    assert!(resolved.grants.is_empty());
    assert_eq!(resolved.revision, 11);
    assert_eq!(
        resolved
            .revocations
            .iter()
            .map(|evidence| evidence.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["acme/helper", "acme/research"]
    );

    let mut incomplete_snapshot = snapshot.clone();
    incomplete_snapshot.grants.pop();
    assert_eq!(
        changes
            .validate_against_plan(&plan, Some(&incomplete_snapshot))
            .unwrap_err()
            .code,
        "use.plugin.grant_changes_plan_mismatch"
    );

    let mut future_snapshot = snapshot.clone();
    future_snapshot.grants[0].receipt_revision = 11;
    assert_eq!(
        future_snapshot.validate().unwrap_err().code,
        "use.plugin.grant_snapshot_invalid"
    );

    let mut duplicate_package = snapshot.clone();
    duplicate_package
        .grants
        .insert(1, snapshot.grants[0].clone());
    duplicate_package.grants[1].grant_digest = DIGEST_C.to_string();
    assert_eq!(
        duplicate_package.validate().unwrap_err().code,
        "use.plugin.grant_snapshot_invalid"
    );

    let mut denied_plan = plan;
    denied_plan.authority.decision = a3s_use_core::PlanPolicyDecision::Deny;
    denied_plan.authority.confirmation_required = false;
    assert_eq!(
        changes
            .finalize_against_plan(
                &denied_plan,
                Some(&snapshot),
                None,
                &[],
                denied_plan.created_at_ms + 200,
            )
            .unwrap_err()
            .code,
        "use.plugin.plan_denied"
    );
}

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
