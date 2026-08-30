use rusqlite::{params, Connection};

use super::grant_fixtures::*;
use super::*;

#[tokio::test]
async fn reviewed_grants_are_derived_across_both_scope_lifecycles() {
    for installation in [
        control_installation(),
        InstallationId::new(InstallationKind::User, control_installation().id).unwrap(),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let store =
            ControlStore::new(temporary.path().join("state"), installation.clone()).unwrap();
        store.initialize().await.unwrap();
        let mut history = ControlProjectionHistory::default();
        let mut prior = None;

        for (index, action) in [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
            PluginOperationAction::Disable,
            PluginOperationAction::Enable,
            PluginOperationAction::Uninstall,
        ]
        .into_iter()
        .enumerate()
        {
            let reviewed = reviewed_grant_operation_for(
                &installation,
                &format!("operation:grant-lifecycle:{index}"),
                action,
                prior.as_ref(),
                None,
                None,
            );
            store.register_operation(reviewed.clone()).await.unwrap();
            let transition = prior.as_ref().map_or_else(
                || transition(installation.clone(), &reviewed),
                |generation| projected_transition(&reviewed, generation, &history),
            );
            let transition = bind_action_effects(transition, &reviewed);
            let committed = store.commit_transition(transition).await.unwrap();

            assert_eq!(committed.snapshot.installation, installation);
            match action {
                PluginOperationAction::Install => {
                    assert_eq!(committed.grants.len(), 1);
                    assert_eq!(committed.grants[0].receipt_revision, 2);
                }
                PluginOperationAction::Upgrade => {
                    assert_eq!(committed.grants.len(), 1);
                    assert_eq!(committed.grants[0].receipt_revision, 3);
                    assert_ne!(
                        committed.grants[0].grant.package_digest,
                        prior.as_ref().unwrap().grants[0].grant.package_digest
                    );
                }
                PluginOperationAction::Disable | PluginOperationAction::Uninstall => {
                    assert!(committed.grants.is_empty());
                }
                PluginOperationAction::Enable => {
                    assert_eq!(committed.grants.len(), 1);
                    assert_eq!(committed.grants[0].receipt_revision, 5);
                }
            }

            apply_all_effects(&store, &reviewed, reviewed.reviewed_at_ms + 20).await;
            history.observe(&committed).unwrap();
            prior = Some(committed);
        }
    }
}

#[test]
fn installing_another_root_retains_unrelated_active_grants_exactly() {
    let first = reviewed_grant_operation(
        "operation:grant-retention:first",
        PluginOperationAction::Install,
        None,
        None,
    );
    let first_projection = first
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            first.reviewed_at_ms + 10,
        )
        .unwrap();
    let first_generation = generation_from_projection(&first, &first_projection);
    let mut history = ControlProjectionHistory::default();
    history.observe(&first_generation).unwrap();

    let second = reviewed_grant_operation_for(
        &control_installation(),
        "operation:grant-retention:second",
        PluginOperationAction::Install,
        Some(&first_generation),
        None,
        Some(permissioned_package_lock_named("acme/analytics", '8')),
    );
    let second_projection = second
        .project_generation(
            Some(&first_generation),
            &history,
            second.reviewed_at_ms + 10,
        )
        .unwrap();

    assert_eq!(second_projection.grants.len(), 2);
    assert_eq!(
        second_projection
            .grants
            .iter()
            .find(|grant| grant.package_id() == first.root_package_id())
            .unwrap(),
        &first_generation.grants[0]
    );
    assert!(second_projection
        .grants
        .iter()
        .any(|grant| grant.package_id() == "acme/analytics"));
}

#[test]
fn permission_free_operation_retains_unrelated_active_grants_without_grant_evidence() {
    let first = reviewed_grant_operation(
        "operation:grant-retention:permissioned",
        PluginOperationAction::Install,
        None,
        None,
    );
    let first_projection = first
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            first.reviewed_at_ms + 10,
        )
        .unwrap();
    let first_generation = generation_from_projection(&first, &first_projection);
    let mut history = ControlProjectionHistory::default();
    history.observe(&first_generation).unwrap();

    let second = operation_at(
        "operation:grant-retention:permission-free",
        PluginOperationAction::Install,
        first_generation.snapshot.generation,
        first_generation.capability.generation,
    );
    assert!(second.authorization.grant_transition.is_none());
    let projected = second
        .project_generation(
            Some(&first_generation),
            &history,
            second.reviewed_at_ms + 10,
        )
        .unwrap();

    assert_eq!(projected.grants, first_generation.grants);
}

