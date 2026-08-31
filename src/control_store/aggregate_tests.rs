use std::sync::Arc;

use a3s_use_core::{
    InstallationId, InstallationKind, InstallationPackageSelection, InstallationRootSelection,
    InstallationSnapshot, PlanActor, PlanAuthority, PlanEnforcementProfile, PlanPackageChangeKind,
    PlanPackageRole, PlanPolicyDecision, PlanQualifiedSurfaceRef, PlannedOperationImpact,
    PlannedPackageTransition, PlannedProviderEvidence, PlannedStateEvidence,
    PlannedWorkspaceImpact, PluginCatalogRecord, PluginOperationAction,
    PluginOperationConfirmation, PluginOperationPlanBinding, PluginOperationPlanDraft,
    PluginOperationPlanEnvelope, PluginPackageLock, PluginPackageLockHost, PluginPackageResolver,
    PluginSurfaceKind, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{
    ControlAppliedEffect, ControlAppliedEffectEvidence, ControlCapabilitySelection,
    ControlCapabilityStatus, ControlEffectClaim, ControlEffectIntent, ControlEffectKind,
    ControlEffectObservation, ControlEffectOutcome, ControlEffectOwner, ControlEffectStatus,
    ControlEffectSubject, ControlGeneration, ControlGrantSelection, ControlOperationStatus,
    ControlPackageLifecycle, ControlProjectionHistory, ControlProviderSelection,
    ControlRuntimeBindingObservation, ControlSurfaceObservationState, ControlTransition,
    ProjectedControlGeneration, ReviewedControlOperation,
};
use super::*;
use crate::plugin_lifecycle::PluginLifecycleAction;

mod effect_fixtures;
mod effect_observations;
mod effects;
pub(super) mod fixtures;
mod generations;
mod grant_fixtures;
mod grants;
mod operations;
mod payloads;
mod projections;
mod providers;

use fixtures::*;

#[tokio::test]
async fn aggregate_transition_is_atomic_idempotent_and_generation_bound() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:install:1");

    let registered = store.register_operation(reviewed.clone()).await.unwrap();
    assert_eq!(registered.status, ControlOperationStatus::Reviewed);
    assert_eq!(
        store.register_operation(reviewed.clone()).await.unwrap(),
        registered
    );

    let conflicting = operation_at_with_policy(
        reviewed.operation_id(),
        PluginOperationAction::Install,
        0,
        0,
        'b',
    );
    assert_eq!(
        store
            .register_operation(conflicting)
            .await
            .unwrap_err()
            .code,
        "use.control_store.conflict"
    );

    let candidate = transition(control_installation(), &reviewed);
    let committed = store.commit_transition(candidate.clone()).await.unwrap();
    assert_eq!(committed.snapshot, candidate.snapshot);
    assert_eq!(committed.grants, candidate.grants);
    assert_eq!(committed.provider_selections, candidate.provider_selections);
    assert_eq!(
        committed.capability_status,
        ControlCapabilityStatus::Candidate
    );
    assert_eq!(store.commit_transition(candidate).await.unwrap(), committed);

    let inspection = store.inspect().await.unwrap();
    assert_eq!(inspection.metadata.current_generation, 1);
    assert_eq!(inspection.metadata.published_capability_generation, 0);
    assert_eq!(store.current_generation().await.unwrap(), Some(committed));
    assert_eq!(
        store
            .operation(reviewed.operation_id())
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::EffectsPending
    );
    let effects = store.effects(reviewed.operation_id()).await.unwrap();
    assert_eq!(effects.len(), 3);
    assert!(effects
        .iter()
        .all(|effect| effect.status == ControlEffectStatus::Pending));

    let stale = operation("operation:stale:1");
    assert_eq!(
        store.register_operation(stale).await.unwrap_err().code,
        "use.control_store.generation_changed"
    );
}

