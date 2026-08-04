use std::collections::{BTreeMap, BTreeSet};

use semver::Version;

use crate::UseResult;

use super::plan::{
    plan_error, PlanPackageChangeKind, PlannedPackageState, PlannedPackageTransition,
    PlannedPluginRelease, PlannedSurfaceChange, PluginPlanSource, SurfaceChangeKind,
};
use super::validation::{valid_package_id, valid_sha256, valid_target};
use super::{
    CatalogMcpTransport, CatalogSurface, PluginPermissionCeiling, PluginSurfaceKind,
    PluginSurfaceRef, ToolWorkloadClass, MAX_PLUGIN_PLAN_ITEMS,
};

impl PlannedPackageTransition {
    /// Build one exact package transition and derive its complete surface delta.
    pub fn resolved(
        package_id: impl Into<String>,
        role: super::PlanPackageRole,
        change: PlanPackageChangeKind,
        before: Option<PlannedPackageState>,
        after: Option<PlannedPackageState>,
        source: Option<PluginPlanSource>,
    ) -> UseResult<Self> {
        let surfaces = if change == PlanPackageChangeKind::Retain {
            Vec::new()
        } else {
            surface_changes(before.as_ref(), after.as_ref())?
        };
        let transition = Self {
            package_id: package_id.into(),
            role,
            change,
            before,
            after,
            source,
            surfaces,
        };
        transition.validate()?;
        Ok(transition)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        if !valid_package_id(&self.package_id) || self.surfaces.len() > MAX_PLUGIN_PLAN_ITEMS {
            return Err(plan_error("A planned package transition is invalid."));
        }
        if let Some(before) = &self.before {
            before.validate(&self.package_id)?;
        }
        if let Some(after) = &self.after {
            after.validate(&self.package_id)?;
        }

        let shape_is_valid = match self.change {
            PlanPackageChangeKind::Add => {
                self.before.is_none() && self.after.is_some() && self.source.is_some()
            }
            PlanPackageChangeKind::Remove => {
                self.before.is_some() && self.after.is_none() && self.source.is_none()
            }
            PlanPackageChangeKind::Replace => {
                self.before.is_some()
                    && self.after.is_some()
                    && self.before != self.after
                    && self.source.is_some()
            }
            PlanPackageChangeKind::Retain => {
                self.before.is_some()
                    && self.before == self.after
                    && self.source.is_none()
                    && self.surfaces.is_empty()
            }
        };
        if !shape_is_valid {
            return Err(plan_error(
                "A planned package transition has an inconsistent before, after, or source state.",
            ));
        }
        if let (Some(source), Some(after)) = (&self.source, &self.after) {
            source.validate(after)?;
        }
        self.validate_surface_changes()
    }

    fn validate_surface_changes(&self) -> UseResult<()> {
        if self.change == PlanPackageChangeKind::Retain {
            return Ok(());
        }
        let expected = surface_changes(self.before.as_ref(), self.after.as_ref())?;
        if self.surfaces != expected {
            return Err(plan_error(
                "Planned surface changes do not equal the resolved package surface delta.",
            ));
        }
        Ok(())
    }
}

fn surface_changes(
    before: Option<&PlannedPackageState>,
    after: Option<&PlannedPackageState>,
) -> UseResult<Vec<PlannedSurfaceChange>> {
    let before = surface_digests(before)?;
    let after = surface_digests(after)?;
    let references = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut expected = Vec::with_capacity(references.len());
    for surface in references {
        let before_digest = before.get(&surface).cloned();
        let after_digest = after.get(&surface).cloned();
        let change = match (before_digest.is_some(), after_digest.is_some()) {
            (false, true) => SurfaceChangeKind::Add,
            (true, false) => SurfaceChangeKind::Remove,
            (true, true) => SurfaceChangeKind::Replace,
            (false, false) => {
                return Err(plan_error(
                    "A planned surface change has no before or after descriptor.",
                ));
            }
        };
        expected.push(PlannedSurfaceChange {
            surface,
            change,
            before_digest,
            after_digest,
        });
    }
    Ok(expected)
}

impl PlannedPackageState {
    fn validate(&self, package_id: &str) -> UseResult<()> {
        self.release.validate(package_id)?;
        self.permissions
            .validate()
            .map_err(|_| plan_error("A planned package permission ceiling is invalid."))?;
        if self.permissions.descriptor_digest()? != self.release.permission_ceiling_digest {
            return Err(plan_error(
                "A planned package permission digest does not match its content.",
            ));
        }
        validate_surface_permissions(&self.release.surfaces, &self.permissions)
    }
}

