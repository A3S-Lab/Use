use std::io::Write;

use a3s_use_extension::UsePaths;
use tempfile::TempDir;

use super::aggregate_tests::fixtures::control_installation;
use super::payload_installation_restore_staging_tests::absent_snapshot;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

const ERROR_CODE: &str = "use.control_store.complete_restore_activation_invalid";
const ATTEMPT_DIRECTORY: &str = ".control-installation-restore";

#[tokio::test]
async fn terminal_receipt_is_self_contained_canonical_and_installation_bound() {
    let verified = absent_snapshot(15_650).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
    drop(staged);
    let receipt = state_root.join(ATTEMPT_DIRECTORY);
    let before = receipt_inventory(&receipt);

    let installation = super::validate_terminal_restore_receipt_blocking(&receipt).unwrap();
    let replay = super::validate_terminal_restore_receipt_blocking(&receipt).unwrap();

    assert_eq!(installation, control_installation());
    assert_eq!(replay, installation);
    assert_eq!(receipt_inventory(&receipt), before);
}

#[tokio::test]
async fn terminal_receipt_is_excluded_from_legacy_inventory_and_reachability() {
    let verified = absent_snapshot(15_675).await;
    let target = TempDir::new().unwrap();
    let roots = UsePaths::new(target.path().join("data"), target.path().join("state"));
    let paths = roots.for_installation(control_installation()).unwrap();
    let state_root = paths.installation_state_root().to_path_buf();
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
    drop(staged);

    // The current production scanners still read legacy authority. Remove the
    // inactive Control database only to isolate the operational receipt rule.
    std::fs::remove_file(state_root.join("control.sqlite3")).unwrap();
    let state_inventory = crate::state_backup::scan_state_for_restore(&paths, None).unwrap();
    let reachability =
        crate::artifact_reachability::ArtifactReachabilityInspector::new(roots.clone())
            .inspect_references()
            .await
            .unwrap();

    assert!(state_inventory.is_empty());
    assert!(reachability.entries.is_empty());

    std::fs::OpenOptions::new()
        .append(true)
        .open(state_root.join(ATTEMPT_DIRECTORY).join("activation.json"))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();
    let backup_error = crate::state_backup::scan_state_for_restore(&paths, None).unwrap_err();
    let reachability_error =
        crate::artifact_reachability::ArtifactReachabilityInspector::new(roots)
            .inspect_references()
            .await
            .unwrap_err();

    assert_eq!(backup_error.code, "use.state_backup_nonterminal");
    assert_eq!(
        reachability_error.code,
        "use.artifact_reachability.reference_invalid"
    );
}

#[tokio::test]
async fn terminal_reopen_rejects_attempt_descriptor_tampering() {
    let verified = absent_snapshot(15_700).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
    drop(staged);
    std::fs::OpenOptions::new()
        .append(true)
        .open(state_root.join(ATTEMPT_DIRECTORY).join("attempt.json"))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    let error = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join("control.sqlite3").is_file());
}

#[tokio::test]
async fn terminal_reopen_rejects_activation_journal_tampering() {
    let verified = absent_snapshot(15_800).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
    drop(staged);
    std::fs::OpenOptions::new()
        .append(true)
        .open(state_root.join(ATTEMPT_DIRECTORY).join("activation.json"))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    let error = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join("control.sqlite3").is_file());
}

#[tokio::test]
async fn terminal_reopen_rejects_unknown_receipt_entries() {
    let verified = absent_snapshot(15_900).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
    drop(staged);
    std::fs::write(
        state_root.join(ATTEMPT_DIRECTORY).join("foreign.json"),
        b"foreign",
    )
    .unwrap();

    let error = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join("control.sqlite3").is_file());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn terminal_reopen_rejects_linked_receipt_entries() {
    let verified = absent_snapshot(16_000).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
    drop(staged);
    let outside = target.path().join("outside-terminal-receipt");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("preserved"), b"preserved").unwrap();
    let linked = state_root.join(ATTEMPT_DIRECTORY).join("linked-receipt");
    crate::test_filesystem::create_directory_link(&outside, &linked);

    let error = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert_eq!(
        std::fs::read(outside.join("preserved")).unwrap(),
        b"preserved"
    );
    crate::test_filesystem::remove_directory_link(&linked);
}

fn receipt_inventory(receipt: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = std::fs::read_dir(receipt)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}
