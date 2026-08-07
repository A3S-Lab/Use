use super::super::ReviewedCognitivePackageAuthorizationProvider;
use super::*;

use a3s_use_core::{
    PlanActor, PlanPackageRole, PlanScope, PlanScopeKind, PlannedOperationImpact,
    PlannedPackageTransition, PlannedStateEvidence, PluginCatalogRecord, PluginOperationAction,
    PluginOperationConfirmation, PluginOperationPlanBinding, PluginPackageLock,
    PluginPackageLockHost, PluginPackageResolver, PluginWorkspaceGrantSnapshot,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, WorkspaceGrantEvidence,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};

const INSTALL_PLAN: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/operation-plan-install-v4.json");
const CATALOG_RECORD: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/catalog-record-v3.json");

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

#[derive(Debug)]
struct AgentAsk;

#[async_trait]
impl CognitivePackageAuthorizationProvider for AgentAsk {
    fn name(&self) -> &'static str {
        "test-agent-ask"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        let mut authority =
            StandaloneCognitivePackageAuthorizationProvider.bind_authority(draft)?;
        authority.actor = PlanActor::Agent;
        Ok(authority)
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        let draft = PluginOperationPlanDraft::new(
            plan.action,
            plan.package_id.clone(),
            plan.component_id.clone(),
            plan.packages.clone(),
            plan.providers.clone(),
            Vec::new(),
            plan.impact.clone(),
            plan.state.clone(),
        )?;
        if plan.authority != self.bind_authority(&draft)? {
            return Err(UseError::new(
                "test.agent_authority_changed",
                "The test agent authority changed.",
            ));
        }
        Ok(())
    }

    async fn authorize(
        &self,
        _envelope: &PluginOperationPlanEnvelope,
        _changes: Option<&PluginWorkspaceGrantChangeSet>,
        _now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        Err(UseError::new(
            "test.agent_confirmation_required",
            "The test agent provider requires host confirmation.",
        ))
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

#[test]
fn host_grant_planner_matches_manager_planner_for_permission_bearing_user_install() {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let scope = PlanScope {
        kind: PlanScopeKind::User,
        id: "user/current".to_string(),
    };
    let mut manager_draft = PluginOperationPlanDraft::new(
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
    let binding = binding(
        &scope,
        &manager_draft,
        &StandaloneCognitivePackageAuthorizationProvider,
        "install:test:host-grant-planner",
        1_710_000_000_000,
    );
    let snapshot = empty_snapshot(&scope.id, manager_draft.state.state_revision);
    let mut host_draft = manager_draft.clone();

    let manager_grants =
        plan_workspace_grants(&mut manager_draft, &binding, &snapshot, false, true)
            .unwrap()
            .unwrap();
    let host_grants = bind_cognitive_package_grants(&mut host_draft, &binding, &snapshot).unwrap();

    assert_eq!(
        host_draft.workspace_impacts,
        manager_draft.workspace_impacts
    );
    assert_eq!(host_draft.workspace_impacts.len(), 1);
    assert_eq!(host_draft.workspace_impacts[0].scope_id, scope.id);
    assert!(!host_draft.workspace_impacts[0].enabled_before);
    assert!(host_draft.workspace_impacts[0].enabled_after);
    let change_set_digest = manager_grants.change_set.descriptor_digest().unwrap();
    assert_eq!(
        host_draft.workspace_impacts[0]
            .grant_after_digest
            .as_deref(),
        Some(change_set_digest.as_str())
    );
    let expected_proposal = manager_grants.change_set.changes[0].after.as_ref().unwrap();
    assert_eq!(
        host_grants.proposal(&expected_proposal.package_id),
        Some(expected_proposal)
    );
    assert_eq!(
        host_draft.bind(binding.clone()).unwrap(),
        manager_draft.bind(binding).unwrap()
    );
}

#[test]
fn host_grant_planner_rejects_scope_revision_and_prebound_impact_drift() {
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
    let binding = binding(
        &source.scope,
        &draft,
        &StandaloneCognitivePackageAuthorizationProvider,
        "install:test:host-grant-drift",
        1_710_000_000_000,
    );

    let wrong_scope = empty_snapshot("workspace:other", draft.state.state_revision);
    assert_eq!(
        bind_cognitive_package_grant_impacts(&mut draft.clone(), &binding, &wrong_scope)
            .unwrap_err()
            .code,
        "use.plugin.package_authorization_invalid"
    );

    let wrong_revision = empty_snapshot(&binding.scope.id, draft.state.state_revision + 1);
    assert_eq!(
        bind_cognitive_package_grant_impacts(&mut draft.clone(), &binding, &wrong_revision)
            .unwrap_err()
            .code,
        "use.plugin.package_authorization_invalid"
    );

    draft.workspace_impacts = source.workspace_impacts;
    let snapshot = empty_snapshot(&binding.scope.id, draft.state.state_revision);
    assert_eq!(
        bind_cognitive_package_grant_impacts(&mut draft, &binding, &snapshot)
            .unwrap_err()
            .code,
        "use.plugin.package_authorization_invalid"
    );
}

#[tokio::test]
async fn permission_free_workspace_plan_still_binds_exact_enablement_impact() {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let source_transition = source.packages[0].clone();
    let mut after = source_transition.after.unwrap();
    after
        .release
        .surfaces
        .retain(|surface| surface.kind == a3s_use_core::PluginSurfaceKind::Skill);
    after.permissions.surfaces.clear();
    after.release.permission_ceiling_digest = after.permissions.descriptor_digest().unwrap();
    let transition = PlannedPackageTransition::resolved(
        source.package_id.clone(),
        PlanPackageRole::Root,
        PlanPackageChangeKind::Add,
        None,
        Some(after),
        source_transition.source,
    )
    .unwrap();
    let mut draft = PluginOperationPlanDraft::new(
        source.action,
        source.package_id,
        source.component_id,
        vec![transition],
        Vec::new(),
        Vec::new(),
        source.impact,
        source.state,
    )
    .unwrap();
    let binding = binding(
        &source.scope,
        &draft,
        &StandaloneCognitivePackageAuthorizationProvider,
        "install:test:permission-free-workspace",
        1_710_000_000_000,
    );
    let snapshot = empty_snapshot(&source.scope.id, draft.state.state_revision);

    bind_cognitive_package_grant_impacts(&mut draft, &binding, &snapshot).unwrap();
    assert_eq!(draft.workspace_impacts.len(), 1);
    assert_eq!(draft.workspace_impacts[0].scope_id, source.scope.id);
    assert!(draft.workspace_impacts[0].grant_before_digest.is_none());
    assert!(draft.workspace_impacts[0].grant_after_digest.is_none());
    assert!(!draft.workspace_impacts[0].enabled_before);
    assert!(draft.workspace_impacts[0].enabled_after);

    let envelope = PluginOperationPlanEnvelope::new(draft.bind(binding).unwrap()).unwrap();
    let admitted_at_ms = envelope.plan.created_at_ms + 100;
    let authorization = authorize_planned_operation(
        &StandaloneCognitivePackageAuthorizationProvider,
        &envelope,
        None,
        admitted_at_ms,
    )
    .await
    .unwrap();
    authorization
        .validate_against(&envelope, admitted_at_ms)
        .unwrap();
    assert!(authorization.resolved_grants.is_none());
    assert!(authorization.grant_ceilings.is_empty());

    let mut drifted = envelope;
    drifted.plan.workspace_impacts[0].enabled_before = true;
    assert_eq!(
        authorization
            .validate_against(&drifted, admitted_at_ms)
            .unwrap_err()
            .code,
        "use.plugin.plan_invalid"
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

#[tokio::test]
async fn reviewed_host_authorization_preserves_exact_plan_and_confirmation() {
    let (expected, expected_grants) = install_plan(&ConfirmAll);
    let confirmed_at_ms = expected.plan.created_at_ms + 100;
    let confirmation = CognitivePackageAuthorizationEvidence::confirmed(
        &expected,
        Some(&expected_grants.change_set),
        confirmed_at_ms,
    )
    .unwrap()
    .operation_confirmation
    .unwrap();
    let provider = ReviewedCognitivePackageAuthorizationProvider::new(
        expected.clone(),
        Some(confirmation.clone()),
    )
    .unwrap();

    let (actual, actual_grants) = install_plan_with_operation(
        &provider,
        "install:untrusted:replacement",
        expected.plan.created_at_ms + 10,
    );
    assert_eq!(actual, expected);
    assert_eq!(actual_grants, expected_grants);

    let authorization =
        authorize_planned_operation(&provider, &actual, Some(&actual_grants), confirmed_at_ms)
            .await
            .unwrap();
    assert_eq!(authorization.operation_confirmation, Some(confirmation));
    assert_eq!(authorization.grant_confirmations.len(), 1);
    assert_eq!(
        authorization.grant_confirmations[0].operation_id,
        expected.plan.operation_id
    );
    assert_eq!(
        authorization.grant_confirmations[0].plan_digest,
        expected.plan_digest
    );
}

#[tokio::test]
async fn reviewed_agent_plan_accepts_only_exact_user_confirmation() {
    let (expected, expected_grants) = agent_install_plan();
    let confirmed_at_ms = expected.plan.created_at_ms + 100;
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: expected.plan.operation_id.clone(),
        plan_digest: expected.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms,
    };
    let provider = ReviewedCognitivePackageAuthorizationProvider::new(
        expected.clone(),
        Some(confirmation.clone()),
    )
    .unwrap();

    let authorization = authorize_planned_operation(
        &provider,
        &expected,
        Some(&expected_grants),
        confirmed_at_ms,
    )
    .await
    .unwrap();
    assert_eq!(authorization.operation_confirmation, Some(confirmation));
    assert_eq!(authorization.grant_confirmations.len(), 1);

    let mut wrong = authorization.operation_confirmation.unwrap();
    wrong.plan_digest = digest('9');
    assert_eq!(
        ReviewedCognitivePackageAuthorizationProvider::new(expected, Some(wrong))
            .unwrap_err()
            .code,
        "use.plugin.package_reviewed_authorization_invalid"
    );
}

fn agent_install_plan() -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let source_transition = &source.packages[0];
    let mut after = source_transition.after.clone().unwrap();
    for permission in &mut after.permissions.surfaces {
        permission.secrets.clear();
    }
    after.release.permission_ceiling_digest = after.permissions.descriptor_digest().unwrap();
    let transition = PlannedPackageTransition::resolved(
        source.package_id.clone(),
        PlanPackageRole::Root,
        PlanPackageChangeKind::Add,
        None,
        Some(after),
        source_transition.source.clone(),
    )
    .unwrap();
    let mut draft = PluginOperationPlanDraft::new(
        source.action,
        source.package_id,
        source.component_id,
        vec![transition],
        source.providers,
        Vec::new(),
        source.impact,
        source.state,
    )
    .unwrap();
    let binding = binding(
        &source.scope,
        &draft,
        &AgentAsk,
        "install:test:agent-grant",
        1_710_000_000_000,
    );
    let snapshot = empty_snapshot(&source.scope.id, draft.state.state_revision);
    let planned = plan_workspace_grants(&mut draft, &binding, &snapshot, false, true)
        .unwrap()
        .unwrap();
    let envelope = PluginOperationPlanEnvelope::new(draft.bind(binding).unwrap()).unwrap();
    (envelope, planned)
}

#[test]
fn reviewed_host_authorization_rejects_planner_drift_before_binding() {
    let (expected, expected_grants) = install_plan(&ConfirmAll);
    let confirmation = CognitivePackageAuthorizationEvidence::confirmed(
        &expected,
        Some(&expected_grants.change_set),
        expected.plan.created_at_ms + 100,
    )
    .unwrap()
    .operation_confirmation;
    let provider =
        ReviewedCognitivePackageAuthorizationProvider::new(expected, confirmation).unwrap();
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
    draft.impact.download_bytes += 1;
    let default = PluginOperationPlanBinding {
        operation_id: "install:untrusted:replacement".to_string(),
        created_at_ms: 1_710_000_000_010,
        expires_at_ms: 1_710_000_600_010,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "other".to_string(),
        },
        authority: provider.expected_plan().plan.authority.clone(),
    };

    assert_eq!(
        provider.bind_operation(&draft, default).unwrap_err().code,
        "use.plugin.package_reviewed_plan_mismatch"
    );
}

#[test]
fn reviewed_host_authorization_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ReviewedCognitivePackageAuthorizationProvider>();
}

