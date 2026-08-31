use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginSurfaceRef, UseResult};
use a3s_use_extension::{ExtensionManifest, ManifestPluginSurface};

use crate::surface_graph::{schedule_surface_graph, SurfaceGraphInput};

use super::model::{
    checkpoint_domain, checkpoint_key, lifecycle_error, PluginLifecycleAction,
    PluginLifecycleCheckpoint, PluginLifecycleCheckpointKind, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginLifecycleSurface, PluginSurfaceHost,
    PLUGIN_LIFECYCLE_INTENT_SCHEMA,
};

impl PluginLifecycleIntent {
    pub fn from_manifest(
        spec: PluginLifecycleIntentSpec,
        manifest: &ExtensionManifest,
    ) -> UseResult<Self> {
        let selected = manifest
            .plugin_surfaces()?
            .into_iter()
            .map(|surface| surface.surface)
            .collect::<Vec<_>>();
        Self::from_manifest_selection(spec, manifest, &selected)
    }

    /// Build the lifecycle schedule for the exact surface set bound by the
    /// immutable operation plan.
    pub fn from_manifest_selection(
        spec: PluginLifecycleIntentSpec,
        manifest: &ExtensionManifest,
        selected_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<Self> {
        if spec.package_id != manifest.package_id {
            return Err(lifecycle_error(
                "The lifecycle package identity does not match the admitted manifest.",
            ));
        }
        let surfaces = lifecycle_surfaces(manifest.plugin_surfaces()?, selected_surfaces)?;
        let checkpoint_domain = checkpoint_domain(
            &spec.operation_id,
            &spec.plan_digest,
            &spec.scope,
            &spec.package_id,
            spec.generation,
            spec.action,
        );
        let checkpoints = checkpoints(&checkpoint_domain, spec.action, &surfaces)?;
        let intent = Self {
            schema: PLUGIN_LIFECYCLE_INTENT_SCHEMA.to_string(),
            operation_id: spec.operation_id,
            plan_digest: spec.plan_digest,
            scope: spec.scope,
            package_id: spec.package_id,
            package_digest: spec.package_digest,
            manifest_digest: spec.manifest_digest,
            generation: spec.generation,
            action: spec.action,
            retained_ui_state_surfaces: spec.retained_ui_state_surfaces,
            surfaces,
            checkpoints,
        };
        intent.validate()?;
        Ok(intent)
    }
}

fn lifecycle_surfaces(
    manifest_surfaces: Vec<ManifestPluginSurface>,
    selected_surfaces: &[PluginSurfaceRef],
) -> UseResult<Vec<PluginLifecycleSurface>> {
    if manifest_surfaces.is_empty() || manifest_surfaces.len() > 256 {
        return Err(lifecycle_error(
            "A cognitive package must declare between one and 256 named surfaces.",
        ));
    }
    let by_ref = manifest_surfaces
        .iter()
        .map(|surface| (surface.surface.clone(), surface))
        .collect::<BTreeMap<_, _>>();
    if by_ref.len() != manifest_surfaces.len() {
        return Err(lifecycle_error(
            "A cognitive package surface appears more than once.",
        ));
    }

    let selected = selected_surfaces.iter().cloned().collect::<BTreeSet<_>>();
    if selected.is_empty()
        || selected.len() != selected_surfaces.len()
        || selected.iter().any(|surface| !by_ref.contains_key(surface))
        || by_ref
            .values()
            .any(|surface| !surface.optional && !selected.contains(&surface.surface))
        || selected.iter().any(|reference| {
            by_ref.get(reference).is_some_and(|surface| {
                surface
                    .dependencies
                    .iter()
                    .any(|dependency| !selected.contains(dependency))
            })
        })
    {
        return Err(lifecycle_error(
            "The selected lifecycle surfaces do not form the manifest's required dependency closure.",
        ));
    }

    let manifest_surfaces = manifest_surfaces
        .into_iter()
        .filter(|surface| selected.contains(&surface.surface))
        .collect::<Vec<_>>();
    let scheduled = schedule_surface_graph(
        manifest_surfaces
            .iter()
            .map(|surface| SurfaceGraphInput {
                surface: surface.surface.clone(),
                optional: surface.optional,
                dependencies: surface.dependencies.clone(),
            })
            .collect(),
    )
    .map_err(|error| lifecycle_error(error.message))?;
    let mut by_ref = manifest_surfaces
        .iter()
        .cloned()
        .map(|surface| (surface.surface.clone(), surface))
        .collect::<BTreeMap<_, _>>();
    let surfaces = scheduled
        .into_iter()
        .map(|scheduled| {
            let surface = by_ref
                .remove(&scheduled.surface)
                .ok_or_else(|| lifecycle_error("A scheduled lifecycle surface disappeared."))?;
            Ok(PluginLifecycleSurface {
                host: PluginSurfaceHost::for_kind(surface.surface.kind),
                required: scheduled.required,
                level: scheduled.level,
                activation: surface.activation,
                dependencies: scheduled.dependencies,
                surface: surface.surface,
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    validate_surfaces(&surfaces)?;
    Ok(surfaces)
}

pub(super) fn checkpoints(
    checkpoint_domain: &str,
    action: PluginLifecycleAction,
    surfaces: &[PluginLifecycleSurface],
) -> UseResult<Vec<PluginLifecycleCheckpoint>> {
    validate_surfaces(surfaces)?;
    let mut raw = Vec::new();
    match action {
        PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade => {
            raw.push((PluginLifecycleCheckpointKind::PackageCommitted, None, true));
            raw.extend(surfaces.iter().map(|surface| {
                (
                    PluginLifecycleCheckpointKind::SurfacePrepared,
                    Some(surface.surface.clone()),
                    surface.required,
                )
            }));
            raw.push((
                PluginLifecycleCheckpointKind::CapabilityPublished,
                None,
                true,
            ));
        }
        PluginLifecycleAction::Enable => {
            raw.extend(surfaces.iter().map(|surface| {
                (
                    PluginLifecycleCheckpointKind::SurfacePrepared,
                    Some(surface.surface.clone()),
                    surface.required,
                )
            }));
            raw.push((
                PluginLifecycleCheckpointKind::CapabilityPublished,
                None,
                true,
            ));
        }
        PluginLifecycleAction::Disable => {
            raw.push((PluginLifecycleCheckpointKind::CapabilityHidden, None, true));
            raw.push((PluginLifecycleCheckpointKind::CallsDrained, None, true));
            raw.extend(surfaces.iter().rev().map(|surface| {
                (
                    PluginLifecycleCheckpointKind::SurfaceStopped,
                    Some(surface.surface.clone()),
                    true,
                )
            }));
        }
        PluginLifecycleAction::Uninstall => {
            raw.push((PluginLifecycleCheckpointKind::CapabilityHidden, None, true));
            raw.push((PluginLifecycleCheckpointKind::CallsDrained, None, true));
            raw.extend(surfaces.iter().rev().map(|surface| {
                (
                    PluginLifecycleCheckpointKind::SurfaceRemoved,
                    Some(surface.surface.clone()),
                    true,
                )
            }));
            raw.push((PluginLifecycleCheckpointKind::PackageRemoved, None, true));
        }
    }
    raw.into_iter()
        .enumerate()
        .map(|(index, (kind, surface, required))| {
            let sequence = u32::try_from(index + 1).map_err(|_| {
                lifecycle_error("The cognitive-package checkpoint sequence is too large.")
            })?;
            Ok(PluginLifecycleCheckpoint {
                sequence,
                idempotency_key: checkpoint_key(
                    checkpoint_domain,
                    sequence,
                    kind,
                    surface.as_ref(),
                ),
                kind,
                surface,
                required,
            })
        })
        .collect()
}

pub(super) fn validate_surfaces(surfaces: &[PluginLifecycleSurface]) -> UseResult<()> {
    if surfaces.is_empty() || surfaces.len() > 256 {
        return Err(lifecycle_error(
            "The cognitive-package lifecycle surface inventory is empty or too large.",
        ));
    }
    let by_ref = surfaces
        .iter()
        .map(|surface| (surface.surface.clone(), surface))
        .collect::<BTreeMap<_, _>>();
    if by_ref.len() != surfaces.len() {
        return Err(lifecycle_error(
            "The cognitive-package lifecycle surface inventory contains duplicates.",
        ));
    }
    let expected_order = {
        let mut values = surfaces.to_vec();
        values.sort_by(|left, right| {
            left.level
                .cmp(&right.level)
                .then_with(|| left.surface.cmp(&right.surface))
        });
        values
    };
    if expected_order != surfaces {
        return Err(lifecycle_error(
            "Lifecycle surfaces must be sorted by dependency level and identity.",
        ));
    }
    for surface in surfaces {
        if surface.host != PluginSurfaceHost::for_kind(surface.surface.kind)
            || surface
                .dependencies
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || surface.dependencies.iter().any(|dependency| {
                by_ref
                    .get(dependency)
                    .is_none_or(|candidate| candidate.level >= surface.level)
            })
        {
            return Err(lifecycle_error(
                "A lifecycle surface has invalid host, dependency, or level evidence.",
            ));
        }
        if surface.required
            && surface.dependencies.iter().any(|dependency| {
                by_ref
                    .get(dependency)
                    .is_none_or(|candidate| !candidate.required)
            })
        {
            return Err(lifecycle_error(
                "A required lifecycle surface depends on a non-required surface.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
