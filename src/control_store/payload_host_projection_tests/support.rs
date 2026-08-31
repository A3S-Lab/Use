use a3s_use_core::{
    PluginDesiredState, PluginHostPackageState, PluginObservedState, PluginOperationAction,
    PluginOperationPlanEnvelope,
};
use a3s_use_extension::ExtensionPaths;
use tempfile::TempDir;

use super::super::aggregate_tests::fixtures::{
    apply_all_effects, control_installation, operation, operation_at, transition,
};
use super::super::model::{ControlGeneration, ReviewedControlOperation};
use super::super::payload_owner::*;
use super::super::ControlStore;
use crate::cognitive_package::{
    host_projection_snapshot_fixture_sources, write_host_projection_no_change_fixture,
    write_host_projection_snapshot_fixture, HostProjectionSnapshotFixtureOutcome,
};

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

pub(in crate::control_store) fn paths(temporary: &TempDir) -> ExtensionPaths {
    ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap()
}

pub(in crate::control_store) async fn seed_host_projection(
    store: &ControlStore,
    paths: &ExtensionPaths,
) -> Vec<(String, Vec<u8>)> {
    let installation = control_installation();
    let (completed, generation, completed_at_ms) =
        seed_completed_control(store, "host-projection-install").await;
    let completed_state = host_state(&generation, &completed.envelope);
    write_host_projection_snapshot_fixture(
        paths.state_root(),
        &installation,
        completed.envelope.clone(),
        completed_state,
        HostProjectionSnapshotFixtureOutcome::Completed {
            completed_at_ms,
            result_digest: digest('f'),
        },
    )
    .await
    .unwrap();

    let cancelled = operation_at(
        "host-projection-disable",
        PluginOperationAction::Disable,
        1,
        1,
    );
    store.register_operation(cancelled.clone()).await.unwrap();
    store
        .cancel_operation(
            cancelled.operation_id(),
            cancelled.plan_digest(),
            &digest('c'),
            3_000,
        )
        .await
        .unwrap();
    let cancelled_state = host_state(&generation, &cancelled.envelope);
    write_host_projection_snapshot_fixture(
        paths.state_root(),
        &installation,
        cancelled.envelope.clone(),
        cancelled_state,
        HostProjectionSnapshotFixtureOutcome::Cancelled {
            cancelled_at_ms: 3_000,
        },
    )
    .await
    .unwrap();

    host_projection_snapshot_fixture_sources(paths.state_root(), &installation)
        .await
        .unwrap()
}

pub(super) async fn seed_host_no_change(store: &ControlStore, paths: &ExtensionPaths) {
    let installation = control_installation();
    let (completed, generation, _) =
        seed_completed_control(store, "host-projection-no-change-install").await;
    let state = host_state(&generation, &completed.envelope);
    let package_id =
        a3s_use_core::PluginPackageId::parse(completed.root_package_id().to_owned()).unwrap();
    write_host_projection_no_change_fixture(paths.state_root(), &installation, package_id, state)
        .await
        .unwrap();
}

pub(super) async fn seed_host_desired_state_drift(store: &ControlStore, paths: &ExtensionPaths) {
    let installation = control_installation();
    let (completed, generation, completed_at_ms) =
        seed_completed_control(store, "host-projection-drift-install").await;
    let mut state = host_state(&generation, &completed.envelope);
    state.desired = PluginDesiredState::InstalledDisabled;
    state.observed = PluginObservedState::Installed;
    state.validate().unwrap();
    write_host_projection_snapshot_fixture(
        paths.state_root(),
        &installation,
        completed.envelope,
        state,
        HostProjectionSnapshotFixtureOutcome::Completed {
            completed_at_ms,
            result_digest: digest('f'),
        },
    )
    .await
    .unwrap();
}

pub(super) fn remove_operation_indexes(paths: &ExtensionPaths) {
    for scope in host_scope_directories(paths) {
        let operations = scope.join("operations");
        if !operations.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(operations).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::remove_file(path).unwrap();
            }
        }
    }
}

