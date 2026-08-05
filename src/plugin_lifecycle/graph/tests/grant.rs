use std::path::PathBuf;

use a3s_use_core::{
    CatalogPlanningTarget, CatalogSurface, PlanEnforcementProfile, PlanQualifiedSurfaceRef,
    PlannedProviderEvidence, PlannedWorkspaceImpact, PluginPermissionCeiling, PluginSurfaceKind,
    PluginSurfaceRef, PluginWorkspaceGrant, PluginWorkspaceGrantSnapshot, ResolvedWorkspaceGrant,
    ResolvedWorkspaceGrantChangeSet, ToolWorkloadClass, WorkspaceGrantAuthority,
    WorkspaceGrantEvidence, PLUGIN_WORKSPACE_GRANT_SCHEMA, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};
use a3s_use_extension::{
    StoredWorkspaceGrant, WorkspaceGrantCandidateCeiling, WorkspaceGrantLifecyclePhase,
    WorkspaceGrantReceipt, WorkspaceGrantStore,
};

use super::*;

const TOOL_MANIFEST: &str =
    include_str!("../../../../crates/extension/fixtures/manifests/plugin-v3.acl");
const PERMISSION_CEILING: &[u8] =
    include_bytes!("../../../../crates/core/fixtures/plugins/permission-ceiling-v1.json");
const SCOPE_ID: &str = "workspace-01";
const TRANSITIONED_AT_MS: u64 = 1_200;

struct GrantInstallFixture {
    _temp: tempfile::TempDir,
    grant_root: PathBuf,
    envelope: PluginOperationPlanEnvelope,
    units: Vec<PluginPackageLifecycleUnit>,
    host: Arc<RecordingHost>,
    resolved: ResolvedWorkspaceGrantChangeSet,
    ceilings: Vec<WorkspaceGrantCandidateCeiling>,
}

impl GrantInstallFixture {
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

struct GrantUpgradeFixture {
    _temp: tempfile::TempDir,
    grant_root: PathBuf,
    envelope: PluginOperationPlanEnvelope,
    prior_lock: a3s_use_core::PluginPackageLock,
    candidates: Vec<PluginPackageLifecycleUnit>,
    retirements: Vec<PluginPackageLifecycleUnit>,
    host: Arc<RecordingHost>,
    resolved: ResolvedWorkspaceGrantChangeSet,
    ceilings: Vec<WorkspaceGrantCandidateCeiling>,
    prior: WorkspaceGrantReceipt,
    candidate_digest: String,
    ceiling: PluginPermissionCeiling,
}

impl GrantUpgradeFixture {
    fn store(&self) -> WorkspaceGrantStore {
        WorkspaceGrantStore::new(&self.grant_root)
    }

    fn grants(&self) -> PluginGrantLifecycleUnit {
        PluginGrantLifecycleUnit::new(
            self.store(),
            self.envelope.clone(),
            self.resolved.clone(),
            self.ceilings.clone(),
        )
        .unwrap()
    }
}

struct GrantUninstallFixture {
    _temp: tempfile::TempDir,
    grant_root: PathBuf,
    envelope: PluginOperationPlanEnvelope,
    units: Vec<PluginPackageLifecycleUnit>,
    host: Arc<RecordingHost>,
    resolved: ResolvedWorkspaceGrantChangeSet,
    prior: WorkspaceGrantReceipt,
}

impl GrantUninstallFixture {
    fn store(&self) -> WorkspaceGrantStore {
        WorkspaceGrantStore::new(&self.grant_root)
    }

