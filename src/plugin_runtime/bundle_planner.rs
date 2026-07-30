use std::collections::BTreeMap;
use std::path::PathBuf;

use a3s_runtime::contract::{ArtifactRef, IsolationLevel, NetworkMode};
use a3s_use_core::{
    ExecutablePlanningSurface, PlannedPackageState, PluginPlanningBundle, PluginSurfaceKind,
    PluginWorkspaceGrantProposal, SurfacePermissionCeiling, ToolWorkloadContract, UseError,
    UseResult,
};
use a3s_use_extension::{
    PluginMcpLaunch, PluginMcpSurface, SurfaceActivation, ToolServiceSurface, ToolTaskSource,
    ToolTaskSurface,
};

use super::{
    plan_mcp_service_release, plan_tool_service_release, plan_tool_task_release,
    RuntimeResourcePolicy, RuntimeSurfaceContext, RuntimeSurfacePlan, RuntimeTaskInvocation,
    RuntimeWorkloadPolicy,
};

const PLANNING_RELEASE_PATH: &str = "planning/release.json";

/// Convert verified executable bundle semantics into provider-neutral Runtime
/// templates for one exact selected package state.
///
/// The authorization input is the canonical pre-confirmation grant proposal.
/// Binding a final grant here would create a digest cycle for `ask`: the final
/// grant contains confirmation evidence that itself binds the operation plan.
///
/// This first safe slice accepts only containerized releases whose authority
/// is fully representable by Runtime 0.2: resources and private Service
/// networking. Filesystem, egress allowlists, secrets, child processes, and
/// native execution fail closed until typed host adapters can map them.
pub fn plan_runtime_bundle(
    bundle: &PluginPlanningBundle,
    package: &PlannedPackageState,
    proposal: &PluginWorkspaceGrantProposal,
    generation: u64,
) -> UseResult<Vec<RuntimeSurfacePlan>> {
    bundle.validate()?;
    proposal.validate_against(&package.permissions)?;
    if generation == 0
        || package.release.package_id != bundle.package_id
        || package.release.version != bundle.version
        || package.release.channel != bundle.channel
        || package.release.target != bundle.target
        || package.release.package_sha256 != bundle.package_sha256
        || package.release.manifest_sha256 != bundle.manifest_sha256
        || package.release.permission_ceiling_digest != bundle.permission_ceiling_digest
        || proposal.package_id != bundle.package_id
        || proposal.package_digest != bundle.package_sha256
        || proposal.permission_ceiling_digest != bundle.permission_ceiling_digest
        || proposal.permissions != package.permissions
    {
        return Err(bundle_plan_error(
            "The Runtime planning inputs do not describe one exact package and grant proposal.",
        ));
    }

    let selected = package
        .release
        .surfaces
        .iter()
        .filter(|surface| {
            matches!(
                surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            )
        })
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    if selected.is_empty() || selected.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(bundle_plan_error(
            "Runtime bundle planning requires sorted selected executable surfaces.",
        ));
    }

    let authorization_digest = proposal.descriptor_digest()?;
    let mut plans = Vec::with_capacity(selected.len());
    for surface_ref in selected {
        let surface = bundle
            .surfaces
            .iter()
            .find(|surface| surface.reference() == surface_ref)
            .ok_or_else(|| {
                bundle_plan_error(
                    "The planning bundle omits a selected executable package surface.",
                )
            })?;
        let permission = package
            .permissions
            .surfaces
            .iter()
            .find(|permission| permission.surface == surface_ref)
            .ok_or_else(|| {
                bundle_plan_error(
                    "A selected executable surface has no resolved authorization proposal.",
                )
            })?;
        let policy = representable_policy(surface, permission)?;
        let context = RuntimeSurfaceContext::new(
            bundle.package_id.clone(),
            bundle.package_sha256.clone(),
            proposal.scope_id.clone(),
            authorization_digest.clone(),
            surface_ref,
            generation,
        )?;
        plans.push(plan_surface(context, surface, policy)?);
    }
    Ok(plans)
}

