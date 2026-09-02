use std::collections::BTreeMap;

use a3s_runtime::contract::{
    IsolationLevel, RuntimeMount, RuntimeObservation, RuntimeUnitSpec, SecretReference,
};
use a3s_use_core::{
    PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlanScope, PlannedProviderEvidence,
    PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};

pub const RUNTIME_SERVICE_BINDING_SCHEMA: &str = "a3s.use.runtime-service-binding.v3";
pub const RUNTIME_TASK_BINDING_SCHEMA: &str = "a3s.use.runtime-task-binding.v4";
/// Canonical, path-free payload used to reconstruct one committed Runtime
/// surface after a host restart.
pub const RUNTIME_SURFACE_PLAN_SCHEMA: &str = "a3s.use.runtime-surface-plan.v1";
pub const MAX_RUNTIME_SURFACE_PLAN_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSurfaceContext {
    pub(super) package_id: String,
    pub(super) package_digest: String,
    pub(super) scope: PlanScope,
    pub(super) grant_digest: String,
    pub(super) surface: PluginSurfaceRef,
    pub(super) generation: u64,
}

impl RuntimeSurfaceContext {
    pub fn new(
        package_id: impl Into<String>,
        package_digest: impl Into<String>,
        scope: PlanScope,
        grant_digest: impl Into<String>,
        surface: PluginSurfaceRef,
        generation: u64,
    ) -> UseResult<Self> {
        let context = Self {
            package_id: package_id.into(),
            package_digest: package_digest.into(),
            scope,
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

    pub fn scope(&self) -> &PlanScope {
        &self.scope
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

    pub fn validate(&self) -> UseResult<()> {
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
        if self.scope.validate().is_err() {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourcePolicy {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub pids: u32,
    pub ephemeral_storage_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

    /// Validate the complete plan, including the semantics digest that binds
    /// its context, immutable release descriptor, Runtime spec, and public
    /// surface contract. A plan loaded after restart must pass this check
    /// before a provider can be contacted.
    pub fn validate(&self) -> UseResult<()> {
        self.context.validate()?;
        if !valid_sha256(&self.descriptor_digest) {
            return Err(runtime_contract_error(
                "Runtime surface descriptor digests must be canonical SHA-256 values.",
            ));
        }
        self.spec.validate().map_err(runtime_contract_error)?;
        if self.spec.generation != self.context.generation {
            return Err(runtime_contract_error(
                "Runtime surface plan and context generations must be identical.",
            ));
        }
        let semantics = self
            .spec
            .semantics_profile_digest
            .as_deref()
            .ok_or_else(|| {
                runtime_contract_error("Runtime surface plans must carry a semantics digest.")
            })?;
        if !valid_sha256(semantics) {
            return Err(runtime_contract_error(
                "Runtime semantics profile digests must be canonical SHA-256 values.",
            ));
        }
        let expected = super::planner::runtime_semantics_profile_digest(
            &self.context,
            &self.descriptor_digest,
            &self.spec,
            &self.contract,
        )?;
        if semantics != expected {
            return Err(runtime_contract_error(
                "Runtime surface semantics do not match the plan digest.",
            ));
        }
        validate_contract(&self.context, &self.spec, &self.contract)
    }

    /// Encode a plan as bounded canonical JSON. The document contains no host
    /// package root, provider endpoint, or secret value; an entrypoint inside
    /// the immutable Runtime artifact may remain part of the reviewed spec.
    pub fn to_canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let document = RuntimeSurfacePlanDocumentRef {
            schema: RUNTIME_SURFACE_PLAN_SCHEMA,
            plan: self,
        };
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        document.serialize(&mut serializer).map_err(|error| {
            runtime_contract_error(format!(
                "Failed to encode the canonical Runtime surface plan: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_BYTES {
            return Err(runtime_contract_error(
                "The canonical Runtime surface plan exceeds its size bound.",
            ));
        }
        Ok(bytes)
    }

    /// Decode and semantically validate a canonical plan payload obtained
    /// from a host-owned durable source.
    pub fn from_canonical_bytes(bytes: &[u8]) -> UseResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_BYTES {
            return Err(runtime_contract_error(
                "The Runtime surface plan payload exceeds its size bound.",
            ));
        }
        let document: RuntimeSurfacePlanDocument =
            serde_json::from_slice(bytes).map_err(|error| {
                runtime_contract_error(format!(
                    "Failed to decode the Runtime surface plan at line {}, column {}.",
                    error.line(),
                    error.column()
                ))
            })?;
        if document.schema != RUNTIME_SURFACE_PLAN_SCHEMA {
            return Err(runtime_contract_error(
                "The Runtime surface plan schema is unsupported.",
            ));
        }
        document.plan.validate()?;
        if document.plan.to_canonical_bytes()? != bytes {
            return Err(runtime_contract_error(
                "The Runtime surface plan payload is not canonical JSON.",
            ));
        }
        Ok(document.plan)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeSurfacePlanDocument {
    schema: String,
    plan: RuntimeSurfacePlan,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSurfacePlanDocumentRef<'a> {
    schema: &'a str,
    plan: &'a RuntimeSurfacePlan,
}

fn validate_contract(
    context: &RuntimeSurfaceContext,
    spec: &RuntimeUnitSpec,
    contract: &RuntimeSurfaceContract,
) -> UseResult<()> {
    let valid_text = |value: &str, label: &str, max: usize| {
        if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
            Err(runtime_contract_error(format!(
                "Runtime {label} is empty, oversized, or contains control characters."
            )))
        } else {
            Ok(())
        }
    };
    match (context.surface.kind, contract, spec.class) {
        (
            PluginSurfaceKind::Tool,
            RuntimeSurfaceContract::ToolTask {
                command_name,
                max_stdout_bytes,
                max_stderr_bytes,
                ..
            },
            a3s_runtime::contract::RuntimeUnitClass::Task,
        ) => {
            valid_text(command_name, "Tool Task command", 256)?;
            if *max_stdout_bytes == 0 || *max_stderr_bytes == 0 {
                return Err(runtime_contract_error(
                    "Runtime Tool Task output bounds must be positive.",
                ));
            }
        }
        (
            PluginSurfaceKind::Tool,
            RuntimeSurfaceContract::ToolService {
                port_name,
                base_path,
                shutdown_grace_ms,
                api_contract_digest,
            },
            a3s_runtime::contract::RuntimeUnitClass::Service,
        ) => {
            valid_text(port_name, "Tool Service port", 63)?;
            validate_path(base_path, "Tool Service base path")?;
            if *shutdown_grace_ms == 0 {
                return Err(runtime_contract_error(
                    "Runtime Tool Service shutdown grace must be positive.",
                ));
            }
            if let Some(digest) = api_contract_digest {
                if !valid_sha256(digest) {
                    return Err(runtime_contract_error(
                        "Runtime API contract digests must be canonical SHA-256 values.",
                    ));
                }
            }
        }
        (
            PluginSurfaceKind::Mcp,
            RuntimeSurfaceContract::McpService {
                port_name,
                endpoint_path,
                protocol_version,
                shutdown_grace_ms,
            },
            a3s_runtime::contract::RuntimeUnitClass::Service,
        ) => {
            valid_text(port_name, "MCP Service port", 63)?;
            validate_path(endpoint_path, "MCP endpoint path")?;
            valid_text(protocol_version, "MCP protocol version", 64)?;
            if *shutdown_grace_ms == 0 {
                return Err(runtime_contract_error(
                    "Runtime MCP Service shutdown grace must be positive.",
                ));
            }
        }
        _ => {
            return Err(runtime_contract_error(
                "Runtime surface contract, kind, and unit class are inconsistent.",
            ));
        }
    }
    Ok(())
}

fn validate_path(value: &str, label: &str) -> UseResult<()> {
    if value.is_empty()
        || value.len() > 1024
        || !value.starts_with('/')
        || value.contains('\0')
        || value
            .split('/')
            .any(|segment| matches!(segment, ".." | "."))
    {
        return Err(runtime_contract_error(format!(
            "Runtime {label} must be a bounded absolute path without traversal."
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePreparedTaskBinding {
    pub schema: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub package_digest: String,
    pub scope: PlanScope,
    pub grant_digest: String,
    pub descriptor_digest: String,
    pub provider_id: String,
    pub provider_build_id: String,
    pub capability_digest: String,
    pub enforcement: PlanEnforcementProfile,
    pub semantics_profile_digest: String,
    pub template_spec: Box<RuntimeUnitSpec>,
    pub contract: RuntimeSurfaceContract,
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
            scope: self.plan.context.scope,
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
    pub scope: PlanScope,
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