    fn grants(&self) -> PluginGrantLifecycleUnit {
        PluginGrantLifecycleUnit::new(
            self.store(),
            self.envelope.clone(),
            self.resolved.clone(),
            Vec::new(),
        )
        .unwrap()
    }
}

#[tokio::test]
async fn install_persists_grants_before_package_prepare_and_replays_after_failure() {
    let fixture = grant_install_fixture();
    *fixture.host.fail_once.lock().await = Some("acme/root:tool-prepare".to_string());
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(TRANSITIONED_AT_MS);

    let error = graph
        .apply_install_with_grants(&fixture.envelope, &fixture.units, &fixture.grants(), || {
            time.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.test_injected_failure");

    let replay_grants = fixture.grants();
    let prepared = replay_grants.observe().await.unwrap().unwrap();
    assert_eq!(prepared.phase, WorkspaceGrantLifecyclePhase::Prepared);
    let candidate = &fixture.resolved.grants[0].grant;
    assert!(matches!(
        WorkspaceGrantStore::new(&fixture.grant_root)
            .observe(SCOPE_ID, &candidate.package_id, &candidate.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(_))
    ));

    let records = graph
        .apply_install_with_grants(&fixture.envelope, &fixture.units, &replay_grants, || {
            time.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        replay_grants.observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::Completed
    );
}

#[tokio::test]
async fn upgrade_publication_failure_rolls_back_candidate_grant_and_preserves_prior() {
    let fixture = grant_upgrade_fixture().await;
    *fixture.host.fail_exact_publication_once.lock().await = true;
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(TRANSITIONED_AT_MS);

    let error = graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &fixture.grants(),
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.test_publication_failure");

    let store = fixture.store();
    assert_eq!(
        fixture.grants().observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::RolledBack
    );
    assert!(store
        .observe(SCOPE_ID, "acme/root", &fixture.candidate_digest)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(fixture.prior.clone()))
    );
}

#[tokio::test]
async fn upgrade_retires_only_prior_grant_after_exact_cutover_and_replays_idempotently() {
    let fixture = grant_upgrade_fixture().await;
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(TRANSITIONED_AT_MS);
    let grants = fixture.grants();

    graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &grants,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap();

    let store = fixture.store();
    let candidate = store
        .resolve_active(
            SCOPE_ID,
            "acme/root",
            &fixture.candidate_digest,
            &fixture.ceiling,
            time.load(Ordering::Relaxed),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(candidate.revision, fixture.resolved.revision);
    let prior = store
        .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
        .await
        .unwrap();
    assert!(matches!(prior, Some(StoredWorkspaceGrant::Revoked(_))));
    let first_journal = grants.observe().await.unwrap().unwrap();
    assert_eq!(first_journal.phase, WorkspaceGrantLifecyclePhase::Completed);

    let replay_grants = fixture.grants();
    graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &replay_grants,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap();
    assert_eq!(
        replay_grants.observe().await.unwrap().unwrap().cutover,
        first_journal.cutover
    );
}

#[tokio::test]
async fn generation_drift_fails_closed_without_retiring_prior_and_corrected_replay_completes() {
    let fixture = grant_upgrade_fixture().await;
    *fixture.host.drift_cutover_generation_once.lock().await = true;
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(TRANSITIONED_AT_MS);
    let grants = fixture.grants();

    let error = graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &grants,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.grant_composition_invalid");
    assert_eq!(
        grants.observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::Prepared
    );
    assert_eq!(
        fixture
            .store()
            .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(fixture.prior.clone()))
    );

    graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &fixture.grants(),
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.grants().observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::Completed
    );
}

#[tokio::test]
async fn upgrade_keeps_prior_grant_until_old_generation_calls_are_drained() {
    let fixture = grant_upgrade_fixture().await;
    *fixture.host.fail_once.lock().await = Some("acme/root:drain".to_string());
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(TRANSITIONED_AT_MS);
    let grants = fixture.grants();

    let error = graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &grants,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.test_injected_failure");
    assert_eq!(
        grants.observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::CutoverCommitted
    );
    assert_eq!(
        fixture
            .store()
            .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(fixture.prior.clone()))
    );

    graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &fixture.grants(),
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Revoked(_))
    ));
}

