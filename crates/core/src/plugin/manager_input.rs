use std::collections::BTreeSet;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::{valid_segment, valid_sha256};
use super::{
    InstallationId, PlanScope, PlanScopeKind, PluginOperationPlan, PluginPackageId,
    PluginReleaseChannel, PluginSurfaceKind, PluginSurfaceRef,
    MAX_PLUGIN_HOST_OPERATION_WATCH_TIMEOUT_MS,
};

const MANAGER_INPUT_ERROR: &str = "use.plugin.manager_input_invalid";
const MAX_QUERY_BYTES: usize = 256;
const MAX_CURSOR_BYTES: usize = 512;
const MAX_VERSION_SELECTOR_BYTES: usize = 64;
const MAX_SELECTED_SURFACES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerSearchInput {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PluginSurfaceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<PluginReleaseChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerInspectInput {
    pub package_id: PluginPackageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<PluginReleaseChannel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerListInstalledInput {
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerPackageScopeInput {
    pub package_id: PluginPackageId,
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerInstallPlanInput {
    pub package_id: PluginPackageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_requirement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<PluginReleaseChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surfaces: Option<Vec<PluginSurfaceRef>>,
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerUpgradePlanInput {
    pub package_id: PluginPackageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_requirement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<PluginReleaseChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surfaces: Option<Vec<PluginSurfaceRef>>,
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerApplyPlanInput {
    pub operation_id: String,
    pub plan_digest: String,
}

/// Exact identity of one reviewed Plugin Manager operation.
///
/// Operation status and cancellation deliberately require the package, scope,
/// operation ID, and plan digest together.  A caller cannot accidentally
/// observe or cancel a different operation that happens to reuse an ID in
/// another managed scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerOperationInput {
    pub package_id: PluginPackageId,
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
    pub operation_id: String,
    pub plan_digest: String,
}

/// Long-poll options for one exact reviewed Plugin Manager operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerOperationWatchInput {
    pub package_id: PluginPackageId,
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
    pub operation_id: String,
    pub plan_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_revision: Option<String>,
    #[serde(default)]
    pub timeout_ms: u64,
}

impl PluginManagerSearchInput {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_text(&self.query, MAX_QUERY_BYTES)
            || self.limit.is_some_and(|limit| limit == 0 || limit > 50)
            || self
                .cursor
                .as_deref()
                .is_some_and(|cursor| !valid_cursor(cursor))
        {
            return Err(manager_input_error(
                "The plugin catalog query, cursor, or page limit is invalid.",
            ));
        }
        Ok(())
    }

    pub fn page_limit(&self) -> u16 {
        self.limit.unwrap_or(20)
    }
}

impl PluginManagerInspectInput {
    pub fn validate(&self) -> UseResult<()> {
        if self.version.as_deref().is_some_and(|version| {
            version.len() > MAX_VERSION_SELECTOR_BYTES
                || Version::parse(version).is_err()
                || Version::parse(version).is_ok_and(|parsed| parsed.to_string() != version)
        }) {
            return Err(manager_input_error(
                "The inspected plugin version must be canonical SemVer.",
            ));
        }
        Ok(())
    }
}

impl PluginManagerListInstalledInput {
    pub fn validate(&self) -> UseResult<()> {
        validate_scope(self.scope_kind, &self.scope_id)?;
        if self.limit.is_some_and(|limit| limit == 0 || limit > 100)
            || self
                .cursor
                .as_deref()
                .is_some_and(|cursor| !valid_cursor(cursor))
        {
            return Err(manager_input_error(
                "The installed-plugin cursor or page limit is invalid.",
            ));
        }
        Ok(())
    }

    pub fn scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }

    pub fn page_limit(&self) -> usize {
        usize::from(self.limit.unwrap_or(50))
    }
}

impl PluginManagerPackageScopeInput {
    pub fn validate(&self) -> UseResult<()> {
        validate_scope(self.scope_kind, &self.scope_id)
    }

    pub fn scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }
}

impl PluginManagerInstallPlanInput {
    pub fn validate(&self) -> UseResult<()> {
        validate_scope(self.scope_kind, &self.scope_id)?;
        if self
            .registry_name
            .as_deref()
            .is_some_and(|name| !valid_segment(name))
        {
            return Err(manager_input_error(
                "The selected Registry name is invalid.",
            ));
        }
        validate_version_requirement(self.version_requirement.as_deref())?;
        validate_surfaces(self.surfaces.as_deref())
    }

    pub fn scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }

    pub fn canonical_surfaces(&self) -> Vec<PluginSurfaceRef> {
        canonical_surfaces(self.surfaces.as_deref().unwrap_or_default())
    }

    pub fn canonical_version_requirement(&self) -> Option<String> {
        canonical_version_requirement(self.version_requirement.as_deref())
    }
}

