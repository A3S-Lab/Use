use std::collections::BTreeMap;

use a3s_use_core::{
    CatalogMcpTransport, PlanQualifiedSurfaceRef, PlannedPackageState, PlannedProviderEvidence,
    PluginOperationPlan, PluginSurfaceKind, SurfaceChangeKind, ToolWorkloadClass, UseError,
    UseResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExpectedRuntimeKind {
    Task,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedRuntimeSurface {
    pub package_digest: String,
    pub kind: ExpectedRuntimeKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ExpectedRuntimeScope {
    pub candidates: BTreeMap<PlanQualifiedSurfaceRef, ExpectedRuntimeSurface>,
    pub retirements: BTreeMap<PlanQualifiedSurfaceRef, ExpectedRuntimeSurface>,
}

pub(crate) fn expected_grant_operations(plan: &PluginOperationPlan) -> BTreeMap<String, String> {
    plan.workspace_impacts
        .iter()
        .filter_map(|impact| {
            impact
                .grant_after_digest
                .as_ref()
                .map(|digest| (impact.scope_id.clone(), digest.clone()))
        })
        .collect()
}

pub(crate) fn expected_runtime_operations(
    plan: &PluginOperationPlan,
) -> UseResult<BTreeMap<String, ExpectedRuntimeScope>> {
    let mut scopes = BTreeMap::new();
    for impact in &plan.workspace_impacts {
        let mut expected = ExpectedRuntimeScope::default();
        for package in &plan.packages {
            for change in &package.surfaces {
                let surface = PlanQualifiedSurfaceRef {
                    package_id: package.package_id.clone(),
                    surface: change.surface.clone(),
                };
                if impact.enabled_after
                    && matches!(
                        change.change,
                        SurfaceChangeKind::Add | SurfaceChangeKind::Replace
                    )
                {
                    if let Some(expected_surface) =
                        expected_surface(package.after.as_ref(), &surface)?
                    {
                        if expected
                            .candidates
                            .insert(surface.clone(), expected_surface)
                            .is_some()
                        {
                            return Err(lifecycle_error(
                                "The reviewed plan contains duplicate Runtime candidate ownership.",
                            ));
                        }
                    }
                }
                if impact.enabled_before
                    && matches!(
                        change.change,
                        SurfaceChangeKind::Remove | SurfaceChangeKind::Replace
                    )
                {
                    if let Some(expected_surface) =
                        expected_surface(package.before.as_ref(), &surface)?
                    {
                        if expected
                            .retirements
                            .insert(surface, expected_surface)
                            .is_some()
                        {
                            return Err(lifecycle_error(
                                "The reviewed plan contains duplicate Runtime retirement ownership.",
                            ));
                        }
                    }
                }
            }
        }
        if !expected.candidates.is_empty() || !expected.retirements.is_empty() {
            scopes.insert(impact.scope_id.clone(), expected);
        }
    }
    Ok(scopes)
}

pub(crate) fn planned_providers(
    plan: &PluginOperationPlan,
) -> UseResult<BTreeMap<PlanQualifiedSurfaceRef, PlannedProviderEvidence>> {
    let mut providers = BTreeMap::new();
    for provider in &plan.providers {
        if providers
            .insert(provider.surface.clone(), provider.clone())
            .is_some()
        {
            return Err(lifecycle_error(
                "The reviewed plan contains duplicate Runtime provider evidence.",
            ));
        }
    }
    Ok(providers)
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(crate) fn lifecycle_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_binding_invalid", message)
}

fn expected_surface(
    state: Option<&PlannedPackageState>,
    surface: &PlanQualifiedSurfaceRef,
) -> UseResult<Option<ExpectedRuntimeSurface>> {
    let state = state.ok_or_else(|| {
        lifecycle_error("A Runtime surface change is missing its reviewed package state.")
    })?;
    let catalog = state
        .release
        .surfaces
        .iter()
        .find(|candidate| {
            candidate.kind == surface.surface.kind && candidate.id == surface.surface.id
        })
        .ok_or_else(|| {
            lifecycle_error("A Runtime surface change is absent from its reviewed release.")
        })?;
    let kind = match (catalog.kind, catalog.workload, catalog.mcp_transport) {
        (PluginSurfaceKind::Tool, Some(ToolWorkloadClass::Task), _) => {
            Some(ExpectedRuntimeKind::Task)
        }
        (PluginSurfaceKind::Tool, Some(ToolWorkloadClass::Service), _) => {
            Some(ExpectedRuntimeKind::Service)
        }
        (PluginSurfaceKind::Mcp, _, Some(CatalogMcpTransport::StreamableHttp)) => {
            Some(ExpectedRuntimeKind::Service)
        }
        _ => None,
    };
    Ok(kind.map(|kind| ExpectedRuntimeSurface {
        package_digest: state.release.package_sha256.clone(),
        kind,
    }))
}
