//! Shared helpers for CognitivePackageHostManager.

use a3s_use_core::{
    PlanPackageRole, PluginDesiredState, PluginHostApplyRequest, PluginHostApplyResult,
    PluginHostCapabilities, PluginHostEnablementPlanRequest, PluginHostPackageState,
    PluginHostPlanRequest, PluginObservedState, PluginOperationAction, PluginOperationPlanEnvelope,
    PluginPackageLock, PluginSurfaceRef, UseError, UseResult, VerifiedPluginCatalogRecord,
    PLUGIN_HOST_APPLY_RESULT_SCHEMA,
};
use a3s_use_extension::TrustedRegistry;
use serde::Serialize;

use super::super::host_store::{
    digest_value, StoredPluginHostOutcome, StoredPluginHostPlan, StoredPluginHostRequest,
};
use super::super::registry_access::RegistryAccess;
use super::super::{CognitivePackageEnablementRequest, CognitiveRegistryAccess};
use super::HOST_OPERATION_OUTCOME_SCHEMA;
pub(crate) struct AppliedOutcome {
    pub(crate) completed_at_ms: u64,
    pub(crate) operation_result_digest: String,
    pub(crate) state: PluginHostPackageState,
    pub(crate) replayed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphOperationOutcome<'a> {
    pub(crate) schema: &'a str,
    pub(crate) operation_id: &'a str,
    pub(crate) plan_digest: &'a str,
    pub(crate) completed_at_ms: u64,
    pub(crate) state: &'a PluginHostPackageState,
}

pub(crate) fn graph_outcome_digest(
    envelope: &PluginOperationPlanEnvelope,
    completed_at_ms: u64,
    state: &PluginHostPackageState,
) -> UseResult<String> {
    digest_value(&GraphOperationOutcome {
        schema: HOST_OPERATION_OUTCOME_SCHEMA,
        operation_id: &envelope.plan.operation_id,
        plan_digest: &envelope.plan_digest,
        completed_at_ms,
        state,
    })
}

