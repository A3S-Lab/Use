use a3s_use_core::{
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt, UseResult,
};
use a3s_use_extension::ExtensionManifest;

use crate::plugin_runtime::{RuntimeSurfaceObservationSnapshot, RuntimeSurfaceObservedState};

use super::{
    reconcile, reconcile_error, PluginDesiredState, SurfaceObservations, SurfaceObservedState,
    SurfaceReconcileSnapshot,
};

pub(crate) fn reconcile_with_runtime(
    manifest: &ExtensionManifest,
    desired: PluginDesiredState,
    compatible: bool,
    observations: &SurfaceObservations,
    runtime: Option<&RuntimeSurfaceObservationSnapshot>,
) -> UseResult<SurfaceReconcileSnapshot> {
    reconcile_with_runtime_and_knowledge(manifest, desired, compatible, observations, runtime, &[])
}

pub(crate) fn reconcile_with_runtime_and_knowledge(
    manifest: &ExtensionManifest,
    desired: PluginDesiredState,
    compatible: bool,
    observations: &SurfaceObservations,
    runtime: Option<&RuntimeSurfaceObservationSnapshot>,
    knowledge: &[(OkfProjectionReceipt, OkfKnowledgeObservation)],
) -> UseResult<SurfaceReconcileSnapshot> {
    let mut merged = observations.clone();
    if let Some(runtime) = runtime {
        runtime.validate_for_manifest(manifest)?;
        for observation in runtime.surfaces() {
            let state = match observation.state() {
                RuntimeSurfaceObservedState::Unbound => continue,
                RuntimeSurfaceObservedState::Prepared => SurfaceObservedState::Prepared,
                RuntimeSurfaceObservedState::Starting => SurfaceObservedState::Starting,
                RuntimeSurfaceObservedState::Healthy => SurfaceObservedState::Healthy,
                RuntimeSurfaceObservedState::Draining => SurfaceObservedState::Draining,
                RuntimeSurfaceObservedState::Stopped => SurfaceObservedState::Stopped,
                RuntimeSurfaceObservedState::Failed
                | RuntimeSurfaceObservedState::Missing
                | RuntimeSurfaceObservedState::Stale => SurfaceObservedState::Failed,
            };
            if merged
                .insert(observation.surface().clone(), state)
                .is_some()
            {
                return Err(reconcile_error(
                    "Two host adapters reported the same plugin surface.",
                ));
            }
        }
    }
    for (receipt, observation) in knowledge {
        observation.validate_for_receipt(receipt)?;
        let surface = &receipt.surface.surface;
        let manifest_surface = manifest
            .okf
            .iter()
            .find(|candidate| candidate.id == surface.id);
        if receipt.surface.package_id != manifest.package_id
            || !manifest_surface.is_some_and(|candidate| candidate.bundle == receipt.bundle)
        {
            return Err(reconcile_error(
                "An OKF Knowledge observation does not match the installed manifest generation.",
            ));
        }
        let state = match observation.state {
            OkfKnowledgeObservedState::Failed => SurfaceObservedState::Failed,
            OkfKnowledgeObservedState::Promoted => SurfaceObservedState::Healthy,
            OkfKnowledgeObservedState::Removed => SurfaceObservedState::Stopped,
            OkfKnowledgeObservedState::Staged => SurfaceObservedState::Prepared,
        };
        if merged.insert(surface.clone(), state).is_some() {
            return Err(reconcile_error(
                "Two host adapters reported the same plugin surface.",
            ));
        }
    }
    reconcile(manifest, desired, compatible, &merged)
}
