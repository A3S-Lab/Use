#[allow(dead_code)]
#[path = "support/plugin_workspace_grant_fixtures.rs"]
mod fixtures;

use a3s_use_core::{
    CatalogSurface, PlanPackageChangeKind, PlanPackageRole, PlannedPackageState,
    PlannedPackageTransition, PlannedPluginRelease, PlannedSecretChange, PlannedStateEvidence,
    PluginOperationAction, PluginOperationPlanBinding, PluginPermissionCeiling, PluginPlanSource,
    PluginReleaseChannel, PluginSurfaceKind, PluginWorkspaceGrantPlan,
    PluginWorkspaceGrantSnapshot, WorkspaceGrantEvidence, PLUGIN_PERMISSION_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};
use fixtures::{multi_package_install, multi_package_uninstall, DIGEST_C, DIGEST_E};

#[test]
fn install_planning_binds_the_complete_before_snapshot_and_sorted_proposals() {
    let (mut plan, _) = multi_package_install();
    let snapshot = empty_snapshot(&plan);

    let grants = PluginWorkspaceGrantPlan::resolve(
        &binding(&plan),
        plan.state.state_revision,
        &plan.packages,
        &snapshot,
        false,
        true,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        grants.change_set().before_snapshot_digest,
        Some(snapshot.descriptor_digest().unwrap())
    );
    assert_eq!(
        grants
            .change_set()
            .changes
            .iter()
            .map(|change| change.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["acme/helper", "acme/research"]
    );
    assert!(grants
        .change_set()
        .changes
        .iter()
        .all(|change| change.before.is_none() && change.after.is_some()));
    assert!(grants.change_set().changes.iter().all(|change| {
        let proposal = change.after.as_ref().unwrap();
        proposal.operation_id == plan.operation_id
            && proposal.scope_id == plan.scope.id
            && proposal.authority.actor == plan.authority.actor
            && proposal.authority.decision == plan.authority.decision
            && proposal.authority.policy_digest == plan.authority.policy_digest
            && proposal.created_at_ms == plan.created_at_ms
            && proposal.apply_expires_at_ms == plan.expires_at_ms
    }));

    plan.workspace_impacts = vec![grants.impact().clone()];
    plan.validate().unwrap();
    grants
        .change_set()
        .validate_against_plan(&plan, Some(&snapshot))
        .unwrap();
}

#[test]
fn upgrade_planning_keeps_the_old_grant_until_candidate_cutover() {
    let (mut plan, _) = multi_package_install();
    plan.packages
        .retain(|package| package.role == PlanPackageRole::Root);
    plan.providers
        .retain(|provider| provider.surface.package_id == plan.package_id);
    let installed = plan.packages.remove(0);
    let before = installed.after.unwrap();
    let mut after = before.clone();
    after.permissions.surfaces[0]
        .resources
        .as_mut()
        .unwrap()
        .cpu_millis -= 1;
    after.release.permission_ceiling_digest = after.permissions.descriptor_digest().unwrap();
    let transition = PlannedPackageTransition::resolved(
        plan.package_id.clone(),
        PlanPackageRole::Root,
        PlanPackageChangeKind::Replace,
        Some(before.clone()),
        Some(after.clone()),
        installed.source,
    )
    .unwrap();
    plan.action = PluginOperationAction::Upgrade;
    plan.operation_id = "upgrade:acme-research:0002".to_string();
    plan.created_at_ms += 1_000;
    plan.expires_at_ms += 1_000;
    plan.packages = vec![transition];
    plan.secret_changes = Vec::<PlannedSecretChange>::new();
    plan.workspace_impacts.clear();
    plan.impact.reclaimed_bytes = 4_194_304;
    plan.impact.drain_required = true;
    plan.state = PlannedStateEvidence {
        state_revision: 10,
        capability_generation: plan.state.capability_generation,
        receipt_digest: Some(DIGEST_C.to_string()),
    };
    let snapshot = PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: plan.scope.id.clone(),
        state_revision: plan.state.state_revision,
        grants: vec![WorkspaceGrantEvidence {
            package_id: plan.package_id.clone(),
            package_digest: before.release.package_sha256.clone(),
            receipt_revision: 9,
            grant_digest: DIGEST_E.to_string(),
        }],
    };

    let grants = PluginWorkspaceGrantPlan::resolve(
        &binding(&plan),
        plan.state.state_revision,
        &plan.packages,
        &snapshot,
        true,
        true,
    )
    .unwrap()
    .unwrap();

    assert_eq!(grants.change_set().changes.len(), 1);
    let change = &grants.change_set().changes[0];
    assert_eq!(change.before.as_ref(), snapshot.grants.first());
    assert_eq!(
        change.after.as_ref().unwrap().package_digest,
        after.release.package_sha256
    );
    assert_eq!(
        change.after.as_ref().unwrap().permissions,
        after.permissions
    );

    plan.workspace_impacts = vec![grants.impact().clone()];
    plan.validate().unwrap();
    grants
        .change_set()
        .validate_against_plan(&plan, Some(&snapshot))
        .unwrap();
}