#[tokio::test]
async fn uninstall_hides_graph_once_drains_then_retires_grant_before_package_removal() {
    let fixture = grant_uninstall_fixture().await;
    *fixture.host.fail_once.lock().await = Some("acme/root:drain".to_string());
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(TRANSITIONED_AT_MS);
    let grants = fixture.grants();

    let error = graph
        .apply_uninstall_with_grants(&fixture.envelope, &fixture.units, &grants, || {
            time.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.test_injected_failure");
    assert_eq!(
        grants.observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::CutoverCommitted
    );
    assert_eq!(
        fixture
            .store()
            .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(fixture.prior.clone()))
    );

    graph
        .apply_uninstall_with_grants(&fixture.envelope, &fixture.units, &fixture.grants(), || {
            time.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .unwrap();

    assert!(matches!(
        fixture
            .store()
            .observe(SCOPE_ID, "acme/root", &fixture.prior.grant.package_digest)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Revoked(_))
    ));
    let calls = fixture.host.calls.lock().await;
    assert_eq!(
        calls.first().map(String::as_str),
        Some("hide-batch:acme/root")
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "hide-batch:acme/root")
            .count(),
        2
    );
    assert!(!calls.iter().any(|call| call == "acme/root:hide"));
    let remove = calls
        .iter()
        .position(|call| call == "acme/root:remove")
        .unwrap();
    assert!(remove > 0);
}

fn grant_install_fixture() -> GrantInstallFixture {
    let catalog = tool_catalog("1.0.0", 'a');
    let lock = package_lock(catalog.clone());
    let transition = catalog
        .install_transition(PlanPackageRole::Root, &[])
        .unwrap();
    let change_set_digest = digest('2');
    let envelope = operation_envelope(
        PluginOperationAction::Install,
        "install:acme-root:grant-1",
        vec![transition],
        provider_evidence(),
        PlannedWorkspaceImpact {
            scope_id: SCOPE_ID.to_string(),
            grant_before_digest: None,
            grant_after_digest: Some(change_set_digest.clone()),
            enabled_before: false,
            enabled_after: true,
        },
        1,
        None,
        Some(lock.clone()),
        None,
    );
    let ceiling = catalog.record.permission_ceiling.clone();
    let state = envelope.plan.packages[0].after.as_ref().unwrap();
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

    let temp = tempfile::tempdir().unwrap();
    let grant_root = temp.path().join("grant-state");
    let host = Arc::new(RecordingHost::default());
    host.cutover_generation_before.store(1, Ordering::Relaxed);
    let manifest = tool_manifest("1.0.0");
    let units = vec![package_unit(
        temp.path(),
        host.clone(),
        &envelope,
        manifest,
        1,
        PluginLifecycleAction::Install,
    )];
    GrantInstallFixture {
        _temp: temp,
        grant_root,
        envelope,
        units,
        host,
        resolved,
        ceilings,
    }
}

async fn grant_upgrade_fixture() -> GrantUpgradeFixture {
    let prior_catalog = tool_catalog("1.0.0", 'a');
    let candidate_catalog = tool_catalog("1.1.0", 'b');
    let prior_lock = package_lock(prior_catalog.clone());
    let candidate_lock = package_lock(candidate_catalog.clone());
    let transition = candidate_catalog
        .replace_transition(&prior_catalog, PlanPackageRole::Root, &[], &[])
        .unwrap();
    let ceiling = candidate_catalog.record.permission_ceiling.clone();
    let prior_state = transition.before.as_ref().unwrap();
    let prior = WorkspaceGrantReceipt::new(
        2,
        workspace_grant(&ceiling, &prior_state.release.package_sha256, 1_100),
    )
    .unwrap();
    let before_snapshot_digest = snapshot_digest(2, &prior);
    let change_set_digest = digest('2');
    let envelope = operation_envelope(
        PluginOperationAction::Upgrade,
        "upgrade:acme-root:grant-2",
        vec![transition],
        provider_evidence(),
        PlannedWorkspaceImpact {
            scope_id: SCOPE_ID.to_string(),
            grant_before_digest: Some(before_snapshot_digest.clone()),
            grant_after_digest: Some(change_set_digest.clone()),
            enabled_before: true,
            enabled_after: true,
        },
        2,
        Some(digest('8')),
        Some(candidate_lock),
        Some(prior_lock.clone()),
    );
    let candidate_state = envelope.plan.packages[0].after.as_ref().unwrap();
    let candidate_digest = candidate_state.release.package_sha256.clone();
    let resolved = ResolvedWorkspaceGrantChangeSet {
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        change_set_digest,
        scope_id: SCOPE_ID.to_string(),
        state_revision_before: 2,
        revision: 3,
        capability_generation_before: 2,
        capability_generation_after: 3,
        before_snapshot_digest: Some(before_snapshot_digest),
        transitioned_at_ms: TRANSITIONED_AT_MS,
        revocation_authority: authority(),
        grants: vec![ResolvedWorkspaceGrant {
            proposal_digest: digest('4'),
            grant: workspace_grant(&ceiling, &candidate_digest, TRANSITIONED_AT_MS),
        }],
        revocations: vec![grant_evidence(&prior)],
    };
    resolved.validate().unwrap();
    let ceilings = vec![candidate_ceiling(&resolved.grants[0].grant, &ceiling)];

    let temp = tempfile::tempdir().unwrap();
    let grant_root = temp.path().join("grant-state");
    WorkspaceGrantStore::new(&grant_root)
        .put(&prior, &ceiling, TRANSITIONED_AT_MS)
        .await
        .unwrap();
    let host = Arc::new(RecordingHost::default());
    host.cutover_generation_before.store(2, Ordering::Relaxed);
    let candidates = vec![package_unit(
        temp.path(),
        host.clone(),
        &envelope,
        tool_manifest("1.1.0"),
        2,
        PluginLifecycleAction::Upgrade,
    )];
    let retirements = vec![package_unit(
        temp.path(),
        host.clone(),
        &envelope,
        tool_manifest("1.0.0"),
        1,
        PluginLifecycleAction::Uninstall,
    )];
    GrantUpgradeFixture {
        _temp: temp,
        grant_root,
        envelope,
        prior_lock,
        candidates,
        retirements,
        host,
        resolved,
        ceilings,
        prior,
        candidate_digest,
        ceiling,
    }
}

async fn grant_uninstall_fixture() -> GrantUninstallFixture {
    let catalog = tool_catalog("1.0.0", 'a');
    let lock = package_lock(catalog.clone());
    let transition = catalog
        .remove_transition(PlanPackageRole::Root, &[])
        .unwrap();
    let ceiling = catalog.record.permission_ceiling.clone();
    let prior_state = transition.before.as_ref().unwrap();
    let prior = WorkspaceGrantReceipt::new(
        3,
        workspace_grant(&ceiling, &prior_state.release.package_sha256, 1_100),
    )
    .unwrap();
    let before_snapshot_digest = snapshot_digest(3, &prior);
    let change_set_digest = digest('2');
    let envelope = operation_envelope(
        PluginOperationAction::Uninstall,
        "uninstall:acme-root:grant-3",
        vec![transition],
        Vec::new(),
        PlannedWorkspaceImpact {
            scope_id: SCOPE_ID.to_string(),
            grant_before_digest: Some(before_snapshot_digest.clone()),
            grant_after_digest: Some(change_set_digest.clone()),
            enabled_before: true,
            enabled_after: false,
        },
        3,
        Some(digest('8')),
        Some(lock),
        None,
    );
    let resolved = ResolvedWorkspaceGrantChangeSet {
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        change_set_digest,
        scope_id: SCOPE_ID.to_string(),
        state_revision_before: 3,
        revision: 4,
        capability_generation_before: 3,
        capability_generation_after: 4,
        before_snapshot_digest: Some(before_snapshot_digest),
        transitioned_at_ms: TRANSITIONED_AT_MS,
        revocation_authority: authority(),
        grants: Vec::new(),
        revocations: vec![grant_evidence(&prior)],
    };
    resolved.validate().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let grant_root = temp.path().join("grant-state");
    WorkspaceGrantStore::new(&grant_root)
        .put(&prior, &ceiling, TRANSITIONED_AT_MS)
        .await
        .unwrap();
    let host = Arc::new(RecordingHost::default());
    host.cutover_generation_before.store(3, Ordering::Relaxed);
    let units = vec![package_unit(
        temp.path(),
        host.clone(),
        &envelope,
        tool_manifest("1.0.0"),
        1,
        PluginLifecycleAction::Uninstall,
    )];
    GrantUninstallFixture {
        _temp: temp,
        grant_root,
        envelope,
        units,
        host,
        resolved,
        prior,
    }
}

#[allow(clippy::too_many_arguments)]
fn operation_envelope(
    action: PluginOperationAction,
    operation_id: &str,
    transitions: Vec<PlannedPackageTransition>,
    providers: Vec<PlannedProviderEvidence>,
    workspace_impact: PlannedWorkspaceImpact,
    state_revision: u64,
    receipt_digest: Option<String>,
    candidate_lock: Option<a3s_use_core::PluginPackageLock>,
    prior_lock: Option<a3s_use_core::PluginPackageLock>,
) -> PluginOperationPlanEnvelope {
    let release = transitions[0]
        .after
        .as_ref()
        .or(transitions[0].before.as_ref())
        .unwrap();
    let impact = match action {
        PluginOperationAction::Install => PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: 1,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Upgrade => PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: 1,
            reclaimed_bytes: 1,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Uninstall => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 0,
            reclaimed_bytes: release.release.surfaces.len() as u64,
            drain_required: false,
            retained_data: true,
            okf_changes: Vec::new(),
        },
    };
    let plan = PluginOperationPlanDraft::new(
        action,
        "acme/root",
        "runtime:local",
        transitions,
        providers,
        vec![workspace_impact],
        impact,
        PlannedStateEvidence {
            state_revision,
            capability_generation: state_revision,
            receipt_digest,
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
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
    .unwrap();
    match (candidate_lock, prior_lock) {
        (Some(candidate), Some(prior)) => {
            PluginOperationPlanEnvelope::new_with_upgrade_package_locks(plan, prior, candidate)
                .unwrap()
        }
        (Some(candidate), None) => {
            PluginOperationPlanEnvelope::new_with_package_lock(plan, candidate).unwrap()
        }
        _ => panic!("test operation requires a candidate lock"),
    }
}

fn package_unit(
    root: &std::path::Path,
    host: Arc<RecordingHost>,
    envelope: &PluginOperationPlanEnvelope,
    manifest: ExtensionManifest,
    generation: u64,
    action: PluginLifecycleAction,
) -> PluginPackageLifecycleUnit {
    let transition = &envelope.plan.packages[0];
    let state = match action {
        PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade => {
            transition.after.as_ref()
        }
        PluginLifecycleAction::Uninstall => transition.before.as_ref(),
        _ => None,
    }
    .unwrap();
    let intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            scope_id: SCOPE_ID.to_string(),
            package_id: "acme/root".to_string(),
            package_digest: state.release.package_sha256.clone(),
            manifest_digest: state.release.manifest_sha256.clone(),
            generation,
            action,
        },
        &manifest,
    )
    .unwrap();
    let journal = root.join(format!("journal-{generation}-{}", action.name()));
    PluginPackageLifecycleUnit::new(coordinator(&journal, host), intent, manifest).unwrap()
}