pub(super) fn remove_one_cancellation_alias(paths: &ExtensionPaths) {
    for scope in host_scope_directories(paths) {
        let cancellations = scope.join("cancellations");
        if !cancellations.is_dir() {
            continue;
        }
        let path = std::fs::read_dir(cancellations)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_file())
            .unwrap();
        std::fs::remove_file(path).unwrap();
        return;
    }
    panic!("fixture cancellation alias is missing");
}

#[cfg(unix)]
pub(super) fn first_host_request_path(paths: &ExtensionPaths) -> std::path::PathBuf {
    host_scope_directories(paths)
        .into_iter()
        .flat_map(|scope| {
            std::fs::read_dir(scope.join("requests"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
        })
        .find(|path| path.is_file())
        .unwrap()
}

fn host_scope_directories(paths: &ExtensionPaths) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(paths.state_root().join("plugin-host-manager"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir() && path.file_name().unwrap() != "diagnostics")
        .collect()
}

async fn seed_completed_control(
    store: &ControlStore,
    operation_id: &str,
) -> (ReviewedControlOperation, ControlGeneration, u64) {
    let installation = control_installation();
    let completed = operation(operation_id);
    store.register_operation(completed.clone()).await.unwrap();
    store
        .commit_transition(transition(installation, &completed))
        .await
        .unwrap();
    apply_all_effects(store, &completed, 1_000).await;
    let completed_at_ms = store
        .operation(completed.operation_id())
        .await
        .unwrap()
        .unwrap()
        .completed_at_ms
        .unwrap();
    let generation = store.current_generation().await.unwrap().unwrap();
    (completed, generation, completed_at_ms)
}

fn host_state(
    generation: &ControlGeneration,
    envelope: &PluginOperationPlanEnvelope,
) -> PluginHostPackageState {
    let package = generation
        .snapshot
        .package_selection(&envelope.plan.package_id)
        .unwrap();
    let desired = if package.enabled {
        PluginDesiredState::Enabled
    } else {
        PluginDesiredState::InstalledDisabled
    };
    let state = PluginHostPackageState {
        version: Some(package.package.catalog.record.version.clone()),
        package_generation: Some(package.state_generation),
        package_digest: package.package.catalog.record.package.sha256.clone(),
        manifest_digest: package
            .package
            .catalog
            .record
            .package
            .manifest_sha256
            .clone(),
        receipt_digest: envelope
            .plan
            .state
            .receipt_digest
            .clone()
            .or_else(|| Some(digest('8'))),
        capability_generation: generation.capability.generation,
        capability_revision: generation.capability.descriptor_digest.clone(),
        desired,
        observed: if package.enabled {
            PluginObservedState::Ready
        } else {
            PluginObservedState::Installed
        },
        selected_surfaces: package.selected_surfaces.clone(),
    };
    state.validate().unwrap();
    state
}

pub(in crate::control_store) fn registry() -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(|owner| {
                if owner == ControlPayloadOwnerId::ArtifactStore {
                    ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
                } else {
                    let schema = match owner {
                        ControlPayloadOwnerId::HostProtocolProjection => {
                            CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA.to_owned()
                        }
                        ControlPayloadOwnerId::KnowledgePayload => {
                            CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA.to_owned()
                        }
                        ControlPayloadOwnerId::PlanningAndDiagnosticObservations => {
                            CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA.to_owned()
                        }
                        _ => format!("a3s.use.test.{}-snapshot.v1", owner.as_str()),
                    };
                    ControlPayloadOwnerRegistration::snapshotted(
                        owner,
                        schema,
                        ControlPayloadOwnerLimits::new(128, 32 * 1024 * 1024, 512 * 1024).unwrap(),
                    )
                    .unwrap()
                }
            })
            .collect(),
    )
    .unwrap()
}
