use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{OkfBundleContract, UseResult};

use super::validation::{
    strictly_sorted_unique, valid_catalog_text, valid_package_id, valid_repository_url,
    valid_sha256, valid_spdx_expression, valid_tag, valid_target, valid_target_name,
};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginPackageDependency,
    PluginPermissionCeiling, PluginSurfaceKind, PluginSurfaceRef, ToolWorkloadClass,
    PLUGIN_CATALOG_SCHEMA, PLUGIN_CATALOG_SCHEMA_V2, PLUGIN_CATALOG_SCHEMA_V3,
};

pub(super) const CATALOG_ERROR: &str = "use.plugin.catalog_invalid";
pub(super) const MAX_CATALOG_SURFACES: usize = 256;
const MAX_REMOTE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PLANNING_TARGET_BYTES: u64 = 512 * 1024;
const MAX_EXPANDED_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PACKAGE_FILES: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogRecord {
    pub schema: String,
    pub package_id: String,
    pub display_name: String,
    pub description: String,
    pub publisher: String,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub version: String,
    pub channel: PluginReleaseChannel,
    pub requires_use: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginPackageDependency>,
    pub target: String,
    pub surfaces: Vec<CatalogSurface>,
    pub permission_ceiling: PluginPermissionCeiling,
    pub permission_ceiling_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning: Option<CatalogPlanningTarget>,
    pub archive: CatalogArchive,
    pub package: CatalogPackage,
    pub license: String,
    pub repository: String,
    pub availability: CatalogAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginReleaseChannel {
    Beta,
    Nightly,
    Stable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSurface {
    pub kind: PluginSurfaceKind,
    pub id: String,
    pub optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload: Option<ToolWorkloadClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_transport: Option<CatalogMcpTransport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_tool_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub okf_bundle: Option<OkfBundleContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<PluginSurfaceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogMcpTransport {
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogArchive {
    pub target_name: String,
    pub length: u64,
    pub sha256: String,
}

/// Exact small TUF target containing executable-surface planning evidence.
///
/// Keeping this separate from the package archive lets clients review Tool and
/// MCP Runtime plans without downloading or expanding the full plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogPlanningTarget {
    pub target_name: String,
    pub length: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogPackage {
    pub expanded_bytes: u64,
    pub file_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogAvailability {
    Available,
    Deprecated {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        replacement: Option<String>,
    },
    Withdrawn {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        advisory_url: Option<String>,
    },
}

impl PluginCatalogRecord {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin catalog record",
            CATALOG_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        let schema_v1 = self.schema == PLUGIN_CATALOG_SCHEMA;
        let schema_v2 = self.schema == PLUGIN_CATALOG_SCHEMA_V2;
        let schema_v3 = self.schema == PLUGIN_CATALOG_SCHEMA_V3;
        let complete_package_schema = schema_v2 || schema_v3;
        if (!schema_v1 && !complete_package_schema)
            || !valid_package_id(&self.package_id)
            || !valid_catalog_text(&self.display_name, 128)
            || !valid_catalog_text(&self.description, 2048)
            || !super::validation::valid_segment(&self.publisher)
            || self.package_id.split('/').next() != Some(self.publisher.as_str())
            || self.keywords.len() > 64
            || self.categories.len() > 64
            || !strictly_sorted_unique(&self.keywords)
            || !strictly_sorted_unique(&self.categories)
            || self.keywords.iter().any(|value| !valid_tag(value))
            || self.categories.iter().any(|value| !valid_tag(value))
            || Version::parse(&self.version)
                .map(|version| version.to_string() != self.version)
                .unwrap_or(true)
            || VersionReq::parse(&self.requires_use).is_err()
            || !valid_target(&self.target)
            || self.surfaces.is_empty()
            || self.surfaces.len() > MAX_CATALOG_SURFACES
        {
            return Err(catalog_error(
                "The plugin catalog identity, search metadata, compatibility, or target is invalid.",
            ));
        }
        if (!schema_v3 && !self.dependencies.is_empty())
            || PluginPackageDependency::validate_set(&self.package_id, &self.dependencies).is_err()
        {
            return Err(catalog_error(
                "Package dependencies require catalog schema version 3 and a canonical sorted dependency set.",
            ));
        }

        let mut surface_refs = BTreeSet::new();
        let mut tool_classes = BTreeMap::new();
        let mut mcp_transports = BTreeMap::new();
        let mut previous = None;
        for surface in &self.surfaces {
            surface.validate()?;
            let reference = PluginSurfaceRef {
                kind: surface.kind,
                id: surface.id.clone(),
            };
            if previous.as_ref().is_some_and(|value| value >= &reference)
                || !surface_refs.insert(reference.clone())
            {
                return Err(catalog_error(
                    "Catalog surfaces must be sorted and unique by kind and ID.",
                ));
            }
            previous = Some(reference.clone());
            if let Some(workload) = surface.workload {
                tool_classes.insert(surface.id.as_str(), workload);
            }
            if let Some(transport) = surface.mcp_transport {
                mcp_transports.insert(surface.id.as_str(), transport);
            }
        }
        if !schema_v3
            && self.surfaces.iter().any(|surface| {
                matches!(
                    surface.kind,
                    PluginSurfaceKind::Flow | PluginSurfaceKind::Okf
                )
            })
        {
            return Err(catalog_error(
                "Flow and OKF catalog surfaces require plugin catalog schema version 3.",
            ));
        }
        self.validate_surface_dependencies(&surface_refs, complete_package_schema)?;

        self.permission_ceiling
            .validate()
            .map_err(|_| catalog_error("The catalog permission ceiling is invalid."))?;
        if self.permission_ceiling.descriptor_digest()? != self.permission_ceiling_digest {
            return Err(catalog_error(
                "The catalog permission ceiling digest does not match its content.",
            ));
        }
        for permission in &self.permission_ceiling.surfaces {
            if !surface_refs.contains(&permission.surface) {
                return Err(catalog_error(
                    "The permission ceiling references a surface absent from the catalog.",
                ));
            }
            for binding in &permission.ui_http {
                if tool_classes.get(binding.tool_id.as_str()) != Some(&ToolWorkloadClass::Service) {
                    return Err(catalog_error(
                        "A UI HTTP permission must bind a cataloged Tool Service.",
                    ));
                }
                let tool = PluginSurfaceRef {
                    kind: PluginSurfaceKind::Tool,
                    id: binding.tool_id.clone(),
                };
                if complete_package_schema
                    && !self
                        .surfaces
                        .iter()
                        .find(|surface| surface.reference() == permission.surface)
                        .is_some_and(|surface| surface.requires.contains(&tool))
                {
                    return Err(catalog_error(
                        "A complete plugin catalog UI HTTP binding must declare its Tool dependency.",
                    ));
                }
            }
            let resources = permission.resources.as_ref();
            let long_running_resources = resources.is_some_and(|value| {
                value.task_timeout_ms.is_none()
                    && value.max_stdout_bytes.is_none()
                    && value.max_stderr_bytes.is_none()
            });
            match permission.surface.kind {
                PluginSurfaceKind::Tool => match tool_classes.get(permission.surface.id.as_str()) {
                    Some(ToolWorkloadClass::Task)
                        if !permission.private_service
                            && resources.is_some_and(|value| {
                                value.task_timeout_ms.is_some()
                                    && value.max_stdout_bytes.is_some()
                                    && value.max_stderr_bytes.is_some()
                            }) => {}
                    Some(ToolWorkloadClass::Service)
                        if !permission.native_execution
                            && permission.private_service
                            && long_running_resources => {}
                    _ => {
                        return Err(catalog_error(
                                "A Tool permission ceiling does not match its Task or Service workload.",
                            ));
                    }
                },
                PluginSurfaceKind::Mcp => {
                    match mcp_transports.get(permission.surface.id.as_str()) {
                        Some(CatalogMcpTransport::Stdio)
                            if permission.native_execution
                                && !permission.private_service
                                && long_running_resources => {}
                        Some(CatalogMcpTransport::StreamableHttp)
                            if !permission.native_execution
                                && permission.private_service
                                && long_running_resources => {}
                        _ => {
                            return Err(catalog_error(
                                "An MCP permission ceiling does not match its declared transport.",
                            ));
                        }
                    }
                }
                PluginSurfaceKind::Ui => {}
                PluginSurfaceKind::Flow | PluginSurfaceKind::Okf | PluginSurfaceKind::Skill => {
                    return Err(catalog_error(
                        "Flow, OKF, and Skill surfaces cannot carry runtime permission ceilings.",
                    ));
                }
            }
        }
        for surface in &self.surfaces {
            if matches!(
                surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            ) && !self.permission_ceiling.surfaces.iter().any(|permission| {
                permission.surface.kind == surface.kind && permission.surface.id == surface.id
            }) {
                return Err(catalog_error(
                    "Every executable Tool and MCP surface requires a permission ceiling.",
                ));
            }
        }

        self.archive
            .validate(&self.package_id, &self.version, self.channel, &self.target)?;
        let has_executable = self.surfaces.iter().any(|surface| {
            matches!(
                surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            )
        });
        match (&self.planning, schema_v3, has_executable) {
            (Some(planning), true, true) => {
                planning.validate(&self.package_id, &self.version, self.channel, &self.target)?
            }
            (None, false, _) | (None, true, false) => {}
            _ => {
                return Err(catalog_error(
                    "Catalog-v3 records must carry one planning target exactly when they contain executable surfaces.",
                ))
            }
        }
        self.package.validate(complete_package_schema)?;
        if !valid_spdx_expression(&self.license) || !valid_repository_url(&self.repository) {
            return Err(catalog_error(
                "The plugin catalog license or repository identity is invalid.",
            ));
        }
        self.availability.validate(&self.package_id)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin catalog record", CATALOG_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn is_package_plan_ready(&self) -> bool {
        matches!(
            self.schema.as_str(),
            PLUGIN_CATALOG_SCHEMA_V2 | PLUGIN_CATALOG_SCHEMA_V3
        )
    }

    fn validate_surface_dependencies(
        &self,
        surface_refs: &BTreeSet<PluginSurfaceRef>,
        schema_v2: bool,
    ) -> UseResult<()> {
        super::catalog_selection::validate_surface_dependencies(
            &self.surfaces,
            surface_refs,
            schema_v2,
        )
    }
}

impl CatalogSurface {
    pub(super) fn validate(&self) -> UseResult<()> {
        if !super::validation::valid_segment(&self.id) {
            return Err(catalog_error("A catalog surface ID is invalid."));
        }
        match self.kind {
            PluginSurfaceKind::Tool
                if self.workload.is_some()
                    && self.mcp_transport.is_none()
                    && self.mcp_tool_count.is_none()
                    && self.okf_bundle.is_none() =>
            {
                Ok(())
            }
            PluginSurfaceKind::Mcp
                if self.workload.is_none()
                    && self.mcp_transport.is_some()
                    && self.mcp_tool_count.is_none_or(|count| count <= 10_000)
                    && self.okf_bundle.is_none() =>
            {
                Ok(())
            }
            PluginSurfaceKind::Flow | PluginSurfaceKind::Skill | PluginSurfaceKind::Ui
                if self.workload.is_none()
                    && self.mcp_transport.is_none()
                    && self.mcp_tool_count.is_none()
                    && self.okf_bundle.is_none() =>
            {
                Ok(())
            }
            PluginSurfaceKind::Okf
                if self.workload.is_none()
                    && self.mcp_transport.is_none()
                    && self.mcp_tool_count.is_none()
                    && self
                        .okf_bundle
                        .as_ref()
                        .is_some_and(|bundle| bundle.validate().is_ok()) =>
            {
                Ok(())
            }
            _ => Err(catalog_error(
                "Catalog surface workload metadata does not match its surface kind.",
            )),
        }
    }

    pub fn reference(&self) -> PluginSurfaceRef {
        PluginSurfaceRef {
            kind: self.kind,
            id: self.id.clone(),
        }
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin catalog surface", CATALOG_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl CatalogArchive {
    pub(super) fn validate(
        &self,
        package_id: &str,
        version: &str,
        channel: PluginReleaseChannel,
        target: &str,
    ) -> UseResult<()> {
        let channel = channel.as_str();
        let prefix = format!("extensions/{package_id}/{version}/{channel}/{target}/");
        if self.length == 0
            || self.length > MAX_REMOTE_ARCHIVE_BYTES
            || !valid_sha256(&self.sha256)
            || !valid_target_name(&self.target_name)
            || !self.target_name.starts_with(&prefix)
        {
            return Err(catalog_error(
                "The plugin catalog archive identity or size is invalid.",
            ));
        }
        Ok(())
    }
}

impl CatalogPlanningTarget {
    pub(super) fn validate(
        &self,
        package_id: &str,
        version: &str,
        channel: PluginReleaseChannel,
        target: &str,
    ) -> UseResult<()> {
        let expected_target_name = format!(
            "extensions/{package_id}/{version}/{}/{target}/planning-v1.json",
            channel.as_str()
        );
        if self.length == 0
            || self.length > MAX_PLANNING_TARGET_BYTES
            || !valid_sha256(&self.sha256)
            || self.target_name != expected_target_name
        {
            return Err(catalog_error(
                "The plugin catalog planning target identity or size is invalid.",
            ));
        }
        Ok(())
    }
}

impl CatalogPackage {
    fn validate(&self, schema_v2: bool) -> UseResult<()> {
        if self.expanded_bytes == 0
            || self.expanded_bytes > MAX_EXPANDED_PACKAGE_BYTES
            || self.file_count == 0
            || self.file_count > MAX_PACKAGE_FILES
            || self
                .sha256
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
            || self
                .manifest_sha256
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
            || (schema_v2 && self.sha256.is_none())
            || schema_v2 != self.manifest_sha256.is_some()
        {
            return Err(catalog_error(
                "The plugin catalog package estimate or digest is invalid.",
            ));
        }
        Ok(())
    }
}

impl CatalogAvailability {
    fn validate(&self, package_id: &str) -> UseResult<()> {
        match self {
            Self::Available => Ok(()),
            Self::Deprecated {
                message,
                replacement,
            } => {
                if !valid_catalog_text(message, 1024)
                    || replacement
                        .as_deref()
                        .is_some_and(|value| value == package_id || !valid_package_id(value))
                {
                    return Err(catalog_error("The plugin deprecation metadata is invalid."));
                }
                Ok(())
            }
            Self::Withdrawn {
                reason,
                advisory_url,
            } => {
                if !valid_catalog_text(reason, 1024)
                    || advisory_url
                        .as_deref()
                        .is_some_and(|value| !valid_repository_url(value))
                {
                    return Err(catalog_error("The plugin withdrawal metadata is invalid."));
                }
                Ok(())
            }
        }
    }
}

impl PluginReleaseChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Beta => "beta",
            Self::Nightly => "nightly",
            Self::Stable => "stable",
        }
    }
}

pub(super) fn catalog_error(message: impl Into<String>) -> crate::UseError {
    contract_error(CATALOG_ERROR, message)
}
