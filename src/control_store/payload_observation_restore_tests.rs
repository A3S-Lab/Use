use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_knowledge_tests::support::paths;
use super::payload_owner::*;

pub(in crate::control_store) mod support;

use support::*;

#[test]
fn observation_restore_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlObservationPayloadRestoreResult>();
    assert_send_sync::<StagedControlObservationPayloadRestore>();
}

#[tokio::test]
async fn verified_observation_restore_stages_then_activates_exact_terminal_records() {
    let fixture = verified_observation_fixture().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, fixture.installation.clone());
    let staging = restore_staging(&target_paths);
    let staged = fixture
        .verified
        .stage_clean_restore(target_paths.installation_state_root(), staging)
        .await
        .unwrap();

    assert!(staged.candidate_path().unwrap().is_file());
    for (path, _) in fixture.terminal_records() {
        assert!(!target_paths
            .installation_state_root()
            .join("operations")
            .join(path)
            .exists());
    }

    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let first = staged.activate(&maintenance).await.unwrap();
    first.validate(&fixture.registry).unwrap();
    first
        .validate_for_snapshot(&fixture.registry, &fixture.snapshot)
        .unwrap();
    assert!(matches!(
        first.payload,
        ControlObservationPayloadRestoreState::Archive {
            terminal_records: 2,
            ..
        }
    ));
    assert!(!serde_json::to_string(&first)
        .unwrap()
        .contains(&target.path().display().to_string()));

    for (path, bytes) in fixture.terminal_records() {
        assert_eq!(
            std::fs::read(
                target_paths
                    .installation_state_root()
                    .join("operations")
                    .join(path)
            )
            .unwrap()
            .as_slice(),
            bytes.as_slice()
        );
    }
    assert!(!target_paths
        .installation_state_root()
        .join("operations")
        .join(fixture.active_record().0.as_str())
        .exists());

    let replay = staged.activate(&maintenance).await.unwrap();
    assert_eq!(replay, first);
    let mut tampered = first;
    let ControlObservationPayloadRestoreState::Archive {
        terminal_records, ..
    } = &mut tampered.payload
    else {
        unreachable!();
    };
    *terminal_records += 1;
    assert_eq!(
        tampered
            .validate_for_snapshot(&fixture.registry, &fixture.snapshot)
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_restore_invalid"
    );
}

#[tokio::test]
async fn observation_restore_replays_only_after_activation_has_started() {
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
    let candidate = staged.candidate_path().unwrap().to_path_buf();
    let activating = candidate.with_file_name("control-observations.archive.activating");
    std::fs::rename(&candidate, &activating).unwrap();
    let first = fixture.terminal_records().next().unwrap();
    write_fixture(&target_paths.installation_state_root(), first);

    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let result = staged.activate(&maintenance).await.unwrap();
    result
        .validate_for_snapshot(&fixture.registry, &fixture.snapshot)
        .unwrap();
    for (path, bytes) in fixture.terminal_records() {
        assert_eq!(
            std::fs::read(
                target_paths
                    .installation_state_root()
                    .join("operations")
                    .join(path)
            )
            .unwrap()
            .as_slice(),
            bytes.as_slice()
        );
    }
}

#[tokio::test]
async fn observation_restore_recovers_deterministic_record_partial_boundaries() {
    let fixture = verified_observation_fixture().await;
    let first_entry = &fixture.snapshot.manifest.entries[0];
    let first_record = fixture
        .terminal_records()
        .find(|(path, _)| path == &first_entry.path)
        .unwrap();
    for boundary in ["incomplete", "complete", "published"] {
        let target = TempDir::new().unwrap();
        let target_paths = paths(&target, fixture.installation.clone());
        let staging = restore_staging(&target_paths);
        let staged = fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap();
        let candidate = staged.candidate_path().unwrap().to_path_buf();
        std::fs::rename(
            &candidate,
            candidate.with_file_name("control-observations.archive.activating"),
        )
        .unwrap();
        let digest = first_entry.sha256.strip_prefix("sha256:").unwrap();
        let partial = staging.join(format!("record-0000000000-{digest}.partial"));
        match boundary {
            "incomplete" => {
                std::fs::write(&partial, &first_record.1[..first_record.1.len() / 2]).unwrap();
            }
            "complete" => std::fs::write(&partial, &first_record.1).unwrap(),
            "published" => {
                std::fs::write(&partial, &first_record.1).unwrap();
                write_fixture(&target_paths.installation_state_root(), first_record);
            }
            _ => unreachable!(),
        }

        let resumed = fixture
            .verified
            .stage_clean_restore(target_paths.installation_state_root(), staging.clone())
            .await
            .unwrap();
        let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
            .acquire_exclusive()
            .await
            .unwrap();
        resumed.activate(&maintenance).await.unwrap();
        assert!(!partial.exists(), "boundary {boundary} left a partial");
        for (path, bytes) in fixture.terminal_records() {
            assert_eq!(
                std::fs::read(
                    target_paths
                        .installation_state_root()
                        .join("operations")
                        .join(path)
                )
                .unwrap()
                .as_slice(),
                bytes.as_slice(),
                "boundary {boundary} restored different bytes"
            );
        }
    }
}

#[tokio::test]
async fn observation_restore_refuses_existing_terminal_or_active_records_before_activation() {
    let fixture = verified_observation_fixture().await;
    for existing in [
        fixture.terminal_records().next().unwrap(),
        fixture.active_record(),
    ] {
        let target = TempDir::new().unwrap();
        let target_paths = paths(&target, fixture.installation.clone());
        write_fixture(&target_paths.installation_state_root(), existing);
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
            "use.control_store.observation_payload_restore_target_not_empty"
        );
        assert_eq!(
            std::fs::read(
                target_paths
                    .installation_state_root()
                    .join("operations")
                    .join(existing.0.as_str())
            )
            .unwrap(),
            existing.1
        );
        assert!(staged.candidate_path().unwrap().is_file());
    }
}

#[tokio::test]
async fn absent_observation_restore_creates_no_payload_records() {
    let fixture = verified_absent_observation_fixture().await;
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
    assert!(staged.candidate_path().is_none());
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let result = staged.activate(&maintenance).await.unwrap();

    assert!(matches!(
        result.payload,
        ControlObservationPayloadRestoreState::Absent
    ));
    assert!(!target_paths
        .installation_state_root()
        .join("operations/package-diagnostic-history")
        .exists());
    assert!(!target_paths
        .installation_state_root()
        .join("operations/package-resolutions")
        .exists());
    assert!(!target_paths
        .installation_state_root()
        .join("operations/package-downloads")
        .exists());
}

#[tokio::test]
async fn observation_restore_requires_the_exact_target_exclusive_guard() {
    let fixture = verified_observation_fixture().await;
    let target = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let target_paths = paths(&target, fixture.installation.clone());
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
        "use.control_store.observation_payload_restore_invalid"
    );
    for (path, _) in fixture.terminal_records() {
        assert!(!target_paths
            .installation_state_root()
            .join("operations")
            .join(path)
            .exists());
    }
}
