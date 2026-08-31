use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_knowledge_tests::support::paths;
use super::payload_observation_restore_tests::support::*;

#[tokio::test]
async fn observation_restore_rejects_candidate_tampering_without_publishing_records() {
    let fixture = verified_observation_fixture().await;
    for tamper in ["truncate", "trailing", "substitute"] {
        let target = TempDir::new().unwrap();
        let target_paths = paths(&target, fixture.installation.clone());
        let staged = fixture
            .verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                restore_staging(&target_paths),
            )
            .await
            .unwrap();
        let candidate = staged.candidate_path().unwrap();
        let mut bytes = std::fs::read(candidate).unwrap();
        match tamper {
            "truncate" => {
                bytes.pop();
            }
            "trailing" => bytes.push(0),
            "substitute" => bytes[0] ^= 1,
            _ => unreachable!(),
        }
        std::fs::write(candidate, bytes).unwrap();
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();

        assert_eq!(
            staged.activate(&maintenance).await.unwrap_err().code,
            "use.control_store.observation_payload_restore_invalid",
            "tamper case {tamper} was accepted"
        );
        for (path, _) in fixture.terminal_records() {
            assert!(!target_paths
                .installation_state_root()
                .join("operations")
                .join(path)
                .exists());
        }
    }
}

#[tokio::test]
async fn observation_restore_rejects_live_owner_staging_and_unowned_staging_entries() {
    let fixture = verified_observation_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, fixture.installation.clone());
    let live_staging = target_paths
        .installation_state_root()
        .join("operations/package-resolutions/restore-staging");
    assert_eq!(
        fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), live_staging)
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_restore_invalid"
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
        "use.control_store.observation_payload_restore_invalid"
    );
    assert_eq!(std::fs::read(staging.join("foreign")).unwrap(), b"sentinel");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn observation_restore_rejects_a_linked_candidate_without_touching_its_target() {
    let fixture = verified_observation_fixture().await;
    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let target_paths = paths(&target, fixture.installation.clone());
    let staged = fixture
        .verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            restore_staging(&target_paths),
        )
        .await
        .unwrap();
    let candidate = staged.candidate_path().unwrap().to_path_buf();
    std::fs::remove_file(&candidate).unwrap();
    std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
    crate::test_filesystem::create_directory_link(outside.path(), &candidate);
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.observation_payload_restore_invalid"
    );
    assert_eq!(
        std::fs::read(outside.path().join("sentinel")).unwrap(),
        b"outside"
    );
    crate::test_filesystem::remove_directory_link(&candidate);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn observation_restore_rejects_a_linked_live_owner_root_without_touching_the_target() {
    let fixture = verified_observation_fixture().await;
    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let target_paths = paths(&target, fixture.installation.clone());
    let staged = fixture
        .verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            restore_staging(&target_paths),
        )
        .await
        .unwrap();
    let operations = target_paths.installation_state_root().join("operations");
    std::fs::create_dir_all(&operations).unwrap();
    std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
    let linked = operations.join("package-diagnostic-history");
    crate::test_filesystem::create_directory_link(outside.path(), &linked);
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.observation_payload_restore_invalid"
    );
    assert_eq!(
        std::fs::read(outside.path().join("sentinel")).unwrap(),
        b"outside"
    );
    crate::test_filesystem::remove_directory_link(&linked);
}

#[tokio::test]
async fn observation_restore_rejects_ambiguous_candidate_and_activation_files() {
    let fixture = verified_observation_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, fixture.installation.clone());
    let staged = fixture
        .verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            restore_staging(&target_paths),
        )
        .await
        .unwrap();
    let candidate = staged.candidate_path().unwrap();
    let activating = candidate.with_file_name("control-observations.archive.activating");
    std::fs::copy(candidate, activating).unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.observation_payload_restore_invalid"
    );
}
