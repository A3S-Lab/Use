use a3s_use_extension::StateMaintenanceLock;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::payload_owner::*;

pub(in crate::control_store) mod support;

use support::*;

#[test]
fn host_projection_restore_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlHostProjectionRestoreResult>();
    assert_send_sync::<StagedControlHostProjectionRestore>();
}

#[tokio::test]
async fn verified_host_restore_publishes_one_complete_canonical_owner_root() {
    let fixture = verified_host_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = target_paths(&target);
    let staging = restore_staging(&target_paths);
    let staged = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();

    let candidate = staged.candidate_path().unwrap();
    assert!(candidate.is_dir());
    assert!(!live_host_root(&target_paths).exists());

    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let result = staged.activate(&maintenance).await.unwrap();
    result.validate(&fixture.registry).unwrap();
    result
        .validate_for_snapshot(&fixture.registry, &fixture.snapshot)
        .unwrap();
    assert!(matches!(
        result.payload,
        ControlHostProjectionRestoreState::Archive {
            source_records: 3,
            ..
        }
    ));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains(&target.path().display().to_string()));
    assert!(!candidate.exists());
    assert_eq!(restored_sources(&target_paths).await, fixture.sources);
    assert_canonical_indexes(&target_paths, 2, 1, 1);

    let replay = staged.activate(&maintenance).await.unwrap();
    assert_eq!(replay, result);

    let mut tampered = result;
    let ControlHostProjectionRestoreState::Archive { source_records, .. } = &mut tampered.payload
    else {
        unreachable!();
    };
    *source_records += 1;
    assert_eq!(
        tampered
            .validate_for_snapshot(&fixture.registry, &fixture.snapshot)
            .unwrap_err()
            .code,
        "use.control_store.host_projection_restore_invalid"
    );
}

#[tokio::test]
async fn host_restore_reopens_the_same_staged_attempt_after_atomic_publication() {
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
    let first = staged.activate(&maintenance).await.unwrap();
    drop(maintenance);

    let reopened = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging)
        .await
        .unwrap();
    assert!(!reopened.candidate_path().unwrap().exists());
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    assert_eq!(reopened.activate(&maintenance).await.unwrap(), first);
}

#[tokio::test]
async fn host_restore_normalizes_legacy_aliases_instead_of_restoring_them() {
    let fixture = verified_host_fixture().await;
    assert!(fixture.snapshot.manifest.validated_index_records > 3);
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
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    staged.activate(&maintenance).await.unwrap();

    assert_canonical_indexes(&target_paths, 2, 1, 1);
    assert_eq!(restored_index_count(&target_paths).await, 3);
}

#[tokio::test]
async fn host_restore_recovers_an_interrupted_candidate_record() {
    let fixture = verified_host_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = target_paths(&target);
    let staging = restore_staging(&target_paths);
    let staged = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    let record = walk_files(staged.candidate_path().unwrap())
        .into_iter()
        .find(|path| path.to_string_lossy().contains("requests"))
        .unwrap();
    let bytes = std::fs::read(&record).unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let file_name = record.file_name().unwrap().to_string_lossy();
    let partial = record.with_file_name(format!(".{file_name}.{digest}.restore-partial"));
    std::fs::remove_file(&record).unwrap();
    std::fs::write(&partial, &bytes[..bytes.len() / 2]).unwrap();

    let resumed = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging)
        .await
        .unwrap();

    assert!(!partial.exists());
    assert_eq!(std::fs::read(record).unwrap(), bytes);
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    resumed.activate(&maintenance).await.unwrap();
    assert_eq!(restored_sources(&target_paths).await, fixture.sources);
}