impl PluginManagerUpgradePlanInput {
    pub fn validate(&self) -> UseResult<()> {
        validate_scope(self.scope_kind, &self.scope_id)?;
        validate_version_requirement(self.version_requirement.as_deref())?;
        validate_surfaces(self.surfaces.as_deref())
    }

    pub fn scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }

    pub fn canonical_surfaces(&self) -> Vec<PluginSurfaceRef> {
        canonical_surfaces(self.surfaces.as_deref().unwrap_or_default())
    }

    pub fn canonical_version_requirement(&self) -> Option<String> {
        canonical_version_requirement(self.version_requirement.as_deref())
    }
}

impl PluginManagerApplyPlanInput {
    pub fn validate(&self) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            manager_input_error("The applied plugin operation identity is invalid.")
        })?;
        if !valid_sha256(&self.plan_digest) {
            return Err(manager_input_error(
                "The applied plugin plan digest is invalid.",
            ));
        }
        Ok(())
    }
}

impl PluginManagerOperationInput {
    pub fn validate(&self) -> UseResult<()> {
        validate_scope(self.scope_kind, &self.scope_id)?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            manager_input_error("The observed plugin operation identity is invalid.")
        })?;
        if !valid_sha256(&self.plan_digest) {
            return Err(manager_input_error(
                "The observed plugin operation plan digest is invalid.",
            ));
        }
        Ok(())
    }

    pub fn scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }
}

impl PluginManagerOperationWatchInput {
    pub fn validate(&self) -> UseResult<()> {
        PluginManagerOperationInput {
            package_id: self.package_id.clone(),
            scope_kind: self.scope_kind,
            scope_id: self.scope_id.clone(),
            operation_id: self.operation_id.clone(),
            plan_digest: self.plan_digest.clone(),
        }
        .validate()?;
        if self.timeout_ms > MAX_PLUGIN_HOST_OPERATION_WATCH_TIMEOUT_MS
            || self
                .after_revision
                .as_deref()
                .is_some_and(|revision| !valid_sha256(revision))
        {
            return Err(manager_input_error(
                "The plugin operation watch revision or timeout is invalid.",
            ));
        }
        Ok(())
    }

    pub fn scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }
}

fn validate_scope(kind: PlanScopeKind, id: &str) -> UseResult<()> {
    InstallationId::new(kind, id)
        .map(|_| ())
        .map_err(|_| manager_input_error("The plugin manager installation identity is invalid."))
}

fn validate_version_requirement(requirement: Option<&str>) -> UseResult<()> {
    if requirement.is_some_and(|requirement| {
        requirement.is_empty()
            || requirement.len() > MAX_VERSION_SELECTOR_BYTES
            || VersionReq::parse(requirement).is_err()
    }) {
        return Err(manager_input_error(
            "The plugin version requirement is invalid.",
        ));
    }
    Ok(())
}

fn canonical_version_requirement(requirement: Option<&str>) -> Option<String> {
    requirement.and_then(|requirement| {
        Version::parse(requirement)
            .ok()
            .map(|version| version.to_string())
            .or_else(|| {
                VersionReq::parse(requirement)
                    .ok()
                    .map(|requirement| requirement.to_string())
            })
    })
}

fn validate_surfaces(surfaces: Option<&[PluginSurfaceRef]>) -> UseResult<()> {
    let Some(surfaces) = surfaces else {
        return Ok(());
    };
    let unique = surfaces.iter().collect::<BTreeSet<_>>();
    if surfaces.is_empty()
        || surfaces.len() > MAX_SELECTED_SURFACES
        || unique.len() != surfaces.len()
        || surfaces.iter().any(|surface| !valid_segment(&surface.id))
    {
        return Err(manager_input_error(
            "Selected plugin surfaces must be bounded, canonical, and unique.",
        ));
    }
    Ok(())
}

fn canonical_surfaces(surfaces: &[PluginSurfaceRef]) -> Vec<PluginSurfaceRef> {
    let mut surfaces = surfaces.to_vec();
    surfaces.sort();
    surfaces
}

fn valid_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_CURSOR_BYTES && !value.chars().any(char::is_control)
}

fn manager_input_error(message: impl Into<String>) -> UseError {
    UseError::new(MANAGER_INPUT_ERROR, message)
}
