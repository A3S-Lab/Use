use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    InstallationSnapshot, PlanQualifiedSurfaceRef, PluginOperationAction, PluginSurfaceKind,
    UseResult,
};

use super::super::{
    validate_provider_selections, ControlGeneration, ControlProviderSelection,
    ReviewedControlOperation, MAX_CONTROL_PROVIDER_SELECTIONS,
};

pub(super) fn project_provider_selections(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    target: &InstallationSnapshot,
) -> UseResult<Vec<ControlProviderSelection>> {
    if let Some(prior) = prior {
        validate_provider_selections(&prior.provider_selections, &prior.snapshot)?;
    }
    let mut selections = prior
        .into_iter()
        .flat_map(|generation| generation.provider_selections.iter().cloned())
        .map(|selection| (selection.qualified_surface().clone(), selection))
        .collect::<BTreeMap<_, _>>();

    selections.retain(|surface, _| target_requires_provider(target, surface));

    if matches!(
        operation.action(),
        PluginOperationAction::Install
            | PluginOperationAction::Upgrade
            | PluginOperationAction::Enable
    ) {
        let reviewed_packages = operation
            .envelope
            .plan
            .packages
            .iter()
            .map(|transition| transition.package_id.as_str())
            .collect::<BTreeSet<_>>();
        selections.retain(|surface, _| !reviewed_packages.contains(surface.package_id.as_str()));
        for provider in &operation.envelope.plan.providers {
            let selection = ControlProviderSelection::from_evidence(provider.clone())?;
            if !target_requires_provider(target, selection.qualified_surface())
                || selections
                    .insert(selection.qualified_surface().clone(), selection)
                    .is_some()
            {
                return Err(super::projection_error(
                    "The reviewed provider selection does not bind one exact target surface.",
                ));
            }
        }
    }

    let required = target
        .packages
        .iter()
        .filter(|package| package.enabled)
        .flat_map(|package| {
            package
                .selected_surfaces
                .iter()
                .filter(|surface| {
                    matches!(
                        surface.kind,
                        PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                    )
                })
                .map(move |surface| PlanQualifiedSurfaceRef {
                    package_id: package.package_id().to_string(),
                    surface: surface.clone(),
                })
        })
        .collect::<BTreeSet<_>>();
    if required.len() > MAX_CONTROL_PROVIDER_SELECTIONS
        || selections.keys().cloned().collect::<BTreeSet<_>>() != required
    {
        return Err(super::projection_error(
            "The projected provider selection does not exactly cover enabled executable surfaces.",
        ));
    }

    let projected = selections.into_values().collect::<Vec<_>>();
    for selection in &projected {
        selection.validate()?;
    }
    Ok(projected)
}

fn target_requires_provider(
    target: &InstallationSnapshot,
    surface: &PlanQualifiedSurfaceRef,
) -> bool {
    target
        .package_selection(&surface.package_id)
        .is_some_and(|package| {
            package.enabled
                && matches!(
                    surface.surface.kind,
                    PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                )
                && package.selected_surfaces.contains(&surface.surface)
        })
}