#[tokio::test]
async fn host_restore_recovers_complete_or_incomplete_archive_partials() {
    let fixture = verified_host_fixture().await;
    for complete in [false, true] {
        let target = TempDir::new().unwrap();
        let target_paths = target_paths(&target);
        let staging = restore_staging(&target_paths);
        let staged = fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap();
        std::fs::remove_dir_all(staged.candidate_path().unwrap()).unwrap();
        let archive = staging.join("control-host-projection.archive");
        let partial = staging.join("control-host-projection.archive.partial");
        std::fs::rename(&archive, &partial).unwrap();
        if !complete {
            let length = std::fs::metadata(&partial).unwrap().len();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&partial)
                .unwrap()
                .set_len(length / 2)
                .unwrap();
        }

        let resumed = fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap();

        assert!(archive.is_file());
        assert!(!partial.exists());
        assert!(resumed.candidate_path().unwrap().is_dir());
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();
        resumed.activate(&maintenance).await.unwrap();
        assert_eq!(restored_sources(&target_paths).await, fixture.sources);
    }
}

#[tokio::test]
async fn host_restore_recovers_complete_or_incomplete_activation_marker_partials() {
    let fixture = verified_host_fixture().await;
    let donor = TempDir::new().unwrap();
    let donor_paths = target_paths(&donor);
    let donor_staging = restore_staging(&donor_paths);
    let donor_restore = fixture
        .verified
        .stage_clean_restore(donor_paths.installation_state_root(), donor_staging.clone())
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(donor_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    donor_restore.activate(&maintenance).await.unwrap();
    drop(maintenance);
    let marker_bytes =
        std::fs::read(donor_staging.join("control-host-projection.activating.json")).unwrap();

    for complete in [false, true] {
        let target = TempDir::new().unwrap();
        let target_paths = target_paths(&target);
        let staging = restore_staging(&target_paths);
        let staged = fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap();
        let partial = staging.join("control-host-projection.activating.json.partial");
        let bytes = if complete {
            marker_bytes.as_slice()
        } else {
            &marker_bytes[..marker_bytes.len() / 2]
        };
        std::fs::write(&partial, bytes).unwrap();
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();

        staged.activate(&maintenance).await.unwrap();

        assert!(!partial.exists());
        assert_eq!(
            std::fs::read(staging.join("control-host-projection.activating.json")).unwrap(),
            marker_bytes
        );
        assert_eq!(restored_sources(&target_paths).await, fixture.sources);
    }
}

#[tokio::test]
async fn absent_host_restore_creates_no_owner_root() {
    let fixture = verified_absent_host_fixture().await;
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
    assert!(staged.candidate_path().is_none());
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    let result = staged.activate(&maintenance).await.unwrap();

    assert!(matches!(
        result.payload,
        ControlHostProjectionRestoreState::Absent
    ));
    assert!(!live_host_root(&target_paths).exists());
}

#[tokio::test]
async fn host_restore_refuses_any_preexisting_owner_root() {
    let fixture = verified_host_fixture().await;
    for existing in ["empty", "exact"] {
        let target = TempDir::new().unwrap();
        let target_paths = target_paths(&target);
        if existing == "empty" {
            std::fs::create_dir_all(live_host_root(&target_paths)).unwrap();
        } else {
            write_existing_request(&target_paths, &fixture.sources[0]);
        }
        let staged = fixture
            .verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                restore_staging(&target_paths),
            )
            .await
            .unwrap();
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();

        assert_eq!(
            staged.activate(&maintenance).await.unwrap_err().code,
            "use.control_store.host_projection_restore_target_not_empty"
        );
        assert!(staged.candidate_path().unwrap().exists());
    }
}

#[tokio::test]
async fn host_restore_requires_the_exact_target_exclusive_guard() {
    let fixture = verified_host_fixture().await;
    let target = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let target_paths = target_paths(&target);
    let staged = fixture
        .verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            restore_staging(&target_paths),
        )
        .await
        .unwrap();
    let foreign = StateMaintenanceLock::new(other.path())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&foreign).await.unwrap_err().code,
        "use.control_store.host_projection_restore_invalid"
    );
    assert!(!live_host_root(&target_paths).exists());
}
