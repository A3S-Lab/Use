use a3s_use_extension::ExtensionManifest;

use super::*;
use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleCheckpointOutcome, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginLifecycleOperationStatus,
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
    let manifest = ExtensionManifest::parse_acl(OPTIONAL_SKILL_PACKAGE).unwrap();
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: operation_id.to_string(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope_id: "workspace:guide".to_string(),
            package_id: "acme/guide".to_string(),
            package_digest: format!("sha256:{}", "2".repeat(64)),
            manifest_digest: format!("sha256:{}", "3".repeat(64)),
            generation: 9,
            action: PluginLifecycleAction::Install,
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
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
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

    let reopened = PluginLifecycleJournalStore::new(temp.path().join("state"));
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
            .load_active("workspace:guide", "acme/guide")
            .await
            .unwrap(),
        Some(completed)
    );
}

#[tokio::test]
async fn rejects_conflicting_operation_until_current_one_completes() {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
    let first = intent("install:acme-guide:1");
    let second = intent("install:acme-guide:2");
    store.begin(&first).await.unwrap();

    let error = store.begin(&second).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_busy");
}

#[tokio::test]
async fn rejects_out_of_order_and_required_optional_failure_checkpoints() {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
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
        .load_active("workspace:guide", "acme/guide")
        .await
        .unwrap()
        .unwrap()
        .receipts
        .is_empty());
}

#[tokio::test]
async fn rolling_back_and_rolled_back_states_round_trip_and_replay_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
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
            .load_active("workspace:guide", "acme/guide")
            .await
            .unwrap(),
        Some(rolled_back)
    );
}

#[tokio::test]
async fn rollback_states_reject_forward_progress_and_changed_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
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
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
    let intent = intent("install:acme-guide:4");
    store.begin(&intent).await.unwrap();

    let mut entries = tokio::fs::read_dir(store.root()).await.unwrap();
    let scope = entries.next_entry().await.unwrap().unwrap().path();
    let active = scope.join("acme").join("guide").join("active.json");
    let bytes = tokio::fs::read(&active).await.unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["intent"]["generation"] = serde_json::json!(0);
    tokio::fs::write(&active, serde_json::to_vec(&value).unwrap())
        .await
        .unwrap();

    let error = store
        .load_active("workspace:guide", "acme/guide")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_record_invalid");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_operation_record_fails_closed() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let store = PluginLifecycleJournalStore::new(temp.path().join("state"));
    let intent = intent("install:acme-guide:5");
    store.begin(&intent).await.unwrap();

    let mut entries = tokio::fs::read_dir(store.root()).await.unwrap();
    let scope = entries.next_entry().await.unwrap().unwrap().path();
    let directory = scope.join("acme").join("guide");
    let active = directory.join("active.json");
    let target = directory.join("outside.json");
    tokio::fs::rename(&active, &target).await.unwrap();
    symlink(&target, &active).unwrap();

    let error = store
        .load_active("workspace:guide", "acme/guide")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_record_invalid");
}
