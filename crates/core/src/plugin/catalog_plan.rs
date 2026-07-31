use std::collections::BTreeSet;

use crate::{UseError, UseResult};

use super::{
    PlanPackageChangeKind, PlanPackageRole, PlannedPackageState, PlannedPackageTransition,
    PlannedPluginRelease, PluginPermissionCeiling, PluginPlanSource, PluginSurfaceRef,
    VerifiedPluginCatalogRecord,
};

impl VerifiedPluginCatalogRecord {
    /// Resolve one exact package state from complete signed catalog evidence
    /// and an
    /// observed or requested surface selection.
    pub fn selected_state(
        &self,
        requested_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageState> {
        self.validate()?;
        if !self.record.is_package_plan_ready() {
            return Err(catalog_plan_error(
                "A complete package state requires catalog-v2 or newer package evidence.",
            ));
        }
        let surfaces = self.record.resolve_surfaces(requested_surfaces)?;
        let selected = surfaces
            .iter()
            .map(|surface| surface.reference())
            .collect::<BTreeSet<_>>();
        let permissions = PluginPermissionCeiling {
            schema: self.record.permission_ceiling.schema.clone(),
            surfaces: self
                .record
                .permission_ceiling
                .surfaces
                .iter()
                .filter(|permission| selected.contains(&permission.surface))
                .cloned()
                .collect(),
        };
        let permission_ceiling_digest = permissions.descriptor_digest()?;
        let package_sha256 = self.record.package.sha256.clone().ok_or_else(|| {
            catalog_plan_error("The complete catalog record omitted its expanded package digest.")
        })?;
        let manifest_sha256 = self.record.package.manifest_sha256.clone().ok_or_else(|| {
            catalog_plan_error("The complete catalog record omitted its manifest digest.")
        })?;
        Ok(PlannedPackageState {
            release: PlannedPluginRelease {
                package_id: self.record.package_id.clone(),
                version: self.record.version.clone(),
                channel: self.record.channel,
                target: self.record.target.clone(),
                package_sha256,
                manifest_sha256,
                permission_ceiling_digest,
                surfaces,
            },
            permissions,
        })
    }

    /// Derive one exact install transition from complete signed catalog evidence.
    ///
    /// Surface selection narrows activation and permission evidence. It does
    /// not narrow the archive download or expanded package footprint.
    pub fn install_transition(
        &self,
        role: PlanPackageRole,
        requested_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        let state = self.selected_state(requested_surfaces)?;
        PlannedPackageTransition::resolved(
            self.record.package_id.clone(),
            role,
            PlanPackageChangeKind::Add,
            None,
            Some(state),
            Some(PluginPlanSource::Registry {
                provenance: self.provenance.clone(),
                archive: self.record.archive.clone(),
            }),
        )
    }

    /// Derive one exact uninstall transition from installed complete catalog
    /// evidence and the surface set observed in the current capability state.
    pub fn remove_transition(
        &self,
        role: PlanPackageRole,
        active_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        let before = self.selected_state(active_surfaces)?;
        PlannedPackageTransition::resolved(
            self.record.package_id.clone(),
            role,
            PlanPackageChangeKind::Remove,
            Some(before),
            None,
            None,
        )
    }

    /// Derive one exact registry upgrade transition. `self` is the candidate
    /// release; `installed` is the verified catalog evidence retained by the
    /// active receipt.
    pub fn replace_transition(
        &self,
        installed: &VerifiedPluginCatalogRecord,
        role: PlanPackageRole,
        active_surfaces: &[PluginSurfaceRef],
        requested_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PlannedPackageTransition> {
        if installed.record.package_id != self.record.package_id {
            return Err(catalog_plan_error(
                "An upgrade candidate must have the installed package identity.",
            ));
        }
        let before = installed.selected_state(active_surfaces)?;
        let after = self.selected_state(requested_surfaces)?;
        PlannedPackageTransition::resolved(
            self.record.package_id.clone(),
            role,
            PlanPackageChangeKind::Replace,
            Some(before),
            Some(after),
            Some(PluginPlanSource::Registry {
                provenance: self.provenance.clone(),
                archive: self.record.archive.clone(),
            }),
        )
    }
}

fn catalog_plan_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.catalog_plan_evidence_missing", message)
}
