use a3s_use_core::{
    CatalogSurface, InstallationPackageSelection, InstallationSnapshot, PlanPackageChangeKind,
    PlannedPackageState, PluginOperationAction, PluginPackageLock, PluginSurfaceKind,
    PluginSurfaceRef, UseResult,
};

use crate::plugin_lifecycle::PluginLifecycleAction;
use crate::surface_graph::{schedule_surface_graph, ScheduledSurface, SurfaceGraphInput};

use super::super::{
    ControlCapabilitySelection, ControlEffectIntent, ControlEffectKind, ControlEffectOwner,
    ControlEffectSubject, ControlGeneration, ControlPackageLifecycle, ControlProviderSelection,
    ReviewedControlOperation, MAX_CONTROL_EFFECTS, MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES,
};

pub(super) fn project_effects(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    target: &InstallationSnapshot,
    target_lifecycles: &[ControlPackageLifecycle],
    target_providers: &[ControlProviderSelection],
    capability: &ControlCapabilitySelection,
) -> UseResult<Vec<ControlEffectIntent>> {
    let mut projection = EffectProjection {
        operation,
        effects: Vec::new(),
        payload_bytes: 0,
    };
    match operation.action() {
        PluginOperationAction::Install => {
            let candidate = operation.envelope.package_lock.as_ref().ok_or_else(|| {
                super::projection_error("An install effect projection omitted its package lock.")
            })?;
            projection.prepare_changed_packages(
                candidate,
                target,
                target_lifecycles,
                target_providers,
            )?;
            projection.capability_cutover(target, capability)?;
        }
        PluginOperationAction::Upgrade => {
            let prior = prior.ok_or_else(|| {
                super::projection_error("An upgrade effect projection has no prior generation.")
            })?;
            let candidate = operation.envelope.package_lock.as_ref().ok_or_else(|| {
                super::projection_error("An upgrade effect projection omitted its candidate lock.")
            })?;
            projection.prepare_changed_packages(
                candidate,
                target,
                target_lifecycles,
                target_providers,
            )?;
            projection.capability_cutover(target, capability)?;
            let retired = operation
                .envelope
                .prior_package_lock
                .as_ref()
                .ok_or_else(|| {
                    super::projection_error(
                        "An upgrade effect projection omitted its prior package lock.",
                    )
                })?;
            projection.retire_changed_packages(prior, retired, ControlEffectKind::SurfaceRemove)?;
        }
        PluginOperationAction::Enable => {
            let transition = operation
                .envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == operation.root_package_id())
                .ok_or_else(|| {
                    super::projection_error("An enable effect projection omitted its package.")
                })?;
            let state = transition.after.as_ref().ok_or_else(|| {
                super::projection_error("An enable effect projection omitted its target state.")
            })?;
            projection.prepare_package(
                target,
                target_lifecycles,
                target_providers,
                &transition.package_id,
                state,
                PluginLifecycleAction::Enable,
            )?;
            projection.capability_cutover(target, capability)?;
        }
        PluginOperationAction::Disable => {
            let prior = prior.ok_or_else(|| {
                super::projection_error("A disable effect projection has no prior generation.")
            })?;
            projection.capability_cutover(target, capability)?;
            projection.retire_package(
                prior,
                operation.root_package_id(),
                PluginLifecycleAction::Disable,
                ControlEffectKind::SurfaceStop,
            )?;
        }
        PluginOperationAction::Uninstall => {
            let prior = prior.ok_or_else(|| {
                super::projection_error("An uninstall effect projection has no prior generation.")
            })?;
            projection.capability_cutover(target, capability)?;
            let retired = operation.envelope.package_lock.as_ref().ok_or_else(|| {
                super::projection_error(
                    "An uninstall effect projection omitted its installed package lock.",
                )
            })?;
            projection.retire_changed_packages(prior, retired, ControlEffectKind::SurfaceRemove)?;
        }
    }
    if projection.effects.is_empty() || projection.effects.len() > MAX_CONTROL_EFFECTS {
        return Err(super::projection_error(
            "The projected external effect inventory is empty or exceeds its bound.",
        ));
    }
    Ok(projection.effects)
}

struct EffectProjection<'a> {
    operation: &'a ReviewedControlOperation,
    effects: Vec<ControlEffectIntent>,
    payload_bytes: usize,
}