#[tokio::test]
async fn action_semantics_reject_impossible_root_state_transitions() {
    let (_temporary, store) = initialized_store().await;
    for (index, action) in [
        PluginOperationAction::Upgrade,
        PluginOperationAction::Enable,
        PluginOperationAction::Disable,
        PluginOperationAction::Uninstall,
    ]
    .into_iter()
    .enumerate()
    {
        let reviewed = operation_at(&format!("operation:invalid-action:{index}"), action, 0, 0);
        store.register_operation(reviewed.clone()).await.unwrap();
        let error = store
            .commit_transition(transition(control_installation(), &reviewed))
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.control_store.input_invalid");
        assert_eq!(
            store
                .operation(reviewed.operation_id())
                .await
                .unwrap()
                .unwrap()
                .status,
            ControlOperationStatus::Reviewed
        );
    }
    assert_eq!(
        store.inspect().await.unwrap().metadata.current_generation,
        0
    );
}

#[tokio::test]
async fn outbox_lease_unknown_reconciliation_and_terminal_replay_are_exact() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:effects:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    store.commit_transition(candidate.clone()).await.unwrap();

    let first = store
        .claim_next_effect(claim(reviewed.operation_id(), "claim:first", 30, 40, false))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.intent, candidate.effects[0]);
    assert_eq!(first.attempt, 1);
    assert!(store
        .claim_next_effect(claim(reviewed.operation_id(), "claim:busy", 35, 45, false))
        .await
        .unwrap()
        .is_none());

    assert_eq!(
        store
            .claim_next_effect(claim(
                reviewed.operation_id(),
                "claim:expired-implicit",
                41,
                50,
                false,
            ))
            .await
            .unwrap_err()
            .code,
        "use.control_store.reconciliation_required"
    );
    let replay = store
        .claim_next_effect(claim(reviewed.operation_id(), "claim:replay", 41, 50, true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.intent.idempotency_key, first.intent.idempotency_key);
    assert_eq!(replay.attempt, 2);
    let unknown = observation(
        reviewed.operation_id(),
        &replay.intent,
        &replay.claim_token,
        ControlEffectOutcome::Unknown,
        'a',
        45,
    );
    assert!(store.record_effect_observation(unknown).await.unwrap());

    assert_eq!(
        store
            .claim_next_effect(claim(
                reviewed.operation_id(),
                "claim:implicit",
                51,
                60,
                false
            ))
            .await
            .unwrap_err()
            .code,
        "use.control_store.reconciliation_required"
    );
    let reconciled = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:explicit",
            51,
            60,
            true,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reconciled.intent.idempotency_key,
        first.intent.idempotency_key
    );
    assert_eq!(reconciled.attempt, 3);
    let applied = observation(
        reviewed.operation_id(),
        &reconciled.intent,
        &reconciled.claim_token,
        ControlEffectOutcome::Applied,
        'b',
        55,
    );
    assert!(store
        .record_effect_observation(applied.clone())
        .await
        .unwrap());
    assert!(!store.record_effect_observation(applied).await.unwrap());

    for (index, expected) in candidate.effects.iter().enumerate().skip(1) {
        let now = 61 + u64::try_from(index).unwrap() * 10;
        let token = format!("claim:remaining:{index}");
        let claimed = store
            .claim_next_effect(claim(reviewed.operation_id(), &token, now, now + 9, false))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&claimed.intent, expected);
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(u32::try_from(index).unwrap(), 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
    }
    assert!(store
        .claim_next_effect(claim(reviewed.operation_id(), "claim:none", 91, 100, false))
        .await
        .unwrap()
        .is_none());

    let completed = store
        .complete_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('d'),
            100,
        )
        .await
        .unwrap();
    assert_eq!(completed.status, ControlOperationStatus::Completed);
    assert_eq!(
        store
            .inspect()
            .await
            .unwrap()
            .metadata
            .published_capability_generation,
        1
    );
    assert!(
        store
            .current_generation()
            .await
            .unwrap()
            .unwrap()
            .capability_status
            == ControlCapabilityStatus::Published
    );
}

