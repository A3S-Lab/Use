use std::path::Path;

use a3s_use_core::InstallationId;
use a3s_use_extension::ExtensionPaths;

use super::journal::{
    ActiveStateRestoreMarker, StateRestoreOperation, StateRestoreOperationStatus,
};
use super::{StateBackupManager, StateRestoreManager};

#[derive(Debug, Clone)]
pub(crate) struct StateRestoreHistoryFixture {
    pub(crate) plan_digest: String,
    pub(crate) planned_operation: Vec<u8>,
    pub(crate) completed_operation: Vec<u8>,
    pub(crate) active_marker: Vec<u8>,
}

pub(crate) async fn restore_history_fixture(
    installation: &InstallationId,
    started_at_ms: u64,
) -> StateRestoreHistoryFixture {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation.clone(),
    )
    .unwrap();
    let value = paths.state_root().join("knowledge/value.bin");
    std::fs::create_dir_all(value.parent().unwrap()).unwrap();
    std::fs::write(&value, format!("candidate-{started_at_ms}")).unwrap();
    let candidate = temporary.path().join("candidate.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&candidate)
        .await
        .unwrap();
    std::fs::write(&value, format!("live-{started_at_ms}")).unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&candidate).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let rollback = temporary.path().join("rollback.a3s-use-state-backup");
    let rollback = StateBackupManager::new(paths)
        .backup(&rollback)
        .await
        .unwrap();
    let planned = StateRestoreOperation::new(
        plan,
        plan_digest.clone(),
        rollback.descriptor_digest().unwrap(),
        started_at_ms,
    )
    .unwrap();
    let active_marker =
        serde_json::to_vec(&ActiveStateRestoreMarker::new(&planned).unwrap()).unwrap();
    let planned_operation = serde_json::to_vec(&planned).unwrap();
    let mut completed = planned;
    for status in [
        StateRestoreOperationStatus::Staged,
        StateRestoreOperationStatus::Publishing,
        StateRestoreOperationStatus::Published,
        StateRestoreOperationStatus::CandidatesRemoved,
        StateRestoreOperationStatus::Verified,
    ] {
        completed.advance(status, None).unwrap();
    }
    completed
        .advance(
            StateRestoreOperationStatus::Completed,
            Some(started_at_ms + 1),
        )
        .unwrap();

    StateRestoreHistoryFixture {
        plan_digest,
        planned_operation,
        completed_operation: serde_json::to_vec(&completed).unwrap(),
        active_marker,
    }
}

pub(crate) fn write_restore_history_operation(state_root: &Path, plan_digest: &str, bytes: &[u8]) {
    let segment = plan_digest.strip_prefix("sha256:").unwrap();
    let directory = state_root.join("operations/state-restores").join(segment);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("operation.json"), bytes).unwrap();
}
