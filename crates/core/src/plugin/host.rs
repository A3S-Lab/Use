use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::{valid_machine_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanScope, PlanScopeKind,
    PluginHostApplyRequest, PluginHostApplyResult, PluginHostCancelRequest, PluginHostCancelResult,
    PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult, PluginHostObservationRequest,
    PluginHostObservationResult, PluginHostOperationObservationRequest,
    PluginHostOperationObservationResult, PluginHostOperationWatchRequest, PluginHostPlanRequest,
    PluginHostPlanResult, PluginSurfaceKind, PLUGIN_CATALOG_SCHEMA_V3,
    PLUGIN_HOST_APPLY_REQUEST_SCHEMA, PLUGIN_HOST_APPLY_RESULT_SCHEMA,
    PLUGIN_HOST_CANCEL_REQUEST_SCHEMA, PLUGIN_HOST_CANCEL_RESULT_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA,
    PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA, PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA,
    PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA, PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA,
    PLUGIN_HOST_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_PLAN_RESULT_SCHEMA,
    PLUGIN_OPERATION_PLAN_SCHEMA_V4,
};

pub const PLUGIN_MANAGED_SCOPE_SCHEMA_V2: &str = "a3s.use.plugin-managed-scope.v2";
pub const PLUGIN_HOST_CAPABILITIES_SCHEMA_V6: &str = "a3s.use.plugin-host-capabilities.v6";
pub const PLUGIN_HOST_PROTOCOL_LEVEL_V6: u32 = 6;

const MANAGED_SCOPE_ERROR: &str = "use.plugin.managed_scope_invalid";
const HOST_CAPABILITIES_ERROR: &str = "use.plugin.host_capabilities_invalid";

/// Host-derived scope identity and the exact exclusive mutation fence.
///
/// This value contains no workspace path or bearer credential. A manager must
/// compare the complete value with its durable current fence before mutation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagedScope {
    pub schema: String,
    pub host_id: String,
    pub scope_kind: PlanScopeKind,
    pub scope_id: String,
    pub authority_id: String,
    pub fence_generation: u64,
    pub fence_digest: String,
}

/// Exact current A3S Use host protocol supported by one manager build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostCapabilities {
    pub schema: String,
    pub protocol_level: u32,
    pub host_id: String,
    pub manager_version: String,
    pub manager_build_id: String,
    pub contract_schemas: Vec<String>,
    pub catalog_schemas: Vec<String>,
    pub plan_schemas: Vec<String>,
    pub surface_kinds: Vec<PluginSurfaceKind>,
    pub exclusive_managed_scope_mutation: bool,
}