#[test]
fn uninstall_planning_reproduces_exact_delayed_revocations() {
    let (mut plan, expected, snapshot) = multi_package_uninstall();

    let grants = PluginWorkspaceGrantPlan::resolve(
        &binding(&plan),
        plan.state.state_revision,
        &plan.packages,
        &snapshot,
        true,
        false,
    )
    .unwrap()
    .unwrap();

    assert_eq!(grants.change_set(), &expected);
    assert_eq!(grants.impact(), &plan.workspace_impacts[0]);
    plan.workspace_impacts = vec![grants.impact().clone()];
    grants
        .change_set()
        .validate_against_plan(&plan, Some(&snapshot))
        .unwrap();
}

#[test]
fn grant_planning_rejects_stale_or_incomplete_before_state() {
    let (plan, _, mut snapshot) = multi_package_uninstall();
    snapshot.grants.pop();

    let error = PluginWorkspaceGrantPlan::resolve(
        &binding(&plan),
        plan.state.state_revision,
        &plan.packages,
        &snapshot,
        true,
        false,
    )
    .unwrap_err();

    assert_eq!(error.code, "use.plugin.grant_changes_plan_mismatch");

    let (plan, _) = multi_package_install();
    let mut stale = empty_snapshot(&plan);
    stale.state_revision -= 1;
    let error = PluginWorkspaceGrantPlan::resolve(
        &binding(&plan),
        plan.state.state_revision,
        &plan.packages,
        &stale,
        false,
        true,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.grant_changes_plan_mismatch");
}

#[test]
fn permission_free_transitions_require_no_grant_change_set() {
    let (plan, _) = multi_package_install();
    let permissions = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_string(),
        surfaces: Vec::new(),
    };
    let package_digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let state = PlannedPackageState {
        release: PlannedPluginRelease {
            package_id: "acme/guide".to_string(),
            version: "1.0.0".to_string(),
            channel: PluginReleaseChannel::Stable,
            target: "any".to_string(),
            package_sha256: package_digest.to_string(),
            manifest_sha256:
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
            permission_ceiling_digest: permissions.descriptor_digest().unwrap(),
            surfaces: vec![CatalogSurface {
                kind: PluginSurfaceKind::Skill,
                id: "guide".to_string(),
                optional: false,
                workload: None,
                mcp_transport: None,
                mcp_tool_count: None,
                requires: Vec::new(),
            }],
        },
        permissions,
    };
    let packages = vec![PlannedPackageTransition::resolved(
        "acme/guide",
        PlanPackageRole::Root,
        PlanPackageChangeKind::Add,
        None,
        Some(state),
        Some(PluginPlanSource::ReleaseBundle {
            bundle_digest: DIGEST_E.to_string(),
            package_digest: package_digest.to_string(),
        }),
    )
    .unwrap()];
    let snapshot = empty_snapshot(&plan);

    let grants = PluginWorkspaceGrantPlan::resolve(
        &binding(&plan),
        plan.state.state_revision,
        &packages,
        &snapshot,
        false,
        true,
    )
    .unwrap();

    assert!(grants.is_none());
}

#[test]
fn public_grant_plan_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<PluginWorkspaceGrantPlan>();
}

fn binding(plan: &a3s_use_core::PluginOperationPlan) -> PluginOperationPlanBinding {
    PluginOperationPlanBinding {
        operation_id: plan.operation_id.clone(),
        created_at_ms: plan.created_at_ms,
        expires_at_ms: plan.expires_at_ms,
        scope: plan.scope.clone(),
        authority: plan.authority.clone(),
    }
}

fn empty_snapshot(plan: &a3s_use_core::PluginOperationPlan) -> PluginWorkspaceGrantSnapshot {
    PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: plan.scope.id.clone(),
        state_revision: plan.state.state_revision,
        grants: Vec::new(),
    }
}