fn tool_catalog(version: &str, seed: char) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    record.package_id = "acme/root".to_string();
    record.publisher = "acme".to_string();
    record.display_name = "Grant Graph Fixture".to_string();
    record.description = "Permission-bearing graph lifecycle fixture.".to_string();
    record.version = version.to_string();
    record.dependencies.clear();
    record.repository = "https://github.com/acme/root".to_string();
    record.surfaces = vec![CatalogSurface {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_string(),
        optional: false,
        workload: Some(ToolWorkloadClass::Task),
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    }];
    record.permission_ceiling = permission_ceiling();
    record.permission_ceiling_digest = record.permission_ceiling.descriptor_digest().unwrap();
    record.planning = Some(CatalogPlanningTarget {
        target_name: format!("extensions/acme/root/{version}/stable/linux-x86_64/planning-v1.json"),
        length: 1,
        sha256: digest('e'),
    });
    record.archive.target_name =
        format!("extensions/acme/root/{version}/stable/linux-x86_64/acme-root-{version}.tar.gz");
    record.archive.sha256 = digest(seed);
    record.package.sha256 = Some(digest(seed));
    record.package.manifest_sha256 = Some(digest(seed));
    record.availability = CatalogAvailability::Available;
    record.validate().unwrap();
    let catalog_record_digest = record.descriptor_digest().unwrap();
    VerifiedPluginCatalogRecord::new(
        record,
        VerifiedCatalogProvenance {
            registry_name: "official".to_string(),
            registry_url: "https://packages.example.test/catalog/".to_string(),
            root_sha256: digest('f'),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest,
        },
    )
    .unwrap()
}