impl EffectProjection<'_> {
    fn prepare_changed_packages(
        &mut self,
        lock: &PluginPackageLock,
        target: &InstallationSnapshot,
        target_lifecycles: &[ControlPackageLifecycle],
        target_providers: &[ControlProviderSelection],
    ) -> UseResult<()> {
        for package in lock.install_order()? {
            let transition = self
                .operation
                .envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
                .ok_or_else(|| {
                    super::projection_error(
                        "A candidate package has no reviewed effect transition.",
                    )
                })?;
            let action = match transition.change {
                PlanPackageChangeKind::Add => PluginLifecycleAction::Install,
                PlanPackageChangeKind::Replace => PluginLifecycleAction::Upgrade,
                PlanPackageChangeKind::Retain => continue,
                PlanPackageChangeKind::Remove => {
                    return Err(super::projection_error(
                        "A removed package appears in the candidate effect order.",
                    ))
                }
            };
            let state = transition.after.as_ref().ok_or_else(|| {
                super::projection_error("A candidate effect transition has no target state.")
            })?;
            self.prepare_package(
                target,
                target_lifecycles,
                target_providers,
                package.package_id(),
                state,
                action,
            )?;
        }
        Ok(())
    }

    fn prepare_package(
        &mut self,
        generation: &InstallationSnapshot,
        lifecycles: &[ControlPackageLifecycle],
        providers: &[ControlProviderSelection],
        package_id: &str,
        state: &PlannedPackageState,
        action: PluginLifecycleAction,
    ) -> UseResult<()> {
        let (package, lifecycle_generation) =
            package_incarnation(generation, lifecycles, package_id)?;
        validate_planned_incarnation(package, state)?;
        for scheduled in schedule_surfaces(&state.release.surfaces)? {
            let subject =
                surface_subject(package, lifecycle_generation, action, scheduled.surface)?;
            let owner = surface_owner(&subject, providers)?;
            self.push(
                generation.generation,
                subject,
                owner,
                ControlEffectKind::SurfacePrepare,
                scheduled.required,
            )?;
        }
        Ok(())
    }

    fn capability_cutover(
        &mut self,
        target: &InstallationSnapshot,
        capability: &ControlCapabilitySelection,
    ) -> UseResult<()> {
        self.push(
            target.generation,
            ControlEffectSubject::Installation {
                expected_capability_generation: self.operation.expected_capability_generation,
                capability_generation: capability.generation,
                descriptor_digest: capability.descriptor_digest.clone(),
            },
            ControlEffectOwner::CapabilityIndex,
            ControlEffectKind::CapabilityCutover,
            true,
        )
    }

    fn retire_changed_packages(
        &mut self,
        prior: &ControlGeneration,
        lock: &PluginPackageLock,
        surface_kind: ControlEffectKind,
    ) -> UseResult<()> {
        let ordered = lock
            .removal_order()?
            .into_iter()
            .filter_map(|package| {
                self.operation
                    .envelope
                    .plan
                    .packages
                    .iter()
                    .find(|transition| transition.package_id == package.package_id())
                    .filter(|transition| {
                        matches!(
                            transition.change,
                            PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
                        )
                    })
                    .map(|transition| transition.package_id.as_str())
            })
            .collect::<Vec<_>>();
        for package_id in &ordered {
            self.drain_package(prior, package_id, PluginLifecycleAction::Uninstall)?;
        }
        for package_id in ordered {
            self.retire_surfaces(
                prior,
                package_id,
                PluginLifecycleAction::Uninstall,
                surface_kind,
            )?;
        }
        Ok(())
    }

    fn retire_package(
        &mut self,
        prior: &ControlGeneration,
        package_id: &str,
        action: PluginLifecycleAction,
        surface_kind: ControlEffectKind,
    ) -> UseResult<()> {
        self.drain_package(prior, package_id, action)?;
        self.retire_surfaces(prior, package_id, action, surface_kind)
    }

    fn drain_package(
        &mut self,
        generation: &ControlGeneration,
        package_id: &str,
        action: PluginLifecycleAction,
    ) -> UseResult<()> {
        let (package, lifecycle_generation) = package_incarnation(
            &generation.snapshot,
            &generation.package_lifecycles,
            package_id,
        )?;
        self.push(
            generation.snapshot.generation,
            package_subject(package, lifecycle_generation, action)?,
            ControlEffectOwner::InvocationLeases,
            ControlEffectKind::CallsDrain,
            true,
        )
    }

    fn retire_surfaces(
        &mut self,
        generation: &ControlGeneration,
        package_id: &str,
        action: PluginLifecycleAction,
        kind: ControlEffectKind,
    ) -> UseResult<()> {
        let transition = self
            .operation
            .envelope
            .plan
            .packages
            .iter()
            .find(|transition| transition.package_id == package_id)
            .ok_or_else(|| {
                super::projection_error("A retired package has no reviewed transition.")
            })?;
        let state = transition.before.as_ref().ok_or_else(|| {
            super::projection_error("A retired package has no reviewed prior state.")
        })?;
        let (package, lifecycle_generation) = package_incarnation(
            &generation.snapshot,
            &generation.package_lifecycles,
            package_id,
        )?;
        validate_planned_incarnation(package, state)?;
        for scheduled in schedule_surfaces(&state.release.surfaces)?
            .into_iter()
            .rev()
        {
            let subject =
                surface_subject(package, lifecycle_generation, action, scheduled.surface)?;
            let owner = surface_owner(&subject, &generation.provider_selections)?;
            self.push(generation.snapshot.generation, subject, owner, kind, true)?;
        }
        Ok(())
    }

    fn push(
        &mut self,
        installation_generation: u64,
        subject: ControlEffectSubject,
        owner: ControlEffectOwner,
        kind: ControlEffectKind,
        required: bool,
    ) -> UseResult<()> {
        if self.effects.len() >= MAX_CONTROL_EFFECTS {
            return Err(super::projection_error(
                "The projected external effect inventory exceeds its count bound.",
            ));
        }
        let sequence = u32::try_from(self.effects.len()).map_err(|_| {
            super::projection_error("The projected effect sequence exceeds its integer bound.")
        })?;
        let intent = ControlEffectIntent::new(
            sequence,
            self.operation.envelope.plan.scope.clone(),
            self.operation.plan_digest().to_string(),
            self.operation.action(),
            installation_generation,
            subject,
            owner,
            kind,
            required,
        )?;
        self.payload_bytes = self
            .payload_bytes
            .checked_add(intent.canonical_bytes()?.len())
            .filter(|bytes| *bytes <= MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES)
            .ok_or_else(|| {
                super::projection_error(
                    "The projected external effect inventory exceeds its payload byte bound.",
                )
            })?;
        self.effects.push(intent);
        Ok(())
    }
}

