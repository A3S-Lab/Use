use super::*;

use a3s_use_core::{
    PlanPackageRole, PlanScope, PlannedOperationImpact, PlannedPackageTransition,
    PlannedStateEvidence, PluginOperationAction, PluginOperationPlanBinding, PluginPlanSource,
    PluginWorkspaceGrantSnapshot, WorkspaceGrantEvidence, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};

const INSTALL_PLAN: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/operation-plan-install-v1.json");

#[derive(Debug)]
struct ConfirmAll;

#[async_trait]
impl CognitivePackageAuthorizationProvider for ConfirmAll {
    fn name(&self) -> &'static str {
        "test-confirm-all"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        StandaloneCognitivePackageAuthorizationProvider.bind_authority(draft)
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        StandaloneCognitivePackageAuthorizationProvider.verify_authority(plan)
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        CognitivePackageAuthorizationEvidence::confirmed(envelope, changes, now_ms)
    }
}

#[tokio::test]
async fn standalone_policy_requires_exact_confirmation_and_rejects_grant_free_bypass() {
    let (envelope, planned) = install_plan(&StandaloneCognitivePackageAuthorizationProvider);
    assert_eq!(envelope.plan.authority.decision, PlanPolicyDecision::Ask);
    assert_eq!(envelope.plan.workspace_impacts.len(), 1);
    assert!(planned.change_set.changes[0].after.is_some());

    let admitted_at_ms = envelope.plan.created_at_ms + 100;
    let error = StandaloneCognitivePackageAuthorizationProvider
        .authorize(&envelope, Some(&planned.change_set), admitted_at_ms)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_confirmation_required");
    assert_eq!(
        error.details["planDigest"],
        serde_json::json!(envelope.plan_digest)
    );
    assert_eq!(
        PackageGraphAuthorization::default()
            .validate_against(&envelope, admitted_at_ms)
            .unwrap_err()
            .code,
        "use.plugin.plan_confirmation_required"
    );
}

#[tokio::test]
async fn confirmed_install_persists_replay_stable_plan_bound_grants_and_ceilings() {
    let (envelope, planned) = install_plan(&ConfirmAll);
    let admitted_at_ms = envelope.plan.created_at_ms + 100;
    let authorization =
        authorize_planned_operation(&ConfirmAll, &envelope, Some(&planned), admitted_at_ms)
            .await
            .unwrap();

    assert!(authorization.operation_confirmation.is_some());
    assert_eq!(authorization.grant_confirmations.len(), 1);
    assert_eq!(authorization.grant_ceilings.len(), 1);
    let resolved = authorization.resolved_grants.as_ref().unwrap();
    assert_eq!(resolved.grants.len(), 1);
    assert!(resolved.revocations.is_empty());
    assert_eq!(resolved.plan_digest, envelope.plan_digest);

    let encoded = serde_json::to_vec(&authorization).unwrap();
    let replayed: PackageGraphAuthorization = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(replayed, authorization);
    replayed
        .validate_against(&envelope, admitted_at_ms)
        .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    assert!(replayed
        .lifecycle_unit(WorkspaceGrantStore::new(temporary.path()), &envelope)
        .unwrap()
        .is_some());

    let mut missing_ceiling = replayed;
    missing_ceiling.grant_ceilings.clear();
    assert_eq!(
        missing_ceiling
            .validate_against(&envelope, admitted_at_ms)
            .unwrap_err()
            .code,
        "use.plugin.package_authorization_invalid"
    );
}

#[tokio::test]
async fn upgrade_and_uninstall_bind_exact_prior_grants_before_mutation() {
    let (upgrade_envelope, upgrade_planned) = replacement_plan(&ConfirmAll);
    let upgrade_time = upgrade_envelope.plan.created_at_ms + 100;
    let upgrade = authorize_planned_operation(
        &ConfirmAll,
        &upgrade_envelope,
        Some(&upgrade_planned),
        upgrade_time,
    )
    .await
    .unwrap();
    let resolved = upgrade.resolved_grants.as_ref().unwrap();
    assert_eq!(resolved.grants.len(), 1);
    assert_eq!(resolved.revocations.len(), 1);
    assert_eq!(upgrade.grant_ceilings.len(), 1);

    let (uninstall_envelope, uninstall_planned) = uninstall_plan(&ConfirmAll);
    let uninstall_time = uninstall_envelope.plan.created_at_ms + 100;
    let uninstall = authorize_planned_operation(
        &ConfirmAll,
        &uninstall_envelope,
        Some(&uninstall_planned),
        uninstall_time,
    )
    .await
    .unwrap();
    let resolved = uninstall.resolved_grants.as_ref().unwrap();
    assert!(resolved.grants.is_empty());
    assert_eq!(resolved.revocations.len(), 1);
    assert!(uninstall.grant_confirmations.is_empty());
    assert!(uninstall.grant_ceilings.is_empty());
}