impl PluginManagedScope {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "managed plugin scope",
            MANAGED_SCOPE_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_MANAGED_SCOPE_SCHEMA_V2
            || !valid_opaque_id(&self.host_id)
            || !valid_opaque_id(&self.scope_id)
            || !valid_opaque_id(&self.authority_id)
            || self.fence_generation == 0
            || !valid_sha256(&self.fence_digest)
        {
            return Err(managed_scope_error(
                "The managed plugin scope identity or mutation fence is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "managed plugin scope", MANAGED_SCOPE_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn plan_scope(&self) -> PlanScope {
        PlanScope {
            kind: self.scope_kind,
            id: self.scope_id.clone(),
        }
    }

    /// Require the exact durable managed authority and fence.
    ///
    /// A stale, future, standalone, or different-manager scope is never
    /// adopted implicitly by a remote mutation.
    pub fn verify_current_fence(&self, current: &Self) -> UseResult<()> {
        self.validate()?;
        current.validate()?;
        if self != current {
            return Err(UseError::new(
                "use.plugin.managed_scope_fence_mismatch",
                "The managed plugin scope does not match the host's current mutation fence.",
            ));
        }
        Ok(())
    }
}

impl PluginHostCapabilities {
    pub fn v6(
        host_id: impl Into<String>,
        manager_version: impl Into<String>,
        manager_build_id: impl Into<String>,
    ) -> UseResult<Self> {
        let capabilities = Self {
            schema: PLUGIN_HOST_CAPABILITIES_SCHEMA_V6.to_owned(),
            protocol_level: PLUGIN_HOST_PROTOCOL_LEVEL_V6,
            host_id: host_id.into(),
            manager_version: manager_version.into(),
            manager_build_id: manager_build_id.into(),
            contract_schemas: current_contract_schemas(),
            catalog_schemas: vec![PLUGIN_CATALOG_SCHEMA_V3.to_owned()],
            plan_schemas: vec![PLUGIN_OPERATION_PLAN_SCHEMA_V4.to_owned()],
            surface_kinds: vec![
                PluginSurfaceKind::Flow,
                PluginSurfaceKind::Mcp,
                PluginSurfaceKind::Okf,
                PluginSurfaceKind::Skill,
                PluginSurfaceKind::Tool,
                PluginSurfaceKind::Ui,
            ],
            exclusive_managed_scope_mutation: true,
        };
        capabilities.validate()?;
        Ok(capabilities)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host capabilities",
            HOST_CAPABILITIES_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        let canonical_version = Version::parse(&self.manager_version)
            .is_ok_and(|version| version.to_string() == self.manager_version);
        let surface_kinds = vec![
            PluginSurfaceKind::Flow,
            PluginSurfaceKind::Mcp,
            PluginSurfaceKind::Okf,
            PluginSurfaceKind::Skill,
            PluginSurfaceKind::Tool,
            PluginSurfaceKind::Ui,
        ];
        if !valid_opaque_id(&self.host_id)
            || !canonical_version
            || !valid_opaque_id(&self.manager_build_id)
            || self.schema != PLUGIN_HOST_CAPABILITIES_SCHEMA_V6
            || self.protocol_level != PLUGIN_HOST_PROTOCOL_LEVEL_V6
            || self.contract_schemas != current_contract_schemas()
            || self.catalog_schemas != [PLUGIN_CATALOG_SCHEMA_V3]
            || self.plan_schemas != [PLUGIN_OPERATION_PLAN_SCHEMA_V4]
            || self.surface_kinds != surface_kinds
            || !self.exclusive_managed_scope_mutation
        {
            return Err(host_capabilities_error(
                "The plugin host capability identity or frozen protocol inventory is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin host capabilities", HOST_CAPABILITIES_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn supports_plan_schema(&self, schema: &str) -> bool {
        self.plan_schemas
            .iter()
            .any(|supported| supported == schema)
    }
}

/// Sole typed host port for remote managed-scope plugin operations.
///
/// Implementations are adapters over the shared A3S Use Plugin Manager. They
/// must not install, reconcile, grant, bind, or publish capabilities through a
/// second lifecycle implementation.
#[async_trait]
pub trait PluginHostManager: Send + Sync {
    async fn capabilities(&self) -> UseResult<PluginHostCapabilities>;

    async fn plan(&self, request: PluginHostPlanRequest) -> UseResult<PluginHostPlanResult>;

    async fn apply(&self, request: PluginHostApplyRequest) -> UseResult<PluginHostApplyResult>;

    async fn plan_enablement(
        &self,
        _request: PluginHostEnablementPlanRequest,
    ) -> UseResult<PluginHostEnablementPlanResult> {
        Err(UseError::new(
            "use.plugin.host_enablement_plan_unsupported",
            "This Plugin Host Manager does not implement reviewed enablement planning.",
        ))
    }

    async fn observe(
        &self,
        request: PluginHostObservationRequest,
    ) -> UseResult<PluginHostObservationResult>;

    /// Observe one exact reviewed operation without inspecting private host
    /// files. Implementations must project only Use-owned durable evidence.
    async fn observe_operation(
        &self,
        _request: PluginHostOperationObservationRequest,
    ) -> UseResult<PluginHostOperationObservationResult> {
        Err(UseError::new(
            "use.plugin.host_operation_observation_unsupported",
            "This Plugin Host Manager does not implement operation observation.",
        ))
    }

    /// Long-poll an operation revision. The default implementation performs a
    /// single typed observation and therefore never fabricates change.
    async fn watch_operation(
        &self,
        request: PluginHostOperationWatchRequest,
    ) -> UseResult<PluginHostOperationObservationResult> {
        request.validate()?;
        self.observe_operation(request.observation).await
    }

    /// Request explicit-user cancellation at the manager's typed safe point.
    async fn cancel(&self, _request: PluginHostCancelRequest) -> UseResult<PluginHostCancelResult> {
        Err(UseError::new(
            "use.plugin.host_cancellation_unsupported",
            "This Plugin Host Manager does not implement safe cancellation.",
        ))
    }
}

fn current_contract_schemas() -> Vec<String> {
    vec![
        PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_APPLY_RESULT_SCHEMA.to_owned(),
        PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_CANCEL_RESULT_SCHEMA.to_owned(),
        PLUGIN_HOST_CAPABILITIES_SCHEMA_V6.to_owned(),
        PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA.to_owned(),
        PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA.to_owned(),
        PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA.to_owned(),
        PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        PLUGIN_HOST_PLAN_RESULT_SCHEMA.to_owned(),
        PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
    ]
}

pub(super) fn validate_request_identity(
    request_id: &str,
    assignment_generation: u64,
    capabilities_digest: &str,
    scope: &PluginManagedScope,
) -> UseResult<()> {
    if !valid_machine_id(request_id)
        || assignment_generation == 0
        || !valid_sha256(capabilities_digest)
        || scope.validate().is_err()
    {
        return Err(UseError::new(
            "use.plugin.host_request_invalid",
            "The plugin host request identity, generation, capabilities, or scope is invalid.",
        ));
    }
    Ok(())
}

pub(super) fn verify_capabilities(
    capabilities_digest: &str,
    scope: &PluginManagedScope,
    capabilities: &PluginHostCapabilities,
) -> UseResult<()> {
    capabilities.validate()?;
    if capabilities.host_id != scope.host_id
        || capabilities.descriptor_digest()? != capabilities_digest
    {
        return Err(UseError::new(
            "use.plugin.host_capabilities_mismatch",
            "The request does not bind the target host's exact current Plugin Manager capabilities.",
        ));
    }
    Ok(())
}

pub(super) fn verify_supported_plan_schema(
    capabilities: &PluginHostCapabilities,
    plan_schema: &str,
) -> UseResult<()> {
    capabilities.validate()?;
    if !capabilities.supports_plan_schema(plan_schema) {
        return Err(UseError::new(
            "use.plugin.host_plan_schema_unsupported",
            "The plugin operation plan schema is not supported by the selected host protocol.",
        ));
    }
    Ok(())
}

fn valid_opaque_id(value: &str) -> bool {
    valid_machine_id(value) && !value.contains('/')
}

fn managed_scope_error(message: impl Into<String>) -> UseError {
    contract_error(MANAGED_SCOPE_ERROR, message)
}

fn host_capabilities_error(message: impl Into<String>) -> UseError {
    contract_error(HOST_CAPABILITIES_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_host_protocol_accepts_only_the_current_operation_plan_schema() {
        let capabilities =
            PluginHostCapabilities::v6("host:node-01", "0.3.0", "use:0.3.0:linux-x86_64").unwrap();
        verify_supported_plan_schema(&capabilities, PLUGIN_OPERATION_PLAN_SCHEMA_V4).unwrap();
        let error = verify_supported_plan_schema(&capabilities, "a3s.use.plugin-operation-plan.v3")
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.host_plan_schema_unsupported");
    }
}
