use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{clock_from, coordinator_at, RecordingHosts};
use crate::plugin_lifecycle::test_support::{intent, manifest};
use crate::plugin_lifecycle::{PluginLifecycleAction, PluginLifecycleOperationStatus};

pub(super) const CHECKPOINT_CRASH_EXIT_CODE: i32 = 86;

const CHILD_ROOT_ENV: &str = "A3S_USE_TEST_LIFECYCLE_FAULT_ROOT";
const CHILD_ACTION_ENV: &str = "A3S_USE_TEST_LIFECYCLE_FAULT_ACTION";
const CHILD_CHECKPOINT_ENV: &str = "A3S_USE_TEST_LIFECYCLE_FAULT_CHECKPOINT";

#[tokio::test]
async fn every_lifecycle_checkpoint_recovers_after_an_ambiguous_host_effect() {
    for action in [
        PluginLifecycleAction::Install,
        PluginLifecycleAction::Upgrade,
        PluginLifecycleAction::Enable,
        PluginLifecycleAction::Disable,
        PluginLifecycleAction::Uninstall,
    ] {
        let expected = intent(action);
        for checkpoint in &expected.checkpoints {
            exercise_checkpoint_crash(action, &checkpoint.idempotency_key).await;
        }
    }
}

async fn exercise_checkpoint_crash(action: PluginLifecycleAction, fault_key: &str) {
    let temporary = tempfile::tempdir().expect("create lifecycle fault directory");
    let root = temporary.path().to_path_buf();
    let output = tokio::process::Command::new(
        std::env::current_exe().expect("resolve lifecycle test executable"),
    )
    .arg("lifecycle_checkpoint_crash_child")
    .arg("--ignored")
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(CHILD_ROOT_ENV, &root)
    .env(CHILD_ACTION_ENV, action_name(action))
    .env(CHILD_CHECKPOINT_ENV, fault_key)
    .output()
    .await
    .expect("run lifecycle checkpoint fault child");
    assert_eq!(
        output.status.code(),
        Some(CHECKPOINT_CRASH_EXIT_CODE),
        "fault child did not exit at {action:?} checkpoint {fault_key}: status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let expected = intent(action);
    let host_root = root.join("host");
    let host = Arc::new(RecordingHosts::with_durable_checkpoint_crash(
        host_root.clone(),
        None,
    ));
    let coordinator = coordinator_at(&root.join("state"), host);
    let completed = coordinator
        .apply(&expected, &manifest(), clock_from(10_000))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "failed to recover {action:?} checkpoint {fault_key}: {} ({})",
                error.message, error.code
            )
        });
    assert_eq!(completed.status, PluginLifecycleOperationStatus::Completed);
    assert_eq!(completed.receipts.len(), expected.checkpoints.len());
    assert!(completed.last_failure.is_none());

    let attempts = attempt_counts(&host_root).await;
    assert_eq!(attempts.len(), expected.checkpoints.len());
    for checkpoint in &expected.checkpoints {
        assert_eq!(
            attempts.get(&checkpoint.idempotency_key),
            Some(&if checkpoint.idempotency_key == fault_key {
                2
            } else {
                1
            }),
            "unexpected {action:?} attempt count for checkpoint {} after faulting {fault_key}",
            checkpoint.idempotency_key,
        );
    }
    assert_eq!(
        effect_count(&host_root.join("effects")).await,
        expected.checkpoints.len(),
        "a replay duplicated or omitted a durable {action:?} host effect after faulting {fault_key}",
    );

    let attempts_before_replay = tokio::fs::read(host_root.join("attempts.log"))
        .await
        .expect("read terminal lifecycle attempt log");
    let replayed = coordinator
        .apply(&expected, &manifest(), clock_from(20_000))
        .await
        .expect("replay completed lifecycle operation");
    assert_eq!(replayed, completed);
    assert_eq!(
        tokio::fs::read(host_root.join("attempts.log"))
            .await
            .expect("read replayed lifecycle attempt log"),
        attempts_before_replay,
        "terminal {action:?} replay invoked a host after faulting {fault_key}",
    );
}

async fn attempt_counts(root: &Path) -> BTreeMap<String, usize> {
    let contents = tokio::fs::read_to_string(root.join("attempts.log"))
        .await
        .expect("read durable lifecycle attempts");
    let mut counts = BTreeMap::new();
    for line in contents.lines() {
        let (key, label) = line
            .split_once('\t')
            .expect("durable lifecycle attempt must contain key and label");
        assert!(
            !label.is_empty(),
            "durable lifecycle attempt label is empty"
        );
        *counts.entry(key.to_string()).or_insert(0) += 1;
    }
    counts
}

async fn effect_count(root: &Path) -> usize {
    let mut entries = tokio::fs::read_dir(root)
        .await
        .expect("open durable lifecycle effect directory");
    let mut count = 0;
    while let Some(entry) = entries
        .next_entry()
        .await
        .expect("read durable lifecycle effect entry")
    {
        assert!(
            entry
                .file_type()
                .await
                .expect("inspect durable lifecycle effect")
                .is_file(),
            "durable lifecycle effect must be a regular file"
        );
        count += 1;
    }
    count
}

#[tokio::test]
#[ignore = "subprocess helper for lifecycle checkpoint crash injection"]
async fn lifecycle_checkpoint_crash_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let action = parse_action(
        &std::env::var(CHILD_ACTION_ENV).expect("lifecycle fault child action is missing"),
    );
    let fault_key =
        std::env::var(CHILD_CHECKPOINT_ENV).expect("lifecycle fault child checkpoint is missing");
    let expected = intent(action);
    assert!(
        expected
            .checkpoints
            .iter()
            .any(|checkpoint| checkpoint.idempotency_key == fault_key),
        "lifecycle fault child checkpoint is not canonical"
    );
    let host = Arc::new(RecordingHosts::with_durable_checkpoint_crash(
        root.join("host"),
        Some(fault_key.clone()),
    ));
    let coordinator = coordinator_at(&root.join("state"), host);
    let outcome = coordinator
        .apply(&expected, &manifest(), clock_from(100))
        .await;
    panic!(
        "lifecycle fault child completed without exiting at {action:?} checkpoint {fault_key}: {outcome:?}"
    );
}

fn action_name(action: PluginLifecycleAction) -> &'static str {
    match action {
        PluginLifecycleAction::Install => "install",
        PluginLifecycleAction::Upgrade => "upgrade",
        PluginLifecycleAction::Enable => "enable",
        PluginLifecycleAction::Disable => "disable",
        PluginLifecycleAction::Uninstall => "uninstall",
    }
}

fn parse_action(value: &str) -> PluginLifecycleAction {
    match value {
        "install" => PluginLifecycleAction::Install,
        "upgrade" => PluginLifecycleAction::Upgrade,
        "enable" => PluginLifecycleAction::Enable,
        "disable" => PluginLifecycleAction::Disable,
        "uninstall" => PluginLifecycleAction::Uninstall,
        _ => panic!("unsupported lifecycle fault child action {value:?}"),
    }
}
