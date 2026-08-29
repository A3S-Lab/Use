use a3s_use_core::{PlanScope, PlanScopeKind};
use a3s_use_extension::ExtensionManifest;

use super::*;
use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleCheckpointDiagnosticStatus,
    PluginLifecycleCheckpointOutcome, PluginLifecycleIntent, PluginLifecycleIntentSpec,
    PluginLifecycleOperationStatus, PLUGIN_LIFECYCLE_DIAGNOSTIC_SCHEMA,
};

const OPTIONAL_SKILL_PACKAGE: &str = r#"
extension "acme/guide" {
  schema_version = 3
  version        = "1.0.0"
  route          = "guide"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/guide"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  skill "guide" {
    path          = "skills/guide/SKILL.md"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = true
  }
}
"#;

fn intent(operation_id: &str) -> PluginLifecycleIntent {
    intent_in_scope(operation_id, workspace_scope())
}

fn intent_in_scope(operation_id: &str, scope: PlanScope) -> PluginLifecycleIntent {
    intent_in_scope_with_action(operation_id, scope, PluginLifecycleAction::Install)
}

fn intent_in_scope_with_action(
    operation_id: &str,
    scope: PlanScope,
    action: PluginLifecycleAction,
) -> PluginLifecycleIntent {
    let manifest = ExtensionManifest::parse_acl(OPTIONAL_SKILL_PACKAGE).unwrap();
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: operation_id.to_string(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope,
            package_id: "acme/guide".to_string(),
            package_digest: format!("sha256:{}", "2".repeat(64)),
            manifest_digest: format!("sha256:{}", "3".repeat(64)),
            generation: 9,
            action,
            retained_ui_state_surfaces: Vec::new(),
        },
        &manifest,
    )
    .unwrap()
}

fn evidence(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

#[tokio::test]
async fn resumes_exact_checkpoint_and_replays_terminal_record() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:1");

    let begun = store.begin(&intent).await.unwrap();
    let package = begun.next_checkpoint().unwrap().clone();
    let failed = store
        .record_failure(
            &intent,
            &package.idempotency_key,
            "use.plugin.download_failed",
            evidence('a'),
            10,
        )
        .await
        .unwrap();
    assert_eq!(failed.next_checkpoint(), Some(&package));
    assert!(failed.last_failure.is_some());

    let reopened =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    assert_eq!(reopened.begin(&intent).await.unwrap(), failed);
    let package_applied = reopened
        .record_checkpoint(
            &intent,
            &package.idempotency_key,
            PluginLifecycleCheckpointOutcome::Applied,
            evidence('b'),
            None,
            20,
        )
        .await
        .unwrap();
    assert!(package_applied.last_failure.is_none());

    let skill = package_applied.next_checkpoint().unwrap().clone();
    assert!(!skill.required);
    let degraded = reopened
        .record_checkpoint(
            &intent,
            &skill.idempotency_key,
            PluginLifecycleCheckpointOutcome::OptionalFailed,
            evidence('c'),
            Some("use.plugin.skill_projection_failed".to_string()),
            30,
        )
        .await
        .unwrap();
    let publication = degraded.next_checkpoint().unwrap().clone();
    let published = reopened
        .record_checkpoint(
            &intent,
            &publication.idempotency_key,
            PluginLifecycleCheckpointOutcome::Applied,
            evidence('d'),
            None,
            40,
        )
        .await
        .unwrap();
    assert!(published.next_checkpoint().is_none());

    let completed = reopened.complete(&intent, 50).await.unwrap();
    assert_eq!(completed.status, PluginLifecycleOperationStatus::Completed);
    assert_eq!(reopened.complete(&intent, 60).await.unwrap(), completed);
    assert_eq!(
        reopened
            .load_active(&intent.scope, "acme/guide")
            .await
            .unwrap(),
        Some(completed)
    );
}

#[tokio::test]
async fn rejects_conflicting_operation_until_current_one_completes() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let first = intent("install:acme-guide:1");
    let second = intent("install:acme-guide:2");
    store.begin(&first).await.unwrap();

    let error = store.begin(&second).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_busy");
}