impl PlannedPluginRelease {
    fn validate(&self, package_id: &str) -> UseResult<()> {
        if self.package_id != package_id
            || Version::parse(&self.version)
                .map(|value| value.to_string() != self.version)
                .unwrap_or(true)
            || !valid_target(&self.target)
            || !valid_sha256(&self.package_sha256)
            || !valid_sha256(&self.manifest_sha256)
            || !valid_sha256(&self.permission_ceiling_digest)
            || self.surfaces.is_empty()
            || self.surfaces.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(plan_error("A planned plugin release identity is invalid."));
        }
        let mut previous = None;
        for surface in &self.surfaces {
            surface
                .validate()
                .map_err(|_| plan_error("A planned plugin surface is invalid."))?;
            let reference = surface.reference();
            if previous.as_ref().is_some_and(|value| value >= &reference) {
                return Err(plan_error(
                    "Planned plugin surfaces must be sorted and unique.",
                ));
            }
            previous = Some(reference);
        }
        Ok(())
    }
}

impl PluginPlanSource {
    fn validate(&self, after: &PlannedPackageState) -> UseResult<()> {
        match self {
            Self::Registry {
                provenance,
                archive,
            } => {
                if provenance.validate().is_err() {
                    return Err(plan_error("A registry source proof is invalid."));
                }
                archive
                    .validate(
                        &after.release.package_id,
                        &after.release.version,
                        after.release.channel,
                        &after.release.target,
                    )
                    .map_err(|_| plan_error("A registry archive proof is invalid."))
            }
            Self::ReleaseBundle {
                bundle_digest,
                package_digest,
            } => {
                if !valid_sha256(bundle_digest)
                    || !valid_sha256(package_digest)
                    || package_digest != &after.release.package_sha256
                {
                    return Err(plan_error("A release bundle source proof is invalid."));
                }
                Ok(())
            }
            Self::LocalReviewed {
                source_digest,
                package_digest,
                unsigned,
            } => {
                if !*unsigned
                    || !valid_sha256(source_digest)
                    || !valid_sha256(package_digest)
                    || package_digest != &after.release.package_sha256
                {
                    return Err(plan_error("A reviewed local source proof is invalid."));
                }
                Ok(())
            }
        }
    }
}

fn validate_surface_permissions(
    surfaces: &[CatalogSurface],
    permissions: &PluginPermissionCeiling,
) -> UseResult<()> {
    let surface_map = surfaces
        .iter()
        .map(|surface| (surface.reference(), surface))
        .collect::<BTreeMap<_, _>>();
    for permission in &permissions.surfaces {
        let Some(surface) = surface_map.get(&permission.surface) else {
            return Err(plan_error(
                "A planned permission references an absent package surface.",
            ));
        };
        for binding in &permission.ui_http {
            let tool_ref = PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: binding.tool_id.clone(),
            };
            if surface_map.get(&tool_ref).and_then(|value| value.workload)
                != Some(ToolWorkloadClass::Service)
            {
                return Err(plan_error(
                    "A planned UI permission must bind a Tool Service in the same package.",
                ));
            }
        }
        let resources = permission.resources.as_ref();
        let long_running = resources.is_some_and(|value| {
            value.task_timeout_ms.is_none()
                && value.max_stdout_bytes.is_none()
                && value.max_stderr_bytes.is_none()
        });
        let matches_surface = match surface.kind {
            PluginSurfaceKind::Tool => match surface.workload {
                Some(ToolWorkloadClass::Task) => {
                    !permission.private_service
                        && resources.is_some_and(|value| {
                            value.task_timeout_ms.is_some()
                                && value.max_stdout_bytes.is_some()
                                && value.max_stderr_bytes.is_some()
                        })
                }
                Some(ToolWorkloadClass::Service) => {
                    !permission.native_execution && permission.private_service && long_running
                }
                None => false,
            },
            PluginSurfaceKind::Mcp => match surface.mcp_transport {
                Some(CatalogMcpTransport::Stdio) => {
                    permission.native_execution && !permission.private_service && long_running
                }
                Some(CatalogMcpTransport::StreamableHttp) => {
                    !permission.native_execution && permission.private_service && long_running
                }
                None => false,
            },
            PluginSurfaceKind::Ui => true,
            PluginSurfaceKind::Flow | PluginSurfaceKind::Okf | PluginSurfaceKind::Skill => false,
        };
        if !matches_surface {
            return Err(plan_error(
                "A planned permission ceiling does not match its surface workload.",
            ));
        }
    }
    for surface in surfaces {
        if matches!(
            surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        ) && !permissions
            .surfaces
            .iter()
            .any(|permission| permission.surface == surface.reference())
        {
            return Err(plan_error(
                "Every planned executable surface requires a permission ceiling.",
            ));
        }
    }
    Ok(())
}

fn surface_digests(
    state: Option<&PlannedPackageState>,
) -> UseResult<BTreeMap<PluginSurfaceRef, String>> {
    let mut digests = BTreeMap::new();
    if let Some(state) = state {
        for surface in &state.release.surfaces {
            digests.insert(surface.reference(), surface.descriptor_digest()?);
        }
    }
    Ok(digests)
}
