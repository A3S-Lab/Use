use std::collections::BTreeMap;
use std::path::PathBuf;

use a3s_runtime::contract::{ArtifactRef, IsolationLevel, NetworkMode};
use a3s_use_core::{
    ExecutablePlanningSurface, PlanQualifiedSurfaceRef, PlannedPackageState, PluginPlanningBundle,
    PluginSurfaceKind, PluginWorkspaceGrantProposal, SurfacePermissionCeiling,
    ToolWorkloadContract, UseError, UseResult,
};
use a3s_use_extension::{
    PluginMcpLaunch, PluginMcpSurface, SurfaceActivation, ToolServiceSurface, ToolTaskSource,
    ToolTaskSurface,
};

use super::provider_selector::canonicalize_provider_assignments;
use super::{
    plan_mcp_service_release, plan_tool_service_release, plan_tool_task_release,
    RuntimeAuthorityBindings, RuntimeProviderAssignment, RuntimeResourcePolicy,
    RuntimeSurfaceContext, RuntimeSurfacePlan, RuntimeTaskInvocation, RuntimeWorkloadPolicy,
};

const PLANNING_RELEASE_PATH: &str = "planning/release.json";

/// Convert verified executable bundle semantics into provider-neutral Runtime
/// templates for one exact selected package state.
///
/// The authorization input is the canonical pre-confirmation grant proposal.
/// Binding a final grant here would create a digest cycle for `ask`: the final
/// grant contains confirmation evidence that itself binds the operation plan.
///
/// This base path accepts only containerized releases whose authority is fully
/// representable without host resource resolution: resources and private
/// Service networking. Filesystem and secret authority use the provider-bound
/// variant below. Exact egress allowlists, child processes, native execution,
/// and UI HTTP authority remain fail-closed under Runtime 0.2.
pub fn plan_runtime_bundle(
    bundle: &PluginPlanningBundle,
    package: &PlannedPackageState,
    proposal: &PluginWorkspaceGrantProposal,
    generation: u64,
) -> UseResult<Vec<RuntimeSurfacePlan>> {
    plan_runtime_bundle_with_authority(
        bundle,
        package,
        proposal,
        &RuntimeAuthorityBindings::default(),
        &[],
        generation,
    )
}

/// Convert a verified executable bundle with exact provider-bound,
/// host-owned filesystem and secret bindings into Runtime templates.
pub fn plan_runtime_bundle_with_authority(
    bundle: &PluginPlanningBundle,
    package: &PlannedPackageState,
    proposal: &PluginWorkspaceGrantProposal,
    authority: &RuntimeAuthorityBindings,
    assignments: &[RuntimeProviderAssignment],
    generation: u64,
) -> UseResult<Vec<RuntimeSurfacePlan>> {
    proposal.validate_against(&package.permissions)?;
    if package.release.package_id != bundle.package_id
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
    plan_runtime_bundle_with_authorization(
        bundle,
        package,
        &proposal.scope_id,
        &proposal.descriptor_digest()?,
        authority,
        assignments,
        generation,
    )
}

pub(super) fn plan_runtime_bundle_with_authorization(
    bundle: &PluginPlanningBundle,
    package: &PlannedPackageState,
    scope_id: &str,
    authorization_digest: &str,
    authority: &RuntimeAuthorityBindings,
    assignments: &[RuntimeProviderAssignment],
    generation: u64,
) -> UseResult<Vec<RuntimeSurfacePlan>> {
    validate_runtime_bundle_package(bundle, package, generation)?;
    authority.validate_against(&bundle.package_id, &package.permissions)?;
    authority.validate_provider_assignments(assignments)?;

    let selected = selected_runtime_surface_refs(package);
    if selected.is_empty() || selected.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(bundle_plan_error(
            "Runtime bundle planning requires sorted selected executable surfaces.",
        ));
    }
    if !authority.surfaces().is_empty() {
        let expected = selected
            .iter()
            .map(|surface| PlanQualifiedSurfaceRef {
                package_id: bundle.package_id.clone(),
                surface: surface.clone(),
            })
            .collect::<Vec<_>>();
        canonicalize_provider_assignments(&expected, assignments.to_vec())?;
    }

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
        let qualified_surface = PlanQualifiedSurfaceRef {
            package_id: bundle.package_id.clone(),
            surface: surface_ref.clone(),
        };
        let (mounts, secrets) = authority.resources_for(&qualified_surface);
        let policy = representable_policy(surface, permission, mounts, secrets)?;
        let context = RuntimeSurfaceContext::new(
            bundle.package_id.clone(),
            bundle.package_sha256.clone(),
            scope_id,
            authorization_digest,
            surface_ref,
            generation,
        )?;
        plans.push(plan_surface(context, surface, policy)?);
    }
    Ok(plans)
}

pub(super) fn validate_runtime_bundle_package(
    bundle: &PluginPlanningBundle,
    package: &PlannedPackageState,
    generation: u64,
) -> UseResult<()> {
    bundle.validate()?;
    package.permissions.validate()?;
    let release_surfaces = selected_runtime_surface_refs(package);
    let planning_surfaces = bundle
        .surfaces
        .iter()
        .map(ExecutablePlanningSurface::reference)
        .collect::<Vec<_>>();
    let permission_surfaces = package
        .permissions
        .surfaces
        .iter()
        .filter(|permission| {
            matches!(
                permission.surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            )
        })
        .map(|permission| permission.surface.clone())
        .collect::<Vec<_>>();
    if generation == 0
        || package.release.package_id != bundle.package_id
        || package.release.version != bundle.version
        || package.release.channel != bundle.channel
        || package.release.target != bundle.target
        || package.release.package_sha256 != bundle.package_sha256
        || package.release.manifest_sha256 != bundle.manifest_sha256
        || package.release.permission_ceiling_digest != bundle.permission_ceiling_digest
        || package.permissions.descriptor_digest()? != bundle.permission_ceiling_digest
        || release_surfaces.is_empty()
        || release_surfaces.windows(2).any(|pair| pair[0] >= pair[1])
        || release_surfaces
            .iter()
            .any(|surface| planning_surfaces.binary_search(surface).is_err())
        || release_surfaces != permission_surfaces
    {
        return Err(bundle_plan_error(
            "The Runtime planning inputs do not describe one exact package authorization.",
        ));
    }
    Ok(())
}

pub(super) fn selected_runtime_surface_refs(
    package: &PlannedPackageState,
) -> Vec<a3s_use_core::PluginSurfaceRef> {
    package
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
        .collect()
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
    mounts: Vec<a3s_runtime::contract::RuntimeMount>,
    secrets: Vec<a3s_runtime::contract::SecretReference>,
) -> UseResult<RuntimeWorkloadPolicy> {
    if permission.surface != surface.reference()
        || permission.native_execution
        || permission.child_process
        || !permission.network_egress.is_empty()
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
        mounts,
        secrets,
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