#[tokio::test]
async fn rolling_back_operation_blocks_a_different_intent() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let first = intent("install:acme-guide:rolling-back");
    let second = intent("install:acme-guide:replacement");
    store.begin(&first).await.unwrap();
    let rolling_back = store.start_rollback(&first).await.unwrap();
    assert_eq!(
        rolling_back.status,
        PluginLifecycleOperationStatus::RollingBack
    );

    let error = store.begin(&second).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_busy");
    assert_eq!(
        store
            .load_active(&first.scope, &first.package_id)
            .await
            .unwrap(),
        Some(rolling_back)
    );
}

#[tokio::test]
async fn lifecycle_diagnostics_project_bounded_non_secret_checkpoint_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:diagnostic");
    let begun = store.begin(&intent).await.unwrap();
    let package = begun.next_checkpoint().unwrap();
    store
        .record_failure(
            &intent,
            &package.idempotency_key,
            "use.plugin.download_failed",
            evidence('a'),
            10,
        )
        .await
        .unwrap();

    let diagnostic = store
        .diagnose(&intent.scope, &intent.package_id)
        .await
        .unwrap();
    assert_eq!(diagnostic.schema, PLUGIN_LIFECYCLE_DIAGNOSTIC_SCHEMA);
    assert_eq!(diagnostic.scope, intent.scope);
    assert_eq!(diagnostic.package_id, intent.package_id);
    assert!(diagnostic.previous.is_none());
    let latest = diagnostic.latest.as_ref().unwrap();
    assert_eq!(latest.operation_id, intent.operation_id);
    assert_eq!(latest.status, PluginLifecycleOperationStatus::Applying);
    assert_eq!(latest.completed_checkpoints, 0);
    assert_eq!(latest.total_checkpoints, intent.checkpoints.len() as u32);
    assert_eq!(
        latest.checkpoints[0].status,
        PluginLifecycleCheckpointDiagnosticStatus::Failed
    );
    assert_eq!(
        latest.checkpoints[0].error_code.as_deref(),
        Some("use.plugin.download_failed")
    );
    assert_eq!(
        latest.checkpoints[0].evidence_digest.as_deref(),
        Some(evidence('a').as_str())
    );
    assert_eq!(latest.checkpoints[0].observed_at_ms, Some(10));
    assert_eq!(
        latest.checkpoints[1].status,
        PluginLifecycleCheckpointDiagnosticStatus::Pending
    );

    let value = serde_json::to_value(&diagnostic).unwrap();
    let encoded = serde_json::to_string(&value).unwrap();
    assert!(!encoded.contains("idempotencyKey"));
    assert!(!encoded.contains("credentials"));
    assert!(!encoded.contains("token"));
}

#[tokio::test]
async fn lifecycle_diagnostics_distinguish_latest_and_previous_operations() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let first = intent("install:acme-guide:diagnostic-first");
    let mut record = store.begin(&first).await.unwrap();
    let mut completed_at_ms = 10;
    while let Some(checkpoint) = record.next_checkpoint().cloned() {
        record = store
            .record_checkpoint(
                &first,
                &checkpoint.idempotency_key,
                PluginLifecycleCheckpointOutcome::Applied,
                evidence('a'),
                None,
                completed_at_ms,
            )
            .await
            .unwrap();
        completed_at_ms += 10;
    }
    store.complete(&first, completed_at_ms).await.unwrap();

    let second = intent("install:acme-guide:diagnostic-second");
    store.begin(&second).await.unwrap();
    let diagnostic = store
        .diagnose(&second.scope, &second.package_id)
        .await
        .unwrap();

    let latest = diagnostic.latest.unwrap();
    assert_eq!(latest.operation_id, second.operation_id);
    assert_eq!(latest.status, PluginLifecycleOperationStatus::Applying);
    let previous = diagnostic.previous.unwrap();
    assert_eq!(previous.operation_id, first.operation_id);
    assert_eq!(previous.status, PluginLifecycleOperationStatus::Completed);
    assert_eq!(previous.completed_checkpoints, previous.total_checkpoints);
}

