use std::collections::BTreeSet;
use std::path::PathBuf;

use a3s_use_core::{
    InstallationId, PlanPackageRole, PlannedPackageState, PlannedPackageTransition,
    PluginPlanningBundle, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
    VerifiedPluginCatalogRecord,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::artifact_reference::{validate_receipt_artifact_reference, ExtensionArtifactReference};
use super::{plan_evidence_error, validate_catalog_binding, validate_surface_selection};
use crate::remote::ResolvedRemotePackage;
use crate::{ArtifactStore, ExtensionManifest};

pub const EXTENSION_RECEIPT_SCHEMA_VERSION: u32 = 6;
pub const MAX_EXTENSION_RECEIPT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionTrust {
    LocalExplicit,
    ReleaseBundle,
    RegistryTuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionReceipt {
    pub schema_version: u32,
    pub installation: InstallationId,
    pub package_id: String,
    pub component_id: String,
    /// Optional human-facing alias retained from the admitted manifest.
    /// Package ownership is carried only by the scoped lifecycle identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_alias: Option<String>,
    pub version: String,
    pub package_root: PathBuf,
    pub manifest_sha256: String,
    pub package_sha256: Option<String>,
    pub trust: ExtensionTrust,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<ResolvedRemotePackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_catalog: Option<VerifiedPluginCatalogRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_bundle: Option<PluginPlanningBundle>,
    /// Exact resolved surface set selected by the immutable lifecycle plan.
    pub selected_surfaces: Vec<PluginSurfaceRef>,
    pub installed_at_unix: u64,
    pub enabled: bool,
    pub lifecycle_generation: Option<u64>,
}

impl ExtensionReceipt {
    /// Canonical identity of the complete installed ownership and provenance
    /// record. Secret values are not part of extension receipts.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!("Failed to encode the canonical extension receipt: {error}"),
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    /// Validate this receipt's durable expanded-package reference without
    /// reading the referenced bytes. Missing physical content remains a fact
    /// for the global reachability join rather than making the reference
    /// disappear.
    pub fn artifact_reference(
        &self,
        artifact_store: &ArtifactStore,
    ) -> UseResult<ExtensionArtifactReference> {
        validate_receipt_artifact_reference(self, artifact_store)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtension {
    pub receipt: ExtensionReceipt,
    pub manifest: ExtensionManifest,
}

impl InstalledExtension {
    pub fn surfaces(&self) -> Vec<&'static str> {
        let selected = self
            .receipt
            .selected_surfaces
            .iter()
            .map(|surface| surface.kind)
            .collect::<BTreeSet<_>>();
        [
            (PluginSurfaceKind::Tool, "tool"),
            (PluginSurfaceKind::Mcp, "mcp"),
            (PluginSurfaceKind::Okf, "okf"),
            (PluginSurfaceKind::Flow, "flow"),
            (PluginSurfaceKind::Skill, "skill"),
            (PluginSurfaceKind::Ui, "ui"),
        ]
        .into_iter()
        .filter_map(|(kind, name)| selected.contains(&kind).then_some(name))
        .collect()
    }

    /// Return the exact surface set selected by the reviewed lifecycle plan.
    pub fn selected_surfaces(&self) -> UseResult<Vec<PluginSurfaceRef>> {
        let selected = self.receipt.selected_surfaces.clone();
        validate_surface_selection(
            &self.manifest,
            self.receipt.verified_catalog.as_ref(),
            &selected,
        )?;
        Ok(selected)
    }

    pub fn enabled(&self) -> bool {
        self.receipt.enabled
    }

    pub fn supports_use_version(&self, version: &str) -> bool {
        self.manifest.supports_use_version(version).unwrap_or(false)
    }

    /// Return the verified package-planning evidence retained by this
    /// installed package after checking its internal receipt bindings.
    pub fn plan_ready_catalog(&self) -> UseResult<&VerifiedPluginCatalogRecord> {
        let catalog = self.receipt.verified_catalog.as_ref().ok_or_else(|| {
            plan_evidence_error(
                "The installed extension does not retain verified package-planning evidence.",
            )
        })?;
        if self.receipt.schema_version != EXTENSION_RECEIPT_SCHEMA_VERSION
            || self.receipt.trust != ExtensionTrust::RegistryTuf
        {
            return Err(plan_evidence_error(
                "The installed extension receipt is not plan-ready registry state.",
            ));
        }
        validate_catalog_binding(
            catalog,
            self.receipt.registry.as_ref(),
            &self.manifest,
            &self.receipt.manifest_sha256,
            self.receipt.package_sha256.as_deref().ok_or_else(|| {
                plan_evidence_error("The cognitive-package receipt omitted its package digest.")
            })?,
        )?;
        Ok(catalog)
    }

    /// Return the signed executable-planning target retained at installation.
    ///
    /// Static packages legitimately return `None`. A package whose catalog
    /// declares executable planning must retain the exact validated bundle so
    /// enablement can be reviewed offline without consulting a mutable
    /// Registry again.
    pub fn plan_ready_planning_bundle(&self) -> UseResult<Option<&PluginPlanningBundle>> {
        let catalog = self.plan_ready_catalog()?;
        match (&catalog.record.planning, &self.receipt.planning_bundle) {
            (None, None) => Ok(None),
            (Some(_), Some(bundle)) => {
                bundle.validate_catalog_binding(catalog)?;
                Ok(Some(bundle))
            }
            _ => Err(plan_evidence_error(
                "The installed extension receipt does not retain its exact signed planning bundle.",
            )),
        }
    }

    /// Resolve the exact installed package state using active surfaces
    /// observed by the capability snapshot.
    pub fn planned_state(
        &self,
        active_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageState> {
        self.plan_ready_catalog()?.selected_state(active_surfaces)
    }

    pub fn remove_transition(
        &self,
        role: PlanPackageRole,
        active_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        self.plan_ready_catalog()?
            .remove_transition(role, active_surfaces)
    }

    pub fn replace_transition(
        &self,
        candidate: &VerifiedPluginCatalogRecord,
        role: PlanPackageRole,
        active_surfaces: &[PluginSurfaceRef],
        requested_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        candidate.replace_transition(
            self.plan_ready_catalog()?,
            role,
            active_surfaces,
            requested_surfaces,
        )
    }
}
