use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use a3s_use_core::PlanScopeKind;
use tempfile::TempDir;

use super::tests::{knowledge_files, scope, stage_and_promote, stage_spec};
use super::*;
use crate::okf_knowledge::OkfKnowledgeClient;

#[tokio::test]
async fn removes_oldest_only_after_exact_plan_confirmation() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let files = knowledge_files("retained throughput", "retained latency");
    stage_and_promote(
        &client,
        stage_spec(1, workspace_scope.clone(), "acme/research", &files),
        files,
    )
    .await;
    let backup_directory = temporary.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    for name in [
        "001.a3s-okf-backup",
        "002.a3s-okf-backup",
        "003.a3s-okf-backup",
    ] {
        adapter
            .backup(&workspace_scope, backup_directory.join(name))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let policy = OkfKnowledgeBackupRetentionPolicy::new(2, 1024 * 1024 * 1024).unwrap();
    let stale_plan = SqliteOkfKnowledgeAdapter::plan_backup_retention(
        &backup_directory,
        &workspace_scope,
        policy,
    )
    .await
    .unwrap();
    assert_eq!(
        stale_plan.schema,
        OKF_KNOWLEDGE_BACKUP_RETENTION_PLAN_SCHEMA
    );
    assert_eq!(stale_plan.remove.len(), 1);
    assert_eq!(stale_plan.remove[0].file_name, "001.a3s-okf-backup");
    assert_eq!(stale_plan.retain.len(), 2);
    let stale_digest = stale_plan.descriptor_digest().unwrap();

    adapter
        .backup(
            &workspace_scope,
            backup_directory.join("004.a3s-okf-backup"),
        )
        .await
        .unwrap();
    let error = SqliteOkfKnowledgeAdapter::apply_backup_retention(
        &backup_directory,
        &workspace_scope,
        policy,
        &stale_digest,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.code,
        "use.okf.knowledge_backup_retention_plan_mismatch"
    );
    assert!(backup_directory.join("001.a3s-okf-backup").is_file());

    let plan = SqliteOkfKnowledgeAdapter::plan_backup_retention(
        &backup_directory,
        &workspace_scope,
        policy,
    )
    .await
    .unwrap();
    assert_eq!(
        plan.remove
            .iter()
            .map(|backup| backup.file_name.as_str())
            .collect::<Vec<_>>(),
        vec!["001.a3s-okf-backup", "002.a3s-okf-backup"]
    );
    let result = SqliteOkfKnowledgeAdapter::apply_backup_retention(
        &backup_directory,
        &workspace_scope,
        policy,
        &plan.descriptor_digest().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(result.schema, OKF_KNOWLEDGE_BACKUP_RETENTION_RESULT_SCHEMA);
    assert!(result.changed);
    assert_eq!(result.removed, plan.remove);
    assert_eq!(result.retained_backup_count, 2);
    assert!(!backup_directory.join("001.a3s-okf-backup").exists());
    assert!(!backup_directory.join("002.a3s-okf-backup").exists());
    assert!(backup_directory.join("003.a3s-okf-backup").is_file());
    assert!(backup_directory.join("004.a3s-okf-backup").is_file());

    let replay = SqliteOkfKnowledgeAdapter::plan_backup_retention(
        &backup_directory,
        &workspace_scope,
        policy,
    )
    .await
    .unwrap();
    assert!(replay.remove.is_empty());
}

#[test]
fn policy_is_bounded_and_keeps_at_least_one_backup() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OkfKnowledgeBackupRetentionPolicy>();
    assert_send_sync::<OkfKnowledgeBackupRetentionPlan>();
    assert_send_sync::<OkfKnowledgeBackupRetentionResult>();
    let mut unknown = serde_json::to_value(OkfKnowledgeBackupRetentionPolicy::default()).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<OkfKnowledgeBackupRetentionPolicy>(unknown).is_err());
    assert_eq!(
        OkfKnowledgeBackupRetentionPolicy::new(0, 1)
            .unwrap_err()
            .code,
        "use.okf.knowledge_backup_retention_policy_invalid"
    );
    assert_eq!(
        OkfKnowledgeBackupRetentionPolicy::new(1, 0)
            .unwrap_err()
            .code,
        "use.okf.knowledge_backup_retention_policy_invalid"
    );
}

#[tokio::test]
async fn is_scope_isolated_and_fails_closed_on_invalid_candidates() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let user_scope = scope(PlanScopeKind::User);
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let files = knowledge_files("isolated throughput", "isolated latency");
    stage_and_promote(
        &client,
        stage_spec(1, workspace_scope.clone(), "acme/research", &files),
        files,
    )
    .await;
    let backup_directory = temporary.path().join("isolated-backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let backup_path = backup_directory.join("workspace.a3s-okf-backup");
    adapter
        .backup(&workspace_scope, &backup_path)
        .await
        .unwrap();
    let policy = OkfKnowledgeBackupRetentionPolicy::new(1, 1024 * 1024 * 1024).unwrap();

    let user_plan =
        SqliteOkfKnowledgeAdapter::plan_backup_retention(&backup_directory, &user_scope, policy)
            .await
            .unwrap();
    assert_eq!(user_plan.before_backup_count, 0);
    assert!(user_plan.remove.is_empty());
    assert!(backup_path.is_file());

    let too_small = OkfKnowledgeBackupRetentionPolicy::new(1, 1).unwrap();
    let error = SqliteOkfKnowledgeAdapter::plan_backup_retention(
        &backup_directory,
        &workspace_scope,
        too_small,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.code,
        "use.okf.knowledge_backup_retention_policy_unsatisfied"
    );
    assert!(backup_path.is_file());

    let tampered = backup_directory.join("tampered.a3s-okf-backup");
    std::fs::copy(&backup_path, &tampered).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tampered)
        .unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[byte[0] ^ 0xff]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let error = SqliteOkfKnowledgeAdapter::plan_backup_retention(
        &backup_directory,
        &workspace_scope,
        policy,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_backup_invalid");
    assert!(backup_path.is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_linked_directories_and_candidates() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let directory = temporary.path().join("backups");
    std::fs::create_dir(&directory).unwrap();
    let linked_directory = temporary.path().join("linked-backups");
    symlink(&directory, &linked_directory).unwrap();
    let policy = OkfKnowledgeBackupRetentionPolicy::default();
    let error = SqliteOkfKnowledgeAdapter::plan_backup_retention(
        &linked_directory,
        &workspace_scope,
        policy,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.code,
        "use.okf.knowledge_backup_retention_directory_invalid"
    );

    let target = temporary.path().join("outside");
    std::fs::write(&target, b"not a backup").unwrap();
    symlink(&target, directory.join("linked.a3s-okf-backup")).unwrap();
    let error =
        SqliteOkfKnowledgeAdapter::plan_backup_retention(&directory, &workspace_scope, policy)
            .await
            .unwrap_err();
    assert_eq!(
        error.code,
        "use.okf.knowledge_backup_retention_directory_invalid"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"not a backup");
}
