use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginDesiredState, PluginHostPackageState, UseResult};

use super::host_projection_error;
use crate::cognitive_package::{HostProjectionSnapshotRecord, HostProjectionSnapshotRequest};
use crate::control_store::export::VerifiedControlStoreExport;
use crate::control_store::model::{
    ControlGeneration, ControlOperationRecord, ControlOperationStatus,
};

/// Require every durable Host protocol claim to be derivable from the exact
/// canonical Control history. Host observations may retain receipt and health
/// evidence, but they cannot select package identity, desired state, surfaces,
/// package generation, or capability generation.
pub(super) fn reconcile(
    verified: &VerifiedControlStoreExport,
    records: &[HostProjectionSnapshotRecord],
) -> UseResult<()> {
    let operations = verified
        .export
        .authority
        .operations
        .iter()
        .map(|operation| (operation.reviewed.operation_id(), operation))
        .collect::<BTreeMap<_, _>>();
    let cancellations = records
        .iter()
        .filter_map(|record| match record {
            HostProjectionSnapshotRecord::Cancellation(cancellation) => Some((
                cancellation.operation_id.as_str(),
                cancellation.plan_digest.as_str(),
            )),
            HostProjectionSnapshotRecord::Request(_) => None,
        })
        .collect::<BTreeSet<_>>();
    let mut bound_operations = BTreeSet::new();

    for record in records {
        match record {
            HostProjectionSnapshotRecord::Request(request) => {
                reconcile_request(
                    verified,
                    &operations,
                    &cancellations,
                    request,
                    &mut bound_operations,
                )?;
            }
            HostProjectionSnapshotRecord::Cancellation(cancellation) => {
                let operation = operations
                    .get(cancellation.operation_id.as_str())
                    .copied()
                    .ok_or_else(|| {
                        host_projection_error(
                            "A Host cancellation has no reviewed Control operation.",
                        )
                    })?;
                if operation.status != ControlOperationStatus::Cancelled
                    || operation.reviewed.plan_digest() != cancellation.plan_digest
                    || operation.completed_at_ms != Some(cancellation.cancelled_at_ms)
                {
                    return Err(host_projection_error(
                        "A Host cancellation disagrees with exact Control cancellation history.",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn reconcile_request<'a>(
    verified: &'a VerifiedControlStoreExport,
    operations: &BTreeMap<&'a str, &'a ControlOperationRecord>,
    cancellations: &BTreeSet<(&str, &str)>,
    request: &HostProjectionSnapshotRequest,
    bound_operations: &mut BTreeSet<String>,
) -> UseResult<()> {
    let Some(envelope) = &request.envelope else {
        if request.outcome.is_some()
            || request.reviewed_state.is_none()
            || request.expected_package_generation.is_none()
        {
            return Err(host_projection_error(
                "A no-change Host request has invalid observation evidence.",
            ));
        }
        let state = request.reviewed_state.as_ref().ok_or_else(|| {
            host_projection_error("A no-change Host request has no reviewed state.")
        })?;
        if state.package_generation != request.expected_package_generation
            || !verified
                .export
                .authority
                .generations
                .iter()
                .any(|generation| state_matches_generation(state, &request.package_id, generation))
        {
            return Err(host_projection_error(
                "A no-change Host observation is not derivable from Control history.",
            ));
        }
        return Ok(());
    };

    let operation_id = envelope.plan.operation_id.as_str();
    if !bound_operations.insert(operation_id.to_owned()) {
        return Err(host_projection_error(
            "More than one Host request claims the same Control operation.",
        ));
    }
    let operation = operations.get(operation_id).copied().ok_or_else(|| {
        host_projection_error("A Host request has no reviewed Control operation.")
    })?;
    if operation.reviewed.envelope != *envelope
        || request.package_id != envelope.plan.package_id
        || request.operation_binding()
            != Some((
                operation.reviewed.operation_id(),
                operation.reviewed.plan_digest(),
            ))
    {
        return Err(host_projection_error(
            "A Host request does not bind the exact reviewed Control plan.",
        ));
    }

    if let Some(state) = &request.reviewed_state {
        let expected_generation = request.expected_package_generation.ok_or_else(|| {
            host_projection_error("A Host enablement request lost its package generation.")
        })?;
        if state.package_generation != Some(expected_generation)
            || operation.reviewed.expected_generation == 0
            || !generation_by_number(verified, operation.reviewed.expected_generation).is_some_and(
                |generation| state_matches_generation(state, &request.package_id, generation),
            )
        {
            return Err(host_projection_error(
                "A Host enablement observation disagrees with its prior Control generation.",
            ));
        }
    } else if request.expected_package_generation.is_some() {
        return Err(host_projection_error(
            "A graph Host request carries enablement generation evidence.",
        ));
    }

    let cancelled = cancellations.contains(&(operation_id, operation.reviewed.plan_digest()));
    match (&request.outcome, operation.status) {
        (Some(outcome), ControlOperationStatus::Completed) => {
            if cancelled
                || operation.completed_at_ms != Some(outcome.completed_at_ms)
                || operation.result_digest.as_deref()
                    != Some(outcome.operation_result_digest.as_str())
            {
                return Err(host_projection_error(
                    "A Host outcome disagrees with exact Control completion evidence.",
                ));
            }
            let target_generation = operation.reviewed.target_generation()?;
            let generation =
                generation_by_number(verified, target_generation).ok_or_else(|| {
                    host_projection_error("A completed Host outcome has no Control generation.")
                })?;
            if generation.operation_id != operation_id
                || !state_matches_generation(&outcome.state, &request.package_id, generation)
            {
                return Err(host_projection_error(
                    "A Host outcome attempts to select state outside its Control generation.",
                ));
            }
        }
        (None, ControlOperationStatus::Cancelled) if cancelled => {}
        (None, ControlOperationStatus::Reviewed | ControlOperationStatus::EffectsPending)
            if !cancelled => {}
        (None, ControlOperationStatus::Rejected) if !cancelled => {}
        _ => {
            return Err(host_projection_error(
                "Host request terminal evidence disagrees with its Control operation status.",
            ))
        }
    }
    Ok(())
}

fn generation_by_number(
    verified: &VerifiedControlStoreExport,
    generation: u64,
) -> Option<&ControlGeneration> {
    generation
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| verified.export.authority.generations.get(index))
        .filter(|candidate| candidate.snapshot.generation == generation)
}

fn state_matches_generation(
    state: &PluginHostPackageState,
    package_id: &str,
    generation: &ControlGeneration,
) -> bool {
    if state.capability_generation != generation.capability.generation
        || state.capability_revision != generation.capability.descriptor_digest
    {
        return false;
    }
    let Some(package) = generation.snapshot.package_selection(package_id) else {
        return state.version.is_none()
            && state.package_generation.is_none()
            && state.package_digest.is_none()
            && state.manifest_digest.is_none()
            && state.receipt_digest.is_none()
            && state.desired == PluginDesiredState::Absent
            && state.selected_surfaces.is_empty();
    };
    let desired = if package.enabled {
        PluginDesiredState::Enabled
    } else {
        PluginDesiredState::InstalledDisabled
    };
    state.version.as_deref() == Some(package.package.catalog.record.version.as_str())
        && state.package_generation == Some(package.state_generation)
        && state.package_digest == package.package.catalog.record.package.sha256
        && state.manifest_digest == package.package.catalog.record.package.manifest_sha256
        && state.desired == desired
        && state.selected_surfaces == package.selected_surfaces
}
