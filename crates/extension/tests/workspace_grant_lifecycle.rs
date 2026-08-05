#[path = "support/workspace_grant_lifecycle_fixtures.rs"]
mod fixtures;

use a3s_use_extension::{
    StoredWorkspaceGrant, WorkspaceGrantLifecyclePhase, WorkspaceGrantReceipt,
    WorkspaceGrantRevocation, WorkspaceGrantStore,
};
use fixtures::{cutover, digest, in_place_fixture, install_fixture, upgrade_fixture};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::fs;

#[tokio::test]
async fn install_intent_prepares_cuts_over_and_completes_idempotently() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = install_fixture();

    let intent = store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();
    assert_eq!(intent.phase, WorkspaceGrantLifecyclePhase::IntentRecorded);
    assert_eq!(
        store
            .begin_change_set(&fixture.resolved, &fixture.ceilings)
            .await
            .unwrap(),
        intent
    );
    assert_eq!(
        store
            .observe_change_set(&fixture.resolved.operation_id)
            .await
            .unwrap(),
        Some(intent.clone())
    );
    assert_eq!(
        store
            .retire_change_set(&fixture.resolved.operation_id)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.cutover_required"
    );
    assert_eq!(
        store
            .commit_change_set_cutover(
                &fixture.resolved.operation_id,
                cutover(&fixture.resolved),
                1_400,
            )
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.not_prepared"
    );

    let prepared = store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();
    assert_eq!(prepared.phase, WorkspaceGrantLifecyclePhase::Prepared);
    assert_eq!(
        store
            .prepare_change_set(&fixture.resolved.operation_id, 1_250)
            .await
            .unwrap(),
        prepared
    );
    for candidate in &prepared.intent.candidates {
        assert_eq!(
            store
                .resolve_active(
                    &fixture.resolved.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                    &candidate.ceiling,
                    1_250,
                )
                .await
                .unwrap(),
            Some(candidate.receipt.clone())
        );
    }

    let cutover = cutover(&fixture.resolved);
    let committed = store
        .commit_change_set_cutover(
            &fixture.resolved.operation_id,
            cutover.clone(),
            cutover.committed_at_ms,
        )
        .await
        .unwrap();
    assert_eq!(
        committed.phase,
        WorkspaceGrantLifecyclePhase::CutoverCommitted
    );
    assert_eq!(
        store
            .commit_change_set_cutover(
                &fixture.resolved.operation_id,
                cutover.clone(),
                cutover.committed_at_ms,
            )
            .await
            .unwrap(),
        committed
    );

    let completed = store
        .retire_change_set(&fixture.resolved.operation_id)
        .await
        .unwrap();
    assert_eq!(completed.phase, WorkspaceGrantLifecyclePhase::Completed);
    assert_eq!(
        store
            .retire_change_set(&fixture.resolved.operation_id)
            .await
            .unwrap(),
        completed
    );
    assert_eq!(
        store
            .snapshot_scope(&fixture.resolved.scope_id, fixture.resolved.revision,)
            .await
            .unwrap()
            .grants
            .len(),
        2
    );
}

#[tokio::test]
async fn upgrade_recovers_partial_prepare_and_partial_retirement() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = upgrade_fixture();
    for prior in &fixture.priors {
        store.put(prior, &fixture.ceiling, 1_000).await.unwrap();
    }
    let intent = store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();

    store
        .put(
            &intent.intent.candidates[0].receipt,
            &intent.intent.candidates[0].ceiling,
            1_250,
        )
        .await
        .unwrap();
    drop(store);
    let store = WorkspaceGrantStore::new(temporary.path());
    let prepared = store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();
    assert_eq!(prepared.phase, WorkspaceGrantLifecyclePhase::Prepared);
    assert_eq!(
        store
            .snapshot_scope(&fixture.resolved.scope_id, fixture.resolved.revision,)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.snapshot_unstable"
    );

    let cutover = cutover(&fixture.resolved);
    store
        .commit_change_set_cutover(
            &fixture.resolved.operation_id,
            cutover.clone(),
            cutover.committed_at_ms,
        )
        .await
        .unwrap();
    let first_retirement = &intent.intent.retirements[0];
    let first_revocation = WorkspaceGrantRevocation::new(
        fixture.resolved.revision,
        &first_retirement.prior_receipt,
        fixture.resolved.revocation_authority.clone(),
        cutover.committed_at_ms,
    )
    .unwrap();
    store
        .revoke(&first_retirement.prior_receipt, &first_revocation)
        .await
        .unwrap();

    drop(store);
    let store = WorkspaceGrantStore::new(temporary.path());
    let completed = store
        .retire_change_set(&fixture.resolved.operation_id)
        .await
        .unwrap();
    assert_eq!(completed.phase, WorkspaceGrantLifecyclePhase::Completed);
    for retirement in &completed.intent.retirements {
        assert!(matches!(
            store
                .observe(
                    &fixture.resolved.scope_id,
                    &retirement.evidence.package_id,
                    &retirement.evidence.package_digest,
                )
                .await
                .unwrap(),
            Some(StoredWorkspaceGrant::Revoked(_))
        ));
    }
    for candidate in &completed.intent.candidates {
        assert_eq!(
            store
                .observe(
                    &fixture.resolved.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                )
                .await
                .unwrap(),
            Some(StoredWorkspaceGrant::Granted(candidate.receipt.clone()))
        );
    }
    assert_eq!(
        store
            .snapshot_scope(&fixture.resolved.scope_id, fixture.resolved.revision,)
            .await
            .unwrap()
            .grants
            .len(),
        2
    );
}

