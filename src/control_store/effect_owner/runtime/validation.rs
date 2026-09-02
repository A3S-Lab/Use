use super::*;
use a3s_runtime::contract::{
    HealthProbe, NetworkMode, RestartPolicy, RuntimeHealthCheck, RuntimePort, RuntimeUnitClass,
    TransportProtocol,
};
use a3s_use_core::{HttpHealthContract, McpReleaseDescriptor};
use a3s_use_extension::{
    PluginMcpLaunch, ToolTaskSource, ToolWorkload, VerifiedMcpSurfacePayload,
    VerifiedToolSurfacePayload,
};

pub(super) fn validate_plan_identity(
    request: &super::super::super::effect_port::ControlSurfaceEffectRequest,
    authority: &super::super::super::model::ControlRuntimeEffectAuthority,
    selected: &SelectedRuntimeSurface,
) -> UseResult<()> {
    let plan = selected.plan();
    plan.validate()?;
    let context = plan.context();
    // Runtime planning binds the stable pre-confirmation Grant proposal.  The
    // committed Grant itself also contains confirmation/timing evidence; its
    // full descriptor digest must not be used here because that would make an
    // `Ask` plan depend cyclically on its own confirmation-bound plan digest.
    let grant_matches = authority.grant_proposal_digest.as_deref() == Some(context.grant_digest());
    if context.package_id() != request.package_id
        || context.package_digest() != request.package_digest
        || context.scope() != &request.identity.installation
        || context.surface() != &request.surface
        || context.generation() != request.lifecycle_generation
        || !grant_matches
        || authority.provider_selection.evidence.surface != plan.surface()
        || selected.provider().semantics_profile_digest
            != plan
                .spec()
                .semantics_profile_digest
                .as_deref()
                .unwrap_or_default()
    {
        return Err(runtime_error(
            RUNTIME_PLAN_ERROR,
            "The selected Runtime plan does not bind the committed package, scope, surface, or generation.",
        ));
    }
    let contract_matches = matches!(
        (request.surface.kind, plan.contract()),
        (
            PluginSurfaceKind::Tool,
            RuntimeSurfaceContract::ToolTask { .. }
        ) | (
            PluginSurfaceKind::Tool,
            RuntimeSurfaceContract::ToolService { .. }
        ) | (
            PluginSurfaceKind::Mcp,
            RuntimeSurfaceContract::McpService { .. }
        )
    );
    if !contract_matches {
        return Err(runtime_error(
            RUNTIME_PLAN_ERROR,
            "The selected Runtime contract does not match the committed surface kind.",
        ));
    }
    Ok(())
}

