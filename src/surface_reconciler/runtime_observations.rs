use a3s_use_core::UseResult;
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
    reconcile(manifest, desired, compatible, &merged)
}