#[tokio::test]
async fn lifecycle_rejects_conflict_future_cutover_and_candidate_drift() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = install_fixture();
    assert_eq!(
        store
            .begin_change_set(&fixture.resolved, &fixture.ceilings[..1])
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.ceiling_mismatch"
    );
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();

    let mut conflicting = fixture.resolved.clone();
    conflicting.plan_digest = digest('7');
    assert_eq!(
        store
            .begin_change_set(&conflicting, &fixture.ceilings)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.conflict"
    );

    let prepared = store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();
    let cutover = cutover(&fixture.resolved);
    assert_eq!(
        store
            .commit_change_set_cutover(
                &fixture.resolved.operation_id,
                cutover.clone(),
                cutover.committed_at_ms - 1,
            )
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.cutover_in_future"
    );

    let candidate = &prepared.intent.candidates[0];
    let mut changed_grant = candidate.receipt.grant.clone();
    changed_grant.granted_at_ms += 50;
    let changed = WorkspaceGrantReceipt::new(fixture.resolved.revision + 1, changed_grant).unwrap();
    store
        .put(&changed, &candidate.ceiling, 1_300)
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_change_set_cutover(
                &fixture.resolved.operation_id,
                cutover.clone(),
                cutover.committed_at_ms,
            )
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.candidate_changed"
    );

    let stale_temporary = TempDir::new().unwrap();
    let stale_store = WorkspaceGrantStore::new(stale_temporary.path());
    let stale_fixture = upgrade_fixture();
    for prior in &stale_fixture.priors {
        stale_store
            .put(prior, &stale_fixture.ceiling, 1_000)
            .await
            .unwrap();
    }
    let mut superseding_grant = stale_fixture.priors[0].grant.clone();
    superseding_grant.granted_at_ms += 25;
    let superseding =
        WorkspaceGrantReceipt::new(stale_fixture.priors[0].revision + 1, superseding_grant)
            .unwrap();
    stale_store
        .put(&superseding, &stale_fixture.ceiling, 1_100)
        .await
        .unwrap();
    assert_eq!(
        stale_store
            .begin_change_set(&stale_fixture.resolved, &stale_fixture.ceilings)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.snapshot_changed"
    );
}

#[tokio::test]
async fn same_generation_grant_replacement_is_not_revoked_after_cutover() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = in_place_fixture();
    store
        .put(&fixture.priors[0], &fixture.ceiling, 1_000)
        .await
        .unwrap();
    let intent = store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();
    let prepared = store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();
    let cutover = cutover(&fixture.resolved);
    store
        .commit_change_set_cutover(
            &fixture.resolved.operation_id,
            cutover.clone(),
            cutover.committed_at_ms,
        )
        .await
        .unwrap();
    let completed = store
        .retire_change_set(&fixture.resolved.operation_id)
        .await
        .unwrap();
    assert_eq!(completed.phase, WorkspaceGrantLifecyclePhase::Completed);
    assert_eq!(
        store
            .observe(
                &fixture.resolved.scope_id,
                &intent.intent.candidates[0].receipt.grant.package_id,
                &intent.intent.candidates[0].receipt.grant.package_digest,
            )
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(
            prepared.intent.candidates[0].receipt.clone()
        ))
    );
}