#[test]
fn reviewed_host_authorization_preserves_upgrade_and_uninstall_bindings() {
    let (expected_upgrade, expected_upgrade_grants) = replacement_plan(&ConfirmAll);
    let upgrade_confirmation = CognitivePackageAuthorizationEvidence::confirmed(
        &expected_upgrade,
        Some(&expected_upgrade_grants.change_set),
        expected_upgrade.plan.created_at_ms + 100,
    )
    .unwrap()
    .operation_confirmation;
    let upgrade_provider = ReviewedCognitivePackageAuthorizationProvider::new(
        expected_upgrade.clone(),
        upgrade_confirmation,
    )
    .unwrap();
    let (actual_upgrade, actual_upgrade_grants) = replacement_plan_with_operation(
        &upgrade_provider,
        "upgrade:untrusted:replacement",
        expected_upgrade.plan.created_at_ms + 10,
    );
    assert_eq!(actual_upgrade, expected_upgrade);
    assert_eq!(actual_upgrade_grants, expected_upgrade_grants);

    let (expected_uninstall, expected_uninstall_grants) = uninstall_plan(&ConfirmAll);
    let uninstall_confirmation = CognitivePackageAuthorizationEvidence::confirmed(
        &expected_uninstall,
        Some(&expected_uninstall_grants.change_set),
        expected_uninstall.plan.created_at_ms + 100,
    )
    .unwrap()
    .operation_confirmation;
    let uninstall_provider = ReviewedCognitivePackageAuthorizationProvider::new(
        expected_uninstall.clone(),
        uninstall_confirmation,
    )
    .unwrap();
    let (actual_uninstall, actual_uninstall_grants) = uninstall_plan_with_operation(
        &uninstall_provider,
        "uninstall:untrusted:replacement",
        expected_uninstall.plan.created_at_ms + 10,
    );
    assert_eq!(actual_uninstall, expected_uninstall);
    assert_eq!(actual_uninstall_grants, expected_uninstall_grants);
}