#[tokio::test]
async fn caller_cannot_choose_grant_bytes_digest_or_receipt_revision() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = reviewed_grant_operation(
        "operation:grant-projection:caller",
        PluginOperationAction::Install,
        None,
        None,
    );
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);

    let mut revision_tampered = candidate.clone();
    revision_tampered.grants[0].receipt_revision += 1;
    assert_eq!(
        store
            .commit_transition(revision_tampered)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let mut bytes_tampered = candidate.clone();
    bytes_tampered.grants[0].grant.granted_at_ms += 1;
    bytes_tampered.grants[0].grant_digest =
        bytes_tampered.grants[0].grant.descriptor_digest().unwrap();
    assert_eq!(
        store
            .commit_transition(bytes_tampered)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let committed = store.commit_transition(candidate.clone()).await.unwrap();
    assert_eq!(committed.grants, candidate.grants);
}

#[test]
fn authorization_requires_exact_grant_evidence_presence() {
    let permissioned = reviewed_grant_operation(
        "operation:grant-evidence:missing",
        PluginOperationAction::Install,
        None,
        None,
    );
    assert_eq!(
        ReviewedControlOperation::new(
            permissioned.envelope.clone(),
            permissioned.authorization.operation_confirmation.clone(),
            None,
            permissioned.authorization.grant_confirmations.clone(),
            0,
            0,
            permissioned.reviewed_at_ms,
        )
        .unwrap_err()
        .code,
        "use.control_store.input_invalid"
    );

    let permission_free = operation("operation:grant-evidence:injected");
    assert_eq!(
        ReviewedControlOperation::new(
            permission_free.envelope.clone(),
            permission_free.authorization.operation_confirmation.clone(),
            permissioned.authorization.grant_transition.clone(),
            Vec::new(),
            0,
            0,
            permission_free.reviewed_at_ms,
        )
        .unwrap_err()
        .code,
        "use.control_store.input_invalid"
    );
}

#[test]
fn reviewed_snapshot_must_equal_the_exact_prior_control_generation() {
    let install = reviewed_grant_operation(
        "operation:grant-snapshot:install",
        PluginOperationAction::Install,
        None,
        None,
    );
    let projected = install
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            install.reviewed_at_ms + 10,
        )
        .unwrap();
    let installed = generation_from_projection(&install, &projected);
    let mut history = ControlProjectionHistory::default();
    history.observe(&installed).unwrap();

    let mut stale = installed.clone();
    stale.grants[0].receipt_revision = 1;
    let stale_snapshot = grant_snapshot(&control_installation(), Some(&stale), 2);
    let disable = reviewed_grant_operation(
        "operation:grant-snapshot:disable",
        PluginOperationAction::Disable,
        Some(&installed),
        Some(stale_snapshot),
    );
    assert_eq!(
        disable
            .project_generation(Some(&installed), &history, disable.reviewed_at_ms + 10,)
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );
}

#[tokio::test]
async fn database_and_offline_export_reject_self_consistent_grant_revision_tampering() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = reviewed_grant_operation(
        "operation:grant-tamper:install",
        PluginOperationAction::Install,
        None,
        None,
    );
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let export = store.export().await.unwrap();

    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE control_grant SET receipt_revision = ?1 WHERE generation = 1",
            params![9_i64],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        store.inspect().await.unwrap_err().code,
        "use.control_store.corrupt"
    );

    let mut tampered: serde_json::Value = serde_json::from_slice(&export).unwrap();
    tampered["authority"]["generations"][0]["grants"][0]["receiptRevision"] = serde_json::json!(9);
    assert_eq!(
        store
            .verify_export(canonical_json(&tampered))
            .await
            .unwrap_err()
            .code,
        "use.control_store.export_invalid"
    );
}
