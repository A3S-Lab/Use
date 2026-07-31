use a3s_runtime::contract::{
    ArtifactRef, HealthProbe, NetworkMode, ResourceLimits, RestartPolicy, RuntimeHealthCheck,
    RuntimeNetworkSpec, RuntimePort, RuntimeProcessSpec, RuntimeUnitClass, RuntimeUnitSpec,
    TransportProtocol,
};
use a3s_use_core::{
    McpReleaseDescriptor, PluginSurfaceKind, ToolReleaseDescriptor, ToolWorkloadContract, UseError,
    UseResult,
};
use a3s_use_extension::{
    PluginMcpLaunch, PluginMcpSurface, ToolServiceSurface, ToolTaskSource, ToolTaskSurface,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{
    runtime_contract_error, runtime_input_error, RuntimeSurfaceContext, RuntimeSurfaceContract,
    RuntimeSurfacePlan, RuntimeTaskInvocation, RuntimeWorkloadPolicy,
};

const SEMANTICS_PROFILE_SCHEMA: &str = "a3s.use.runtime-semantics.v1";

pub fn plan_tool_task_release(
    context: RuntimeSurfaceContext,
    surface: &ToolTaskSurface,
    descriptor: &ToolReleaseDescriptor,
    artifact: ArtifactRef,
    invocation: RuntimeTaskInvocation,
    policy: RuntimeWorkloadPolicy,
    task_network: NetworkMode,
) -> UseResult<RuntimeSurfacePlan> {
    require_surface_kind(&context, PluginSurfaceKind::Tool)?;
    descriptor.validate()?;
    if !matches!(surface.source, ToolTaskSource::Release { .. }) || surface.interactive {
        return Err(runtime_input_error(
            "Only non-interactive release-backed Tool Tasks can be mapped to A3S Runtime.",
        ));
    }
    verify_artifact(
        &artifact,
        &descriptor.artifact.digest,
        &descriptor.artifact.media_type,
    )?;
    let (
        entrypoint,
        timeout_ms,
        max_stdout_bytes,
        max_stderr_bytes,
        success_exit_codes,
        interactive,
    ) = match &descriptor.workload {
        ToolWorkloadContract::Task {
            entrypoint,
            timeout_ms,
            max_stdout_bytes,
            max_stderr_bytes,
            success_exit_codes,
            interactive,
            ..
        } => (
            entrypoint,
            *timeout_ms,
            *max_stdout_bytes,
            *max_stderr_bytes,
            success_exit_codes,
            *interactive,
        ),
        ToolWorkloadContract::Service { .. } => {
            return Err(runtime_input_error(
                "A Tool Service release cannot be planned as a Runtime Task.",
            ))
        }
    };
    if interactive || success_exit_codes.as_slice() != [0] {
        return Err(UseError::new(
            "use.plugin.runtime.task_semantics_unsupported",
            "Runtime Task bindings currently require non-interactive execution with exit code 0 as the only success code.",
        ));
    }
    if timeout_ms != surface.timeout_ms {
        return Err(runtime_input_error(
            "The Tool Task manifest timeout does not match its release descriptor.",
        ));
    }
    if task_network == NetworkMode::Service {
        return Err(runtime_input_error(
            "Runtime Tasks cannot request Service network mode.",
        ));
    }

    let contract = RuntimeSurfaceContract::ToolTask {
        command_name: surface.command.clone(),
        json_output: surface.json_output,
        max_stdout_bytes,
        max_stderr_bytes,
    };
    let unit_id = unit_id(&context, "task", Some(invocation.invocation_id()))?;
    let resources = runtime_resources(&policy, Some(timeout_ms));
    let spec = RuntimeUnitSpec {
        schema: RuntimeUnitSpec::SCHEMA.to_string(),
        unit_id,
        generation: context.generation,
        class: RuntimeUnitClass::Task,
        artifact,
        process: RuntimeProcessSpec {
            command: entrypoint.clone(),
            args: invocation.args,
            working_directory: policy.working_directory,
            environment: policy.non_secret_environment,
        },
        mounts: policy.mounts,
        secrets: policy.secrets,
        network: RuntimeNetworkSpec {
            mode: task_network,
            ports: Vec::new(),
        },
        resources,
        isolation: policy.isolation,
        health: None,
        restart: RestartPolicy::Never,
        outputs: Vec::new(),
        semantics_profile_digest: None,
    };
    finish_plan(context, descriptor.descriptor_digest()?, spec, contract)
}

pub fn plan_tool_service_release(
    context: RuntimeSurfaceContext,
    surface: &ToolServiceSurface,
    descriptor: &ToolReleaseDescriptor,
    artifact: ArtifactRef,
    policy: RuntimeWorkloadPolicy,
) -> UseResult<RuntimeSurfacePlan> {
    require_surface_kind(&context, PluginSurfaceKind::Tool)?;
    descriptor.validate()?;
    verify_artifact(
        &artifact,
        &descriptor.artifact.digest,
        &descriptor.artifact.media_type,
    )?;
    let (
        port_name,
        port,
        base_path,
        health,
        startup_timeout_ms,
        shutdown_grace_ms,
        api_contract_digest,
    ) = match &descriptor.workload {
        ToolWorkloadContract::Service {
            port_name,
            port,
            base_path,
            health,
            startup_timeout_ms,
            shutdown_grace_ms,
            api_contract_digest,
            ..
        } => (
            port_name,
            *port,
            base_path,
            health,
            *startup_timeout_ms,
            *shutdown_grace_ms,
            api_contract_digest,
        ),
        ToolWorkloadContract::Task { .. } => {
            return Err(runtime_input_error(
                "A Tool Task release cannot be planned as a Runtime Service.",
            ))
        }
    };
    let contract = RuntimeSurfaceContract::ToolService {
        port_name: port_name.clone(),
        base_path: surface.base_path.clone(),
        shutdown_grace_ms,
        api_contract_digest: api_contract_digest.clone(),
    };
    if base_path != &surface.base_path {
        return Err(runtime_input_error(
            "The Tool Service manifest base path does not match its release descriptor.",
        ));
    }
    let spec = service_spec(
        &context,
        artifact,
        policy,
        port_name,
        port,
        health.path.as_str(),
        health.interval_ms,
        health.timeout_ms,
        health.success_threshold,
        health.failure_threshold,
        startup_timeout_ms,
    );
    finish_plan(context, descriptor.descriptor_digest()?, spec?, contract)
}

pub fn plan_mcp_service_release(
    context: RuntimeSurfaceContext,
    surface: &PluginMcpSurface,
    descriptor: &McpReleaseDescriptor,
    artifact: ArtifactRef,
    policy: RuntimeWorkloadPolicy,
) -> UseResult<RuntimeSurfacePlan> {
    require_surface_kind(&context, PluginSurfaceKind::Mcp)?;
    descriptor.validate()?;
    if !matches!(surface.launch, PluginMcpLaunch::StreamableHttp { .. }) {
        return Err(runtime_input_error(
            "Only Streamable HTTP MCP releases can be mapped to A3S Runtime.",
        ));
    }
    verify_artifact(
        &artifact,
        &descriptor.artifact.digest,
        &descriptor.artifact.media_type,
    )?;
    let service = &descriptor.service;
    let contract = RuntimeSurfaceContract::McpService {
        port_name: service.port_name.clone(),
        endpoint_path: service.endpoint_path.clone(),
        protocol_version: service.protocol_version.clone(),
        shutdown_grace_ms: service.shutdown_grace_ms,
    };
    let spec = service_spec(
        &context,
        artifact,
        policy,
        &service.port_name,
        service.port,
        &service.health.path,
        service.health.interval_ms,
        service.health.timeout_ms,
        service.health.success_threshold,
        service.health.failure_threshold,
        service.startup_timeout_ms,
    );
    finish_plan(context, descriptor.descriptor_digest()?, spec?, contract)
}

#[allow(clippy::too_many_arguments)]
fn service_spec(
    context: &RuntimeSurfaceContext,
    artifact: ArtifactRef,
    policy: RuntimeWorkloadPolicy,
    port_name: &str,
    port: u16,
    health_path: &str,
    health_interval_ms: u64,
    health_timeout_ms: u64,
    health_success_threshold: u32,
    health_failure_threshold: u32,
    startup_timeout_ms: u64,
) -> UseResult<RuntimeUnitSpec> {
    let resources = runtime_resources(&policy, None);
    Ok(RuntimeUnitSpec {
        schema: RuntimeUnitSpec::SCHEMA.to_string(),
        unit_id: unit_id(context, "service", None)?,
        generation: context.generation,
        class: RuntimeUnitClass::Service,
        artifact,
        process: RuntimeProcessSpec {
            command: Vec::new(),
            args: Vec::new(),
            working_directory: policy.working_directory,
            environment: policy.non_secret_environment,
        },
        mounts: policy.mounts,
        secrets: policy.secrets,
        network: RuntimeNetworkSpec {
            mode: NetworkMode::Service,
            ports: vec![RuntimePort {
                name: port_name.to_string(),
                container_port: port,
                protocol: TransportProtocol::Tcp,
            }],
        },
        resources,
        isolation: policy.isolation,
        health: Some(RuntimeHealthCheck {
            probe: HealthProbe::Http {
                port: port_name.to_string(),
                path: health_path.to_string(),
                expected_statuses: vec![200],
            },
            interval_ms: health_interval_ms,
            timeout_ms: health_timeout_ms,
            start_period_ms: startup_timeout_ms,
            success_threshold: health_success_threshold,
            failure_threshold: health_failure_threshold,
        }),
        restart: RestartPolicy::Always,
        outputs: Vec::new(),
        semantics_profile_digest: None,
    })
}

fn runtime_resources(
    policy: &RuntimeWorkloadPolicy,
    execution_timeout_ms: Option<u64>,
) -> ResourceLimits {
    ResourceLimits {
        cpu_millis: policy.resources.cpu_millis,
        memory_bytes: policy.resources.memory_bytes,
        pids: policy.resources.pids,
        ephemeral_storage_bytes: policy.resources.ephemeral_storage_bytes,
        execution_timeout_ms,
    }
}

fn finish_plan(
    context: RuntimeSurfaceContext,
    descriptor_digest: String,
    mut spec: RuntimeUnitSpec,
    contract: RuntimeSurfaceContract,
) -> UseResult<RuntimeSurfacePlan> {
    spec.validate().map_err(runtime_contract_error)?;
    let mut semantics_spec = spec.clone();
    if matches!(contract, RuntimeSurfaceContract::ToolTask { .. }) {
        semantics_spec.unit_id = "use:task-template".to_string();
        semantics_spec.process.args.clear();
    }
    let profile = RuntimeSemanticsProfile {
        schema: SEMANTICS_PROFILE_SCHEMA,
        package_id: &context.package_id,
        package_digest: &context.package_digest,
        scope_id: &context.scope_id,
        grant_digest: &context.grant_digest,
        surface_kind: context.surface.kind,
        surface_id: &context.surface.id,
        descriptor_digest: &descriptor_digest,
        runtime_spec: &semantics_spec,
        contract: &contract,
    };
    let bytes = serde_json::to_vec(&profile).map_err(|error| {
        runtime_contract_error(format!(
            "Failed to encode the Runtime semantics profile: {error}"
        ))
    })?;
    spec.semantics_profile_digest = Some(format!("sha256:{:x}", Sha256::digest(bytes)));
    spec.validate().map_err(runtime_contract_error)?;
    Ok(RuntimeSurfacePlan {
        context,
        descriptor_digest,
        spec,
        contract,
    })
}

fn verify_artifact(
    artifact: &ArtifactRef,
    descriptor_digest: &str,
    descriptor_media_type: &str,
) -> UseResult<()> {
    artifact.validate().map_err(runtime_contract_error)?;
    if artifact.digest != descriptor_digest || artifact.media_type != descriptor_media_type {
        return Err(UseError::new(
            "use.plugin.runtime.artifact_mismatch",
            "The resolved Runtime artifact does not match the signed release descriptor.",
        )
        .with_detail("expectedDigest", descriptor_digest)
        .with_detail("actualDigest", artifact.digest.clone())
        .with_detail("expectedMediaType", descriptor_media_type)
        .with_detail("actualMediaType", artifact.media_type.clone()));
    }
    Ok(())
}

fn require_surface_kind(
    context: &RuntimeSurfaceContext,
    expected: PluginSurfaceKind,
) -> UseResult<()> {
    if context.surface.kind != expected {
        return Err(runtime_input_error(
            "The Runtime release type does not match the named plugin surface kind.",
        ));
    }
    Ok(())
}

fn unit_id(
    context: &RuntimeSurfaceContext,
    class: &str,
    instance: Option<&str>,
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct UnitIdentity<'a> {
        schema: &'static str,
        package_id: &'a str,
        package_digest: &'a str,
        scope_id: &'a str,
        surface_kind: PluginSurfaceKind,
        surface_id: &'a str,
        generation: u64,
        instance: Option<&'a str>,
    }

    let identity = UnitIdentity {
        schema: "a3s.use.runtime-unit-identity.v1",
        package_id: &context.package_id,
        package_digest: &context.package_digest,
        scope_id: &context.scope_id,
        surface_kind: context.surface.kind,
        surface_id: &context.surface.id,
        generation: context.generation,
        instance,
    };
    let bytes = serde_json::to_vec(&identity).map_err(|error| {
        runtime_contract_error(format!(
            "Failed to encode the Runtime unit identity: {error}"
        ))
    })?;
    Ok(format!("use:{class}:{:x}", Sha256::digest(bytes)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSemanticsProfile<'a> {
    schema: &'static str,
    package_id: &'a str,
    package_digest: &'a str,
    scope_id: &'a str,
    grant_digest: &'a str,
    surface_kind: PluginSurfaceKind,
    surface_id: &'a str,
    descriptor_digest: &'a str,
    runtime_spec: &'a RuntimeUnitSpec,
    contract: &'a RuntimeSurfaceContract,
}