fn install_plan(
    provider: &dyn CognitivePackageAuthorizationProvider,
) -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    install_plan_with_operation(provider, "install:test:grant", 1_710_000_000_000)
}

fn install_plan_with_operation(
    provider: &dyn CognitivePackageAuthorizationProvider,
    operation_id: &str,
    created_at_ms: u64,
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
    let binding = binding(&source.scope, &draft, provider, operation_id, created_at_ms);
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
    replacement_plan_with_operation(provider, "upgrade:test:grant", 1_710_000_000_000)
}

fn replacement_plan_with_operation(
    provider: &dyn CognitivePackageAuthorizationProvider,
    operation_id: &str,
    created_at_ms: u64,
) -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    let source = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let (prior_lock, candidate_lock) = replacement_package_locks();
    let before = prior_lock
        .package(&source.package_id)
        .unwrap()
        .catalog
        .selected_state(&[])
        .unwrap();
    let transition = candidate_lock
        .package(&source.package_id)
        .unwrap()
        .catalog
        .replace_transition(
            &prior_lock.package(&source.package_id).unwrap().catalog,
            PlanPackageRole::Root,
            &[],
            &[],
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
    let binding = binding(&source.scope, &draft, provider, operation_id, created_at_ms);
    let snapshot = prior_snapshot(&source.scope.id, 4, &before);
    let planned = plan_workspace_grants(&mut draft, &binding, &snapshot, true, true)
        .unwrap()
        .unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_upgrade_package_locks(
        draft.bind(binding).unwrap(),
        prior_lock,
        candidate_lock,
    )
    .unwrap();
    (envelope, planned)
}

