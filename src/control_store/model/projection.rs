use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    InstallationPackageSelection, InstallationRootSelection, InstallationSnapshot,
    LockedPluginPackage, PlanPackageChangeKind, PlanPackageRole, PlannedPackageState,
    PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageLock, PluginPackageLockHost,
    UseResult,
};

use super::{
    generation_exhausted, input_error, valid_machine_id, ControlGeneration,
    ControlPackageLifecycle, ReviewedControlOperation,
};

pub(in crate::control_store) const MAX_CONTROL_HISTORY_PACKAGES: usize = 65_536;

/// Store-owned cursors needed to allocate the next package state and lifecycle
/// incarnations without reusing an identity after uninstall and reinstall.
///
/// This is reconstructed from committed generations. It is never supplied by
/// package content or an embedding host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::control_store) struct ControlProjectionHistory {
    last_lifecycle_generation: u64,
    package_state_generations: BTreeMap<String, u64>,
}

impl ControlProjectionHistory {
    pub(in crate::control_store) fn new(
        last_lifecycle_generation: u64,
        package_state_generations: BTreeMap<String, u64>,
    ) -> UseResult<Self> {
        if package_state_generations.len() > MAX_CONTROL_HISTORY_PACKAGES
            || (last_lifecycle_generation == 0 && !package_state_generations.is_empty())
            || package_state_generations
                .iter()
                .any(|(package_id, generation)| !valid_machine_id(package_id) || *generation == 0)
        {
            return Err(input_error(
                "The Control Store package-generation history is invalid or exceeds its bound.",
            ));
        }
        Ok(Self {
            last_lifecycle_generation,
            package_state_generations,
        })
    }

    pub(in crate::control_store) fn observe(
        &mut self,
        generation: &ControlGeneration,
    ) -> UseResult<()> {
        generation.snapshot.validate()?;
        if generation.package_lifecycles.len() != generation.snapshot.packages.len() {
            return Err(input_error(
                "A Control Store generation has incomplete package lifecycle history.",
            ));
        }
        for (package, lifecycle) in generation
            .snapshot
            .packages
            .iter()
            .zip(&generation.package_lifecycles)
        {
            if lifecycle.package_id != package.package_id() || lifecycle.lifecycle_generation == 0 {
                return Err(input_error(
                    "A Control Store generation has invalid package lifecycle history.",
                ));
            }
            self.last_lifecycle_generation = self
                .last_lifecycle_generation
                .max(lifecycle.lifecycle_generation);
            self.observe_state_generation(package.package_id(), package.state_generation)?;
        }
        Ok(())
    }

    pub(in crate::control_store) const fn last_lifecycle_generation(&self) -> u64 {
        self.last_lifecycle_generation
    }

    fn state_generation(&self, package_id: &str) -> u64 {
        self.package_state_generations
            .get(package_id)
            .copied()
            .unwrap_or(0)
    }

