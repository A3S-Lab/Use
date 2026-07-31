use a3s_use_core::UseResult;
use a3s_use_extension::ExtensionManifest;

use crate::plugin_runtime::{RuntimeSurfaceObservationSnapshot, RuntimeSurfaceObservedState};

use super::{
    reconcile, reconcile_error, reconcile_scoped, PluginDesiredState, SurfaceObservations,
    SurfaceObservedState, SurfaceReconcileSnapshot,
};

pub(crate) fn reconcile_with_runtime(
    manifest: &ExtensionManifest,
    desired: PluginDesiredState,
    compatible: bool,
    observations: &SurfaceObservations,
    runtime: Option<&RuntimeSurfaceObservationSnapshot>,
) -> UseResult<SurfaceReconcileSnapshot> {
    reconcile_with_runtime_mode(
        manifest,
        desired,
        compatible,
        observations,
        runtime,
        reconcile,
    )
}

pub(crate) fn reconcile_scoped_with_runtime(
    manifest: &ExtensionManifest,
    desired: PluginDesiredState,
    compatible: bool,
    observations: &SurfaceObservations,
    runtime: Option<&RuntimeSurfaceObservationSnapshot>,
) -> UseResult<SurfaceReconcileSnapshot> {
    reconcile_with_runtime_mode(
        manifest,
        desired,
        compatible,
        observations,
        runtime,
        reconcile_scoped,
    )
}

fn reconcile_with_runtime_mode(
    manifest: &ExtensionManifest,
    desired: PluginDesiredState,
    compatible: bool,
    observations: &SurfaceObservations,
    runtime: Option<&RuntimeSurfaceObservationSnapshot>,
    reconcile: fn(
        &ExtensionManifest,
        PluginDesiredState,
        bool,
        &SurfaceObservations,
    ) -> UseResult<SurfaceReconcileSnapshot>,
) -> UseResult<SurfaceReconcileSnapshot> {
    let mut merged = observations.clone();
    if let Some(runtime) = runtime {
        runtime.validate_for_manifest(manifest)?;
        for observation in runtime.surfaces() {
            if merged.contains_key(observation.surface()) {
                return Err(reconcile_error(
                    "Two host adapters reported the same plugin surface.",
                ));
            }
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
            merged.insert(observation.surface().clone(), state);
        }
    }
    reconcile(manifest, desired, compatible, &merged)
}