fn plan_surface(
    context: RuntimeSurfaceContext,
    surface: &ExecutablePlanningSurface,
    policy: RuntimeWorkloadPolicy,
) -> UseResult<RuntimeSurfacePlan> {
    match surface {
        ExecutablePlanningSurface::ToolTask {
            command,
            json_output,
            timeout_ms,
            descriptor,
            artifact,
            ..
        } => plan_tool_task_release(
            context,
            &ToolTaskSurface {
                source: ToolTaskSource::Release {
                    release: PathBuf::from(PLANNING_RELEASE_PATH),
                },
                command: command.clone(),
                json_output: *json_output,
                interactive: false,
                timeout_ms: *timeout_ms,
            },
            descriptor,
            runtime_artifact(artifact),
            RuntimeTaskInvocation::new("planning-template", Vec::new())?,
            policy,
            NetworkMode::None,
        ),
        ExecutablePlanningSurface::ToolService {
            base_path,
            descriptor,
            artifact,
            ..
        } => plan_tool_service_release(
            context,
            &ToolServiceSurface {
                release: PathBuf::from(PLANNING_RELEASE_PATH),
                base_path: base_path.clone(),
                contract: None,
            },
            descriptor,
            runtime_artifact(artifact),
            policy,
        ),
        ExecutablePlanningSurface::McpService {
            id,
            activation,
            descriptor,
            artifact,
        } => plan_mcp_service_release(
            context,
            &PluginMcpSurface {
                id: id.clone(),
                activation: match activation {
                    a3s_use_core::PlanningSurfaceActivation::Eager => SurfaceActivation::Eager,
                    a3s_use_core::PlanningSurfaceActivation::Lazy => SurfaceActivation::Lazy,
                },
                optional: false,
                launch: PluginMcpLaunch::StreamableHttp {
                    release: PathBuf::from(PLANNING_RELEASE_PATH),
                },
            },
            descriptor,
            runtime_artifact(artifact),
            policy,
        ),
    }
}

fn representable_policy(
    surface: &ExecutablePlanningSurface,
    permission: &SurfacePermissionCeiling,
) -> UseResult<RuntimeWorkloadPolicy> {
    if permission.surface != surface.reference()
        || permission.native_execution
        || permission.child_process
        || !permission.filesystem.is_empty()
        || !permission.network_egress.is_empty()
        || !permission.secrets.is_empty()
        || !permission.ui_http.is_empty()
    {
        return Err(unsupported_authority());
    }
    let resources = permission
        .resources
        .as_ref()
        .ok_or_else(unsupported_authority)?;
    let shape_matches = match surface {
        ExecutablePlanningSurface::ToolTask { descriptor, .. } => {
            let ToolWorkloadContract::Task {
                timeout_ms,
                max_stdout_bytes,
                max_stderr_bytes,
                ..
            } = descriptor.workload
            else {
                return Err(unsupported_authority());
            };
            !permission.private_service
                && resources.task_timeout_ms == Some(timeout_ms)
                && resources.max_stdout_bytes == Some(max_stdout_bytes)
                && resources.max_stderr_bytes == Some(max_stderr_bytes)
        }
        ExecutablePlanningSurface::ToolService { .. }
        | ExecutablePlanningSurface::McpService { .. } => {
            permission.private_service
                && resources.task_timeout_ms.is_none()
                && resources.max_stdout_bytes.is_none()
                && resources.max_stderr_bytes.is_none()
        }
    };
    if !shape_matches {
        return Err(unsupported_authority());
    }

    Ok(RuntimeWorkloadPolicy {
        isolation: IsolationLevel::Container,
        resources: RuntimeResourcePolicy {
            cpu_millis: resources.cpu_millis,
            memory_bytes: resources.memory_bytes,
            pids: resources.pids,
            ephemeral_storage_bytes: Some(resources.ephemeral_storage_bytes),
        },
        mounts: Vec::new(),
        secrets: Vec::new(),
        non_secret_environment: BTreeMap::new(),
        working_directory: None,
    })
}

fn runtime_artifact(artifact: &a3s_use_core::PlanningArtifactRef) -> ArtifactRef {
    ArtifactRef {
        uri: artifact.uri.clone(),
        digest: artifact.digest.clone(),
        media_type: artifact.media_type.clone(),
    }
}

fn unsupported_authority() -> UseError {
    UseError::new(
        "use.plugin.runtime.authorization_unsupported",
        "The selected executable authority cannot yet be represented by the locked Runtime contract.",
    )
}

fn bundle_plan_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.bundle_plan_invalid", message)
}
