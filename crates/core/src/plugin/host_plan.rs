use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::host::{validate_request_identity, verify_capabilities, verify_supported_plan_schema};
use super::validation::strictly_sorted_unique;
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanPackageRole,
    PluginHostCapabilities, PluginManagedScope, PluginOperationAction, PluginOperationPlanEnvelope,
    PluginPackageId, PluginPackageLock, PluginPlanSource, PluginSurfaceRef,
    VerifiedPluginCatalogRecord, MAX_PLUGIN_PLAN_ITEMS,
};

pub const PLUGIN_HOST_PLAN_REQUEST_SCHEMA: &str = "a3s.use.plugin-host-plan-request.v1";
pub const PLUGIN_HOST_PLAN_RESULT_SCHEMA: &str = "a3s.use.plugin-host-plan-result.v1";

const PLAN_REQUEST_ERROR: &str = "use.plugin.host_plan_request_invalid";
const PLAN_RESULT_ERROR: &str = "use.plugin.host_plan_result_invalid";

/// Exact managed-scope input for creating one canonical A3S Use plan.
///
/// Policy authority, provider choice, operation identity, and confirmation are
/// deliberately absent. The trusted host binds those values while delegating
/// planning to the shared Plugin Manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostPlanRequest {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub action: PluginOperationAction,
    pub package_id: PluginPackageId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<VerifiedPluginCatalogRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_lock: Option<PluginPackageLock>,
    pub selected_surfaces: Vec<PluginSurfaceRef>,
}

/// Immutable manager plan returned for review and later digest-only apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostPlanResult {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub plan: PluginOperationPlanEnvelope,
    pub replayed: bool,
}