fn replacement_package_locks() -> (PluginPackageLock, PluginPackageLock) {
    let prior_record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let prior = verified_catalog(prior_record.clone(), 39);

    let mut candidate_record = prior_record;
    candidate_record.version = "2.1.0".to_string();
    candidate_record.archive.target_name = candidate_record
        .archive
        .target_name
        .replace("/2.0.0/", "/2.1.0/")
        .replace("-2.0.0-", "-2.1.0-");
    let candidate_planning_target = candidate_record
        .planning
        .as_ref()
        .unwrap()
        .target_name
        .replace("/2.0.0/", "/2.1.0/");
    candidate_record.planning.as_mut().unwrap().target_name = candidate_planning_target;
    candidate_record.archive.sha256 = digest('d');
    candidate_record.package.sha256 = Some(digest('d'));
    candidate_record.package.manifest_sha256 = Some(digest('e'));
    candidate_record.validate().unwrap();
    let candidate = verified_catalog(candidate_record, 40);

    let host = PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap();
    let prior_lock = PluginPackageResolver::new(host.clone())
        .resolve(prior, Vec::new())
        .unwrap();
    let candidate_lock = PluginPackageResolver::new(host)
        .resolve(candidate, Vec::new())
        .unwrap();
    (prior_lock, candidate_lock)
}

fn verified_catalog(
    record: PluginCatalogRecord,
    targets_version: u64,
) -> VerifiedPluginCatalogRecord {
    let catalog_record_digest = record.descriptor_digest().unwrap();
    VerifiedPluginCatalogRecord::new(
        record,
        VerifiedCatalogProvenance {
            registry_name: "official".to_string(),
            registry_url: "https://plugins.a3s.dev/catalog".to_string(),
            root_sha256: digest('f'),
            root_version: 7,
            timestamp_version: targets_version + 3,
            snapshot_version: targets_version + 2,
            targets_version,
            catalog_record_digest,
        },
    )
    .unwrap()
}

fn uninstall_plan(
    provider: &dyn CognitivePackageAuthorizationProvider,
) -> (PluginOperationPlanEnvelope, PlannedWorkspaceGrantOperation) {
    uninstall_plan_with_operation(provider, "uninstall:test:grant", 1_710_000_000_000)
}

fn uninstall_plan_with_operation(
    provider: &dyn CognitivePackageAuthorizationProvider,
    operation_id: &str,
    created_at_ms: u64,
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
    let binding = binding(&source.scope, &draft, provider, operation_id, created_at_ms);
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
    created_at_ms: u64,
) -> PluginOperationPlanBinding {
    let default = PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
        created_at_ms,
        expires_at_ms: created_at_ms + 600_000,
        scope: scope.clone(),
        authority: provider.bind_authority(draft).unwrap(),
    };
    provider.bind_operation(draft, default).unwrap()
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
