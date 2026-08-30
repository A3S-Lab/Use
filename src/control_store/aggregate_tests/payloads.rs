use rusqlite::{params, Connection};

use super::*;

#[tokio::test]
async fn effect_payload_is_committed_canonically_and_reopens_after_restart() {
    let (temporary, store) = initialized_store().await;
    let reviewed = operation("operation:payload-restart:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    let expected_intent = candidate.effects[0].clone();
    let expected_payload = canonical_json(&expected_intent);
    let expected_digest = format!("sha256:{:x}", Sha256::digest(&expected_payload));
    store.commit_transition(candidate).await.unwrap();

    let connection = Connection::open(store.database_path()).unwrap();
    let (payload, payload_digest): (Vec<u8>, String) = connection
        .query_row(
            "SELECT payload_json, payload_digest FROM effect_outbox WHERE sequence = 0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(payload, expected_payload);
    assert_eq!(payload_digest, expected_digest);
    let graph_subject: (String, Option<String>, Option<i64>, Option<String>) = connection
        .query_row(
            "SELECT subject_kind, package_id, package_lifecycle_generation, surface_id
             FROM lifecycle_checkpoint WHERE sequence = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        graph_subject,
        ("installation".to_string(), None, None, None)
    );
    drop(connection);
    drop(store);

    let reopened =
        ControlStore::new(temporary.path().join("state"), control_installation()).unwrap();
    reopened.initialize().await.unwrap();
    let claimed = reopened
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:payload-restart",
            30,
            40,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.intent, expected_intent);
}

#[tokio::test]
async fn self_consistent_payload_tampering_cannot_override_relational_authority() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:payload-tamper:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    let mut tampered = candidate.effects[0].clone();
    store.commit_transition(candidate).await.unwrap();

    tampered.provider_id = "provider:tampered".to_string();
    let payload = canonical_json(&tampered);
    let payload_digest = format!("sha256:{:x}", Sha256::digest(&payload));
    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE effect_outbox SET payload_json = ?1, payload_digest = ?2 WHERE sequence = 0",
            params![payload, payload_digest],
        )
        .unwrap();
    drop(connection);

    let error = store.effects(reviewed.operation_id()).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.corrupt");
}

#[tokio::test]
async fn self_consistent_artifact_tampering_cannot_retarget_execution() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:artifact-tamper:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    let mut tampered = candidate.effects[0].clone();
    store.commit_transition(candidate).await.unwrap();

    let ControlEffectSubject::Package { package_digest, .. } = &mut tampered.subject else {
        panic!("package commit must retain a package subject");
    };
    *package_digest = digest('e');
    rewrite_payload(&store, 0, &tampered);

    let error = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:artifact-tamper",
            30,
            40,
            false,
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.corrupt");
}

#[tokio::test]
async fn self_consistent_capability_tampering_cannot_retarget_graph_cutover() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:capability-tamper:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    let mut tampered = candidate.effects[1].clone();
    store.commit_transition(candidate).await.unwrap();

    let ControlEffectSubject::Installation {
        descriptor_digest, ..
    } = &mut tampered.subject
    else {
        panic!("capability publication must retain an installation subject");
    };
    *descriptor_digest = digest('e');
    rewrite_payload(&store, 1, &tampered);

    let error = store.effects(reviewed.operation_id()).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.corrupt");
}

#[tokio::test]
async fn missing_outbox_entries_cannot_make_an_operation_appear_complete() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:missing-outbox:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();

    let connection = Connection::open(store.database_path()).unwrap();
    connection.execute("DELETE FROM effect_outbox", []).unwrap();
    drop(connection);

    let error = store
        .complete_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('f'),
            50,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.corrupt");
}

#[tokio::test]
async fn surface_payload_binds_the_exact_selected_artifact_and_surface() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:surface-payload:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let mut candidate = transition(control_installation(), &reviewed);
    let surface = candidate.snapshot.packages[0].selected_surfaces[0].clone();
    let ControlEffectSubject::Package {
        package_id,
        lifecycle_generation,
        package_digest,
        manifest_digest,
        action,
    } = candidate.effects[0].subject.clone()
    else {
        panic!("package commit must start with a package subject");
    };
    candidate.effects[0].kind = ControlEffectKind::SurfacePrepare;
    candidate.effects[0].subject = ControlEffectSubject::Surface {
        package_id,
        lifecycle_generation,
        package_digest,
        manifest_digest,
        action,
        surface: surface.clone(),
    };

    let mut wrong = candidate.clone();
    if let ControlEffectSubject::Surface { surface, .. } = &mut wrong.effects[0].subject {
        surface.id = "missing-surface".to_string();
    }
    assert_eq!(
        store.commit_transition(wrong).await.unwrap_err().code,
        "use.control_store.input_invalid"
    );

    store.commit_transition(candidate.clone()).await.unwrap();
    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:surface-payload",
            30,
            40,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.intent, candidate.effects[0]);
    assert_eq!(claimed.intent.subject.surface(), Some(&surface));
}

fn rewrite_payload(store: &ControlStore, sequence: u32, intent: &ControlEffectIntent) {
    let payload = canonical_json(intent);
    let payload_digest = format!("sha256:{:x}", Sha256::digest(&payload));
    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE effect_outbox SET payload_json = ?1, payload_digest = ?2 WHERE sequence = ?3",
            params![payload, payload_digest, sequence],
        )
        .unwrap();
}
