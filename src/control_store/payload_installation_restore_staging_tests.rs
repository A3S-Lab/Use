use std::path::PathBuf;

use tempfile::TempDir;

use super::aggregate_tests::fixtures::{control_installation, operation};
use super::payload_host_projection_tests::support::seed_host_projection_for_completed_operation;
use super::payload_installation_snapshot_tests::{paths, registry, seed_observations};
use super::payload_knowledge_tests::support::seed_control_knowledge;
use super::payload_owner::*;
use super::ControlStore;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;
use crate::plugin_runtime::test_support::{
    artifact, capabilities, context, evidence, policy, service_descriptor,
};
use crate::plugin_runtime::{
    plan_tool_service_release, RuntimeSurfacePlanKey, RuntimeSurfacePlanPublication,
    RuntimeSurfacePlanStore,
};
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation,
};
use a3s_use_core::PluginSurfaceKind;
use a3s_use_extension::StateMaintenanceLock;
use a3s_use_extension::ToolServiceSurface;

#[tokio::test]
async fn complete_restore_stages_every_owner_under_one_exclusive_attempt() {
    let verified = populated_snapshot(10_000).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");

    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();

    assert!(staged.holds_exclusive_fence(&state_root));
    assert!(StateMaintenanceLock::new(&state_root)
        .try_acquire_shared()
        .await
        .unwrap()
        .is_none());
    assert!(staged.control_candidate_path().is_file());
    assert!(staged
        .host_projection_candidate_path()
        .is_some_and(|path| path.is_dir()));
    assert!(staged
        .runtime_plan_candidate_path()
        .is_some_and(|path| path.is_dir()));
    assert!(staged
        .knowledge_candidate_path()
        .is_some_and(|path| path.is_file()));
    assert!(staged
        .observation_candidate_path()
        .is_some_and(|path| path.is_file()));
    assert!(staged
        .restore_coordinator_candidate_path()
        .is_some_and(|path| path.is_dir()));
    for candidate in [
        Some(staged.control_candidate_path()),
        staged.host_projection_candidate_path(),
        staged.runtime_plan_candidate_path(),
        staged.knowledge_candidate_path(),
        staged.observation_candidate_path(),
        staged.restore_coordinator_candidate_path(),
    ]
    .into_iter()
    .flatten()
    {
        assert!(candidate.starts_with(staged.staging_directory()));
    }
    assert!(!state_root.join("control.sqlite3").exists());
    assert!(!state_root.join("plugin-host-manager").exists());
    assert!(!state_root.join("knowledge").exists());
    assert!(!state_root.join("operations").exists());

    let attempt_digest = staged.attempt_digest().to_owned();
    let staging_directory = staged.staging_directory().to_path_buf();
    drop(staged);

    let reopened = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert_eq!(reopened.attempt_digest(), attempt_digest);
    assert_eq!(reopened.staging_directory(), staging_directory);
    assert!(reopened.holds_exclusive_fence(&state_root));
}

#[tokio::test]
async fn complete_restore_stages_absence_without_inventing_owner_candidates() {
    let verified = absent_snapshot(11_000).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");

    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();

    assert!(staged.control_candidate_path().is_file());
    assert!(staged.host_projection_candidate_path().is_none());
    assert!(staged.runtime_plan_candidate_path().is_none());
    assert!(staged.knowledge_candidate_path().is_none());
    assert!(staged.observation_candidate_path().is_none());
    assert!(staged.restore_coordinator_candidate_path().is_none());
    assert!(staged.holds_exclusive_fence(&state_root));
}

#[tokio::test]
async fn complete_restore_rebuilds_an_interrupted_control_candidate() {
    let verified = absent_snapshot(11_100).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let candidate = staged.control_candidate_path().to_path_buf();
    let control_directory = candidate.parent().unwrap().to_path_buf();
    drop(staged);

    std::fs::remove_file(control_directory.join("candidate.json")).unwrap();
    std::fs::rename(
        &candidate,
        control_directory.join("control.sqlite3.partial"),
    )
    .unwrap();

    let reopened = verified
        .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(reopened.control_candidate_path().is_file());
    assert!(!control_directory.join("control.sqlite3.partial").exists());
    assert!(control_directory.join("candidate.json").is_file());
}

#[test]
fn complete_restore_staging_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StagedControlInstallationRestore>();
}

pub(in crate::control_store) async fn absent_snapshot(
    created_at_ms: u64,
) -> VerifiedControlInstallationSnapshot {
    let archive_root = TempDir::new().unwrap();
    absent_snapshot_at(
        archive_root.path().join("absent.complete-snapshot"),
        created_at_ms,
    )
    .await
}

pub(in crate::control_store) async fn absent_snapshot_at(
    archive: PathBuf,
    created_at_ms: u64,
) -> VerifiedControlInstallationSnapshot {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    session
        .snapshot_complete_set(
            archive.clone(),
            OkfKnowledgeStoragePolicy::default(),
            created_at_ms,
        )
        .await
        .unwrap();
    drop(session);

    VerifiedControlInstallationSnapshot::verify_offline(registry, archive)
        .await
        .unwrap()
}

pub(in crate::control_store) async fn populated_snapshot(
    created_at_ms: u64,
) -> VerifiedControlInstallationSnapshot {
    let archive_root = TempDir::new().unwrap();
    populated_snapshot_at(
        archive_root.path().join("populated.complete-snapshot"),
        created_at_ms,
    )
    .await
}

pub(in crate::control_store) async fn populated_snapshot_at(
    archive: PathBuf,
    created_at_ms: u64,
) -> VerifiedControlInstallationSnapshot {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let completed = operation("knowledge-snapshot-operation");
    seed_control_knowledge(&store, &paths).await;
    seed_host_projection_for_completed_operation(&store, &paths, &completed).await;
    seed_observations(&paths, &installation);
    seed_runtime_plan(&paths).await;
    let restore = restore_history_fixture(&installation, 2_000).await;
    write_restore_history_operation(
        &paths.installation_state_root(),
        &restore.plan_digest,
        &restore.completed_operation,
    );
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    session
        .snapshot_complete_set(
            archive.clone(),
            OkfKnowledgeStoragePolicy::default(),
            created_at_ms,
        )
        .await
        .unwrap();
    drop(session);

    VerifiedControlInstallationSnapshot::verify_offline(registry, archive)
        .await
        .unwrap()
}

pub(in crate::control_store) fn candidate_bytes(path: PathBuf) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

async fn seed_runtime_plan(paths: &a3s_use_extension::ExtensionPaths) {
    let descriptor = service_descriptor();
    let surface = ToolServiceSurface {
        release: PathBuf::from("releases/service.json"),
        base_path: "/api".to_owned(),
        contract: None,
    };
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &surface,
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::from_plan(&plan, &provider).unwrap();
    RuntimeSurfacePlanStore::from_extension_paths(paths)
        .put(&key, &plan)
        .await
        .unwrap();
    let publication = RuntimeSurfacePlanPublication::new(key, plan).unwrap();
    assert_eq!(
        RuntimeSurfacePlanStore::from_extension_paths(paths)
            .publish(std::slice::from_ref(&publication))
            .await
            .unwrap()
            .existing,
        1
    );
}