impl PluginHostPlanRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host plan request",
            PLAN_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_PLAN_REQUEST_SCHEMA {
            return Err(plan_request_error(
                "The plugin host plan request schema is unsupported.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            plan_request_error("The plugin host plan request identity or scope is invalid.")
        })?;
        if self.selected_surfaces.len() > MAX_PLUGIN_PLAN_ITEMS
            || !strictly_sorted_unique(&self.selected_surfaces)
        {
            return Err(plan_request_error(
                "The requested plugin surfaces must be bounded, sorted, and unique.",
            ));
        }

        match (self.action, &self.candidate) {
            (PluginOperationAction::Install | PluginOperationAction::Upgrade, Some(candidate)) => {
                candidate.validate().map_err(|_| {
                    plan_request_error("The selected verified catalog record is invalid.")
                })?;
                if candidate.record.package_id != self.package_id.as_str()
                    || !candidate.record.is_package_plan_ready()
                    || candidate
                        .record
                        .resolve_surfaces(&self.selected_surfaces)
                        .is_err()
                {
                    return Err(plan_request_error(
                        "The selected catalog record or surface set cannot plan this package.",
                    ));
                }
                if !candidate.record.dependencies.is_empty() && self.package_lock.is_none() {
                    return Err(plan_request_error(
                        "A package with dependencies requires one exact resolved package lock.",
                    ));
                }
                if let Some(package_lock) = &self.package_lock {
                    package_lock.validate().map_err(|_| {
                        plan_request_error("The selected cognitive-package lock is invalid.")
                    })?;
                    if package_lock.root_package_id != self.package_id.as_str()
                        || package_lock
                            .package(self.package_id.as_str())
                            .map(|package| &package.catalog)
                            != Some(candidate)
                    {
                        return Err(plan_request_error(
                            "The selected root catalog record does not match its package lock.",
                        ));
                    }
                }
            }
            (PluginOperationAction::Uninstall, None) if self.selected_surfaces.is_empty() => {
                if let Some(package_lock) = &self.package_lock {
                    package_lock.validate().map_err(|_| {
                        plan_request_error("The uninstall package lock is invalid.")
                    })?;
                    if package_lock.root_package_id != self.package_id.as_str() {
                        return Err(plan_request_error(
                            "The uninstall package lock does not match the requested root.",
                        ));
                    }
                }
            }
            _ => {
                return Err(plan_request_error(
                    "Install and upgrade require one exact catalog candidate; uninstall does not.",
                ))
            }
        }
        Ok(())
    }

    pub fn validate_for_capabilities(
        &self,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;

        if let Some(candidate) = &self.candidate {
            let selected = candidate.record.resolve_surfaces(&self.selected_surfaces)?;
            validate_supported_surface_kinds(
                selected.iter().map(|surface| surface.kind),
                capabilities,
            )?;
        }
        if let Some(package_lock) = &self.package_lock {
            for package in &package_lock.packages {
                if package.package_id() == self.package_id.as_str() {
                    continue;
                }
                let selected = package.catalog.record.resolve_surfaces(&[])?;
                validate_supported_surface_kinds(
                    selected.iter().map(|surface| surface.kind),
                    capabilities,
                )?;
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin host plan request", PLAN_REQUEST_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostPlanResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host plan result",
            PLAN_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_PLAN_RESULT_SCHEMA {
            return Err(plan_result_error(
                "The plugin host plan result schema is unsupported.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            plan_result_error("The plugin host plan result identity or scope is invalid.")
        })?;
        self.plan
            .validate()
            .map_err(|_| plan_result_error("The canonical plugin plan is invalid."))?;
        if self.plan.plan.package_id != self.package_id.as_str()
            || self.plan.plan.scope != self.scope.plan_scope()
        {
            return Err(plan_result_error(
                "The canonical plan does not bind the result package and managed scope.",
            ));
        }
        Ok(())
    }

    pub fn validate_for(
        &self,
        request: &PluginHostPlanRequest,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        request.validate_for_capabilities(capabilities)?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;
        verify_supported_plan_schema(capabilities, &self.plan.plan.schema)?;
        if self.request_id != request.request_id
            || self.assignment_generation != request.assignment_generation
            || self.capabilities_digest != request.capabilities_digest
            || self.scope != request.scope
            || self.package_id != request.package_id
            || self.plan.plan.action != request.action
        {
            return Err(UseError::new(
                "use.plugin.host_plan_result_mismatch",
                "The plugin host plan result does not bind the exact request.",
            ));
        }
        validate_supported_surface_kinds(
            self.plan
                .plan
                .packages
                .iter()
                .flat_map(|package| [package.before.as_ref(), package.after.as_ref()])
                .flatten()
                .flat_map(|state| state.release.surfaces.iter().map(|surface| surface.kind)),
            capabilities,
        )?;
        verify_plan_selection(request, self)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin host plan result", PLAN_RESULT_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn validate_supported_surface_kinds(
    surface_kinds: impl IntoIterator<Item = super::PluginSurfaceKind>,
    capabilities: &PluginHostCapabilities,
) -> UseResult<()> {
    if surface_kinds
        .into_iter()
        .any(|kind| !capabilities.surface_kinds.contains(&kind))
    {
        return Err(UseError::new(
            "use.plugin.host_surface_unsupported",
            "The plugin plan contains a surface kind that the selected host protocol does not support.",
        ));
    }
    Ok(())
}

fn verify_plan_selection(
    request: &PluginHostPlanRequest,
    result: &PluginHostPlanResult,
) -> UseResult<()> {
    if request.package_lock != result.plan.package_lock {
        return Err(UseError::new(
            "use.plugin.host_plan_result_mismatch",
            "The canonical plan substituted or omitted the requested package lock.",
        ));
    }
    let root = result
        .plan
        .plan
        .packages
        .iter()
        .find(|package| package.role == PlanPackageRole::Root)
        .ok_or_else(|| {
            UseError::new(
                "use.plugin.host_plan_result_mismatch",
                "The canonical plan has no root package transition.",
            )
        })?;
    match &request.candidate {
        Some(candidate) => {
            let expected = candidate.selected_state(&request.selected_surfaces)?;
            let source_matches = matches!(
                &root.source,
                Some(PluginPlanSource::Registry { provenance, archive })
                    if provenance == &candidate.provenance && archive == &candidate.record.archive
            );
            if root.after.as_ref() != Some(&expected) || !source_matches {
                return Err(UseError::new(
                    "use.plugin.host_plan_result_mismatch",
                    "The canonical plan substituted the selected catalog release or surfaces.",
                ));
            }
        }
        None if root.after.is_none() && root.source.is_none() => {}
        None => {
            return Err(UseError::new(
                "use.plugin.host_plan_result_mismatch",
                "The uninstall plan contains unrequested candidate package evidence.",
            ))
        }
    }
    Ok(())
}

fn plan_request_error(message: impl Into<String>) -> UseError {
    contract_error(PLAN_REQUEST_ERROR, message)
}

fn plan_result_error(message: impl Into<String>) -> UseError {
    contract_error(PLAN_RESULT_ERROR, message)
}
