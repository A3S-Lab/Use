use std::collections::BTreeSet;

use rusqlite::{params, Connection};

use super::grant_fixtures::*;
use super::*;

#[tokio::test]
async fn caller_cannot_choose_provider_or_capability_projection() {
    for installation in [
        control_installation(),
        InstallationId::new(InstallationKind::User, control_installation().id).unwrap(),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let store =
            ControlStore::new(temporary.path().join("state"), installation.clone()).unwrap();
        store.initialize().await.unwrap();
        let reviewed = reviewed_grant_operation_for(
            &installation,
            "operation:provider-authority:install",
            PluginOperationAction::Install,
            None,
            None,
            None,
        );
        store.register_operation(reviewed.clone()).await.unwrap();
        let candidate = transition(installation, &reviewed);

        assert_eq!(candidate.provider_selections.len(), 3);
        assert_eq!(
            candidate
                .provider_selections
                .iter()
                .map(|selection| selection.evidence.clone())
                .collect::<Vec<_>>(),
            reviewed.envelope.plan.providers
        );
        assert_ne!(candidate.capability.descriptor_digest, digest('5'));

        let mut tampered_provider = candidate.clone();
        let mut evidence = tampered_provider.provider_selections[0].evidence.clone();
        evidence.provider_id = "provider:tampered".to_string();
        tampered_provider.provider_selections[0] =
            ControlProviderSelection::from_evidence(evidence).unwrap();
        assert_eq!(
            store
                .commit_transition(tampered_provider)
                .await
                .unwrap_err()
                .code,
            "use.control_store.input_invalid"
        );

        let mut tampered_capability = candidate.clone();
        tampered_capability.capability.descriptor_digest = digest('e');
        if let ControlEffectSubject::Installation {
            descriptor_digest, ..
        } = &mut tampered_capability.effects[1].subject
        {
            *descriptor_digest = digest('e');
        }
        assert_eq!(
            store
                .commit_transition(tampered_capability)
                .await
                .unwrap_err()
                .code,
            "use.control_store.input_invalid"
        );

        store.commit_transition(candidate).await.unwrap();
    }
}

#[test]
fn provider_projection_covers_all_lifecycle_actions_without_claiming_static_hosts() {
    let mut history = ControlProjectionHistory::default();
    let install = reviewed_grant_operation(
        "operation:provider-lifecycle:install",
        PluginOperationAction::Install,
        None,
        None,
    );
    let installed = install
        .project_generation(None, &history, install.reviewed_at_ms + 10)
        .unwrap();
    assert_eq!(installed.provider_selections.len(), 3);
    assert!(installed
        .provider_selections
        .iter()
        .all(|selection| matches!(
            selection.surface().kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )));
    history = installed.history_after.clone();
    let mut prior = generation_from_projection(&install, &installed);
    let mut capability_digests = BTreeSet::from([installed.capability.descriptor_digest]);

    for (index, action) in [
        PluginOperationAction::Upgrade,
        PluginOperationAction::Disable,
        PluginOperationAction::Enable,
        PluginOperationAction::Uninstall,
    ]
    .into_iter()
    .enumerate()
    {
        let reviewed = reviewed_grant_operation(
            &format!("operation:provider-lifecycle:{index}"),
            action,
            Some(&prior),
            None,
        );
        let projected = reviewed
            .project_generation(Some(&prior), &history, reviewed.reviewed_at_ms + 10)
            .unwrap();
        let expected_count = match action {
            PluginOperationAction::Upgrade | PluginOperationAction::Enable => 3,
            PluginOperationAction::Disable | PluginOperationAction::Uninstall => 0,
            PluginOperationAction::Install => unreachable!(),
        };
        assert_eq!(projected.provider_selections.len(), expected_count);
        assert!(capability_digests.insert(projected.capability.descriptor_digest.clone()));
        history = projected.history_after.clone();
        prior = generation_from_projection(&reviewed, &projected);
    }
}

#[test]
fn installing_another_root_retains_unrelated_provider_selections_exactly() {
    let installation = control_installation();
    let first = reviewed_grant_operation_for(
        &installation,
        "operation:provider-multi-root:first",
        PluginOperationAction::Install,
        None,
        None,
        Some(permissioned_package_lock_named("acme/root-a", 'a')),
    );
    let first_projection = first
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            first.reviewed_at_ms + 10,
        )
        .unwrap();
    let first_generation = generation_from_projection(&first, &first_projection);
    let second = reviewed_grant_operation_for(
        &installation,
        "operation:provider-multi-root:second",
        PluginOperationAction::Install,
        Some(&first_generation),
        None,
        Some(permissioned_package_lock_named("acme/root-b", 'b')),
    );
    let second_projection = second
        .project_generation(
            Some(&first_generation),
            &first_projection.history_after,
            second.reviewed_at_ms + 10,
        )
        .unwrap();

    assert_eq!(second_projection.provider_selections.len(), 6);
    assert_eq!(
        second_projection
            .provider_selections
            .iter()
            .filter(|selection| selection.package_id() == "acme/root-a")
            .cloned()
            .collect::<Vec<_>>(),
        first_projection.provider_selections
    );
}

#[tokio::test]
async fn database_and_offline_export_reject_provider_projection_tampering() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = reviewed_grant_operation(
        "operation:provider-tamper:install",
        PluginOperationAction::Install,
        None,
        None,
    );
    store.register_operation(reviewed.clone()).await.unwrap();
    let committed = store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let export = store.export().await.unwrap();
    let mut tampered_evidence = committed.provider_selections[0].evidence.clone();
    tampered_evidence.provider_id = "provider:tampered".to_string();
    let tampered_selection =
        ControlProviderSelection::from_evidence(tampered_evidence.clone()).unwrap();

    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE provider_selection SET provider_id = ?1, selection_digest = ?2
             WHERE generation = 1 AND package_id = ?3 AND surface_id = ?4",
            params![
                tampered_evidence.provider_id,
                tampered_selection.selection_digest,
                tampered_evidence.surface.package_id,
                tampered_evidence.surface.surface.id,
            ],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        store.inspect().await.unwrap_err().code,
        "use.control_store.corrupt"
    );

    let mut tampered: serde_json::Value = serde_json::from_slice(&export).unwrap();
    tampered["authority"]["generations"][0]["providerSelections"][0]["evidence"]["providerId"] =
        serde_json::json!("provider:tampered");
    tampered["authority"]["generations"][0]["providerSelections"][0]["selectionDigest"] =
        serde_json::json!(tampered_selection.selection_digest);
    assert_eq!(
        store
            .verify_export(canonical_json(&tampered))
            .await
            .unwrap_err()
            .code,
        "use.control_store.export_invalid"
    );
}
