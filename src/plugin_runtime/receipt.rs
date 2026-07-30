use a3s_runtime::ProviderId;
use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind, UseResult};
use serde::{Deserialize, Serialize};

use super::model::{
    runtime_input_error, valid_machine_id, valid_sha256, valid_surface_segment,
    RuntimePreparedTaskBinding, RuntimeServiceBindingReceipt, RuntimeServiceReadinessEvidence,
    RuntimeSurfaceContract, RUNTIME_SERVICE_BINDING_SCHEMA, RUNTIME_TASK_BINDING_SCHEMA,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "bindingKind",
    content = "receipt",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum RuntimeBindingReceipt {
    Task(RuntimePreparedTaskBinding),
    Service(RuntimeServiceBindingReceipt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeBindingReadiness {
    Prepared,
    Healthy,
}

impl RuntimeBindingReceipt {
    pub fn validate(&self) -> UseResult<()> {
        match self {
            Self::Task(receipt) => validate_task(receipt),
            Self::Service(receipt) => validate_service(receipt),
        }
    }

    pub fn surface(&self) -> &PlanQualifiedSurfaceRef {
        match self {
            Self::Task(receipt) => &receipt.surface,
            Self::Service(receipt) => &receipt.surface,
        }
    }

    pub fn scope_id(&self) -> &str {
        match self {
            Self::Task(receipt) => &receipt.scope_id,
            Self::Service(receipt) => &receipt.scope_id,
        }
    }

    pub fn generation(&self) -> u64 {
        match self {
            Self::Task(receipt) => receipt.generation,
            Self::Service(receipt) => receipt.generation,
        }
    }

    pub fn package_digest(&self) -> &str {
        match self {
            Self::Task(receipt) => &receipt.package_digest,
            Self::Service(receipt) => &receipt.package_digest,
        }
    }

    pub fn semantics_profile_digest(&self) -> &str {
        match self {
            Self::Task(receipt) => &receipt.semantics_profile_digest,
            Self::Service(receipt) => &receipt.semantics_profile_digest,
        }
    }

    pub fn provider_id(&self) -> &str {
        match self {
            Self::Task(receipt) => &receipt.provider_id,
            Self::Service(receipt) => &receipt.provider_id,
        }
    }

    pub fn provider_build_id(&self) -> &str {
        match self {
            Self::Task(receipt) => &receipt.provider_build_id,
            Self::Service(receipt) => &receipt.provider_build_id,
        }
    }

    pub fn capability_digest(&self) -> &str {
        match self {
            Self::Task(receipt) => &receipt.capability_digest,
            Self::Service(receipt) => &receipt.capability_digest,
        }
    }

    pub fn readiness(&self) -> RuntimeBindingReadiness {
        match self {
            Self::Task(_) => RuntimeBindingReadiness::Prepared,
            Self::Service(_) => RuntimeBindingReadiness::Healthy,
        }
    }
}

fn validate_task(receipt: &RuntimePreparedTaskBinding) -> UseResult<()> {
    if receipt.schema != RUNTIME_TASK_BINDING_SCHEMA
        || receipt.surface.surface.kind != PluginSurfaceKind::Tool
        || !valid_binding_identity(&receipt.surface, &receipt.scope_id)
        || receipt.generation == 0
        || !valid_sha256(&receipt.package_digest)
        || !valid_sha256(&receipt.descriptor_digest)
        || !valid_sha256(&receipt.capability_digest)
        || !valid_sha256(&receipt.artifact_digest)
        || !valid_sha256(&receipt.semantics_profile_digest)
        || ProviderId::parse(receipt.provider_id.as_str()).is_err()
        || !valid_machine_id(&receipt.provider_build_id)
        || !valid_media_type(&receipt.artifact_media_type)
    {
        return Err(runtime_input_error(
            "The prepared Runtime Task binding receipt is invalid.",
        ));
    }
    Ok(())
}

fn validate_service(receipt: &RuntimeServiceBindingReceipt) -> UseResult<()> {
    if receipt.schema != RUNTIME_SERVICE_BINDING_SCHEMA
        || !valid_binding_identity(&receipt.surface, &receipt.scope_id)
        || receipt.generation == 0
        || receipt.runtime_started_at_ms == 0
        || receipt.observation_revision == 0
        || receipt.runtime_started_at_ms > receipt.observation_revision
        || receipt.last_healthy_at_ms == 0
        || receipt.last_healthy_at_ms > receipt.observation_revision
        || !valid_sha256(&receipt.package_digest)
        || !valid_sha256(&receipt.descriptor_digest)
        || !valid_sha256(&receipt.capability_digest)
        || !valid_sha256(&receipt.spec_digest)
        || !valid_sha256(&receipt.semantics_profile_digest)
        || ProviderId::parse(receipt.provider_id.as_str()).is_err()
        || !valid_machine_id(&receipt.provider_build_id)
        || !valid_runtime_unit_id(&receipt.unit_id)
    {
        return Err(runtime_input_error(
            "The Runtime Service binding receipt is invalid.",
        ));
    }
    super::model::RuntimeEndpointRef::parse(receipt.endpoint_ref.as_str().to_string())?;
    validate_service_contract(receipt)
}

fn validate_service_contract(receipt: &RuntimeServiceBindingReceipt) -> UseResult<()> {
    match (
        &receipt.surface.surface.kind,
        &receipt.contract,
        &receipt.readiness,
    ) {
        (
            PluginSurfaceKind::Tool,
            RuntimeSurfaceContract::ToolService {
                port_name,
                base_path,
                shutdown_grace_ms,
                api_contract_digest,
            },
            RuntimeServiceReadinessEvidence::HttpHealthy,
        ) => {
            if !valid_port_name(port_name)
                || !valid_http_path(base_path)
                || *shutdown_grace_ms == 0
                || api_contract_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_sha256(digest))
            {
                return Err(runtime_input_error(
                    "The HTTP Tool binding contract is invalid.",
                ));
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
            RuntimeServiceReadinessEvidence::McpInitialized { initialize },
        ) => {
            if !valid_port_name(port_name)
                || !valid_http_path(endpoint_path)
                || protocol_version.is_empty()
                || protocol_version.len() > 64
                || protocol_version.chars().any(char::is_control)
                || *shutdown_grace_ms == 0
            {
                return Err(runtime_input_error(
                    "The Streamable HTTP MCP binding contract is invalid.",
                ));
            }
            initialize.validate(protocol_version, receipt.observation_revision)?;
        }
        _ => {
            return Err(runtime_input_error(
                "The Runtime Service binding kind, surface, and readiness evidence disagree.",
            ))
        }
    }
    Ok(())
}

fn valid_binding_identity(surface: &PlanQualifiedSurfaceRef, scope_id: &str) -> bool {
    let segments = surface.package_id.split('/').collect::<Vec<_>>();
    surface.package_id.len() <= 128
        && segments.len() == 2
        && segments
            .iter()
            .all(|segment| valid_surface_segment(segment))
        && valid_surface_segment(&surface.surface.id)
        && valid_machine_id(scope_id)
}

fn valid_media_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_runtime_unit_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
}

fn valid_port_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

fn valid_http_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 2048
        && !value.contains("//")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'?' | b'#' | b'\\'))
        && !value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
}
