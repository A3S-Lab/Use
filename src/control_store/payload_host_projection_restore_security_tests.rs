use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_host_projection_restore_tests::support::*;

#[tokio::test]
async fn host_restore_rejects_source_or_derived_candidate_tampering() {
    let fixture = verified_host_fixture().await;
    for target_kind in ["source", "derived", "lock"] {
        let target = TempDir::new().unwrap();
        let target_paths = target_paths(&target);
        let staged = fixture
            .verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                restore_staging(&target_paths),
            )
            .await
            .unwrap();
        let candidate = staged.candidate_path().unwrap();
        let target_file = match target_kind {
            "source" => walk_files(candidate)
                .into_iter()
                .find(|path| path.to_string_lossy().contains("requests"))
                .unwrap(),
            "derived" => walk_files(candidate)
                .into_iter()
                .find(|path| path.to_string_lossy().contains("operations"))
                .unwrap(),
            "lock" => {
                let scope = std::fs::read_dir(candidate)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| path.is_dir() && path.file_name().unwrap() != "diagnostics")
                    .unwrap();
                let path = scope.join(".store.lock");
                std::fs::write(&path, b"foreign").unwrap();
                path
            }
            _ => unreachable!(),
        };
        if target_kind != "lock" {
            let mut bytes = std::fs::read(&target_file).unwrap();
            bytes[0] ^= 1;
            std::fs::write(&target_file, bytes).unwrap();
        }
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();

        assert_eq!(
            staged.activate(&maintenance).await.unwrap_err().code,
            "use.control_store.host_projection_restore_invalid",
            "tamper case {target_kind} was accepted"
        );
        assert!(!live_host_root(&target_paths).exists());
    }
}

#[tokio::test]
async fn host_restore_rejects_staged_archive_drift_before_publication() {
    let fixture = verified_host_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = target_paths(&target);
    let staging = restore_staging(&target_paths);
    let staged = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    let archive = staging.join("control-host-projection.archive");
    let mut bytes = std::fs::read(&archive).unwrap();
    bytes[0] ^= 1;
    std::fs::write(archive, bytes).unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.host_projection_restore_invalid"
    );
    assert!(!live_host_root(&target_paths).exists());
}

#[tokio::test]
async fn host_restore_rejects_a_rebound_activation_marker() {
    let fixture = verified_host_fixture().await;
    for marker in [b"{}".to_vec(), vec![b'x'; 128 * 1024]] {
        let target = TempDir::new().unwrap();
        let target_paths = target_paths(&target);
        let staging = restore_staging(&target_paths);
        let staged = fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap();
        std::fs::write(
            staging.join("control-host-projection.activating.json"),
            marker,
        )
        .unwrap();
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();

        assert_eq!(
            staged.activate(&maintenance).await.unwrap_err().code,
            "use.control_store.host_projection_restore_invalid"
        );
        assert!(!live_host_root(&target_paths).exists());
    }
}

#[tokio::test]
async fn host_restore_rejects_activation_marker_drift_during_terminal_replay() {
    let fixture = verified_host_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = target_paths(&target);
    let staging = restore_staging(&target_paths);
    let staged = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    staged.activate(&maintenance).await.unwrap();
    std::fs::write(
        staging.join("control-host-projection.activating.json"),
        b"{}",
    )
    .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.host_projection_restore_invalid"
    );
    assert_eq!(restored_sources(&target_paths).await, fixture.sources);
}

#[tokio::test]
async fn host_restore_rejects_live_owner_staging_and_unowned_staging_entries() {
    let fixture = verified_host_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = target_paths(&target);
    assert_eq!(
        fixture
            .verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                live_host_root(&target_paths).join("restore-staging"),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.host_projection_restore_invalid"
    );

    let staging = restore_staging(&target_paths);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("foreign"), b"sentinel").unwrap();
    assert_eq!(
        fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap_err()
            .code,
        "use.control_store.host_projection_restore_invalid"
    );
    assert_eq!(std::fs::read(staging.join("foreign")).unwrap(), b"sentinel");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn host_restore_rejects_linked_candidate_and_live_roots() {
    let fixture = verified_host_fixture().await;
    for linked_kind in ["candidate", "live"] {
        let target = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target_paths = target_paths(&target);
        let staged = fixture
            .verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                restore_staging(&target_paths),
            )
            .await
            .unwrap();
        let link = if linked_kind == "candidate" {
            let candidate = staged.candidate_path().unwrap().to_path_buf();
            std::fs::remove_dir_all(&candidate).unwrap();
            candidate
        } else {
            live_host_root(&target_paths)
        };
        std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
        crate::test_filesystem::create_directory_link(outside.path(), &link);
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();

        assert_eq!(
            staged.activate(&maintenance).await.unwrap_err().code,
            "use.control_store.host_projection_restore_invalid",
            "linked {linked_kind} root was accepted"
        );
        assert_eq!(
            std::fs::read(outside.path().join("sentinel")).unwrap(),
            b"outside"
        );
        crate::test_filesystem::remove_directory_link(&link);
    }
}