fn install_plan(
    provider: &dyn CognitivePackageAuthorizationProvider,
) -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let mut draft = PluginOperationPlanDraft::new(
        source.action,
        source.package_id,
        source.component_id,
        source.packages,
        source.providers,
        Vec::new(),
        source.impact,
        source.state,
    )
    .unwrap();
    let binding = binding(&source.scope, &draft, provider, "install:test:grant");
    let snapshot = empty_snapshot(&source.scope.id, draft.state.state_revision);
    let planned = plan_workspace_grants(&mut draft, &binding, &snapshot, false, true)
        .unwrap()
        .unwrap();
    let envelope = PluginOperationPlanEnvelope::new(draft.bind(binding).unwrap()).unwrap();
    (envelope, planned)
}

fn replacement_plan(
    provider: &dyn CognitivePackageAuthorizationProvider,
) -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let before = source.packages[0].after.clone().unwrap();
    let mut after = before.clone();
    after.release.version = "2.0.0".to_string();
    after.release.package_sha256 = digest('d');
    after.release.manifest_sha256 = digest('e');
    let transition = PlannedPackageTransition::resolved(
        source.package_id.clone(),
        PlanPackageRole::Root,
        PlanPackageChangeKind::Replace,
        Some(before.clone()),
        Some(after),
        Some(PluginPlanSource::LocalReviewed {
            source_digest: digest('f'),
            package_digest: digest('d'),
            unsigned: true,
        }),
    )
    .unwrap();
    let mut draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Upgrade,
        source.package_id,
        source.component_id,
        vec![transition],
        source.providers,
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: 1,
            reclaimed_bytes: 1,
            drain_required: has_private_service(&before),
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 4,
            capability_generation: source.state.capability_generation,
            receipt_digest: Some(digest('c')),
        },
    )
    .unwrap();
    let binding = binding(&source.scope, &draft, provider, "upgrade:test:grant");
    let snapshot = prior_snapshot(&source.scope.id, 4, &before);
    let planned = plan_workspace_grants(&mut draft, &binding, &snapshot, true, true)
        .unwrap()
        .unwrap();
    let envelope = PluginOperationPlanEnvelope::new(draft.bind(binding).unwrap()).unwrap();
    (envelope, planned)
}

fn uninstall_plan(
    provider: &dyn CognitivePackageAuthorizationProvider,
) -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let before = source.packages[0].after.clone().unwrap();
    let transition = PlannedPackageTransition::resolved(
        source.package_id.clone(),
        PlanPackageRole::Root,
        PlanPackageChangeKind::Remove,
        Some(before.clone()),
        None,
        None,
    )
    .unwrap();
    let mut draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Uninstall,
        source.package_id,
        source.component_id,
        vec![transition],
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 0,
            reclaimed_bytes: 1,
            drain_required: has_private_service(&before),
            retained_data: true,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 4,
            capability_generation: source.state.capability_generation,
            receipt_digest: Some(digest('c')),
        },
    )
    .unwrap();
    let binding = binding(&source.scope, &draft, provider, "uninstall:test:grant");
    let snapshot = prior_snapshot(&source.scope.id, 4, &before);
    let planned = plan_workspace_grants(&mut draft, &binding, &snapshot, true, false)
        .unwrap()
        .unwrap();
    let envelope = PluginOperationPlanEnvelope::new(draft.bind(binding).unwrap()).unwrap();
    (envelope, planned)
}

fn binding(
    scope: &PlanScope,
    draft: &PluginOperationPlanDraft,
    provider: &dyn CognitivePackageAuthorizationProvider,
    operation_id: &str,
) -> PluginOperationPlanBinding {
    PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
        created_at_ms: 1_710_000_000_000,
        expires_at_ms: 1_710_000_600_000,
        scope: scope.clone(),
        authority: provider.bind_authority(draft).unwrap(),
    }
}

fn empty_snapshot(scope_id: &str, state_revision: u64) -> PluginWorkspaceGrantSnapshot {
    PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: scope_id.to_string(),
        state_revision,
        grants: Vec::new(),
    }
}

fn prior_snapshot(
    scope_id: &str,
    state_revision: u64,
    before: &PlannedPackageState,
) -> PluginWorkspaceGrantSnapshot {
    PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: scope_id.to_string(),
        state_revision,
        grants: vec![WorkspaceGrantEvidence {
            package_id: before.release.package_id.clone(),
            package_digest: before.release.package_sha256.clone(),
            receipt_revision: state_revision - 1,
            grant_digest: digest('f'),
        }],
    }
}

fn has_private_service(state: &PlannedPackageState) -> bool {
    state
        .permissions
        .surfaces
        .iter()
        .any(|permission| permission.private_service)
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