#[tokio::test]
async fn lifecycle_diagnostics_distinguish_phase_intents_with_one_operation_id() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let operation_id = "install:acme-guide:shared-operation";
    let first = intent_in_scope_with_action(
        operation_id,
        workspace_scope(),
        PluginLifecycleAction::Install,
    );
    let mut record = store.begin(&first).await.unwrap();
    let mut completed_at_ms = 10;
    while let Some(checkpoint) = record.next_checkpoint().cloned() {
        record = store
            .record_checkpoint(
                &first,
                &checkpoint.idempotency_key,
                PluginLifecycleCheckpointOutcome::Applied,
                evidence('a'),
                None,
                completed_at_ms,
            )
            .await
            .unwrap();
        completed_at_ms += 10;
    }
    store.complete(&first, completed_at_ms).await.unwrap();

    let second = intent_in_scope_with_action(
        operation_id,
        workspace_scope(),
        PluginLifecycleAction::Upgrade,
    );
    store.begin(&second).await.unwrap();
    let diagnostic = store
        .diagnose(&second.scope, &second.package_id)
        .await
        .unwrap();

    let latest = diagnostic.latest.unwrap();
    let previous = diagnostic.previous.unwrap();
    assert_eq!(latest.operation_id, previous.operation_id);
    assert_eq!(latest.action, PluginLifecycleAction::Upgrade);
    assert_eq!(previous.action, PluginLifecycleAction::Install);
    assert_ne!(latest.intent_digest, previous.intent_digest);
}

#[tokio::test]
async fn lifecycle_diagnostics_reject_duplicate_intent_history() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:duplicate-diagnostic");
    store.begin(&intent).await.unwrap();
    let directory = operation_directory(&store, &intent.scope);
    std::fs::copy(directory.join("active.json"), directory.join("last.json")).unwrap();

    let error = store
        .diagnose(&intent.scope, &intent.package_id)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_diagnostic_invalid");
}

#[tokio::test]
async fn rejects_out_of_order_and_required_optional_failure_checkpoints() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:3");
    let record = store.begin(&intent).await.unwrap();
    let package = &record.intent.checkpoints[0];
    let skill = &record.intent.checkpoints[1];

    let error = store
        .record_checkpoint(
            &intent,
            &skill.idempotency_key,
            PluginLifecycleCheckpointOutcome::Applied,
            evidence('a'),
            None,
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_operation_conflict");

    let error = store
        .record_checkpoint(
            &intent,
            &package.idempotency_key,
            PluginLifecycleCheckpointOutcome::OptionalFailed,
            evidence('b'),
            Some("use.plugin.package_failed".to_string()),
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_operation_invalid");
    assert!(store
        .load_active(&intent.scope, "acme/guide")
        .await
        .unwrap()
        .unwrap()
        .receipts
        .is_empty());
}

#[tokio::test]
async fn rolling_back_and_rolled_back_states_round_trip_and_replay_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:rollback");
    store.begin(&intent).await.unwrap();

    let rolling_back = store.start_rollback(&intent).await.unwrap();
    assert_eq!(
        rolling_back.status,
        PluginLifecycleOperationStatus::RollingBack
    );
    assert_eq!(store.start_rollback(&intent).await.unwrap(), rolling_back);
    let serialized = serde_json::to_value(&rolling_back).unwrap();
    assert_eq!(serialized["status"], "rolling-back");
    assert!(serialized.get("completedAtMs").is_none());
    assert!(serialized.get("rollbackEvidenceDigest").is_none());

    let rolled_back = store.roll_back(&intent, evidence('e'), 20).await.unwrap();
    assert_eq!(
        rolled_back.status,
        PluginLifecycleOperationStatus::RolledBack
    );
    assert_eq!(rolled_back.rollback_evidence_digest, Some(evidence('e')));
    assert_eq!(rolled_back.completed_at_ms, Some(20));
    assert_eq!(
        store.roll_back(&intent, evidence('e'), 30).await.unwrap(),
        rolled_back
    );
    assert_eq!(
        store
            .load_active(&intent.scope, "acme/guide")
            .await
            .unwrap(),
        Some(rolled_back)
    );
}