fn package_lock(catalog: VerifiedPluginCatalogRecord) -> a3s_use_core::PluginPackageLock {
    PluginPackageResolver::new(PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap())
        .resolve(catalog, Vec::new())
        .unwrap()
}

fn tool_manifest(version: &str) -> ExtensionManifest {
    let mut manifest = ExtensionManifest::parse_acl(TOOL_MANIFEST).unwrap();
    manifest.package_id = "acme/root".to_string();
    manifest.version = version.to_string();
    manifest.route = "root".to_string();
    manifest.dependencies.clear();
    manifest.tools.retain(|surface| surface.id == "convert");
    manifest.mcp_servers.clear();
    manifest.okf.clear();
    manifest.flows.clear();
    manifest.skills.clear();
    manifest.ui.clear();
    manifest
}

fn permission_ceiling() -> PluginPermissionCeiling {
    let mut ceiling = PluginPermissionCeiling::from_json(PERMISSION_CEILING).unwrap();
    ceiling.surfaces.retain(|surface| {
        surface.surface.kind == PluginSurfaceKind::Tool && surface.surface.id == "convert"
    });
    let permission = ceiling.surfaces.first_mut().unwrap();
    permission.native_execution = false;
    permission.secrets.clear();
    ceiling.validate().unwrap();
    ceiling
}