    fn observe_state_generation(&mut self, package_id: &str, generation: u64) -> UseResult<()> {
        if !valid_machine_id(package_id) || generation == 0 {
            return Err(input_error(
                "The Control Store package-generation history is invalid.",
            ));
        }
        if let Some(current) = self.package_state_generations.get_mut(package_id) {
            *current = (*current).max(generation);
            return Ok(());
        }
        if self.package_state_generations.len() >= MAX_CONTROL_HISTORY_PACKAGES {
            return Err(input_error(
                "The Control Store package-generation history exceeds its bound.",
            ));
        }
        self.package_state_generations
            .insert(package_id.to_string(), generation);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ProjectedControlGeneration {
    pub(in crate::control_store) snapshot: InstallationSnapshot,
    pub(in crate::control_store) package_lifecycles: Vec<ControlPackageLifecycle>,
    pub(in crate::control_store) history_after: ControlProjectionHistory,
}

impl ReviewedControlOperation {
    /// Deterministically project the complete next installation graph and its
    /// independent package-generation axes from reviewed authority.
    ///
    /// The caller supplies no target snapshot or generation identities. The
    /// only time input is the transaction commit time, which becomes the root
    /// selection timestamp for install and upgrade.
    pub(in crate::control_store) fn project_generation(
        &self,
        prior: Option<&ControlGeneration>,
        history: &ControlProjectionHistory,
        committed_at_ms: u64,
    ) -> UseResult<ProjectedControlGeneration> {
        self.validate()?;
        if committed_at_ms < self.reviewed_at_ms {
            return Err(projection_error(
                "A Control Store generation cannot commit before its review.",
            ));
        }
        validate_prior(self, prior, history)?;

        let (snapshot, package_lifecycles, last_lifecycle_generation) = match self.action() {
            PluginOperationAction::Enable | PluginOperationAction::Disable => {
                project_enablement(self, prior, history)?
            }
            PluginOperationAction::Install
            | PluginOperationAction::Upgrade
            | PluginOperationAction::Uninstall => {
                project_graph(self, prior, history, committed_at_ms)?
            }
        };
        self.validate_snapshot_transition(prior.map(|generation| &generation.snapshot), &snapshot)?;
        validate_plan_states(
            &self.envelope,
            prior.map(|value| &value.snapshot),
            &snapshot,
        )?;

        let mut history_after = history.clone();
        history_after.last_lifecycle_generation = last_lifecycle_generation;
        for package in &snapshot.packages {
            let previous = history_after.state_generation(package.package_id());
            if package.state_generation < previous {
                return Err(projection_error(
                    "A projected package state generation moved backwards.",
                ));
            }
            history_after
                .observe_state_generation(package.package_id(), package.state_generation)?;
        }
        Ok(ProjectedControlGeneration {
            snapshot,
            package_lifecycles,
            history_after,
        })
    }
}

fn validate_prior(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    history: &ControlProjectionHistory,
) -> UseResult<()> {
    match (operation.expected_generation, prior) {
        (0, None) => {
            if history.last_lifecycle_generation != 0
                || !history.package_state_generations.is_empty()
            {
                return Err(projection_error(
                    "A first Control Store generation has nonempty package history.",
                ));
            }
        }
        (expected, Some(generation))
            if expected > 0 && generation.snapshot.generation == expected =>
        {
            generation.snapshot.validate()?;
            if generation.package_lifecycles.len() != generation.snapshot.packages.len() {
                return Err(projection_error(
                    "The prior Control Store generation has incomplete lifecycle identity.",
                ));
            }
            for (package, lifecycle) in generation
                .snapshot
                .packages
                .iter()
                .zip(&generation.package_lifecycles)
            {
                if lifecycle.package_id != package.package_id()
                    || lifecycle.lifecycle_generation == 0
                    || lifecycle.lifecycle_generation > history.last_lifecycle_generation
                    || history.state_generation(package.package_id()) != package.state_generation
                {
                    return Err(projection_error(
                        "The prior Control Store generation does not match package history.",
                    ));
                }
            }
        }
        _ => {
            return Err(projection_error(
                "The reviewed operation does not follow the exact prior generation.",
            ))
        }
    }
    Ok(())
}

fn project_enablement(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    history: &ControlProjectionHistory,
) -> UseResult<(InstallationSnapshot, Vec<ControlPackageLifecycle>, u64)> {
    let prior = prior.ok_or_else(|| {
        projection_error("Enablement requires an existing Control Store generation.")
    })?;
    let package = prior
        .snapshot
        .package_selection(operation.root_package_id())
        .ok_or_else(|| projection_error("The enablement package is not selected."))?;
    if history.state_generation(package.package_id()) != package.state_generation {
        return Err(projection_error(
            "The enablement package state cursor does not match history.",
        ));
    }
    let enabled = operation.action() == PluginOperationAction::Enable;
    let snapshot = prior
        .snapshot
        .transition_package_enablement(
            operation.root_package_id(),
            package.state_generation,
            enabled,
        )?
        .ok_or_else(|| {
            projection_error("The reviewed enablement operation does not change desired state.")
        })?;
    Ok((
        snapshot,
        prior.package_lifecycles.clone(),
        history.last_lifecycle_generation,
    ))
}

fn project_graph(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    history: &ControlProjectionHistory,
    committed_at_ms: u64,
) -> UseResult<(InstallationSnapshot, Vec<ControlPackageLifecycle>, u64)> {
    let mut roots = prior_root_locks(prior)?;
    let target_host = graph_roots_after(operation, prior, &mut roots, committed_at_ms)?;
    let target_packages = packages_from_roots(&roots)?;
    let transitions = transition_map(&operation.envelope)?;
    let prior_packages = prior
        .map(|generation| {
            generation
                .snapshot
                .packages
                .iter()
                .map(|package| (package.package_id(), package))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let prior_lifecycles = prior
        .map(|generation| {
            generation
                .package_lifecycles
                .iter()
                .map(|lifecycle| (lifecycle.package_id.as_str(), lifecycle))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let mut last_lifecycle_generation = history.last_lifecycle_generation;
    let mut allocated_lifecycles = BTreeMap::new();
    if let Some(candidate) = operation.envelope.package_lock.as_ref().filter(|_| {
        matches!(
            operation.action(),
            PluginOperationAction::Install | PluginOperationAction::Upgrade
        )
    }) {
        for package in candidate.install_order()? {
            let transition = transitions.get(package.package_id()).ok_or_else(|| {
                projection_error("A candidate package has no reviewed transition.")
            })?;
            if !matches!(
                transition.change,
                PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
            ) {
                continue;
            }
            last_lifecycle_generation = last_lifecycle_generation
                .checked_add(1)
                .ok_or_else(generation_exhausted)?;
            allocated_lifecycles
                .insert(package.package_id().to_string(), last_lifecycle_generation);
        }
    }

    let mut selections = Vec::with_capacity(target_packages.len());
    let mut lifecycles = Vec::with_capacity(target_packages.len());
    for (package_id, package) in target_packages {
        let transition = transitions.get(package_id.as_str()).copied();
        let changed = transition.is_some_and(|transition| {
            matches!(
                transition.change,
                PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
            )
        });
        let (state_generation, lifecycle_generation, enabled, selected_surfaces) = if changed {
            let transition = transition.ok_or_else(|| {
                projection_error("A changed package lost its reviewed transition.")
            })?;
            let after = transition.after.as_ref().ok_or_else(|| {
                projection_error("A changed package has no reviewed target state.")
            })?;
            let state_generation = history
                .state_generation(&package_id)
                .checked_add(1)
                .ok_or_else(generation_exhausted)?;
            let lifecycle_generation =
                allocated_lifecycles
                    .get(&package_id)
                    .copied()
                    .ok_or_else(|| {
                        projection_error("A changed package has no lifecycle allocation.")
                    })?;
            (
                state_generation,
                lifecycle_generation,
                true,
                state_surfaces(after),
            )
        } else {
            let prior_package = prior_packages
                .get(package_id.as_str())
                .copied()
                .ok_or_else(|| {
                    projection_error("A retained package is absent from the prior generation.")
                })?;
            let prior_lifecycle = prior_lifecycles
                .get(package_id.as_str())
                .copied()
                .ok_or_else(|| {
                    projection_error("A retained package has no prior lifecycle identity.")
                })?;
            if prior_package.package != package
                || history.state_generation(&package_id) != prior_package.state_generation
            {
                return Err(projection_error(
                    "A retained package changed outside its reviewed transition.",
                ));
            }
            (
                prior_package.state_generation,
                prior_lifecycle.lifecycle_generation,
                prior_package.enabled,
                prior_package.selected_surfaces.clone(),
            )
        };
        selections.push(InstallationPackageSelection::new(
            package,
            state_generation,
            enabled,
            selected_surfaces,
        )?);
        lifecycles.push(ControlPackageLifecycle {
            package_id,
            lifecycle_generation,
        });
    }

    let snapshot = InstallationSnapshot::from_root_locks(
        operation.envelope.plan.scope.clone(),
        operation.target_generation()?,
        target_host,
        roots,
        selections,
    )?;
    Ok((snapshot, lifecycles, last_lifecycle_generation))
}

fn prior_root_locks(
    prior: Option<&ControlGeneration>,
) -> UseResult<Vec<(InstallationRootSelection, PluginPackageLock)>> {
    let Some(prior) = prior else {
        return Ok(Vec::new());
    };
    let locks = prior.snapshot.package_locks()?;
    if locks.len() != prior.snapshot.roots.len() {
        return Err(projection_error(
            "The prior root and package-lock inventories differ.",
        ));
    }
    Ok(prior.snapshot.roots.iter().cloned().zip(locks).collect())
}

fn graph_roots_after(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    roots: &mut Vec<(InstallationRootSelection, PluginPackageLock)>,
    committed_at_ms: u64,
) -> UseResult<PluginPackageLockHost> {
    let root_package_id = operation.root_package_id();
    let prior_host = prior.map(|generation| generation.snapshot.host.clone());
    match operation.action() {
        PluginOperationAction::Install => {
            if roots
                .iter()
                .any(|(root, _)| root.package_id == root_package_id)
            {
                return Err(projection_error(
                    "An install operation cannot replace an existing root.",
                ));
            }
            let candidate = operation.envelope.package_lock.clone().ok_or_else(|| {
                projection_error("An install operation omitted its candidate lock.")
            })?;
            if prior_host
                .as_ref()
                .is_some_and(|host| host != &candidate.host)
            {
                return Err(projection_error(
                    "An install candidate targets another installation host.",
                ));
            }
            roots.push((
                InstallationRootSelection::new(root_package_id, committed_at_ms)?,
                candidate.clone(),
            ));
            Ok(prior_host.unwrap_or(candidate.host))
        }
        PluginOperationAction::Upgrade => {
            let current = operation
                .envelope
                .prior_package_lock
                .as_ref()
                .ok_or_else(|| projection_error("An upgrade omitted its prior lock."))?;
            let candidate = operation
                .envelope
                .package_lock
                .clone()
                .ok_or_else(|| projection_error("An upgrade omitted its candidate lock."))?;
            replace_exact_root(roots, root_package_id, current, candidate, committed_at_ms)?;
            prior_host.ok_or_else(|| projection_error("An upgrade has no prior host."))
        }
        PluginOperationAction::Uninstall => {
            let current = operation
                .envelope
                .package_lock
                .as_ref()
                .ok_or_else(|| projection_error("An uninstall omitted its installed lock."))?;
            remove_exact_root(roots, root_package_id, current)?;
            prior_host.ok_or_else(|| projection_error("An uninstall has no prior host."))
        }
        PluginOperationAction::Enable | PluginOperationAction::Disable => Err(projection_error(
            "Enablement cannot be projected as a graph replacement.",
        )),
    }
}

fn replace_exact_root(
    roots: &mut [(InstallationRootSelection, PluginPackageLock)],
    root_package_id: &str,
    expected: &PluginPackageLock,
    candidate: PluginPackageLock,
    committed_at_ms: u64,
) -> UseResult<()> {
    let index = roots
        .iter()
        .position(|(root, _)| root.package_id == root_package_id)
        .ok_or_else(|| projection_error("The upgraded root is absent."))?;
    if &roots[index].1 != expected {
        return Err(projection_error(
            "The upgraded root differs from its reviewed prior lock.",
        ));
    }
    roots[index] = (
        InstallationRootSelection::new(root_package_id, committed_at_ms)?,
        candidate,
    );
    Ok(())
}

fn remove_exact_root(
    roots: &mut Vec<(InstallationRootSelection, PluginPackageLock)>,
    root_package_id: &str,
    expected: &PluginPackageLock,
) -> UseResult<()> {
    let index = roots
        .iter()
        .position(|(root, _)| root.package_id == root_package_id)
        .ok_or_else(|| projection_error("The uninstalled root is absent."))?;
    if &roots[index].1 != expected {
        return Err(projection_error(
            "The uninstalled root differs from its reviewed lock.",
        ));
    }
    roots.remove(index);
    Ok(())
}

fn packages_from_roots(
    roots: &[(InstallationRootSelection, PluginPackageLock)],
) -> UseResult<BTreeMap<String, LockedPluginPackage>> {
    let mut packages = BTreeMap::new();
    for (_, lock) in roots {
        for package in &lock.packages {
            let package_id = package.package_id().to_string();
            if packages
                .insert(package_id.clone(), package.clone())
                .is_some_and(|existing| existing != *package)
            {
                return Err(projection_error(format!(
                    "Package '{package_id}' has conflicting root selections."
                )));
            }
        }
    }
    Ok(packages)
}

fn transition_map(
    envelope: &PluginOperationPlanEnvelope,
) -> UseResult<BTreeMap<&str, &a3s_use_core::PlannedPackageTransition>> {
    let transitions = envelope
        .plan
        .packages
        .iter()
        .map(|transition| (transition.package_id.as_str(), transition))
        .collect::<BTreeMap<_, _>>();
    if transitions.len() != envelope.plan.packages.len() {
        return Err(projection_error(
            "The reviewed package transition inventory contains duplicates.",
        ));
    }
    Ok(transitions)
}

fn validate_plan_states(
    envelope: &PluginOperationPlanEnvelope,
    prior: Option<&InstallationSnapshot>,
    target: &InstallationSnapshot,
) -> UseResult<()> {
    let expected_packages = expected_transition_packages(envelope)?;
    let actual_packages = envelope
        .plan
        .packages
        .iter()
        .map(|transition| transition.package_id.as_str())
        .collect::<BTreeSet<_>>();
    if actual_packages != expected_packages || actual_packages.len() != envelope.plan.packages.len()
    {
        return Err(projection_error(
            "The reviewed transition inventory does not cover its exact package-lock domain.",
        ));
    }

    for transition in &envelope.plan.packages {
        let before = prior
            .and_then(|snapshot| snapshot.package_selection(&transition.package_id))
            .map(selection_state)
            .transpose()?;
        let after = target
            .package_selection(&transition.package_id)
            .map(selection_state)
            .transpose()?;
        let expected_change = match (&before, &after) {
            (None, Some(_)) => PlanPackageChangeKind::Add,
            (Some(_), None) => PlanPackageChangeKind::Remove,
            (Some(before), Some(after)) if before == after => PlanPackageChangeKind::Retain,
            (Some(_), Some(_)) => PlanPackageChangeKind::Replace,
            (None, None) => {
                return Err(projection_error(
                    "A reviewed package is absent before and after its operation.",
                ))
            }
        };
        let expected_role = if transition.package_id == envelope.plan.package_id {
            PlanPackageRole::Root
        } else {
            PlanPackageRole::Dependency
        };
        if transition.before != before
            || transition.after != after
            || transition.change != expected_change
            || transition.role != expected_role
        {
            return Err(projection_error(
                "A reviewed package transition differs from the projected installation graph.",
            ));
        }
    }
    Ok(())
}

fn expected_transition_packages(
    envelope: &PluginOperationPlanEnvelope,
) -> UseResult<BTreeSet<&str>> {
    match envelope.plan.action {
        PluginOperationAction::Install | PluginOperationAction::Uninstall => envelope
            .package_lock
            .as_ref()
            .ok_or_else(|| projection_error("A graph operation omitted its bound lock."))
            .map(|lock| {
                lock.packages
                    .iter()
                    .map(|package| package.package_id())
                    .collect()
            }),
        PluginOperationAction::Upgrade => {
            let prior = envelope
                .prior_package_lock
                .as_ref()
                .ok_or_else(|| projection_error("An upgrade omitted its prior lock."))?;
            let candidate = envelope
                .package_lock
                .as_ref()
                .ok_or_else(|| projection_error("An upgrade omitted its candidate lock."))?;
            Ok(prior
                .packages
                .iter()
                .chain(&candidate.packages)
                .map(|package| package.package_id())
                .collect())
        }
        PluginOperationAction::Enable | PluginOperationAction::Disable => {
            Ok(BTreeSet::from([envelope.plan.package_id.as_str()]))
        }
    }
}

fn selection_state(selection: &InstallationPackageSelection) -> UseResult<PlannedPackageState> {
    selection
        .package
        .catalog
        .selected_state(&selection.selected_surfaces)
        .map_err(|_| projection_error("A selected package state cannot be reconstructed."))
}

fn state_surfaces(state: &PlannedPackageState) -> Vec<a3s_use_core::PluginSurfaceRef> {
    state
        .release
        .surfaces
        .iter()
        .map(a3s_use_core::CatalogSurface::reference)
        .collect()
}

fn projection_error(message: impl Into<String>) -> a3s_use_core::UseError {
    input_error(message)
}
