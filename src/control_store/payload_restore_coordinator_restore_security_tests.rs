use a3s_use_extension::{StateMaintenanceLock, ACTIVE_STATE_RESTORE_MARKER};
use tempfile::TempDir;

use super::payload_knowledge_tests::support::{control_installation, paths};
use super::payload_restore_coordinator_restore_tests::support::verified_restore_fixture;
use super::payload_restore_coordinator_restore_tests::{operation_path, write_active};
use crate::state_restore::test_support::restore_history_fixture;

#[tokio::test]
async fn activation_requires_an_active_restore_and_the_exact_exclusive_guard() {
    let source = verified_restore_fixture(1, 50_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let staged = source
        .verified
        .stage_restore(
            target_paths.installation_state_root(),
            target_paths
                .installation_state_root()
                .join("coordinator-staging"),
        )
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.restore_coordinator_restore_requires_active"
    );

    let active = restore_history_fixture(&installation, 51_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );
    let foreign_root = target.path().join("foreign-state");
    let foreign = StateMaintenanceLock::new(&foreign_root)
        .acquire_exclusive()
        .await
        .unwrap();
    assert_eq!(
        staged.activate(&foreign).await.unwrap_err().code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
}

#[tokio::test]
async fn activation_rejects_active_identity_collision_and_candidate_tampering() {
    for case in ["collision", "candidate-tamper"] {
        let source = verified_restore_fixture(1, 52_000).await;
        let target = TempDir::new().unwrap();
        let installation = control_installation();
        let target_paths = paths(&target, installation.clone());
        std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
        let staging = target_paths
            .installation_state_root()
            .join(format!("{case}-coordinator-staging"));
        let staged = source
            .verified
            .stage_restore(target_paths.installation_state_root(), staging)
            .await
            .unwrap();
        if case == "candidate-tamper" {
            std::fs::write(
                staged
                    .candidate_path()
                    .unwrap()
                    .join(
                        source.operations[0]
                            .plan_digest
                            .strip_prefix("sha256:")
                            .unwrap(),
                    )
                    .join("operation.json"),
                b"{}",
            )
            .unwrap();
        }
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();
        let active = if case == "collision" {
            source.operations[0].clone()
        } else {
            restore_history_fixture(&installation, 53_000).await
        };
        write_active(
            &target_paths.installation_state_root(),
            &active,
            &active.planned_operation,
        );
        assert_eq!(
            staged.activate(&maintenance).await.unwrap_err().code,
            "use.control_store.restore_coordinator_restore_invalid",
            "case {case} was accepted"
        );
    }
}

#[tokio::test]
async fn complete_restore_activation_binds_the_expected_marker_before_history_mutation() {
    let source = verified_restore_fixture(1, 53_500).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation);
    let state_root = target_paths.installation_state_root();
    std::fs::create_dir_all(&state_root).unwrap();
    let staged = source
        .verified
        .stage_restore(&state_root, state_root.join("bound-coordinator-staging"))
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(&state_root)
        .acquire_exclusive()
        .await
        .unwrap();
    let expected = crate::state_restore::ControlInstallationRestoreActiveMarker::new(
        &format!("sha256:{}", "1".repeat(64)),
        &format!("sha256:{}", "2".repeat(64)),
    )
    .unwrap()
    .canonical_bytes()
    .unwrap();
    let foreign = crate::state_restore::ControlInstallationRestoreActiveMarker::new(
        &format!("sha256:{}", "3".repeat(64)),
        &format!("sha256:{}", "4".repeat(64)),
    )
    .unwrap()
    .canonical_bytes()
    .unwrap();
    std::fs::write(state_root.join(ACTIVE_STATE_RESTORE_MARKER), foreign).unwrap();

    assert_eq!(
        staged
            .activate_for_complete_restore(
                &maintenance,
                &format!("sha256:{}", "1".repeat(64)),
                &expected,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
    assert!(!operation_path(&state_root, &source.operations[0].plan_digest).exists());
    assert!(!state_root.join("operations/state-restores").exists());
}

#[tokio::test]
async fn activation_rejects_unknown_staging_and_rebound_marker_evidence() {
    let source = verified_restore_fixture(1, 54_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let staging = target_paths
        .installation_state_root()
        .join("rebound-coordinator-staging");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 55_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );
    staged.activate(&maintenance).await.unwrap();

    std::fs::write(
        staging.join("control-restore-coordinator.activating.json"),
        b"{}",
    )
    .unwrap();
    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
    std::fs::remove_file(staging.join("control-restore-coordinator.activating.json")).unwrap();
    std::fs::write(staging.join("unknown"), b"unknown").unwrap();
    assert_eq!(
        source
            .verified
            .reopen_staged_restore(
                target_paths.installation_state_root(),
                staging,
                &maintenance,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
}

#[tokio::test]
async fn replay_rejects_tampered_retired_history() {
    let source = verified_restore_fixture(1, 58_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let prior = restore_history_fixture(&installation, 59_000).await;
    crate::state_restore::test_support::write_restore_history_operation(
        &target_paths.installation_state_root(),
        &prior.plan_digest,
        &prior.completed_operation,
    );
    let staging = target_paths
        .installation_state_root()
        .join("retired-tamper-coordinator-staging");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 60_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );
    staged.activate(&maintenance).await.unwrap();

    std::fs::write(
        staging
            .join("retired")
            .join(prior.plan_digest.strip_prefix("sha256:").unwrap())
            .join("operation.json"),
        b"{}",
    )
    .unwrap();
    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn activation_rejects_a_linked_live_history_root() {
    let source = verified_restore_fixture(1, 56_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let staged = source
        .verified
        .stage_restore(
            target_paths.installation_state_root(),
            target_paths
                .installation_state_root()
                .join("linked-coordinator-staging"),
        )
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 57_000).await;
    std::fs::write(
        target_paths
            .installation_state_root()
            .join(ACTIVE_STATE_RESTORE_MARKER),
        &active.active_marker,
    )
    .unwrap();
    let outside = target.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(target_paths.installation_state_root().join("operations")).unwrap();
    crate::test_filesystem::create_directory_link(
        &outside,
        &target_paths
            .installation_state_root()
            .join("operations/state-restores"),
    );
    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
    assert!(!operation_path(
        &target_paths.installation_state_root(),
        &source.operations[0].plan_digest,
    )
    .exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn activation_rejects_a_staging_ancestor_replaced_by_a_link() {
    let source = verified_restore_fixture(1, 61_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let staging_parent = target_paths
        .installation_state_root()
        .join("coordinator-staging-parent");
    let staging = staging_parent.join("attempt");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging)
        .await
        .unwrap();
    let outside = target.path().join("outside-staging");
    std::fs::rename(&staging_parent, &outside).unwrap();
    crate::test_filesystem::create_directory_link(&outside, &staging_parent);
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 62_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.restore_coordinator_restore_invalid"
    );
    assert!(!operation_path(
        &target_paths.installation_state_root(),
        &source.operations[0].plan_digest,
    )
    .exists());
    assert!(outside.join("attempt/restore-history-candidate").is_dir());
}