fn provider_evidence() -> Vec<PlannedProviderEvidence> {
    vec![PlannedProviderEvidence {
        surface: PlanQualifiedSurfaceRef {
            package_id: "acme/root".to_string(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            },
        },
        provider_id: "runtime:test".to_string(),
        provider_build_id: "runtime:test:1".to_string(),
        capability_digest: digest('7'),
        semantics_profile_digest: digest('8'),
        enforcement: PlanEnforcementProfile::Container,
    }]
}

fn workspace_grant(
    ceiling: &PluginPermissionCeiling,
    package_digest: &str,
    granted_at_ms: u64,
) -> PluginWorkspaceGrant {
    PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: SCOPE_ID.to_string(),
        package_id: "acme/root".to_string(),
        package_digest: package_digest.to_string(),
        permission_ceiling_digest: ceiling.descriptor_digest().unwrap(),
        permissions_digest: ceiling.descriptor_digest().unwrap(),
        permissions: ceiling.clone(),
        authority: authority(),
        granted_at_ms,
        expires_at_ms: None,
    }
}

fn authority() -> WorkspaceGrantAuthority {
    WorkspaceGrantAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: digest('9'),
        confirmation_digest: Some(digest('c')),
    }
}

fn candidate_ceiling(
    grant: &PluginWorkspaceGrant,
    ceiling: &PluginPermissionCeiling,
) -> WorkspaceGrantCandidateCeiling {
    WorkspaceGrantCandidateCeiling {
        package_id: grant.package_id.clone(),
        package_digest: grant.package_digest.clone(),
        ceiling: ceiling.clone(),
    }
}

fn grant_evidence(receipt: &WorkspaceGrantReceipt) -> WorkspaceGrantEvidence {
    WorkspaceGrantEvidence {
        package_id: receipt.grant.package_id.clone(),
        package_digest: receipt.grant.package_digest.clone(),
        receipt_revision: receipt.revision,
        grant_digest: receipt.grant_digest.clone(),
    }
}

fn snapshot_digest(state_revision: u64, receipt: &WorkspaceGrantReceipt) -> String {
    PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: SCOPE_ID.to_string(),
        state_revision,
        grants: vec![grant_evidence(receipt)],
    }
    .descriptor_digest()
    .unwrap()
}
