//! Canonical dependency scheduling for one package's selected surfaces.

use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginSurfaceRef, UseError, UseResult};

pub(crate) const MAX_SURFACE_GRAPH_NODES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SurfaceGraphInput {
    pub(crate) surface: PluginSurfaceRef,
    pub(crate) optional: bool,
    pub(crate) dependencies: Vec<PluginSurfaceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledSurface {
    pub(crate) surface: PluginSurfaceRef,
    pub(crate) level: u32,
    pub(crate) required: bool,
    pub(crate) dependencies: Vec<PluginSurfaceRef>,
}

/// Validate and schedule one complete selected surface dependency closure.
///
/// The returned order is stable across hosts: dependency level first, then
/// surface identity. A surface is required when it is mandatory itself or is
/// in the transitive dependency closure of a mandatory surface.
pub(crate) fn schedule_surface_graph(
    inputs: Vec<SurfaceGraphInput>,
) -> UseResult<Vec<ScheduledSurface>> {
    if inputs.is_empty() || inputs.len() > MAX_SURFACE_GRAPH_NODES {
        return Err(surface_graph_error(
            "A surface graph must contain between one and 256 selected surfaces.",
        ));
    }
    let input_count = inputs.len();
    let nodes = inputs
        .into_iter()
        .map(|input| (input.surface.clone(), input))
        .collect::<BTreeMap<_, _>>();
    if nodes.len() != input_count {
        return Err(surface_graph_error(
            "A surface graph contains duplicate surface identities.",
        ));
    }
    for node in nodes.values() {
        if node.dependencies.windows(2).any(|pair| pair[0] >= pair[1])
            || node
                .dependencies
                .iter()
                .any(|dependency| dependency == &node.surface || !nodes.contains_key(dependency))
        {
            return Err(surface_graph_error(
                "Surface dependencies must be sorted, unique, selected, and non-reflexive.",
            ));
        }
    }

    let mut levels = BTreeMap::new();
    while levels.len() < nodes.len() {
        let ready = nodes
            .iter()
            .filter(|(surface, node)| {
                !levels.contains_key(*surface)
                    && node
                        .dependencies
                        .iter()
                        .all(|dependency| levels.contains_key(dependency))
            })
            .map(|(surface, _)| surface.clone())
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(surface_graph_error(
                "The selected surface dependency graph contains a cycle.",
            ));
        }
        for surface in ready {
            let node = nodes.get(&surface).ok_or_else(|| {
                surface_graph_error("A surface disappeared during dependency scheduling.")
            })?;
            let level = node
                .dependencies
                .iter()
                .filter_map(|dependency| levels.get(dependency).copied())
                .max()
                .map_or(Ok(0), |level: u32| {
                    level.checked_add(1).ok_or_else(|| {
                        surface_graph_error("A surface dependency level is exhausted.")
                    })
                })?;
            levels.insert(surface, level);
        }
    }

    let mut required = nodes
        .values()
        .filter(|node| !node.optional)
        .map(|node| node.surface.clone())
        .collect::<BTreeSet<_>>();
    let mut pending = required.iter().cloned().collect::<Vec<_>>();
    while let Some(surface) = pending.pop() {
        let node = nodes.get(&surface).ok_or_else(|| {
            surface_graph_error("A required surface disappeared during closure evaluation.")
        })?;
        for dependency in &node.dependencies {
            if required.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }

    let mut scheduled = nodes
        .into_values()
        .map(|node| {
            let level = levels.get(&node.surface).copied().ok_or_else(|| {
                surface_graph_error("A scheduled surface has no dependency level.")
            })?;
            Ok(ScheduledSurface {
                required: required.contains(&node.surface),
                surface: node.surface,
                level,
                dependencies: node.dependencies,
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    scheduled.sort_by(|left, right| {
        left.level
            .cmp(&right.level)
            .then_with(|| left.surface.cmp(&right.surface))
    });
    Ok(scheduled)
}

fn surface_graph_error(message: impl Into<String>) -> UseError {
    UseError::new("use.surface_graph.invalid", message)
}
