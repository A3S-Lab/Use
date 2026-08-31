use std::path::Path;

use a3s_use_core::{InstallationId, UseResult};
use tokio::fs;

use super::filesystem::{host_io, scan_host_projection_snapshot};
use super::host_snapshot_invalid;

#[cfg(test)]
pub(super) async fn write_fixture(
    state_root: &Path,
    installation: &InstallationId,
    envelope: a3s_use_core::PluginOperationPlanEnvelope,
    state: a3s_use_core::PluginHostPackageState,
    outcome: super::HostProjectionSnapshotFixtureOutcome,
) -> UseResult<()> {
    use a3s_use_core::{
        PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
        PluginHostEnablementPlanStatus, PluginHostPlanRequest, PluginHostPlanResult,
        PluginOperationAction, PluginPackageId, PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA,
        PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA, PLUGIN_HOST_PLAN_REQUEST_SCHEMA,
        PLUGIN_HOST_PLAN_RESULT_SCHEMA,
    };

    use crate::cognitive_package::host_store::{
        PluginHostProtocolStore, StoredPluginHostCancellation, StoredPluginHostOutcome,
        StoredPluginHostPlan, StoredPluginHostRequest,
    };

    installation.validate()?;
    envelope.validate()?;
    state.validate()?;
    let scope = fixture_scope(installation)?;
    let request_id = format!("request:{}", envelope.plan.operation_id);
    let capabilities_digest = format!("sha256:{}", "b".repeat(64));
    let package_id = PluginPackageId::parse(envelope.plan.package_id.clone())?;
    let stored_plan = match envelope.plan.action {
        PluginOperationAction::Enable | PluginOperationAction::Disable => {
            let enabled = envelope.plan.action == PluginOperationAction::Enable;
            let expected_package_generation = state.package_generation.ok_or_else(|| {
                host_snapshot_invalid("An enablement fixture requires installed package state.")
            })?;
            let request = PluginHostEnablementPlanRequest {
                schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
                request_id: request_id.clone(),
                assignment_generation: 1,
                capabilities_digest: capabilities_digest.clone(),
                scope: scope.clone(),
                package_id: package_id.clone(),
                expected_package_generation,
                enabled,
            };
            let result = PluginHostEnablementPlanResult {
                schema: PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA.to_owned(),
                request_id: request_id.clone(),
                assignment_generation: 1,
                capabilities_digest: capabilities_digest.clone(),
                scope: scope.clone(),
                package_id: package_id.clone(),
                expected_package_generation,
                enabled,
                planned_at_ms: envelope.plan.created_at_ms,
                status: PluginHostEnablementPlanStatus::Planned,
                state: state.clone(),
                plan: Some(envelope.clone()),
                replayed: false,
            };
            StoredPluginHostPlan::enablement(request, result)?
        }
        PluginOperationAction::Install | PluginOperationAction::Upgrade => {
            let package_lock = envelope.package_lock.clone().ok_or_else(|| {
                host_snapshot_invalid("A graph fixture requires its candidate package lock.")
            })?;
            let candidate = package_lock
                .package(package_id.as_str())
                .map(|package| package.catalog.clone())
                .ok_or_else(|| host_snapshot_invalid("A graph fixture root is missing."))?;
            let selected_surfaces = candidate
                .record
                .resolve_surfaces(&[])?
                .into_iter()
                .map(|surface| surface.reference())
                .collect();
            let request = PluginHostPlanRequest {
                schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
                request_id: request_id.clone(),
                assignment_generation: 1,
                capabilities_digest: capabilities_digest.clone(),
                scope: scope.clone(),
                action: envelope.plan.action,
                package_id: package_id.clone(),
                candidate: Some(candidate),
                package_lock: Some(package_lock),
                selected_surfaces,
            };
            let result = PluginHostPlanResult {
                schema: PLUGIN_HOST_PLAN_RESULT_SCHEMA.to_owned(),
                request_id: request_id.clone(),
                assignment_generation: 1,
                capabilities_digest: capabilities_digest.clone(),
                scope: scope.clone(),
                package_id: package_id.clone(),
                plan: envelope.clone(),
                replayed: false,
            };
            StoredPluginHostPlan::graph(request, result)?
        }
        PluginOperationAction::Uninstall => {
            return Err(host_snapshot_invalid(
                "The Host projection fixture does not need an uninstall branch.",
            ))
        }
    };
    let record = StoredPluginHostRequest::new(stored_plan)?;
    let store = PluginHostProtocolStore::new(state_root.to_path_buf());
    store.put_plan(&record).await?;
    drop(store.lock_request(&scope, &request_id).await?);
    drop(
        store
            .lock_operation(&scope, &envelope.plan.operation_id)
            .await?,
    );
    match outcome {
        super::HostProjectionSnapshotFixtureOutcome::Completed {
            completed_at_ms,
            result_digest,
        } => {
            let outcome = StoredPluginHostOutcome::new(completed_at_ms, result_digest, state)?;
            store.put_outcome(&record, outcome).await?;
        }
        super::HostProjectionSnapshotFixtureOutcome::Cancelled { cancelled_at_ms } => {
            let cancellation = StoredPluginHostCancellation::new(
                request_id,
                envelope.plan.operation_id,
                envelope.plan_digest,
                cancelled_at_ms,
            )?;
            store.put_cancellation(&scope, &cancellation).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn write_no_change_fixture(
    state_root: &Path,
    installation: &InstallationId,
    package_id: a3s_use_core::PluginPackageId,
    state: a3s_use_core::PluginHostPackageState,
) -> UseResult<()> {
    use a3s_use_core::{
        PluginDesiredState, PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
        PluginHostEnablementPlanStatus, PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA,
        PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA,
    };

    use crate::cognitive_package::host_store::{
        PluginHostProtocolStore, StoredPluginHostPlan, StoredPluginHostRequest,
    };

    installation.validate()?;
    state.validate()?;
    let enabled = match state.desired {
        PluginDesiredState::Enabled => true,
        PluginDesiredState::InstalledDisabled => false,
        PluginDesiredState::Absent => {
            return Err(host_snapshot_invalid(
                "A no-change fixture requires an installed package.",
            ))
        }
    };
    let expected_package_generation = state.package_generation.ok_or_else(|| {
        host_snapshot_invalid("A no-change fixture requires a package generation.")
    })?;
    let scope = fixture_scope(installation)?;
    let request_id = "request:no-change".to_owned();
    let capabilities_digest = format!("sha256:{}", "b".repeat(64));
    let request = PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: request_id.clone(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        package_id: package_id.clone(),
        expected_package_generation,
        enabled,
    };
    let result = PluginHostEnablementPlanResult {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA.to_owned(),
        request_id: request_id.clone(),
        assignment_generation: 1,
        capabilities_digest,
        scope: scope.clone(),
        package_id,
        expected_package_generation,
        enabled,
        planned_at_ms: 5_000,
        status: PluginHostEnablementPlanStatus::NoChange,
        state,
        plan: None,
        replayed: false,
    };
    let record = StoredPluginHostRequest::new(StoredPluginHostPlan::enablement(request, result)?)?;
    let store = PluginHostProtocolStore::new(state_root.to_path_buf());
    store.put_plan(&record).await?;
    drop(store.lock_request(&scope, &request_id).await?);
    Ok(())
}

#[cfg(test)]
fn fixture_scope(installation: &InstallationId) -> UseResult<a3s_use_core::PluginManagedScope> {
    let scope = a3s_use_core::PluginManagedScope {
        schema: a3s_use_core::PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:snapshot-fixture".to_owned(),
        scope_kind: installation.kind,
        scope_id: installation.id.clone(),
        authority_id: "authority:snapshot-fixture".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "a".repeat(64)),
    };
    scope.validate()?;
    Ok(scope)
}

#[cfg(test)]
pub(super) async fn fixture_sources(
    state_root: &Path,
    installation: &InstallationId,
) -> UseResult<Vec<(String, Vec<u8>)>> {
    let inventory =
        scan_host_projection_snapshot(state_root, installation, 128, 32 * 1024 * 1024).await?;
    let mut result = Vec::with_capacity(inventory.sources.len());
    for source in inventory.sources {
        let bytes = fs::read(&source.source)
            .await
            .map_err(|error| host_io("read Host fixture record", error))?;
        result.push((source.logical_path, bytes));
    }
    Ok(result)
}