#[tokio::test]
async fn pre_cutover_rollback_removes_new_candidates_and_preserves_prior_grants() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = upgrade_fixture();
    for prior in &fixture.priors {
        store.put(prior, &fixture.ceiling, 1_000).await.unwrap();
    }
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();
    let prepared = store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();
    assert_eq!(prepared.phase, WorkspaceGrantLifecyclePhase::Prepared);

    drop(store);
    let store = WorkspaceGrantStore::new(temporary.path());
    let rollback_digest = digest('7');
    let rolled_back = store
        .rollback_change_set(
            &fixture.resolved.operation_id,
            rollback_digest.clone(),
            1_275,
            1_275,
        )
        .await
        .unwrap();
    assert_eq!(rolled_back.phase, WorkspaceGrantLifecyclePhase::RolledBack);
    assert_eq!(
        store
            .rollback_change_set(
                &fixture.resolved.operation_id,
                rollback_digest,
                1_275,
                1_300,
            )
            .await
            .unwrap(),
        rolled_back
    );

    for candidate in &rolled_back.intent.candidates {
        assert_eq!(
            store
                .observe(
                    &fixture.resolved.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                )
                .await
                .unwrap(),
            None
        );
    }
    for prior in &fixture.priors {
        assert_eq!(
            store
                .observe(
                    &fixture.resolved.scope_id,
                    &prior.grant.package_id,
                    &prior.grant.package_digest,
                )
                .await
                .unwrap(),
            Some(StoredWorkspaceGrant::Granted(prior.clone()))
        );
    }
    assert_eq!(
        store
            .prepare_change_set(&fixture.resolved.operation_id, 1_300)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.rolled_back"
    );
}

#[tokio::test]
async fn pre_cutover_rollback_restores_an_exact_same_generation_prior_record() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = in_place_fixture();
    let prior = fixture.priors[0].clone();
    store.put(&prior, &fixture.ceiling, 1_000).await.unwrap();
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();
    store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();

    let rolled_back = store
        .rollback_change_set(&fixture.resolved.operation_id, digest('8'), 1_275, 1_275)
        .await
        .unwrap();
    assert_eq!(rolled_back.phase, WorkspaceGrantLifecyclePhase::RolledBack);
    assert_eq!(
        store
            .observe(
                &fixture.resolved.scope_id,
                &prior.grant.package_id,
                &prior.grant.package_digest,
            )
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(prior))
    );
}

#[tokio::test]
async fn cutover_committed_grants_cannot_use_candidate_rollback() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = install_fixture();
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();
    store
        .prepare_change_set(&fixture.resolved.operation_id, 1_250)
        .await
        .unwrap();
    let cutover = cutover(&fixture.resolved);
    store
        .commit_change_set_cutover(
            &fixture.resolved.operation_id,
            cutover.clone(),
            cutover.committed_at_ms,
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .rollback_change_set(&fixture.resolved.operation_id, digest('9'), 1_350, 1_350,)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_operation.cutover_committed"
    );
}

#[tokio::test]
async fn operation_journal_fails_closed_on_unknown_privileged_fields() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let fixture = install_fixture();
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await
        .unwrap();

    let operation_digest = format!(
        "{:x}",
        Sha256::digest(fixture.resolved.operation_id.as_bytes())
    );
    let path = store
        .root()
        .join(".operations")
        .join(format!("{operation_digest}.json"));
    let bytes = fs::read(&path).await.unwrap();
    let mut document = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    document["privilegedToken"] = serde_json::json!("do-not-echo");
    fs::write(&path, serde_json::to_vec_pretty(&document).unwrap())
        .await
        .unwrap();

    let error = store
        .observe_change_set(&fixture.resolved.operation_id)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.grant_operation.invalid");
    assert!(!error.message.contains("do-not-echo"));
}

#[test]
fn lifecycle_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<a3s_use_extension::WorkspaceGrantOperationJournal>();
    assert_send_sync::<a3s_use_extension::WorkspaceGrantOperationIntent>();
    assert_send_sync::<a3s_use_extension::WorkspaceGrantCutoverEvidence>();
    assert_send_sync::<a3s_use_extension::WorkspaceGrantCandidateCeiling>();
}