pub(super) fn validate_tool_payload(
    plan: &RuntimeSurfacePlan,
    payload: &VerifiedToolSurfacePayload,
) -> UseResult<()> {
    let descriptor = payload.descriptor();
    descriptor.validate()?;
    if payload.surface().id != plan.context().surface().id
        || descriptor.descriptor_digest()? != plan.descriptor_digest()
        || descriptor.artifact.digest != plan.spec().artifact.digest
        || descriptor.artifact.media_type != plan.spec().artifact.media_type
    {
        return Err(runtime_error(
            RUNTIME_PLAN_ERROR,
            "The verified Tool release does not match the reviewed Runtime plan.",
        ));
    }
    match (&payload.surface().workload, plan.contract()) {
        (
            ToolWorkload::Task(task),
            RuntimeSurfaceContract::ToolTask {
                command_name,
                json_output,
                max_stdout_bytes,
                max_stderr_bytes,
            },
        ) => {
            let ToolTaskSource::Release { .. } = task.source else {
                return Err(runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "A Runtime Tool Task must be release-backed.",
                ));
            };
            let a3s_use_core::ToolWorkloadContract::Task {
                entrypoint,
                timeout_ms,
                max_stdout_bytes: descriptor_stdout,
                max_stderr_bytes: descriptor_stderr,
                interactive,
                success_exit_codes,
                ..
            } = &descriptor.workload
            else {
                return Err(runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "The Tool release workload is not a Task.",
                ));
            };
            if command_name != &task.command
                || json_output != &task.json_output
                || max_stdout_bytes != descriptor_stdout
                || max_stderr_bytes != descriptor_stderr
                || *interactive
                || task.interactive
                || success_exit_codes.as_slice() != [0]
                || *timeout_ms != task.timeout_ms
                || plan.spec().process.command != *entrypoint
                || plan.spec().class != RuntimeUnitClass::Task
                || plan.spec().resources.execution_timeout_ms != Some(*timeout_ms)
                || plan.spec().restart != RestartPolicy::Never
            {
                return Err(runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "The verified Tool Task semantics differ from the reviewed Runtime plan.",
                ));
            }
        }
        (
            ToolWorkload::Service(surface),
            RuntimeSurfaceContract::ToolService {
                port_name,
                base_path,
                shutdown_grace_ms: contract_shutdown_grace_ms,
                api_contract_digest,
            },
        ) => {
            let a3s_use_core::ToolWorkloadContract::Service {
                port_name: descriptor_port,
                port,
                base_path: descriptor_path,
                health,
                startup_timeout_ms,
                shutdown_grace_ms,
                api_contract_digest: descriptor_contract,
                ..
            } = &descriptor.workload
            else {
                return Err(runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "The Tool release workload is not a Service.",
                ));
            };
            let Some(plan_port) = plan.spec().network.ports.first() else {
                return Err(runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "The Runtime Service omitted its port.",
                ));
            };
            if surface.base_path != *base_path
                || base_path != descriptor_path
                || port_name != descriptor_port
                || api_contract_digest != descriptor_contract
                || contract_shutdown_grace_ms != shutdown_grace_ms
                || !service_spec_matches(
                    plan,
                    plan_port,
                    descriptor_port,
                    *port,
                    health,
                    *startup_timeout_ms,
                )
            {
                return Err(runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "The verified Tool Service semantics differ from the reviewed Runtime plan.",
                ));
            }
        }
        _ => {
            return Err(runtime_error(
                RUNTIME_PLAN_ERROR,
                "The verified Tool surface kind does not match the Runtime contract.",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_mcp_payload(
    plan: &RuntimeSurfacePlan,
    payload: &VerifiedMcpSurfacePayload,
) -> UseResult<()> {
    let descriptor: &McpReleaseDescriptor = payload.descriptor();
    descriptor.validate()?;
    if payload.surface().id != plan.context().surface().id
        || !matches!(
            payload.surface().launch,
            PluginMcpLaunch::StreamableHttp { .. }
        )
        || descriptor.descriptor_digest()? != plan.descriptor_digest()
        || descriptor.artifact.digest != plan.spec().artifact.digest
        || descriptor.artifact.media_type != plan.spec().artifact.media_type
    {
        return Err(runtime_error(
            RUNTIME_PLAN_ERROR,
            "The verified MCP release does not match the reviewed Runtime plan.",
        ));
    }
    let RuntimeSurfaceContract::McpService {
        port_name,
        endpoint_path,
        protocol_version,
        shutdown_grace_ms,
    } = plan.contract()
    else {
        return Err(runtime_error(
            RUNTIME_PLAN_ERROR,
            "The verified MCP surface does not have a Runtime Service contract.",
        ));
    };
    if descriptor.service.port_name != *port_name
        || descriptor.service.endpoint_path != *endpoint_path
        || descriptor.service.protocol_version != *protocol_version
        || descriptor.service.shutdown_grace_ms != *shutdown_grace_ms
        || plan.spec().network.ports.first().is_none_or(|plan_port| {
            !service_spec_matches(
                plan,
                plan_port,
                &descriptor.service.port_name,
                descriptor.service.port,
                &descriptor.service.health,
                descriptor.service.startup_timeout_ms,
            )
        })
    {
        return Err(runtime_error(
            RUNTIME_PLAN_ERROR,
            "The verified MCP Service semantics differ from the reviewed Runtime plan.",
        ));
    }
    Ok(())
}

fn service_spec_matches(
    plan: &RuntimeSurfacePlan,
    plan_port: &RuntimePort,
    port_name: &str,
    port: u16,
    health: &HttpHealthContract,
    startup_timeout_ms: u64,
) -> bool {
    let expected_health = RuntimeHealthCheck {
        probe: HealthProbe::Http {
            port: port_name.to_string(),
            path: health.path.clone(),
            expected_statuses: vec![200],
        },
        interval_ms: health.interval_ms,
        timeout_ms: health.timeout_ms,
        start_period_ms: startup_timeout_ms,
        success_threshold: health.success_threshold,
        failure_threshold: health.failure_threshold,
    };
    plan.spec().class == RuntimeUnitClass::Service
        && plan.spec().process.command.is_empty()
        && plan.spec().process.args.is_empty()
        && plan.spec().network.mode == NetworkMode::Service
        && plan.spec().network.ports.len() == 1
        && plan_port
            == &RuntimePort {
                name: port_name.to_string(),
                container_port: port,
                protocol: TransportProtocol::Tcp,
            }
        && plan.spec().health.as_ref() == Some(&expected_health)
        && plan.spec().restart == RestartPolicy::Always
}

pub(super) fn validate_receipt(
    request: &ControlRuntimeEffectRequest,
    selected: &SelectedRuntimeSurface,
    receipt: &RuntimeBindingReceipt,
) -> UseResult<()> {
    receipt.validate()?;
    let plan = selected.plan();
    let provider = selected.provider();
    let expected_spec_digest = plan
        .spec()
        .digest()
        .map_err(|message| runtime_error(RUNTIME_PLAN_ERROR, message))?;
    let expected_surface = plan.surface();
    let expected_scope = plan.context().scope();
    let common = |surface: &a3s_use_core::PlanQualifiedSurfaceRef,
                  package_digest: &str,
                  scope: &a3s_use_core::PlanScope,
                  descriptor_digest: &str,
                  provider_id: &str,
                  provider_build_id: &str,
                  capability_digest: &str,
                  semantics: &str,
                  enforcement| {
        surface == &expected_surface
            && package_digest == plan.context().package_digest()
            && scope == expected_scope
            && descriptor_digest == plan.descriptor_digest()
            && provider_id == provider.provider_id
            && provider_build_id == provider.provider_build_id
            && capability_digest == provider.capability_digest
            && semantics == provider.semantics_profile_digest
            && enforcement == provider.enforcement
    };
    let matches = match receipt {
        RuntimeBindingReceipt::Task(binding) => {
            common(
                &binding.surface,
                &binding.package_digest,
                &binding.scope,
                &binding.descriptor_digest,
                &binding.provider_id,
                &binding.provider_build_id,
                &binding.capability_digest,
                &binding.semantics_profile_digest,
                binding.enforcement,
            ) && binding.generation() == plan.context().generation()
                && binding.contract == *plan.contract()
                && plan.spec().class == RuntimeUnitClass::Task
        }
        RuntimeBindingReceipt::Service(binding) => {
            common(
                &binding.surface,
                &binding.package_digest,
                &binding.scope,
                &binding.descriptor_digest,
                &binding.provider_id,
                &binding.provider_build_id,
                &binding.capability_digest,
                &binding.semantics_profile_digest,
                binding.enforcement,
            ) && binding.generation == plan.context().generation()
                && binding.spec_digest == expected_spec_digest
                && binding.contract == *plan.contract()
                && plan.spec().class == RuntimeUnitClass::Service
        }
    };
    if !matches
        || request.surface.package_digest != receipt.package_digest()
        || request.surface.lifecycle_generation != receipt.generation()
    {
        return Err(runtime_error(
            RUNTIME_AUTHORITY_ERROR,
            "The retained Runtime receipt does not belong to the committed generation.",
        ));
    }
    Ok(())
}
