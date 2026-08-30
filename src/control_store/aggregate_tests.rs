use std::sync::Arc;

use a3s_use_core::{
    InstallationId, InstallationKind, InstallationPackageSelection, InstallationRootSelection,
    InstallationSnapshot, PluginCatalogRecord, PluginOperationAction, PluginPackageId,
    PluginPackageLock, PluginPackageLockHost, PluginPackageResolver, PluginWorkspaceGrant,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{
    ControlCapabilitySelection, ControlCapabilityStatus, ControlEffectClaim, ControlEffectIntent,
    ControlEffectKind, ControlEffectObservation, ControlEffectOutcome, ControlEffectStatus,
    ControlGrantSelection, ControlOperationStatus, ControlProviderBinding, ControlTransition,
    ReviewedControlOperation,
};
use super::*;

const CATALOG: &[u8] = include_bytes!("../../crates/core/fixtures/plugins/catalog-record-v3.json");
const GRANT: &[u8] = include_bytes!("../../crates/core/fixtures/plugins/workspace-grant-v1.json");

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

fn effect_key(operation_id: &str, sequence: u32) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(format!("{operation_id}\n{sequence}").as_bytes())
    )
}

fn operation(id: &str) -> ReviewedControlOperation {
    operation_at(id, PluginOperationAction::Install, 0, 0)
}

fn operation_at(
    id: &str,
    action: PluginOperationAction,
    expected_generation: u64,
    expected_capability_generation: u64,
) -> ReviewedControlOperation {
    ReviewedControlOperation {
        operation_id: id.to_string(),
        plan_digest: digest('1'),
        authorization_digest: digest('2'),
        action,
        root_package_id: PluginPackageId::parse("acme/research").unwrap(),
        expected_generation,
        expected_capability_generation,
        reviewed_at_ms: 10 + expected_generation * 100,
    }
}

fn control_installation() -> InstallationId {
    InstallationId::new(InstallationKind::Workspace, "workspace-01").unwrap()
}

fn package_lock() -> PluginPackageLock {
    let record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: digest('f'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap()
}

fn snapshot(installation: InstallationId, generation: u64) -> InstallationSnapshot {
    let package_lock = package_lock();
    let selections = package_lock
        .packages
        .iter()
        .map(|package| {
            let selected_surfaces = package
                .catalog
                .record
                .resolve_surfaces(&[])
                .unwrap()
                .into_iter()
                .map(|surface| surface.reference())
                .collect();
            InstallationPackageSelection::new(package.clone(), generation, true, selected_surfaces)
                .unwrap()
        })
        .collect();
    InstallationSnapshot::from_root_locks(
        installation,
        generation,
        package_lock.host.clone(),
        vec![(
            InstallationRootSelection::new(package_lock.root_package_id.clone(), 5).unwrap(),
            package_lock.clone(),
        )],
        selections,
    )
    .unwrap()
}

fn transition(
    installation: InstallationId,
    reviewed: &ReviewedControlOperation,
) -> ControlTransition {
    let snapshot = snapshot(installation, reviewed.target_generation().unwrap());
    let package = &snapshot.packages[0];
    let package_id = package.package_id().to_string();
    let surface = package.selected_surfaces[0].clone();
    let mut grant = PluginWorkspaceGrant::from_json(GRANT).unwrap();
    grant.package_digest = package
        .package
        .catalog
        .record
        .package
        .sha256
        .clone()
        .unwrap();
    let grant_digest = grant.descriptor_digest().unwrap();
    ControlTransition {
        operation_id: reviewed.operation_id.clone(),
        plan_digest: reviewed.plan_digest.clone(),
        snapshot,
        grants: vec![ControlGrantSelection {
            grant,
            grant_digest,
        }],
        bindings: vec![ControlProviderBinding {
            package_id: package_id.clone(),
            surface,
            provider_id: "provider:test".to_string(),
            binding_digest: digest('4'),
        }],
        capability: ControlCapabilitySelection {
            generation: reviewed.target_capability_generation().unwrap(),
            descriptor_digest: digest('5'),
        },
        effects: vec![
            ControlEffectIntent {
                sequence: 0,
                idempotency_key: effect_key(&reviewed.operation_id, 0),
                package_generation: reviewed.target_generation().unwrap(),
                package_id: package_id.clone(),
                provider_id: "provider:test".to_string(),
                kind: ControlEffectKind::PackageCommit,
                payload_digest: digest('7'),
                required: true,
            },
            ControlEffectIntent {
                sequence: 1,
                idempotency_key: effect_key(&reviewed.operation_id, 1),
                package_generation: reviewed.target_generation().unwrap(),
                package_id,
                provider_id: "provider:test".to_string(),
                kind: ControlEffectKind::CapabilityPublish,
                payload_digest: digest('9'),
                required: false,
            },
        ],
        committed_at_ms: reviewed.reviewed_at_ms + 10,
    }
}

async fn initialized_store() -> (tempfile::TempDir, ControlStore) {
    let temporary = tempfile::tempdir().unwrap();
    let store = ControlStore::new(temporary.path().join("state"), control_installation()).unwrap();
    store.initialize().await.unwrap();
    (temporary, store)
}

fn claim(
    operation_id: &str,
    token: &str,
    now_ms: u64,
    lease_until_ms: u64,
    reconcile_unknown: bool,
) -> ControlEffectClaim {
    ControlEffectClaim {
        operation_id: operation_id.to_string(),
        worker_id: "worker:test".to_string(),
        claim_token: token.to_string(),
        now_ms,
        lease_until_ms,
        reconcile_unknown,
    }
}

fn observation(
    operation_id: &str,
    idempotency_key: &str,
    claim_token: &str,
    outcome: ControlEffectOutcome,
    seed: char,
    observed_at_ms: u64,
) -> ControlEffectObservation {
    ControlEffectObservation {
        operation_id: operation_id.to_string(),
        idempotency_key: idempotency_key.to_string(),
        claim_token: claim_token.to_string(),
        outcome,
        evidence_digest: digest(seed),
        error_code: (!matches!(outcome, ControlEffectOutcome::Applied))
            .then(|| "provider.rejected".to_string()),
        observed_at_ms,
    }
}

fn canonical_json<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).unwrap();
    bytes
}

