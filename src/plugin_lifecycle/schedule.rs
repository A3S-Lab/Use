use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginSurfaceRef, UseResult};
use a3s_use_extension::{ExtensionManifest, ManifestPluginSurface};

use super::model::{
    checkpoint_key, lifecycle_error, PluginLifecycleAction, PluginLifecycleCheckpoint,
    PluginLifecycleCheckpointKind, PluginLifecycleIntent, PluginLifecycleIntentSpec,
    PluginLifecycleSurface, PluginSurfaceHost, PLUGIN_LIFECYCLE_INTENT_SCHEMA,
};

impl PluginLifecycleIntent {
    pub fn from_manifest(
        spec: PluginLifecycleIntentSpec,
        manifest: &ExtensionManifest,
    ) -> UseResult<Self> {
        if spec.package_id != manifest.package_id {
            return Err(lifecycle_error(
                "The lifecycle package identity does not match the admitted manifest.",
            ));
        }
        let surfaces = lifecycle_surfaces(manifest.plugin_surfaces()?)?;
        let checkpoints = checkpoints(&spec.operation_id, spec.generation, spec.action, &surfaces)?;
        let intent = Self {
            schema: PLUGIN_LIFECYCLE_INTENT_SCHEMA.to_string(),
            operation_id: spec.operation_id,
            plan_digest: spec.plan_digest,
            scope_id: spec.scope_id,
            package_id: spec.package_id,
            package_digest: spec.package_digest,
            manifest_digest: spec.manifest_digest,
            generation: spec.generation,
            action: spec.action,
            surfaces,
            checkpoints,
        };
        intent.validate()?;
        Ok(intent)
    }
}

fn lifecycle_surfaces(
    manifest_surfaces: Vec<ManifestPluginSurface>,
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

    let levels = dependency_levels(&by_ref)?;
    let required = required_closure(&by_ref);
    let mut surfaces = manifest_surfaces
        .into_iter()
        .map(|surface| {
            let level = levels.get(&surface.surface).copied().ok_or_else(|| {
                lifecycle_error("A cognitive package surface has no dependency level.")
            })?;
            Ok(PluginLifecycleSurface {
                host: PluginSurfaceHost::for_kind(surface.surface.kind),
                required: required.contains(&surface.surface),
                level,
                activation: surface.activation,
                dependencies: surface.dependencies,
                surface: surface.surface,
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    surfaces.sort_by(|left, right| {
        left.level
            .cmp(&right.level)
            .then_with(|| left.surface.cmp(&right.surface))
    });
    validate_surfaces(&surfaces)?;
    Ok(surfaces)
}

fn dependency_levels(
    surfaces: &BTreeMap<PluginSurfaceRef, &ManifestPluginSurface>,
) -> UseResult<BTreeMap<PluginSurfaceRef, u32>> {
    let mut levels = BTreeMap::new();
    while levels.len() < surfaces.len() {
        let ready = surfaces
            .iter()
            .filter(|(reference, surface)| {
                !levels.contains_key(*reference)
                    && surface
                        .dependencies
                        .iter()
                        .all(|dependency| levels.contains_key(dependency))
            })
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(lifecycle_error(
                "The cognitive-package surface graph contains an unknown dependency or cycle.",
            ));
        }
        for reference in ready {
            let surface = surfaces.get(&reference).ok_or_else(|| {
                lifecycle_error("A lifecycle surface disappeared during dependency planning.")
            })?;
            let level = surface
                .dependencies
                .iter()
                .filter_map(|dependency| levels.get(dependency).copied())
                .max()
                .map_or(0, |level| level + 1);
            levels.insert(reference, level);
        }
    }
    Ok(levels)
}

fn required_closure(
    surfaces: &BTreeMap<PluginSurfaceRef, &ManifestPluginSurface>,
) -> BTreeSet<PluginSurfaceRef> {
    let mut required = surfaces
        .values()
        .filter(|surface| !surface.optional)
        .map(|surface| surface.surface.clone())
        .collect::<BTreeSet<_>>();
    let mut pending = required.iter().cloned().collect::<Vec<_>>();
    while let Some(reference) = pending.pop() {
        if let Some(surface) = surfaces.get(&reference) {
            for dependency in &surface.dependencies {
                if required.insert(dependency.clone()) {
                    pending.push(dependency.clone());
                }
            }
        }
    }
    required
}

pub(super) fn checkpoints(
    operation_id: &str,
    generation: u64,
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
                    operation_id,
                    generation,
                    action,
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