#[tokio::test]
async fn expired_claim_survives_restart_and_requires_explicit_reconciliation() {
    let (temporary, store) = initialized_store().await;
    let reviewed = operation("operation:restart-reconcile:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    store.commit_transition(candidate.clone()).await.unwrap();
    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:before-restart",
            30,
            40,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let state_root = store.state_root.clone();
    drop(store);

    let restarted = ControlStore::new(state_root, control_installation()).unwrap();
    restarted.initialize().await.unwrap();
    assert_eq!(
        restarted
            .claim_next_effect(claim(
                reviewed.operation_id(),
                "claim:restart-implicit",
                41,
                50,
                false,
            ))
            .await
            .unwrap_err()
            .code,
        "use.control_store.reconciliation_required"
    );
    let reconciled = restarted
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:restart-explicit",
            41,
            50,
            true,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reconciled.intent, claimed.intent);
    assert_eq!(reconciled.attempt, 2);
    assert_eq!(
        restarted.effects(reviewed.operation_id()).await.unwrap()[0].claim_token,
        Some("claim:restart-explicit".to_string())
    );
    drop(restarted);
    drop(temporary);
}

#[tokio::test]
async fn required_rejection_is_terminal_and_cannot_publish_capabilities() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:rejected:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:required",
            30,
            40,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    store
        .record_effect_observation(observation(
            reviewed.operation_id(),
            &claimed.intent,
            &claimed.claim_token,
            ControlEffectOutcome::Rejected,
            'e',
            35,
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .operation(reviewed.operation_id())
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::Rejected
    );
    assert_eq!(
        store
            .complete_operation(
                reviewed.operation_id(),
                reviewed.plan_digest(),
                &digest('f'),
                40
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.conflict"
    );
    assert_eq!(
        store
            .inspect()
            .await
            .unwrap()
            .metadata
            .published_capability_generation,
        0
    );

    let compensating = operation_at(
        "operation:compensate:1",
        PluginOperationAction::Upgrade,
        1,
        0,
    );
    store
        .register_operation(compensating.clone())
        .await
        .unwrap();
    let prior = store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&prior).unwrap();
    let committed = store
        .commit_transition(projected_transition(&compensating, &prior, &history))
        .await
        .unwrap();
    assert_eq!(committed.snapshot.generation, 2);
    assert_eq!(committed.capability.generation, 1);
    assert_eq!(
        committed.capability_status,
        ControlCapabilityStatus::Candidate
    );
}

#[tokio::test]
async fn authority_export_is_complete_and_semantically_verified_offline() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:export:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();

    let bytes = store.export().await.unwrap();
    let verified = store.verify_export(bytes.clone()).await.unwrap();
    assert_eq!(verified.export.current_generation, 1);
    assert_eq!(verified.export.published_capability_generation, 0);
    assert_eq!(verified.export.authority.generations.len(), 1);
    assert_eq!(verified.export.authority.operations.len(), 1);
    assert_eq!(verified.export.authority.effects.len(), 3);

    tokio::fs::remove_file(store.database_path()).await.unwrap();
    assert_eq!(
        store
            .verify_export(bytes.clone())
            .await
            .unwrap()
            .descriptor_digest,
        verified.descriptor_digest
    );

    let mut tampered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    tampered["authority"]["generations"][0]["capabilityStatus"] = serde_json::json!("published");
    let error = store
        .verify_export(canonical_json(&tampered))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.export_invalid");

    let mut tampered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    tampered["authority"]["generations"][0]["packageLifecycles"][0]["lifecycleGeneration"] =
        serde_json::json!(42);
    let error = store
        .verify_export(canonical_json(&tampered))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.export_invalid");

    let mut tampered: super::export::ControlStoreExport = serde_json::from_slice(&bytes).unwrap();
    tampered.authority.generations[0].snapshot.packages[0].state_generation = 42;
    tampered.authority.generations[0].snapshot_digest = tampered.authority.generations[0]
        .snapshot
        .descriptor_digest()
        .unwrap();
    let error = store
        .verify_export(canonical_json(&tampered))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.export_invalid");

    let mut tampered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    tampered["authority"]["effects"][0]["payloadDigest"] = serde_json::json!(digest('d'));
    let error = store
        .verify_export(canonical_json(&tampered))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.export_invalid");
}