async fn apply_all_effects(store: &ControlStore, reviewed: &ReviewedControlOperation, start: u64) {
    let mut now = start;
    let mut sequence = 0_u32;
    loop {
        let token = format!("claim:{}:{sequence}", reviewed.operation_id);
        let Some(claimed) = store
            .claim_next_effect(claim(&reviewed.operation_id, &token, now, now + 10, false))
            .await
            .unwrap()
        else {
            break;
        };
        store
            .record_effect_observation(observation(
                &reviewed.operation_id,
                &claimed.intent.idempotency_key,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit((sequence % 10) + 1, 10).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
        now += 20;
        sequence += 1;
    }
    store
        .complete_operation(
            &reviewed.operation_id,
            &reviewed.plan_digest,
            &digest('f'),
            now,
        )
        .await
        .unwrap();
}

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

    let mut conflicting = reviewed.clone();
    conflicting.plan_digest = digest('a');
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
    assert_eq!(committed.bindings, candidate.bindings);
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
            .operation(&reviewed.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::EffectsPending
    );
    let effects = store.effects(&reviewed.operation_id).await.unwrap();
    assert_eq!(effects.len(), 2);
    assert!(effects
        .iter()
        .all(|effect| effect.status == ControlEffectStatus::Pending));

    let stale = ReviewedControlOperation {
        operation_id: "operation:stale:1".to_string(),
        ..reviewed
    };
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
                .operation(&reviewed.operation_id)
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
async fn root_lifecycle_actions_form_one_consecutive_capability_history() {
    let (_temporary, store) = initialized_store().await;

    let install = operation_at(
        "operation:lifecycle:install",
        PluginOperationAction::Install,
        0,
        0,
    );
    store.register_operation(install.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &install))
        .await
        .unwrap();
    apply_all_effects(&store, &install, 30).await;

    let disable = operation_at(
        "operation:lifecycle:disable",
        PluginOperationAction::Disable,
        1,
        1,
    );
    store.register_operation(disable.clone()).await.unwrap();
    let mut disabled = transition(control_installation(), &disable);
    disabled.snapshot.packages[0].enabled = false;
    disabled.grants.clear();
    disabled.bindings.clear();
    disabled.effects[0].kind = ControlEffectKind::CapabilityHide;
    disabled.effects[1].kind = ControlEffectKind::GrantRevoke;
    store.commit_transition(disabled).await.unwrap();
    apply_all_effects(&store, &disable, 230).await;

    let enable = operation_at(
        "operation:lifecycle:enable",
        PluginOperationAction::Enable,
        2,
        2,
    );
    store.register_operation(enable.clone()).await.unwrap();
    let mut enabled = transition(control_installation(), &enable);
    enabled.effects[0].kind = ControlEffectKind::GrantApply;
    enabled.effects[1].kind = ControlEffectKind::CapabilityPublish;
    store.commit_transition(enabled).await.unwrap();
    apply_all_effects(&store, &enable, 430).await;

    let uninstall = operation_at(
        "operation:lifecycle:uninstall",
        PluginOperationAction::Uninstall,
        3,
        3,
    );
    store.register_operation(uninstall.clone()).await.unwrap();
    let mut removed = transition(control_installation(), &uninstall);
    removed.snapshot = InstallationSnapshot::from_root_locks(
        control_installation(),
        4,
        removed.snapshot.host.clone(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    removed.grants.clear();
    removed.bindings.clear();
    for effect in &mut removed.effects {
        effect.package_generation = 3;
    }
    removed.effects[0].kind = ControlEffectKind::CapabilityHide;
    removed.effects[1].kind = ControlEffectKind::PackageRemove;
    store.commit_transition(removed).await.unwrap();
    apply_all_effects(&store, &uninstall, 630).await;

    let inspection = store.inspect().await.unwrap();
    assert_eq!(inspection.metadata.current_generation, 4);
    assert_eq!(inspection.metadata.published_capability_generation, 4);
    let export = store
        .verify_export(store.export().await.unwrap())
        .await
        .unwrap();
    assert_eq!(export.export.authority.generations.len(), 4);
    assert!(export.export.authority.generations[..3]
        .iter()
        .all(|generation| generation.capability_status == ControlCapabilityStatus::Retired));
    assert_eq!(
        export.export.authority.generations[3].capability_status,
        ControlCapabilityStatus::Published
    );
    assert!(export.export.authority.generations[3]
        .snapshot
        .roots
        .is_empty());
}

#[tokio::test]
async fn failed_relational_effect_reference_rolls_back_the_whole_transition() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:invalid-reference:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let mut candidate = transition(control_installation(), &reviewed);
    candidate.effects[0].package_generation = 99;

    let error = store.commit_transition(candidate).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.conflict");
    assert_eq!(
        store.inspect().await.unwrap().metadata.current_generation,
        0
    );
    assert!(store.current_generation().await.unwrap().is_none());
    assert_eq!(
        store
            .operation(&reviewed.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::Reviewed
    );
    assert!(store
        .effects(&reviewed.operation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn outbox_lease_unknown_reconciliation_and_terminal_replay_are_exact() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:effects:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    store.commit_transition(candidate.clone()).await.unwrap();

    let first = store
        .claim_next_effect(claim(&reviewed.operation_id, "claim:first", 30, 40, false))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.intent, candidate.effects[0]);
    assert_eq!(first.attempt, 1);
    assert!(store
        .claim_next_effect(claim(&reviewed.operation_id, "claim:busy", 35, 45, false))
        .await
        .unwrap()
        .is_none());

    assert_eq!(
        store
            .claim_next_effect(claim(
                &reviewed.operation_id,
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
        .claim_next_effect(claim(&reviewed.operation_id, "claim:replay", 41, 50, true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.intent.idempotency_key, first.intent.idempotency_key);
    assert_eq!(replay.attempt, 2);
    let unknown = observation(
        &reviewed.operation_id,
        &replay.intent.idempotency_key,
        &replay.claim_token,
        ControlEffectOutcome::Unknown,
        'a',
        45,
    );
    assert!(store.record_effect_observation(unknown).await.unwrap());

    assert_eq!(
        store
            .claim_next_effect(claim(
                &reviewed.operation_id,
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
            &reviewed.operation_id,
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
        &reviewed.operation_id,
        &reconciled.intent.idempotency_key,
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

    let optional = store
        .claim_next_effect(claim(
            &reviewed.operation_id,
            "claim:optional",
            61,
            70,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(optional.intent, candidate.effects[1]);
    store
        .record_effect_observation(observation(
            &reviewed.operation_id,
            &optional.intent.idempotency_key,
            &optional.claim_token,
            ControlEffectOutcome::Rejected,
            'c',
            65,
        ))
        .await
        .unwrap();
    assert!(store
        .claim_next_effect(claim(&reviewed.operation_id, "claim:none", 71, 80, false))
        .await
        .unwrap()
        .is_none());

    let completed = store
        .complete_operation(
            &reviewed.operation_id,
            &reviewed.plan_digest,
            &digest('d'),
            80,
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
            &reviewed.operation_id,
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
                &reviewed.operation_id,
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
            &reviewed.operation_id,
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
        restarted.effects(&reviewed.operation_id).await.unwrap()[0].claim_token,
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
            &reviewed.operation_id,
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
            &reviewed.operation_id,
            &claimed.intent.idempotency_key,
            &claimed.claim_token,
            ControlEffectOutcome::Rejected,
            'e',
            35,
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .operation(&reviewed.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::Rejected
    );
    assert_eq!(
        store
            .complete_operation(
                &reviewed.operation_id,
                &reviewed.plan_digest,
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

    let compensating = ReviewedControlOperation {
        operation_id: "operation:compensate:1".to_string(),
        plan_digest: digest('a'),
        authorization_digest: digest('b'),
        action: PluginOperationAction::Upgrade,
        root_package_id: PluginPackageId::parse("acme/research").unwrap(),
        expected_generation: 1,
        expected_capability_generation: 0,
        reviewed_at_ms: 50,
    };
    store
        .register_operation(compensating.clone())
        .await
        .unwrap();
    let committed = store
        .commit_transition(transition(control_installation(), &compensating))
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
    assert_eq!(verified.export.authority.effects.len(), 2);

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
    let right = ReviewedControlOperation {
        operation_id: "operation:race:right".to_string(),
        plan_digest: digest('a'),
        authorization_digest: digest('b'),
        ..left.clone()
    };
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
            &reviewed.operation_id,
            &reviewed.plan_digest,
            &digest('c'),
            15,
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status, ControlOperationStatus::Cancelled);
    assert_eq!(
        store
            .cancel_operation(
                &reviewed.operation_id,
                &reviewed.plan_digest,
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
