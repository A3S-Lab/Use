//! Deterministic desired/observed state reconciliation for named plugin surfaces.
//!
//! This module does not deploy Runtime workloads. Provider and host adapters
//! submit observations; missing adapters remain explicit `pending` evidence.

use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult};
use a3s_use_extension::{ExtensionManifest, SurfaceActivation};
use serde::Serialize;

pub(crate) use a3s_use_core::{PluginDesiredState, PluginObservedState};

const RECONCILE_SCHEMA_VERSION: u32 = 1;
const MAX_RECONCILE_SURFACES: usize = 256;

mod runtime_observations;
pub(crate) use runtime_observations::reconcile_with_runtime;

pub(crate) type SurfaceObservations = BTreeMap<PluginSurfaceRef, SurfaceObservedState>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SurfaceDesiredState {
    Stopped,
    Prepared,
    Healthy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SurfaceObservedState {
    Pending,
    Prepared,
    Starting,
    Healthy,
    Failed,
    Draining,
    Stopped,
}

const TRANSITIONAL_SURFACE_STATES: [SurfaceObservedState; 2] = [
    SurfaceObservedState::Starting,
    SurfaceObservedState::Draining,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SurfaceOwner {
    FlowHost,
    KnowledgeHost,
    Runtime,
    McpHost,
    SkillHost,
    UiHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SurfaceStateReason {
    FlowObservationMissing,
    KnowledgeObservationMissing,
    PackageNotEnabled,
    RuntimeObservationMissing,
    McpObservationMissing,
    UiObservationMissing,
    DependencyPending,
    DependencyFailed,
    HostIncompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReconciledSurface {
    pub surface: PluginSurfaceRef,
    pub owner: SurfaceOwner,
    pub level: u32,
    pub required: bool,
    pub desired: SurfaceDesiredState,
    pub observed: SurfaceObservedState,
    pub dependencies: Vec<PluginSurfaceRef>,
    pub published: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<SurfaceStateReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SurfaceReconcileSnapshot {
    pub schema_version: u32,
    pub desired: PluginDesiredState,
    pub observed: PluginObservedState,
    pub capability_ready: bool,
    pub surfaces: Vec<ReconciledSurface>,
}

impl SurfaceReconcileSnapshot {
    pub(crate) fn publishes(&self, kind: PluginSurfaceKind, id: &str) -> bool {
        self.surfaces.iter().any(|surface| {
            surface.surface.kind == kind && surface.surface.id == id && surface.published
        })
    }
}

#[derive(Debug, Clone)]
struct SurfaceNode {
    surface: PluginSurfaceRef,
    owner: SurfaceOwner,
    optional: bool,
    activation: SurfaceActivation,
    dependencies: Vec<PluginSurfaceRef>,
}

pub(crate) fn reconcile(
    manifest: &ExtensionManifest,
    desired: PluginDesiredState,
    compatible: bool,
    observations: &SurfaceObservations,
) -> UseResult<SurfaceReconcileSnapshot> {
    if manifest.schema_version != 3 {
        return Err(reconcile_error(
            "Only schema version 3 named surfaces use the Surface Reconciler.",
        ));
    }
    let nodes = surface_nodes(manifest)?;
    if nodes.len() > MAX_RECONCILE_SURFACES {
        return Err(reconcile_error(format!(
            "A plugin may reconcile at most {MAX_RECONCILE_SURFACES} surfaces."
        )));
    }
    validate_observations(&nodes, observations)?;
    let levels = dependency_levels(&nodes)?;
    let required = required_closure(&nodes);
    let mut evaluated = BTreeMap::new();
    let mut ordered = nodes.keys().cloned().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        levels
            .get(left)
            .cmp(&levels.get(right))
            .then_with(|| left.cmp(right))
    });

    let mut surfaces = Vec::with_capacity(nodes.len());
    for reference in ordered {
        let node = nodes.get(&reference).ok_or_else(|| {
            reconcile_error("The surface graph changed while it was being evaluated.")
        })?;
        let surface_desired = desired_surface_state(node, desired);
        let (observed, reason) = observed_surface_state(
            node,
            desired,
            compatible,
            observations.get(&reference).copied(),
            &evaluated,
        );
        evaluated.insert(reference.clone(), (surface_desired, observed));
        surfaces.push(ReconciledSurface {
            surface: reference,
            owner: node.owner,
            level: *levels
                .get(&node.surface)
                .ok_or_else(|| reconcile_error("A reconciled surface has no dependency level."))?,
            required: required.contains(&node.surface),
            desired: surface_desired,
            observed,
            dependencies: node.dependencies.clone(),
            published: false,
            reason,
        });
    }

    let observed = aggregate_state(desired, compatible, &surfaces);
    let capability_ready = desired == PluginDesiredState::Enabled
        && matches!(
            observed,
            PluginObservedState::Ready | PluginObservedState::Degraded
        );
    if capability_ready {
        for surface in &mut surfaces {
            surface.published = surface_state_satisfied(surface.desired, surface.observed);
        }
    }
    Ok(SurfaceReconcileSnapshot {
        schema_version: RECONCILE_SCHEMA_VERSION,
        desired,
        observed,
        capability_ready,
        surfaces,
    })
}

fn surface_nodes(
    manifest: &ExtensionManifest,
) -> UseResult<BTreeMap<PluginSurfaceRef, SurfaceNode>> {
    let mut nodes = BTreeMap::new();
    for surface in manifest.plugin_surfaces()? {
        let owner = match surface.surface.kind {
            PluginSurfaceKind::Flow => SurfaceOwner::FlowHost,
            PluginSurfaceKind::Tool => SurfaceOwner::Runtime,
            PluginSurfaceKind::Mcp => SurfaceOwner::McpHost,
            PluginSurfaceKind::Okf => SurfaceOwner::KnowledgeHost,
            PluginSurfaceKind::Skill => SurfaceOwner::SkillHost,
            PluginSurfaceKind::Ui => SurfaceOwner::UiHost,
        };
        insert_node(
            &mut nodes,
            SurfaceNode {
                surface: surface.surface,
                owner,
                optional: surface.optional,
                activation: surface.activation,
                dependencies: surface.dependencies,
            },
        )?;
    }
    for node in nodes.values() {
        if let Some(dependency) = node
            .dependencies
            .iter()
            .find(|dependency| !nodes.contains_key(*dependency))
        {
            return Err(reconcile_error(format!(
                "Surface '{}:{}' depends on unknown surface '{}:{}'.",
                surface_kind_name(node.surface.kind),
                node.surface.id,
                surface_kind_name(dependency.kind),
                dependency.id
            )));
        }
    }
    Ok(nodes)
}

fn insert_node(
    nodes: &mut BTreeMap<PluginSurfaceRef, SurfaceNode>,
    node: SurfaceNode,
) -> UseResult<()> {
    let reference = node.surface.clone();
    if nodes.insert(reference.clone(), node).is_some() {
        return Err(reconcile_error(format!(
            "Surface '{}:{}' is declared more than once.",
            surface_kind_name(reference.kind),
            reference.id
        )));
    }
    Ok(())
}

fn dependency_levels(
    nodes: &BTreeMap<PluginSurfaceRef, SurfaceNode>,
) -> UseResult<BTreeMap<PluginSurfaceRef, u32>> {
    let mut levels = BTreeMap::new();
    while levels.len() < nodes.len() {
        let ready = nodes
            .iter()
            .filter(|(reference, node)| {
                !levels.contains_key(*reference)
                    && node
                        .dependencies
                        .iter()
                        .all(|dependency| levels.contains_key(dependency))
            })
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(reconcile_error(
                "The named surface dependency graph contains a cycle.",
            ));
        }
        for reference in ready {
            let node = nodes.get(&reference).ok_or_else(|| {
                reconcile_error("A dependency-level surface disappeared during evaluation.")
            })?;
            let level = node
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

fn required_closure(nodes: &BTreeMap<PluginSurfaceRef, SurfaceNode>) -> BTreeSet<PluginSurfaceRef> {
    let mut required = nodes
        .values()
        .filter(|node| !node.optional)
        .map(|node| node.surface.clone())
        .collect::<BTreeSet<_>>();
    let mut pending = required.iter().cloned().collect::<Vec<_>>();
    while let Some(reference) = pending.pop() {
        if let Some(node) = nodes.get(&reference) {
            for dependency in &node.dependencies {
                if required.insert(dependency.clone()) {
                    pending.push(dependency.clone());
                }
            }
        }
    }
    required
}

fn validate_observations(
    nodes: &BTreeMap<PluginSurfaceRef, SurfaceNode>,
    observations: &SurfaceObservations,
) -> UseResult<()> {
    if let Some(reference) = observations
        .keys()
        .find(|reference| !nodes.contains_key(*reference))
    {
        return Err(reconcile_error(format!(
            "Observation references unknown surface '{}:{}'.",
            surface_kind_name(reference.kind),
            reference.id
        )));
    }
    Ok(())
}

fn desired_surface_state(node: &SurfaceNode, desired: PluginDesiredState) -> SurfaceDesiredState {
    if desired != PluginDesiredState::Enabled {
        return SurfaceDesiredState::Stopped;
    }
    match node.owner {
        SurfaceOwner::KnowledgeHost => SurfaceDesiredState::Healthy,
        SurfaceOwner::Runtime | SurfaceOwner::McpHost
            if node.activation == SurfaceActivation::Eager =>
        {
            SurfaceDesiredState::Healthy
        }
        SurfaceOwner::Runtime
        | SurfaceOwner::McpHost
        | SurfaceOwner::FlowHost
        | SurfaceOwner::SkillHost
        | SurfaceOwner::UiHost => SurfaceDesiredState::Prepared,
    }
}

fn observed_surface_state(
    node: &SurfaceNode,
    plugin_desired: PluginDesiredState,
    compatible: bool,
    explicit: Option<SurfaceObservedState>,
    evaluated: &BTreeMap<PluginSurfaceRef, (SurfaceDesiredState, SurfaceObservedState)>,
) -> (SurfaceObservedState, Option<SurfaceStateReason>) {
    if plugin_desired != PluginDesiredState::Enabled {
        return (
            explicit.unwrap_or(SurfaceObservedState::Stopped),
            explicit
                .is_none()
                .then_some(SurfaceStateReason::PackageNotEnabled),
        );
    }
    if !compatible {
        return (
            SurfaceObservedState::Failed,
            Some(SurfaceStateReason::HostIncompatible),
        );
    }

    let (base, base_reason) =
        explicit.map_or_else(|| default_observation(node), |observed| (observed, None));
    if base == SurfaceObservedState::Failed {
        return (base, base_reason);
    }
    let dependency_failed = node.dependencies.iter().any(|dependency| {
        evaluated
            .get(dependency)
            .is_some_and(|(_, observed)| *observed == SurfaceObservedState::Failed)
    });
    if dependency_failed {
        return (
            SurfaceObservedState::Failed,
            Some(SurfaceStateReason::DependencyFailed),
        );
    }
    let dependency_pending = node.dependencies.iter().any(|dependency| {
        evaluated
            .get(dependency)
            .is_none_or(|(desired, observed)| !surface_state_satisfied(*desired, *observed))
    });
    if dependency_pending {
        return (
            SurfaceObservedState::Pending,
            Some(SurfaceStateReason::DependencyPending),
        );
    }
    (base, base_reason)
}

fn default_observation(node: &SurfaceNode) -> (SurfaceObservedState, Option<SurfaceStateReason>) {
    match node.owner {
        SurfaceOwner::SkillHost => (SurfaceObservedState::Prepared, None),
        SurfaceOwner::FlowHost => (
            SurfaceObservedState::Pending,
            Some(SurfaceStateReason::FlowObservationMissing),
        ),
        SurfaceOwner::KnowledgeHost => (
            SurfaceObservedState::Pending,
            Some(SurfaceStateReason::KnowledgeObservationMissing),
        ),
        SurfaceOwner::Runtime => (
            SurfaceObservedState::Pending,
            Some(SurfaceStateReason::RuntimeObservationMissing),
        ),
        SurfaceOwner::McpHost => (
            SurfaceObservedState::Pending,
            Some(SurfaceStateReason::McpObservationMissing),
        ),
        SurfaceOwner::UiHost => (
            SurfaceObservedState::Pending,
            Some(SurfaceStateReason::UiObservationMissing),
        ),
    }
}

fn aggregate_state(
    desired: PluginDesiredState,
    compatible: bool,
    surfaces: &[ReconciledSurface],
) -> PluginObservedState {
    if desired != PluginDesiredState::Absent && !compatible {
        return PluginObservedState::Incompatible;
    }
    match desired {
        PluginDesiredState::Absent => {
            if surfaces
                .iter()
                .all(|surface| surface.observed == SurfaceObservedState::Stopped)
            {
                PluginObservedState::Removed
            } else {
                PluginObservedState::Draining
            }
        }
        PluginDesiredState::InstalledDisabled => {
            if surfaces
                .iter()
                .all(|surface| surface.observed == SurfaceObservedState::Stopped)
            {
                PluginObservedState::Installed
            } else if surfaces
                .iter()
                .any(|surface| surface.observed == SurfaceObservedState::Draining)
            {
                PluginObservedState::Draining
            } else {
                PluginObservedState::Reconciling
            }
        }
        PluginDesiredState::Enabled => {
            if surfaces
                .iter()
                .any(|surface| surface.required && surface.observed == SurfaceObservedState::Failed)
            {
                return PluginObservedState::Broken;
            }
            if surfaces.iter().any(|surface| {
                surface.required && !surface_state_satisfied(surface.desired, surface.observed)
            }) {
                return PluginObservedState::Reconciling;
            }
            if surfaces.iter().any(|surface| {
                !surface.required && !surface_state_satisfied(surface.desired, surface.observed)
            }) {
                PluginObservedState::Degraded
            } else {
                PluginObservedState::Ready
            }
        }
    }
}

fn surface_state_satisfied(desired: SurfaceDesiredState, observed: SurfaceObservedState) -> bool {
    if TRANSITIONAL_SURFACE_STATES.contains(&observed) {
        return false;
    }
    match desired {
        SurfaceDesiredState::Stopped => observed == SurfaceObservedState::Stopped,
        SurfaceDesiredState::Prepared => {
            matches!(
                observed,
                SurfaceObservedState::Prepared | SurfaceObservedState::Healthy
            )
        }
        SurfaceDesiredState::Healthy => observed == SurfaceObservedState::Healthy,
    }
}

#[cfg(test)]
fn surface_ref(kind: PluginSurfaceKind, id: &str) -> PluginSurfaceRef {
    PluginSurfaceRef {
        kind,
        id: id.to_string(),
    }
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

fn reconcile_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.reconcile_invalid", message)
}

#[cfg(test)]
mod tests;