fn package_incarnation<'a>(
    snapshot: &'a InstallationSnapshot,
    lifecycles: &[ControlPackageLifecycle],
    package_id: &str,
) -> UseResult<(&'a InstallationPackageSelection, u64)> {
    let package = snapshot.package_selection(package_id).ok_or_else(|| {
        super::projection_error("An effect package is absent from its installation generation.")
    })?;
    let lifecycle = lifecycles
        .binary_search_by(|lifecycle| lifecycle.package_id.as_str().cmp(package_id))
        .ok()
        .and_then(|index| lifecycles.get(index))
        .ok_or_else(|| {
            super::projection_error("An effect package has no lifecycle incarnation.")
        })?;
    Ok((package, lifecycle.lifecycle_generation))
}

fn validate_planned_incarnation(
    package: &InstallationPackageSelection,
    state: &PlannedPackageState,
) -> UseResult<()> {
    let selected = package
        .package
        .catalog
        .selected_state(&package.selected_surfaces)?;
    if selected != *state {
        return Err(super::projection_error(
            "An effect package differs from its reviewed planned state.",
        ));
    }
    Ok(())
}

fn schedule_surfaces(surfaces: &[CatalogSurface]) -> UseResult<Vec<ScheduledSurface>> {
    schedule_surface_graph(
        surfaces
            .iter()
            .map(|surface| SurfaceGraphInput {
                surface: surface.reference(),
                optional: surface.optional,
                dependencies: surface.requires.clone(),
            })
            .collect(),
    )
    .map_err(|error| super::projection_error(error.message))
}

fn package_subject(
    package: &InstallationPackageSelection,
    lifecycle_generation: u64,
    action: PluginLifecycleAction,
) -> UseResult<ControlEffectSubject> {
    let package_digest = package
        .package
        .catalog
        .record
        .package
        .sha256
        .clone()
        .ok_or_else(|| super::projection_error("An effect package omitted its digest."))?;
    let manifest_digest = package
        .package
        .catalog
        .record
        .package
        .manifest_sha256
        .clone()
        .ok_or_else(|| super::projection_error("An effect package omitted its manifest digest."))?;
    Ok(ControlEffectSubject::Package {
        package_id: package.package_id().to_string(),
        lifecycle_generation,
        package_digest,
        manifest_digest,
        action,
    })
}

fn surface_subject(
    package: &InstallationPackageSelection,
    lifecycle_generation: u64,
    action: PluginLifecycleAction,
    surface: PluginSurfaceRef,
) -> UseResult<ControlEffectSubject> {
    let ControlEffectSubject::Package {
        package_id,
        package_digest,
        manifest_digest,
        ..
    } = package_subject(package, lifecycle_generation, action)?
    else {
        return Err(super::projection_error(
            "A surface effect could not derive its package subject.",
        ));
    };
    Ok(ControlEffectSubject::Surface {
        package_id,
        lifecycle_generation,
        package_digest,
        manifest_digest,
        action,
        surface,
    })
}

fn surface_owner(
    subject: &ControlEffectSubject,
    providers: &[ControlProviderSelection],
) -> UseResult<ControlEffectOwner> {
    let ControlEffectSubject::Surface {
        package_id,
        surface,
        ..
    } = subject
    else {
        return Err(super::projection_error(
            "A surface effect owner received a non-surface subject.",
        ));
    };
    match surface.kind {
        PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp => providers
            .iter()
            .find(|selection| {
                selection.package_id() == package_id && selection.surface() == surface
            })
            .map(|selection| ControlEffectOwner::RuntimeProvider {
                provider_id: selection.evidence.provider_id.clone(),
                selection_digest: selection.selection_digest.clone(),
            })
            .ok_or_else(|| {
                super::projection_error(
                    "An executable surface effect has no exact reviewed Runtime owner.",
                )
            }),
        PluginSurfaceKind::Flow => Ok(ControlEffectOwner::FlowHost),
        PluginSurfaceKind::Okf => Ok(ControlEffectOwner::KnowledgeHost),
        PluginSurfaceKind::Skill => Ok(ControlEffectOwner::SkillHost),
        PluginSurfaceKind::Ui => Ok(ControlEffectOwner::UiHost),
    }
}
