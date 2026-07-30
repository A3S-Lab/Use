use std::collections::BTreeMap;

use a3s_runtime::contract::{
    IsolationLevel, RuntimeMount, RuntimeObservation, RuntimeUnitSpec, SecretReference,
};
use a3s_use_core::{
    PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlannedProviderEvidence, PluginSurfaceKind,
    PluginSurfaceRef, UseError, UseResult,
};
use serde::{Deserialize, Serialize};

pub const RUNTIME_SERVICE_BINDING_SCHEMA: &str = "a3s.use.runtime-service-binding.v2";
pub const RUNTIME_TASK_BINDING_SCHEMA: &str = "a3s.use.runtime-task-binding.v2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSurfaceContext {
    pub(super) package_id: String,
    pub(super) package_digest: String,
    pub(super) scope_id: String,
    pub(super) grant_digest: String,
    pub(super) surface: PluginSurfaceRef,
    pub(super) generation: u64,
}

impl RuntimeSurfaceContext {
    pub fn new(
        package_id: impl Into<String>,
        package_digest: impl Into<String>,
        scope_id: impl Into<String>,
        grant_digest: impl Into<String>,
        surface: PluginSurfaceRef,
        generation: u64,
    ) -> UseResult<Self> {
        let context = Self {
            package_id: package_id.into(),
            package_digest: package_digest.into(),
            scope_id: scope_id.into(),
            grant_digest: grant_digest.into(),
            surface,
            generation,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn grant_digest(&self) -> &str {
        &self.grant_digest
    }

    pub fn surface(&self) -> &PluginSurfaceRef {
        &self.surface
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn qualified_surface(&self) -> PlanQualifiedSurfaceRef {
        PlanQualifiedSurfaceRef {
            package_id: self.package_id.clone(),
            surface: self.surface.clone(),
        }
    }

    fn validate(&self) -> UseResult<()> {
        let package_segments = self.package_id.split('/').collect::<Vec<_>>();
        if self.package_id.len() > 128
            || package_segments.len() != 2
            || package_segments
                .iter()
                .any(|segment| !valid_surface_segment(segment))
        {
            return Err(runtime_input_error(
                "Runtime surface package IDs must use two portable lowercase segments.",
            ));
        }
        if !valid_sha256(&self.package_digest) || !valid_sha256(&self.grant_digest) {
            return Err(runtime_input_error(
                "Runtime surface package and grant digests must be canonical SHA-256 values.",
            ));
        }
        if !valid_machine_id(&self.scope_id) {
            return Err(runtime_input_error(
                "Runtime surface scope IDs must use the portable plan identity contract.",
            ));
        }
        if !matches!(
            self.surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        ) || !valid_surface_segment(&self.surface.id)
        {
            return Err(runtime_input_error(
                "Only named Tool and MCP surfaces can be mapped to A3S Runtime.",
            ));
        }
        if self.generation == 0 {
            return Err(runtime_input_error(
                "Runtime surface generations must be positive.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTaskInvocation {
    pub(super) invocation_id: String,
    pub(super) args: Vec<String>,
}

impl RuntimeTaskInvocation {
    pub fn new(invocation_id: impl Into<String>, args: Vec<String>) -> UseResult<Self> {
        let invocation = Self {
            invocation_id: invocation_id.into(),
            args,
        };
        if !valid_machine_id(&invocation.invocation_id)
            || invocation.args.len() > 256
            || invocation
                .args
                .iter()
                .any(|value| value.is_empty() || value.len() > 32 * 1024 || value.contains('\0'))
        {
            return Err(runtime_input_error(
                "Runtime Task invocation IDs or arguments exceed the portable contract.",
            ));
        }
        Ok(invocation)
    }

    pub fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResourcePolicy {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub pids: u32,
    pub ephemeral_storage_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeWorkloadPolicy {
    pub isolation: IsolationLevel,
    pub resources: RuntimeResourcePolicy,
    pub mounts: Vec<RuntimeMount>,
    pub secrets: Vec<SecretReference>,
    /// Values in this map must already have been classified as non-secret.
    pub non_secret_environment: BTreeMap<String, String>,
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeSurfaceContract {
    ToolTask {
        command_name: String,
        json_output: bool,
        max_stdout_bytes: u64,
        max_stderr_bytes: u64,
    },
    ToolService {
        port_name: String,
        base_path: String,
        shutdown_grace_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        api_contract_digest: Option<String>,
    },
    McpService {
        port_name: String,
        endpoint_path: String,
        protocol_version: String,
        shutdown_grace_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSurfacePlan {
    pub(super) context: RuntimeSurfaceContext,
    pub(super) descriptor_digest: String,
    pub(super) spec: RuntimeUnitSpec,
    pub(super) contract: RuntimeSurfaceContract,
}

impl RuntimeSurfacePlan {
    pub fn context(&self) -> &RuntimeSurfaceContext {
        &self.context
    }

    pub fn surface(&self) -> PlanQualifiedSurfaceRef {
        self.context.qualified_surface()
    }

    pub fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }

    pub fn spec(&self) -> &RuntimeUnitSpec {
        &self.spec
    }

    pub fn contract(&self) -> &RuntimeSurfaceContract {
        &self.contract
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePreparedTaskBinding {
    pub schema: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub package_digest: String,
    pub scope_id: String,
    pub descriptor_digest: String,
    pub provider_id: String,
    pub provider_build_id: String,
    pub capability_digest: String,
    pub enforcement: PlanEnforcementProfile,
    pub artifact_digest: String,
    pub artifact_media_type: String,
    pub generation: u64,
    pub semantics_profile_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeServiceActivation {
    pub(super) plan: RuntimeSurfacePlan,
    pub(super) provider: PlannedProviderEvidence,
    pub(super) observation: RuntimeObservation,
}

impl RuntimeServiceActivation {
    pub fn observation(&self) -> &RuntimeObservation {
        &self.observation
    }

    pub fn into_tool_service_receipt(
        self,
        endpoint_ref: RuntimeEndpointRef,
    ) -> UseResult<RuntimeServiceBindingReceipt> {
        if !matches!(
            self.plan.contract,
            RuntimeSurfaceContract::ToolService { .. }
        ) {
            return Err(runtime_input_error(
                "An MCP Service requires a successful standard initialize probe before binding.",
            ));
        }
        self.into_receipt(endpoint_ref, RuntimeServiceReadinessEvidence::HttpHealthy)
    }

    pub fn into_mcp_service_receipt(
        self,
        endpoint_ref: RuntimeEndpointRef,
        initialize: RuntimeMcpInitializeEvidence,
    ) -> UseResult<RuntimeServiceBindingReceipt> {
        let RuntimeSurfaceContract::McpService {
            protocol_version, ..
        } = &self.plan.contract
        else {
            return Err(runtime_input_error(
                "MCP initialize evidence can bind only a Streamable HTTP MCP Service.",
            ));
        };
        initialize.validate(protocol_version, self.observation.observed_at_ms)?;
        self.into_receipt(
            endpoint_ref,
            RuntimeServiceReadinessEvidence::McpInitialized { initialize },
        )
    }

    fn into_receipt(
        self,
        endpoint_ref: RuntimeEndpointRef,
        readiness: RuntimeServiceReadinessEvidence,
    ) -> UseResult<RuntimeServiceBindingReceipt> {
        let spec_digest = self.plan.spec.digest().map_err(runtime_contract_error)?;
        let semantics_profile_digest =
            self.plan
                .spec
                .semantics_profile_digest
                .clone()
                .ok_or_else(|| {
                    runtime_contract_error("Runtime plan omitted its semantics-profile digest.")
                })?;
        let last_healthy_at_ms = self
            .observation
            .health
            .as_ref()
            .map_or(self.observation.observed_at_ms, |health| {
                health.checked_at_ms
            });
        let runtime_started_at_ms = self.observation.started_at_ms.ok_or_else(|| {
            runtime_contract_error(
                "A running Runtime Service observation omitted its start identity.",
            )
        })?;
        let receipt = RuntimeServiceBindingReceipt {
            schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
            surface: self.plan.surface(),
            package_digest: self.plan.context.package_digest,
            scope_id: self.plan.context.scope_id,
            descriptor_digest: self.plan.descriptor_digest,
            provider_id: self.provider.provider_id,
            provider_build_id: self.provider.provider_build_id,
            capability_digest: self.provider.capability_digest,
            enforcement: self.provider.enforcement,
            unit_id: self.observation.unit_id,
            generation: self.observation.generation,
            spec_digest,
            semantics_profile_digest,
            endpoint_ref,
            runtime_started_at_ms,
            observation_revision: self.observation.observed_at_ms,
            last_healthy_at_ms,
            contract: self.plan.contract,
            readiness,
        };
        super::receipt::RuntimeBindingReceipt::Service(receipt.clone()).validate()?;
        Ok(receipt)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeMcpInitializeEvidence {
    pub protocol_version: String,
    pub initialized_at_ms: u64,
}

impl RuntimeMcpInitializeEvidence {
    pub fn new(protocol_version: impl Into<String>, initialized_at_ms: u64) -> UseResult<Self> {
        let evidence = Self {
            protocol_version: protocol_version.into(),
            initialized_at_ms,
        };
        if evidence.protocol_version.is_empty()
            || evidence.protocol_version.len() > 64
            || evidence.protocol_version.chars().any(char::is_control)
            || evidence.initialized_at_ms == 0
        {
            return Err(runtime_input_error(
                "MCP initialize evidence is outside the bounded protocol contract.",
            ));
        }
        Ok(evidence)
    }

    pub(super) fn validate(&self, expected_protocol: &str, observed_at_ms: u64) -> UseResult<()> {
        if self.protocol_version != expected_protocol || self.initialized_at_ms < observed_at_ms {
            return Err(runtime_input_error(
                "MCP initialize evidence does not match the release protocol or Runtime observation.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeServiceReadinessEvidence {
    HttpHealthy,
    McpInitialized {
        initialize: RuntimeMcpInitializeEvidence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeEndpointRef(String);

impl RuntimeEndpointRef {
    pub fn parse(value: impl Into<String>) -> UseResult<Self> {
        let value = value.into();
        let binding_id = value.strip_prefix("gateway:");
        if binding_id.is_none_or(|binding_id| {
            binding_id.is_empty()
                || binding_id.len() > 256
                || !binding_id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/')
                })
                || binding_id.contains("//")
                || binding_id
                    .split('/')
                    .any(|segment| matches!(segment, "" | "." | ".."))
        }) {
            return Err(runtime_input_error(
                "Runtime endpoint references must be opaque non-secret Gateway binding IDs, not URLs.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeServiceBindingReceipt {
    pub schema: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub package_digest: String,
    pub scope_id: String,
    pub descriptor_digest: String,
    pub provider_id: String,
    pub provider_build_id: String,
    pub capability_digest: String,
    pub enforcement: PlanEnforcementProfile,
    pub unit_id: String,
    pub generation: u64,
    pub spec_digest: String,
    pub semantics_profile_digest: String,
    pub endpoint_ref: RuntimeEndpointRef,
    pub runtime_started_at_ms: u64,
    pub observation_revision: u64,
    pub last_healthy_at_ms: u64,
    pub contract: RuntimeSurfaceContract,
    pub readiness: RuntimeServiceReadinessEvidence,
}

pub(super) fn valid_surface_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(super) fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

pub(super) fn runtime_input_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.input_invalid", message)
}

pub(super) fn runtime_contract_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.contract_invalid", message)
}
