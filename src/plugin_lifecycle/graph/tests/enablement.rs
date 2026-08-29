use std::path::PathBuf;

use a3s_use_core::{
    PlannedWorkspaceImpact, ResolvedWorkspaceGrant, ResolvedWorkspaceGrantChangeSet,
};
use a3s_use_extension::{WorkspaceGrantCandidateCeiling, WorkspaceGrantStore};

use super::grant::{
    authority, candidate_ceiling, provider_evidence, tool_catalog, tool_manifest, workspace_grant,
    SCOPE_ID, TRANSITIONED_AT_MS,
};
use super::*;

struct GrantEnablementFixture {
    _temp: tempfile::TempDir,
    grant_root: PathBuf,
    envelope: PluginOperationPlanEnvelope,
    coordinator: PluginLifecycleCoordinator,
    intent: PluginLifecycleIntent,
    manifest: ExtensionManifest,
    host: Arc<RecordingHost>,
    resolved: ResolvedWorkspaceGrantChangeSet,
    ceilings: Vec<WorkspaceGrantCandidateCeiling>,
}

impl GrantEnablementFixture {
    fn grants(&self) -> PluginGrantLifecycleUnit {
        PluginGrantLifecycleUnit::new(
            WorkspaceGrantStore::new(&self.grant_root),
            self.envelope.clone(),
            self.resolved.clone(),
            self.ceilings.clone(),
        )
        .unwrap()
    }
}

#[tokio::test]
async fn completed_enablement_replay_retries_cutover_acknowledgement_without_republication() {
    let fixture = grant_enablement_fixture();
    *fixture.host.fail_cutover_completion_once.lock().await = true;
    let grants = fixture.grants();
    let time = AtomicU64::new(TRANSITIONED_AT_MS);

    let error = fixture
        .coordinator
        .apply_enable_with_grants(
            &fixture.envelope,
            &fixture.intent,
            &fixture.manifest,
            &grants,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.test_cutover_completion_failure");
    assert!(grants.is_completed().await.unwrap());

    let record = fixture
        .coordinator
        .apply_enable_with_grants(
            &fixture.envelope,
            &fixture.intent,
            &fixture.manifest,
            &fixture.grants(),
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap();
    assert_eq!(record.status, PluginLifecycleOperationStatus::Completed);
    let calls = fixture.host.calls.lock().await;
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "acme/root:single-publish")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("single-cutover-complete:"))
            .count(),
        2
    );
}

fn grant_enablement_fixture() -> GrantEnablementFixture {
    let catalog = tool_catalog("1.0.0", 'a');
    let state = catalog
        .install_transition(PlanPackageRole::Root, &[])
        .unwrap()
        .after
        .unwrap();
    let transition = PlannedPackageTransition {
        package_id: state.release.package_id.clone(),
        role: PlanPackageRole::Root,
        change: PlanPackageChangeKind::Retain,
        before: Some(state.clone()),
        after: Some(state.clone()),
        source: None,
        surfaces: Vec::new(),
    };
    let change_set_digest = digest('2');
    let envelope = PluginOperationPlanEnvelope::new(
        PluginOperationPlanDraft::new(
            PluginOperationAction::Enable,
            "acme/root",
            "runtime:local",
            vec![transition],
            provider_evidence(),
            vec![PlannedWorkspaceImpact {
                scope_id: SCOPE_ID.to_string(),
                grant_before_digest: None,
                grant_after_digest: Some(change_set_digest.clone()),
                enabled_before: false,
                enabled_after: true,
            }],
            PlannedOperationImpact {
                download_bytes: 0,
                installed_bytes_after: 1,
                reclaimed_bytes: 0,
                drain_required: false,
                retained_data: false,
                okf_changes: Vec::new(),
            },
            PlannedStateEvidence {
                state_revision: 1,
                capability_generation: 1,
                receipt_digest: Some(digest('8')),
            },
        )
        .unwrap()
        .bind(PluginOperationPlanBinding {
            operation_id: "enable:acme-root:grant-4".to_string(),
            created_at_ms: 1_000,
            expires_at_ms: 5_000,
            scope: PlanScope {
                kind: PlanScopeKind::Workspace,
                id: SCOPE_ID.to_string(),
            },
            authority: PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Ask,
                policy_digest: digest('9'),
                confirmation_required: true,
            },
        })
        .unwrap(),
    )
    .unwrap();
    let ceiling = catalog.record.permission_ceiling.clone();
    let candidate = workspace_grant(&ceiling, &state.release.package_sha256, TRANSITIONED_AT_MS);
    let resolved = ResolvedWorkspaceGrantChangeSet {
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        change_set_digest,
        scope_id: SCOPE_ID.to_string(),
        state_revision_before: 1,
        revision: 2,
        capability_generation_before: 1,
        capability_generation_after: 2,
        before_snapshot_digest: None,
        transitioned_at_ms: TRANSITIONED_AT_MS,
        revocation_authority: authority(),
        grants: vec![ResolvedWorkspaceGrant {
            proposal_digest: digest('4'),
            grant: candidate,
        }],
        revocations: Vec::new(),
    };
    resolved.validate().unwrap();
    let ceilings = vec![candidate_ceiling(&resolved.grants[0].grant, &ceiling)];
    let manifest = tool_manifest("1.0.0");
    let intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            scope: envelope.plan.scope.clone(),
            package_id: "acme/root".to_string(),
            package_digest: state.release.package_sha256.clone(),
            manifest_digest: state.release.manifest_sha256.clone(),
            generation: 1,
            action: PluginLifecycleAction::Enable,
            retained_ui_state_surfaces: Vec::new(),
        },
        &manifest,
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let grant_root = temp.path().join("grant-state");
    let host = Arc::new(RecordingHost::default());
    host.cutover_generation_before.store(1, Ordering::Relaxed);
    let coordinator = coordinator(
        &temp.path().join("enablement-journal"),
        intent.scope.clone(),
        host.clone(),
    );
    GrantEnablementFixture {
        _temp: temp,
        grant_root,
        envelope,
        coordinator,
        intent,
        manifest,
        host,
        resolved,
        ceilings,
    }
}