#[tokio::test]
async fn clean_restore_stages_and_round_trips_the_exact_authority() {
    let (_source_temporary, source) = initialized_store().await;
    let reviewed = operation("operation:restore:1");
    source.register_operation(reviewed.clone()).await.unwrap();
    source
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    apply_all_effects(&source, &reviewed, 30).await;
    let export = source.export().await.unwrap();

    let target_temporary = tempfile::tempdir().unwrap();
    let target = ControlStore::new(
        target_temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let restored = target.restore(export.clone()).await.unwrap();
    assert_eq!(restored.current_generation, 1);
    assert_eq!(restored.published_capability_generation, 1);
    assert_eq!(target.export().await.unwrap(), export);
    assert!(!target
        .state_root
        .join(super::filesystem::CONTROL_STORE_RESTORE_FILE)
        .exists());

    let error = target.restore(export.clone()).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.restore_target_not_empty");
    assert_eq!(target.export().await.unwrap(), export);

    let wrong_temporary = tempfile::tempdir().unwrap();
    let wrong = ControlStore::new(
        wrong_temporary.path().join("state"),
        InstallationId::new(InstallationKind::User, "shared/current").unwrap(),
    )
    .unwrap();
    let error = wrong.restore(export).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.identity_mismatch");
    assert!(!wrong.database_path().exists());
}

#[tokio::test]
async fn inspection_rejects_relational_projection_drift() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:projection-drift:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    {
        let connection = rusqlite::Connection::open(store.database_path()).unwrap();
        connection
            .execute("UPDATE selected_package SET enabled = 0", [])
            .unwrap();
    }

    let error = store.inspect().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.corrupt");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn clean_restore_rejects_linked_staging_without_touching_external_state() {
    let (_source_temporary, source) = initialized_store().await;
    let export = source.export().await.unwrap();
    let target_temporary = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    tokio::fs::write(external.path().join("sentinel"), b"outside")
        .await
        .unwrap();
    let target = ControlStore::new(
        target_temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    tokio::fs::create_dir_all(&target.state_root).await.unwrap();
    crate::test_filesystem::create_directory_link(
        external.path(),
        &target
            .state_root
            .join(super::filesystem::CONTROL_STORE_RESTORE_FILE),
    );

    let error = target.restore(export).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
    assert_eq!(
        tokio::fs::read(external.path().join("sentinel"))
            .await
            .unwrap(),
        b"outside"
    );
    assert!(!target.database_path().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_transition_cas_commits_exactly_one_generation() {
    let (_temporary, store) = initialized_store().await;
    let store = Arc::new(store);
    let left = operation("operation:race:left");
    let right = operation("operation:race:right");
    store.register_operation(left.clone()).await.unwrap();
    store.register_operation(right.clone()).await.unwrap();

    let left_store = store.clone();
    let right_store = store.clone();
    let left_task = tokio::spawn(async move {
        left_store
            .commit_transition(transition(control_installation(), &left))
            .await
    });
    let right_task = tokio::spawn(async move {
        right_store
            .commit_transition(transition(control_installation(), &right))
            .await
    });
    let results = [left_task.await.unwrap(), right_task.await.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .next()
            .unwrap()
            .code,
        "use.control_store.generation_changed"
    );
    assert_eq!(
        store.inspect().await.unwrap().metadata.current_generation,
        1
    );
}

#[tokio::test]
async fn reviewed_operation_can_be_cancelled_only_before_transition_commit() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:cancelled:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let cancelled = store
        .cancel_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('c'),
            15,
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status, ControlOperationStatus::Cancelled);
    assert_eq!(
        store
            .cancel_operation(
                reviewed.operation_id(),
                reviewed.plan_digest(),
                &digest('c'),
                15,
            )
            .await
            .unwrap(),
        cancelled
    );
    assert_eq!(
        store
            .commit_transition(transition(control_installation(), &reviewed))
            .await
            .unwrap_err()
            .code,
        "use.control_store.conflict"
    );
}
