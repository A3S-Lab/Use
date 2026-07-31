use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::{strictly_sorted_unique, valid_machine_id, valid_package_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginSurfaceRef,
    VerifiedPluginCatalogRecord, INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA, MAX_PLUGIN_PLAN_ITEMS,
};

const INSTALLED_PLAN_EVIDENCE_ERROR: &str = "use.plugin.installed_plan_evidence_invalid";

/// Package-specific installed evidence used to derive upgrade and uninstall
/// plans without trusting mutable paths or an incomplete capability summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledPluginPlanEvidence {
    pub schema: String,
    pub component_id: String,
    pub package_id: String,
    pub version: String,
    pub capability_generation: u64,
    pub capability_revision: String,
    pub receipt_digest: String,
    pub desired_enabled: bool,
    pub selected_surfaces: Vec<PluginSurfaceRef>,
    pub verified_catalog: VerifiedPluginCatalogRecord,
}

impl InstalledPluginPlanEvidence {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "installed plugin planning evidence",
            INSTALLED_PLAN_EVIDENCE_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        self.verified_catalog.validate().map_err(|_| {
            installed_evidence_error("The installed verified catalog evidence is invalid.")
        })?;
        let expected_component = format!("use/{}", self.package_id);
        if self.schema != INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA
            || !valid_package_id(&self.package_id)
            || !valid_machine_id(&self.component_id)
            || self.component_id != expected_component
            || self.version != self.verified_catalog.record.version
            || self.package_id != self.verified_catalog.record.package_id
            || !self.verified_catalog.record.is_package_plan_ready()
            || self.capability_generation == 0
            || !valid_raw_sha256(&self.capability_revision)
            || !valid_sha256(&self.receipt_digest)
            || self.selected_surfaces.is_empty()
            || self.selected_surfaces.len() > MAX_PLUGIN_PLAN_ITEMS
            || !strictly_sorted_unique(&self.selected_surfaces)
        {
            return Err(installed_evidence_error(
                "The installed plugin planning identity or state evidence is invalid.",
            ));
        }

        let selected = self
            .verified_catalog
            .selected_state(&self.selected_surfaces)
            .map_err(|_| {
                installed_evidence_error(
                    "The installed surface selection is not valid complete catalog closure evidence.",
                )
            })?;
        let resolved_surfaces = selected
            .release
            .surfaces
            .iter()
            .map(|surface| surface.reference())
            .collect::<Vec<_>>();
        if resolved_surfaces != self.selected_surfaces {
            return Err(installed_evidence_error(
                "The installed surface selection is not the exact resolved catalog closure.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "installed plugin planning evidence",
            INSTALLED_PLAN_EVIDENCE_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn valid_raw_sha256(value: &str) -> bool {
    valid_sha256(&format!("sha256:{value}"))
}

fn installed_evidence_error(message: impl Into<String>) -> UseError {
    contract_error(INSTALLED_PLAN_EVIDENCE_ERROR, message)
}