pub(crate) fn apply_result(
    request: &PluginHostApplyRequest,
    outcome: &StoredPluginHostOutcome,
    replayed: bool,
    capabilities: &PluginHostCapabilities,
) -> UseResult<PluginHostApplyResult> {
    let result = PluginHostApplyResult {
        schema: PLUGIN_HOST_APPLY_RESULT_SCHEMA.to_string(),
        request_id: request.request_id.clone(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: request.operation_id.clone(),
        plan_digest: request.plan_digest.clone(),
        completed_at_ms: outcome.completed_at_ms,
        operation_result_digest: outcome.operation_result_digest.clone(),
        state: outcome.state.clone(),
        replayed,
    };
    result.validate_for(request, capabilities)?;
    Ok(result)
}

pub(crate) fn require_request_lock(request: &PluginHostPlanRequest) -> UseResult<&PluginPackageLock> {
    request.package_lock.as_ref().ok_or_else(|| {
        host_error(
            "use.plugin.host_package_lock_required",
            "The cognitive-package Host adapter requires the exact resolved package lock for every graph operation.",
        )
    })
}

pub(crate) fn cognitive_enablement_request(
    request: &PluginHostEnablementPlanRequest,
    envelope: &PluginOperationPlanEnvelope,
) -> UseResult<CognitivePackageEnablementRequest> {
    CognitivePackageEnablementRequest::new(
        envelope.plan.operation_id.clone(),
        request.package_id.to_string(),
        request.expected_package_generation,
        request.enabled,
    )
}

pub(crate) fn verify_registry_provenance(
    registry: &TrustedRegistry,
    candidate: &VerifiedPluginCatalogRecord,
) -> UseResult<()> {
    let provenance = &candidate.provenance;
    if !registry.matches_provenance(provenance) {
        return Err(host_error(
            "use.plugin.host_registry_provenance_mismatch",
            "The configured Registry source no longer matches the reviewed catalog provenance.",
        ));
    }
    Ok(())
}

pub(crate) fn validate_state_for_plan(
    state: &PluginHostPackageState,
    envelope: &PluginOperationPlanEnvelope,
) -> UseResult<()> {
    state.validate()?;
    envelope.validate()?;
    let root = envelope
        .plan
        .packages
        .iter()
        .find(|package| package.role == PlanPackageRole::Root)
        .ok_or_else(|| {
            host_error(
                "use.plugin.host_operation_result_mismatch",
                "The reviewed plan omitted its root package transition.",
            )
        })?;
    if envelope.plan.action == PluginOperationAction::Uninstall {
        if state.desired == PluginDesiredState::Absent
            && state.observed == PluginObservedState::Removed
            && state.version.is_none()
            && state.selected_surfaces.is_empty()
        {
            return Ok(());
        }
        return Err(host_error(
            "use.plugin.host_operation_result_mismatch",
            "The uninstall outcome did not remove the reviewed root package.",
        ));
    }
    let expected = root.after.as_ref().ok_or_else(|| {
        host_error(
            "use.plugin.host_operation_result_mismatch",
            "The reviewed operation has no expected root package state.",
        )
    })?;
    let selected_surfaces = expected
        .release
        .surfaces
        .iter()
        .map(a3s_use_core::CatalogSurface::reference)
        .collect::<Vec<PluginSurfaceRef>>();
    let desired = match envelope.plan.action {
        PluginOperationAction::Install | PluginOperationAction::Upgrade => {
            PluginDesiredState::Enabled
        }
        PluginOperationAction::Enable => PluginDesiredState::Enabled,
        PluginOperationAction::Disable => PluginDesiredState::InstalledDisabled,
        PluginOperationAction::Uninstall => {
            return Err(host_error(
                "use.plugin.host_operation_result_mismatch",
                "The uninstall outcome retained an unexpected root package state.",
            ))
        }
    };
    if state.version.as_deref() != Some(expected.release.version.as_str())
        || state.package_digest.as_deref() != Some(expected.release.package_sha256.as_str())
        || state.manifest_digest.as_deref() != Some(expected.release.manifest_sha256.as_str())
        || state.selected_surfaces != selected_surfaces
        || state.desired != desired
    {
        return Err(host_error(
            "use.plugin.host_operation_result_mismatch",
            "The observed package state does not match the exact reviewed operation plan.",
        ));
    }
    Ok(())
}

pub(crate) fn package_outcome_matches(
    current: &PluginHostPackageState,
    completed: &PluginHostPackageState,
) -> bool {
    current.version == completed.version
        && current.package_generation == completed.package_generation
        && current.package_digest == completed.package_digest
        && current.manifest_digest == completed.manifest_digest
        && current.receipt_digest == completed.receipt_digest
        && current.desired == completed.desired
        && current.observed == completed.observed
        && current.selected_surfaces == completed.selected_surfaces
}

pub(crate) const fn registry_access(access: CognitiveRegistryAccess) -> RegistryAccess {
    match access {
        CognitiveRegistryAccess::Refreshed => RegistryAccess::Refreshed,
        CognitiveRegistryAccess::Cached => RegistryAccess::Cached,
    }
}

pub(crate) fn host_operation_observation_mismatch(message: impl Into<String>) -> UseError {
    host_error("use.plugin.host_operation_observation_mismatch", message)
}

pub(crate) fn enablement_operation_id(request: &PluginHostEnablementPlanRequest) -> UseResult<String> {
    let digest = request.descriptor_digest()?;
    let digest = digest.strip_prefix("sha256:").ok_or_else(|| {
        host_error(
            "use.plugin.host_enablement_plan_invalid",
            "The enablement request digest has an invalid encoding.",
        )
    })?;
    let action = if request.enabled { "enable" } else { "disable" };
    Ok(format!("{action}:host:{}", &digest[..32]))
}

pub(crate) fn host_store_conflict() -> UseError {
    host_error(
        "use.plugin.host_store_conflict",
        "The Host request ID already owns a different operation kind or request body.",
    )
}

pub(crate) fn host_outcome_stale() -> UseError {
    host_error(
        "use.plugin.host_outcome_stale",
        "The completed Host outcome no longer matches the current Use-owned package state.",
    )
}

pub(crate) fn host_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

