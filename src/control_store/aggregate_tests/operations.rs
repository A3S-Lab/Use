use a3s_use_core::{
    InstallationId, InstallationKind, PlanActor, PluginGrantConfirmation,
    PluginOperationConfirmation, PLUGIN_GRANT_CONFIRMATION_SCHEMA,
};
use rusqlite::{params, Connection};

use super::*;

#[test]
fn reviewed_authorization_requires_exact_unique_confirmation_evidence() {
    let reviewed = operation("operation:authorization-validation:1");
    assert_eq!(
        ReviewedControlOperation::new(
            reviewed.envelope.clone(),
            None,
            Vec::new(),
            reviewed.expected_generation,
            reviewed.expected_capability_generation,
            reviewed.reviewed_at_ms,
        )
        .unwrap_err()
        .code,
        "use.control_store.input_invalid"
    );

    let mut wrong_confirmation = reviewed
        .authorization
        .operation_confirmation
        .clone()
        .unwrap();
    wrong_confirmation.plan_digest = digest('e');
    assert_eq!(
        ReviewedControlOperation::new(
            reviewed.envelope.clone(),
            Some(wrong_confirmation),
            Vec::new(),
            reviewed.expected_generation,
            reviewed.expected_capability_generation,
            reviewed.reviewed_at_ms,
        )
        .unwrap_err()
        .code,
        "use.control_store.input_invalid"
    );

    let operation_confirmation = reviewed
        .authorization
        .operation_confirmation
        .clone()
        .unwrap();
    let grant_confirmation = PluginGrantConfirmation {
        schema: PLUGIN_GRANT_CONFIRMATION_SCHEMA.to_string(),
        operation_id: reviewed.operation_id().to_string(),
        plan_digest: reviewed.plan_digest().to_string(),
        proposal_digest: digest('8'),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: operation_confirmation.confirmed_at_ms,
    };
    assert_eq!(
        ReviewedControlOperation::new(
            reviewed.envelope.clone(),
            Some(operation_confirmation),
            vec![grant_confirmation.clone(), grant_confirmation],
            reviewed.expected_generation,
            reviewed.expected_capability_generation,
            reviewed.reviewed_at_ms,
        )
        .unwrap_err()
        .code,
        "use.control_store.input_invalid"
    );
}

#[tokio::test]
async fn reviewed_plan_and_authorization_are_canonical_and_survive_restart() {
    let (temporary, store) = initialized_store().await;
    let reviewed = operation("operation:reviewed-evidence:1");
    let expected_plan = reviewed.canonical_plan_bytes().unwrap();
    let expected_authorization = reviewed.authorization.canonical_bytes().unwrap();
    let expected_authorization_digest = reviewed.authorization_digest().unwrap();
    let registered = store.register_operation(reviewed.clone()).await.unwrap();

    let connection = Connection::open(store.database_path()).unwrap();
    let persisted: (Vec<u8>, String, Vec<u8>, String, String, String) = connection
        .query_row(
            "SELECT plan_json, plan_digest, authorization_json, authorization_digest,
                    action, root_package_id
             FROM control_operation WHERE operation_id = ?1",
            [reviewed.operation_id()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(persisted.0, expected_plan);
    assert_eq!(persisted.1, reviewed.plan_digest());
    assert_eq!(persisted.2, expected_authorization);
    assert_eq!(persisted.3, expected_authorization_digest);
    assert_eq!(persisted.4, "install");
    assert_eq!(persisted.5, reviewed.root_package_id());
    drop(connection);
    drop(store);

    let restarted =
        ControlStore::new(temporary.path().join("state"), control_installation()).unwrap();
    restarted.initialize().await.unwrap();
    assert_eq!(
        restarted.operation(reviewed.operation_id()).await.unwrap(),
        Some(registered)
    );
}

#[tokio::test]
async fn reviewed_evidence_scope_and_cursor_are_rejected_before_registration() {
    let (_temporary, store) = initialized_store().await;
    let mut wrong_cursor = operation("operation:wrong-cursor:1");
    wrong_cursor.expected_generation = 1;
    assert_eq!(
        store
            .register_operation(wrong_cursor)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let reviewed = operation("operation:wrong-scope:1");
    let mut plan = reviewed.envelope.plan.clone();
    plan.scope = InstallationId::new(InstallationKind::User, "shared/current").unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(
        plan,
        reviewed.envelope.package_lock.clone().unwrap(),
    )
    .unwrap();
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: reviewed.reviewed_at_ms - 1,
    };
    let wrong_scope = ReviewedControlOperation::new(
        envelope,
        Some(confirmation),
        Vec::new(),
        reviewed.expected_generation,
        reviewed.expected_capability_generation,
        reviewed.reviewed_at_ms,
    )
    .unwrap();
    assert_eq!(
        store
            .register_operation(wrong_scope)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let connection = Connection::open(store.database_path()).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM control_operation", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn canonical_evidence_cannot_be_rebound_to_another_installation() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:scope-tamper:1");
    store.register_operation(reviewed.clone()).await.unwrap();

    let mut plan = reviewed.envelope.plan.clone();
    plan.scope = InstallationId::new(InstallationKind::User, "shared/current").unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(
        plan,
        reviewed.envelope.package_lock.clone().unwrap(),
    )
    .unwrap();
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: reviewed.reviewed_at_ms - 1,
    };
    let rebound = ReviewedControlOperation::new(
        envelope,
        Some(confirmation),
        Vec::new(),
        reviewed.expected_generation,
        reviewed.expected_capability_generation,
        reviewed.reviewed_at_ms,
    )
    .unwrap();
    let plan_json = rebound.canonical_plan_bytes().unwrap();
    let authorization_json = rebound.authorization.canonical_bytes().unwrap();
    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE control_operation
             SET plan_json = ?2, plan_digest = ?3,
                 authorization_json = ?4, authorization_digest = ?5
             WHERE operation_id = ?1",
            params![
                reviewed.operation_id(),
                plan_json,
                rebound.plan_digest(),
                authorization_json,
                rebound.authorization_digest().unwrap(),
            ],
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        store
            .operation(reviewed.operation_id())
            .await
            .unwrap_err()
            .code,
        "use.control_store.corrupt"
    );
}

#[tokio::test]
async fn authorization_tampering_is_rejected_in_database_and_offline_export() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:authorization-tamper:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let export = store.export().await.unwrap();

    let mut authorization: serde_json::Value =
        serde_json::from_slice(&reviewed.authorization.canonical_bytes().unwrap()).unwrap();
    authorization["planDigest"] = serde_json::json!(digest('e'));
    let authorization_json = canonical_json(&authorization);
    let authorization_digest = format!("sha256:{:x}", Sha256::digest(&authorization_json));
    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE control_operation
             SET authorization_json = ?2, authorization_digest = ?3
             WHERE operation_id = ?1",
            params![
                reviewed.operation_id(),
                authorization_json,
                authorization_digest,
            ],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        store
            .operation(reviewed.operation_id())
            .await
            .unwrap_err()
            .code,
        "use.control_store.corrupt"
    );

    let mut tampered: serde_json::Value = serde_json::from_slice(&export).unwrap();
    tampered["authority"]["operations"][0]["reviewed"]["authorization"]["planDigest"] =
        serde_json::json!(digest('e'));
    assert_eq!(
        store
            .verify_export(canonical_json(&tampered))
            .await
            .unwrap_err()
            .code,
        "use.control_store.export_invalid"
    );
}
