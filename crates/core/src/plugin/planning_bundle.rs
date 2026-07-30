use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    McpReleaseDescriptor, ToolReleaseDescriptor, ToolWorkloadContract, UseError, UseResult,
};

use super::validation::{
    strictly_sorted_unique, valid_http_path, valid_package_id, valid_sha256, valid_target,
};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, CatalogMcpTransport,
    CatalogSurface, PluginReleaseChannel, PluginSurfaceKind, PluginSurfaceRef, ToolWorkloadClass,
    VerifiedPluginCatalogRecord, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_CATALOG_SCHEMA_V3,
    PLUGIN_PLANNING_BUNDLE_SCHEMA,
};

const PLANNING_BUNDLE_ERROR: &str = "use.plugin.planning_bundle_invalid";

/// Small executable planning contract downloaded before a plugin archive.
///
/// The signed catalog target digest binds these bytes. Package identity fields
/// deliberately avoid the catalog record digest because that would create a
/// hash cycle: the catalog record already contains this target's digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPlanningBundle {
    pub schema: String,
    pub package_id: String,
    pub version: String,
    pub channel: PluginReleaseChannel,
    pub target: String,
    pub archive_sha256: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub permission_ceiling_digest: String,
    pub surfaces: Vec<ExecutablePlanningSurface>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanningSurfaceActivation {
    Eager,
    Lazy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanningArtifactRef {
    pub uri: String,
    pub digest: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExecutablePlanningSurface {
    ToolTask {
        id: String,
        activation: PlanningSurfaceActivation,
        command: String,
        json_output: bool,
        timeout_ms: u64,
        descriptor: ToolReleaseDescriptor,
        artifact: PlanningArtifactRef,
    },
    ToolService {
        id: String,
        activation: PlanningSurfaceActivation,
        base_path: String,
        descriptor: ToolReleaseDescriptor,
        artifact: PlanningArtifactRef,
    },
    McpService {
        id: String,
        activation: PlanningSurfaceActivation,
        descriptor: McpReleaseDescriptor,
        artifact: PlanningArtifactRef,
    },
}

impl PluginPlanningBundle {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin planning bundle",
            PLANNING_BUNDLE_ERROR,
            Self::validate,
        )
    }

    /// Parse target bytes and bind them to exact verified catalog evidence.
    ///
    /// The repository layer must still fetch the named target through TUF.
    /// This method independently rechecks the catalog-declared raw length and
    /// SHA-256 before accepting its typed content.
    pub fn from_catalog_target(
        input: &[u8],
        catalog: &VerifiedPluginCatalogRecord,
    ) -> UseResult<Self> {
        catalog
            .validate()
            .map_err(|_| planning_error("The verified plugin catalog record is invalid."))?;
        let target = catalog.record.planning.as_ref().ok_or_else(|| {
            planning_error("The verified plugin catalog does not name a planning target.")
        })?;
        if catalog.record.schema != PLUGIN_CATALOG_SCHEMA_V3
            || input.len() as u64 != target.length
            || canonical_digest(input) != target.sha256
        {
            return Err(planning_error(
                "The planning target does not match the verified catalog identity.",
            ));
        }

        let bundle = Self::from_json(input)?;
        bundle.validate_catalog_binding(catalog)?;
        Ok(bundle)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_PLANNING_BUNDLE_SCHEMA
            || !valid_package_id(&self.package_id)
            || semver::Version::parse(&self.version)
                .map(|version| version.to_string() != self.version)
                .unwrap_or(true)
            || !valid_target(&self.target)
            || !valid_sha256(&self.archive_sha256)
            || !valid_sha256(&self.package_sha256)
            || !valid_sha256(&self.manifest_sha256)
            || !valid_sha256(&self.permission_ceiling_digest)
            || self.surfaces.is_empty()
            || self.surfaces.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(planning_error(
                "The plugin planning bundle identity or bounds are invalid.",
            ));
        }

        for surface in &self.surfaces {
            surface.validate()?;
        }
        let references = self
            .surfaces
            .iter()
            .map(ExecutablePlanningSurface::reference)
            .collect::<Vec<_>>();
        if !strictly_sorted_unique(&references) {
            return Err(planning_error(
                "Executable planning surfaces must be sorted and unique.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin planning bundle", PLANNING_BUNDLE_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    /// Recheck typed planning evidence against the exact verified catalog.
    ///
    /// Repository clients use this after transporting a previously verified
    /// bundle through another digest-bound plan or broker boundary.
    pub fn validate_catalog_binding(&self, catalog: &VerifiedPluginCatalogRecord) -> UseResult<()> {
        self.validate()?;
        catalog.validate()?;
        let record = &catalog.record;
        if self.package_id != record.package_id
            || self.version != record.version
            || self.channel != record.channel
            || self.target != record.target
            || self.archive_sha256 != record.archive.sha256
            || record.package.sha256.as_deref() != Some(self.package_sha256.as_str())
            || record.package.manifest_sha256.as_deref() != Some(self.manifest_sha256.as_str())
            || self.permission_ceiling_digest != record.permission_ceiling_digest
        {
            return Err(planning_error(
                "The planning bundle does not match the verified package evidence.",
            ));
        }

        let executable_catalog = record
            .surfaces
            .iter()
            .filter(|surface| {
                matches!(
                    surface.kind,
                    PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                )
            })
            .collect::<Vec<_>>();
        if executable_catalog.len() != self.surfaces.len() {
            return Err(planning_error(
                "The planning bundle does not cover every executable catalog surface.",
            ));
        }
        for (surface, expected) in self.surfaces.iter().zip(executable_catalog) {
            surface.validate_catalog_surface(expected)?;
        }
        Ok(())
    }
}

impl ExecutablePlanningSurface {
    pub fn reference(&self) -> PluginSurfaceRef {
        match self {
            Self::ToolTask { id, .. } | Self::ToolService { id, .. } => PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: id.clone(),
            },
            Self::McpService { id, .. } => PluginSurfaceRef {
                kind: PluginSurfaceKind::Mcp,
                id: id.clone(),
            },
        }
    }

    fn validate(&self) -> UseResult<()> {
        match self {
            Self::ToolTask {
                id,
                command,
                timeout_ms,
                descriptor,
                artifact,
                ..
            } => {
                validate_surface_id(id)?;
                if !valid_command(command) || *timeout_ms == 0 {
                    return Err(planning_error("A Tool Task planning contract is invalid."));
                }
                descriptor
                    .validate()
                    .map_err(|_| planning_error("A Tool Task release descriptor is invalid."))?;
                match &descriptor.workload {
                    ToolWorkloadContract::Task {
                        interactive,
                        timeout_ms: descriptor_timeout,
                        success_exit_codes,
                        ..
                    } if !interactive
                        && descriptor_timeout == timeout_ms
                        && success_exit_codes.as_slice() == [0] => {}
                    _ => return Err(planning_error(
                        "A Tool Task release does not match its install-time launcher contract.",
                    )),
                }
                artifact.validate_for(&descriptor.artifact.digest, &descriptor.artifact.media_type)
            }
            Self::ToolService {
                id,
                base_path,
                descriptor,
                artifact,
                ..
            } => {
                validate_surface_id(id)?;
                if !valid_http_path(base_path) {
                    return Err(planning_error(
                        "A Tool Service planning contract is invalid.",
                    ));
                }
                descriptor
                    .validate()
                    .map_err(|_| planning_error("A Tool Service release descriptor is invalid."))?;
                match &descriptor.workload {
                    ToolWorkloadContract::Service {
                        base_path: descriptor_path,
                        ..
                    } if descriptor_path == base_path => {}
                    _ => {
                        return Err(planning_error(
                            "A Tool Service release does not match its HTTP contract.",
                        ))
                    }
                }
                artifact.validate_for(&descriptor.artifact.digest, &descriptor.artifact.media_type)
            }
            Self::McpService {
                id,
                descriptor,
                artifact,
                ..
            } => {
                validate_surface_id(id)?;
                descriptor
                    .validate()
                    .map_err(|_| planning_error("An MCP Service release descriptor is invalid."))?;
                artifact.validate_for(&descriptor.artifact.digest, &descriptor.artifact.media_type)
            }
        }
    }

    fn validate_catalog_surface(&self, catalog: &CatalogSurface) -> UseResult<()> {
        if self.reference() != catalog.reference() {
            return Err(planning_error(
                "Executable planning surfaces do not match the catalog order or identity.",
            ));
        }
        let shape_matches = match self {
            Self::ToolTask { .. } => {
                catalog.workload == Some(ToolWorkloadClass::Task) && catalog.mcp_transport.is_none()
            }
            Self::ToolService { .. } => {
                catalog.workload == Some(ToolWorkloadClass::Service)
                    && catalog.mcp_transport.is_none()
            }
            Self::McpService { .. } => {
                catalog.workload.is_none()
                    && catalog.mcp_transport == Some(CatalogMcpTransport::StreamableHttp)
            }
        };
        if !shape_matches {
            return Err(planning_error(
                "An executable planning surface does not match its catalog workload.",
            ));
        }
        Ok(())
    }
}

impl PlanningArtifactRef {
    fn validate_for(&self, expected_digest: &str, expected_media_type: &str) -> UseResult<()> {
        let parsed = Url::parse(&self.uri)
            .map_err(|_| planning_error("A planning artifact URI is invalid."))?;
        let digest_suffix = format!("@{}", self.digest);
        if self.uri.len() > 2048
            || parsed.scheme() != "oci"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !self.uri.ends_with(&digest_suffix)
            || !valid_sha256(&self.digest)
            || self.digest != expected_digest
            || self.media_type != expected_media_type
        {
            return Err(planning_error(
                "A planning artifact is not an exact digest-pinned OCI release.",
            ));
        }
        Ok(())
    }
}

fn validate_surface_id(id: &str) -> UseResult<()> {
    if !super::validation::valid_segment(id) {
        return Err(planning_error(
            "An executable planning surface ID is invalid.",
        ));
    }
    Ok(())
}

fn valid_command(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn planning_error(message: impl Into<String>) -> UseError {
    contract_error(PLANNING_BUNDLE_ERROR, message)
}