#[tokio::test]
async fn rollback_states_reject_forward_progress_and_changed_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:rollback-conflict");
    let applying = store.begin(&intent).await.unwrap();
    assert_eq!(
        store
            .roll_back(&intent, evidence('e'), 10)
            .await
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_operation_conflict"
    );

    let rolling_back = store.start_rollback(&intent).await.unwrap();
    assert_eq!(
        store
            .record_checkpoint(
                &intent,
                &applying.next_checkpoint().unwrap().idempotency_key,
                PluginLifecycleCheckpointOutcome::Applied,
                evidence('a'),
                None,
                10,
            )
            .await
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_operation_conflict"
    );
    assert_eq!(
        store.complete(&intent, 10).await.unwrap_err().code,
        "use.plugin.lifecycle_operation_conflict"
    );
    assert_eq!(
        rolling_back.status,
        PluginLifecycleOperationStatus::RollingBack
    );

    store.roll_back(&intent, evidence('e'), 20).await.unwrap();
    assert_eq!(
        store
            .roll_back(&intent, evidence('f'), 30)
            .await
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_operation_conflict"
    );
}

#[tokio::test]
async fn tampered_active_record_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:4");
    store.begin(&intent).await.unwrap();

    let active = operation_directory(&store, &intent.scope).join("active.json");
    let bytes = tokio::fs::read(&active).await.unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["intent"]["generation"] = serde_json::json!(0);
    tokio::fs::write(&active, serde_json::to_vec(&value).unwrap())
        .await
        .unwrap();

    let error = store
        .load_active(&intent.scope, "acme/guide")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_record_invalid");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn linked_operation_record_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        PluginLifecycleJournalStore::new(temp.path().join("state"), workspace_scope()).unwrap();
    let intent = intent("install:acme-guide:5");
    store.begin(&intent).await.unwrap();

    let directory = operation_directory(&store, &intent.scope);
    let active = directory.join("active.json");
    let target = directory.join("outside-record");
    tokio::fs::remove_file(&active).await.unwrap();
    tokio::fs::create_dir(&target).await.unwrap();
    crate::test_filesystem::create_directory_link(&target, &active);

    let error = store
        .load_active(&intent.scope, "acme/guide")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_record_invalid");
}

#[tokio::test]
async fn installation_bound_store_rejects_another_scope_kind() {
    let temp = tempfile::tempdir().unwrap();
    let user_scope = PlanScope {
        kind: PlanScopeKind::User,
        id: "shared".to_owned(),
    };
    let workspace_scope = PlanScope {
        kind: PlanScopeKind::Workspace,
        id: "shared".to_owned(),
    };
    let paths = a3s_use_extension::ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("state"),
        user_scope.clone(),
    )
    .unwrap();
    let store = PluginLifecycleJournalStore::from_extension_paths(&paths);
    let user = intent_in_scope("install:acme-guide:user", user_scope.clone());
    let workspace = intent_in_scope("install:acme-guide:workspace", workspace_scope.clone());

    store.begin(&user).await.unwrap();
    let error = store.begin(&workspace).await.unwrap_err();
    assert_eq!(error.code, "use.installation.identity_mismatch");

    assert_eq!(
        store
            .load_active(&user_scope, "acme/guide")
            .await
            .unwrap()
            .unwrap()
            .intent,
        user
    );
    let error = store
        .load_active(&workspace_scope, "acme/guide")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.installation.identity_mismatch");
    assert!(!operation_directory(&store, &workspace_scope).exists());
}

#[tokio::test]
async fn begin_enters_global_reference_admission_before_publishing_a_journal() {
    let temp = tempfile::tempdir().unwrap();
    let installation = workspace_scope();
    let paths = a3s_use_extension::ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("state"),
        installation,
    )
    .unwrap();
    let store = PluginLifecycleJournalStore::from_extension_paths(&paths);
    let intent = intent("install:acme-guide:admission");
    let collection = paths.artifact_store().acquire_collection().await.unwrap();

    let pending_store = store.clone();
    let pending_intent = intent.clone();
    let begin = tokio::spawn(async move { pending_store.begin(&pending_intent).await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!begin.is_finished());
    assert!(!operation_directory(&store, &intent.scope).exists());

    drop(collection);
    begin.await.unwrap().unwrap();
    assert!(operation_directory(&store, &intent.scope)
        .join("active.json")
        .is_file());
}

fn workspace_scope() -> PlanScope {
    PlanScope {
        kind: PlanScopeKind::Workspace,
        id: "guide".to_owned(),
    }
}

fn operation_directory(
    store: &PluginLifecycleJournalStore,
    scope: &PlanScope,
) -> std::path::PathBuf {
    store
        .root()
        .join(scope.kind.as_str())
        .join(scope.storage_key().unwrap())
        .join("acme")
        .join("guide")
}
