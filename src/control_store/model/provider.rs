use std::collections::BTreeSet;

use a3s_use_core::{
    InstallationSnapshot, PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlannedProviderEvidence,
    PluginSurfaceKind, PluginSurfaceRef, UseResult,
};
use serde::{Deserialize, Serialize};

use super::{corruption_error, input_error, valid_machine_id, valid_sha256};
use crate::plugin_runtime::provider_selection_digest;

pub(in crate::control_store) const MAX_CONTROL_PROVIDER_SELECTIONS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlProviderSelection {
    pub(in crate::control_store) evidence: PlannedProviderEvidence,
    pub(in crate::control_store) selection_digest: String,
}

impl ControlProviderSelection {
    pub(in crate::control_store) fn from_evidence(
        evidence: PlannedProviderEvidence,
    ) -> UseResult<Self> {
        let selection_digest = provider_evidence_digest(&evidence)?;
        let selection = Self {
            evidence,
            selection_digest,
        };
        selection.validate()?;
        Ok(selection)
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.evidence.surface.package_id)
            || !matches!(
                self.evidence.surface.surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            )
            || !valid_machine_id(&self.evidence.surface.surface.id)
            || !valid_machine_id(&self.evidence.provider_id)
            || !valid_machine_id(&self.evidence.provider_build_id)
            || !valid_sha256(&self.evidence.capability_digest)
            || !valid_sha256(&self.evidence.semantics_profile_digest)
            || !valid_sha256(&self.selection_digest)
            || provider_evidence_digest(&self.evidence)? != self.selection_digest
        {
            return Err(input_error(
                "A Control Store provider selection is invalid or noncanonical.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn qualified_surface(&self) -> &PlanQualifiedSurfaceRef {
        &self.evidence.surface
    }

    pub(in crate::control_store) fn package_id(&self) -> &str {
        &self.evidence.surface.package_id
    }

    pub(in crate::control_store) fn surface(&self) -> &PluginSurfaceRef {
        &self.evidence.surface.surface
    }
}

pub(in crate::control_store) fn validate_provider_selections(
    selections: &[ControlProviderSelection],
    snapshot: &InstallationSnapshot,
) -> UseResult<()> {
    let required = snapshot
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
    let selected = selections
        .iter()
        .map(|selection| selection.qualified_surface().clone())
        .collect::<BTreeSet<_>>();
    if selections.len() > MAX_CONTROL_PROVIDER_SELECTIONS
        || selections
            .windows(2)
            .any(|pair| pair[0].qualified_surface() >= pair[1].qualified_surface())
        || selections
            .iter()
            .any(|selection| selection.validate().is_err())
        || selected != required
    {
        return Err(input_error(
            "Control Store provider selections must be canonical, sorted, unique, and bind enabled executable surfaces.",
        ));
    }
    Ok(())
}

pub(in crate::control_store) const fn enforcement_profile_name(
    profile: PlanEnforcementProfile,
) -> &'static str {
    match profile {
        PlanEnforcementProfile::Container => "container",
        PlanEnforcementProfile::NativeUnconfined => "native-unconfined",
        PlanEnforcementProfile::Sandbox => "sandbox",
    }
}

pub(in crate::control_store) fn parse_enforcement_profile(
    value: &str,
) -> UseResult<PlanEnforcementProfile> {
    match value {
        "container" => Ok(PlanEnforcementProfile::Container),
        "native-unconfined" => Ok(PlanEnforcementProfile::NativeUnconfined),
        "sandbox" => Ok(PlanEnforcementProfile::Sandbox),
        _ => Err(corruption_error(
            "A Control Store provider enforcement profile is invalid.",
        )),
    }
}

fn provider_evidence_digest(evidence: &PlannedProviderEvidence) -> UseResult<String> {
    provider_selection_digest(evidence).map_err(|error| {
        input_error(format!(
            "The canonical Control Store provider selection is invalid: {error}"
        ))
    })
}
