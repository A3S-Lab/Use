use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_use::cognitive_package::{
    CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider,
    CognitivePackageEnablementPreparation, CognitivePackageEnablementRequest,
    CognitivePackageHostManager, CognitiveRegistryAccess,
    ReviewedCognitivePackageAuthorizationProvider, StandaloneCognitivePackageLifecycleFactory,
};
use a3s_use_core::{
    CatalogPlanningTarget, ExecutablePlanningSurface, PlanActor, PlanAuthority, PlanPolicyDecision,
    PlanScope, PlanScopeKind, PlanningSurfaceActivation, PluginDesiredState,
    PluginHostApplyRequest, PluginHostCancelRequest, PluginHostCancellationStatus,
    PluginHostEnablementPlanRequest, PluginHostEnablementPlanStatus, PluginHostManager,
    PluginHostObservationRequest, PluginHostObservationStatus, PluginHostOperationCancellability,
    PluginHostOperationObservationRequest, PluginHostOperationPhase, PluginHostPlanRequest,
    PluginManagedScope, PluginOperationAction, PluginOperationConfirmation, PluginOperationPlan,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPermissionCeiling, PluginPlanSource, PluginPlanningBundle, PluginSurfaceRef,
    PluginWorkspaceGrantChangeSet, ToolWorkloadClass, UseResult, PLUGIN_HOST_APPLY_REQUEST_SCHEMA,
    PLUGIN_HOST_CANCEL_REQUEST_SCHEMA, PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA,
    PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA, PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_PLAN_REQUEST_SCHEMA, PLUGIN_MANAGED_SCOPE_SCHEMA_V2,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use a3s_use_extension::{
    CognitivePackageFormFactor, CognitivePackageMediaKind, CognitivePackagePresentationIndexV1,
    CognitivePackagePresentationMediaV1, CognitivePackagePresentationRecordV1,
    CognitivePackagePresentationV1, PluginCatalogSearch, RegistrySourceInput, RegistrySourceStore,
    StoredWorkspaceGrant, VerifiedTargetCachePolicy, WorkspaceGrantLifecyclePhase,
    WorkspaceGrantStore, COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA,
    COGNITIVE_PACKAGE_PRESENTATION_SCHEMA,
};
use async_trait::async_trait;

const POLICY_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MANAGED_SCOPE_ID: &str = "workspace:research";
const PERMISSIONS: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/permission-ceiling-v1.json");

fn managed_extension_paths(home: &std::path::Path, scope: &PluginManagedScope) -> ExtensionPaths {
    extension_paths_for(home, scope.plan_scope())
}

#[derive(Debug)]
struct ConfirmAllPlans {
    authorization_count: Arc<AtomicUsize>,
}

#[path = "graph_grants/host_offline_apply.rs"]
mod host_offline_apply;
#[path = "graph_grants/host_operation_graph_progress.rs"]
mod host_operation_graph_progress;
#[path = "graph_grants/host_operations.rs"]
mod host_operations;
#[path = "graph_grants/host_permission_lifecycle.rs"]
mod host_permission_lifecycle;
#[path = "graph_grants/host_scope_isolation.rs"]
mod host_scope_isolation;
#[path = "graph_grants/host_scope_lifecycle.rs"]
mod host_scope_lifecycle;
#[path = "graph_grants/host_six_surface_lifecycle.rs"]
mod host_six_surface_lifecycle;
#[path = "graph_grants/host_two_scope_matrix.rs"]
mod host_two_scope_matrix;
#[path = "graph_grants/lifecycle.rs"]
mod lifecycle;
#[path = "graph_grants/plugin_manager.rs"]
mod plugin_manager;
#[path = "graph_grants/plugin_manager_cli.rs"]
mod plugin_manager_cli;
#[path = "graph_grants/presentation.rs"]
mod presentation;
#[path = "graph_grants/reviewed_plan.rs"]
mod reviewed_plan;

#[async_trait]
impl CognitivePackageAuthorizationProvider for ConfirmAllPlans {
    fn name(&self) -> &'static str {
        "integration-confirm-all"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        draft.validate()?;
        Ok(test_authority())
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        plan.validate()?;
        if plan.authority != test_authority() {
            return Err(a3s_use_core::UseError::new(
                "test.plugin.authority_changed",
                "The test authorization authority changed after planning.",
            ));
        }
        Ok(())
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        self.authorization_count.fetch_add(1, Ordering::SeqCst);
        CognitivePackageAuthorizationEvidence::confirmed(envelope, changes, now_ms)
    }
}

fn cognitive_tool_targets_version(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    target: &str,
) -> Vec<TestTarget> {
    cognitive_tool_targets_version_with_payload(fixture_root, package_id, route, version, target, 0)
}

pub(super) fn cognitive_tool_targets_version_with_payload(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    target: &str,
    payload_files: usize,
) -> Vec<TestTarget> {
    cognitive_tool_targets_version_with_dependencies_and_payload(
        fixture_root,
        package_id,
        route,
        version,
        target,
        Vec::new(),
        payload_files,
    )
}

pub(super) fn cognitive_tool_targets_version_with_dependencies_and_payload(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    target: &str,
    dependencies: Vec<PluginPackageDependency>,
    payload_files: usize,
) -> Vec<TestTarget> {
    let package_root = fixture_root.join("packages").join(route);
    std::fs::create_dir_all(package_root.join("tools/convert/bin")).unwrap();
    let dependency_blocks = dependencies
        .iter()
        .map(|dependency| {
            format!(
                "\n  dependency \"{}\" {{\n    version = \"{}\"\n  }}\n",
                dependency.package_id, dependency.version_requirement
            )
        })
        .collect::<String>();
    let manifest = format!(
        "extension \"{package_id}\" {{\n  schema_version = 3\n  version = \"{version}\"\n  route = \"{route}\"\n  requires_use = \">=0.3.0, <0.4.0\"\n  actions = [\"read\", \"execute\"]\n{dependency_blocks}\n  repository {{\n    url = \"https://github.com/acme/worker\"\n    revision = \"0123456789abcdef0123456789abcdef01234567\"\n  }}\n\n  tool \"convert\" {{\n    workload = \"task\"\n    interface = \"cli\"\n    executable = \"tools/convert/bin/convert\"\n    command = \"acme-worker-convert\"\n    json_output = true\n    interactive = false\n    timeout_ms = 120000\n    activation = \"lazy\"\n    optional = false\n  }}\n}}\n"
    );
    std::fs::write(package_root.join("a3s-use-extension.acl"), &manifest).unwrap();
    std::fs::write(
        package_root.join("README.md"),
        "# Worker\n\nPermission-bearing cognitive package fixture.\n",
    )
    .unwrap();
    std::fs::write(
        package_root.join("tools/convert/bin/convert"),
        "#!/bin/sh\nset -eu\nprintf '{\"status\":\"ok\"}\\n'\n",
    )
    .unwrap();
    if payload_files > 0 {
        let payload_root = package_root.join("payload");
        std::fs::create_dir_all(&payload_root).unwrap();
        for index in 0..payload_files {
            std::fs::write(
                payload_root.join(format!("{index:04}.bin")),
                b"grant-cutover-payload",
            )
            .unwrap();
        }
    }

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let manifest_sha256 = format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()));
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    catalog.package_id = package_id.to_string();
    catalog.display_name = format!("Worker {version}");
    catalog.description = "Permission-bearing cognitive package fixture.".to_string();
    catalog.publisher = "acme".to_string();
    catalog.keywords = vec!["fixture".to_string()];
    catalog.categories = vec!["test".to_string()];
    catalog.version = version.to_string();
    catalog.channel = PluginReleaseChannel::Stable;
    catalog.requires_use = ">=0.3.0, <0.4.0".to_string();
    catalog.dependencies = dependencies;
    catalog.target = target.to_string();
    catalog.surfaces = vec![CatalogSurface {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_string(),
        optional: false,
        workload: Some(ToolWorkloadClass::Task),
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    }];
    let mut permissions = PluginPermissionCeiling::from_json(PERMISSIONS).unwrap();
    permissions
        .surfaces
        .retain(|permission| permission.surface.id == "convert");
    permissions.validate().unwrap();
    catalog.permission_ceiling = permissions;
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    catalog.archive.target_name = format!(
        "extensions/{package_id}/{version}/stable/{target}/{route}-{version}-{target}.tar.gz"
    );
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.package.manifest_sha256 = Some(manifest_sha256);
    let planning_target =
        format!("extensions/{package_id}/{version}/stable/{target}/planning-v1.json");
    let planning = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_string(),
        package_id: package_id.to_string(),
        version: version.to_string(),
        channel: PluginReleaseChannel::Stable,
        target: target.to_string(),
        archive_sha256: catalog.archive.sha256.clone(),
        package_sha256: catalog.package.sha256.clone().unwrap(),
        manifest_sha256: catalog.package.manifest_sha256.clone().unwrap(),
        permission_ceiling_digest: catalog.permission_ceiling_digest.clone(),
        surfaces: vec![ExecutablePlanningSurface::ToolTaskNative {
            id: "convert".to_string(),
            activation: PlanningSurfaceActivation::Lazy,
            executable: "tools/convert/bin/convert".to_string(),
            command: "acme-worker-convert".to_string(),
            json_output: true,
            timeout_ms: 120_000,
        }],
    };
    let planning_bytes = planning.canonical_bytes().unwrap();
    catalog.planning = Some(CatalogPlanningTarget {
        target_name: planning_target.clone(),
        length: planning_bytes.len() as u64,
        sha256: format!("sha256:{:x}", Sha256::digest(&planning_bytes)),
    });
    catalog.license = "MIT".to_string();
    catalog.repository = "https://github.com/acme/worker".to_string();
    catalog.availability = CatalogAvailability::Available;
    catalog.validate().unwrap();

    vec![
        TestTarget {
            target_name: catalog.archive.target_name.clone(),
            custom: Some(serde_json::to_value(catalog).unwrap()),
            archive,
        },
        TestTarget {
            target_name: planning_target,
            custom: None,
            archive: planning_bytes,
        },
    ]
}

async fn assert_granted(
    home: &std::path::Path,
    scope: &PlanScope,
    package_digest: &str,
    ceiling: &PluginPermissionCeiling,
) {
    let record = WorkspaceGrantStore::new(extension_paths_for(home, scope.clone()).state_root())
        .observe(&scope.id, "acme/worker", package_digest)
        .await
        .unwrap()
        .unwrap();
    let StoredWorkspaceGrant::Granted(receipt) = record else {
        panic!("expected an active Grant receipt");
    };
    receipt.grant.validate_against(ceiling).unwrap();
    assert_eq!(receipt.grant.package_digest, package_digest);
    assert!(receipt.grant.authority.confirmation_digest.is_some());
}

async fn assert_revoked(home: &std::path::Path, scope: &PlanScope, package_digest: &str) {
    let record = WorkspaceGrantStore::new(extension_paths_for(home, scope.clone()).state_root())
        .observe(&scope.id, "acme/worker", package_digest)
        .await
        .unwrap()
        .unwrap();
    let StoredWorkspaceGrant::Revoked(revocation) = record else {
        panic!("expected an exact Grant revocation");
    };
    assert_eq!(revocation.package_digest, package_digest);
    assert!(revocation.authority.confirmation_digest.is_some());
}

fn test_authority() -> PlanAuthority {
    PlanAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: POLICY_DIGEST.to_string(),
        confirmation_required: true,
    }
}

struct HostLifecycleContext<'a> {
    host: &'a CognitivePackageHostManager,
    scope: &'a PluginManagedScope,
    capabilities_digest: &'a str,
    package_id: &'a PluginPackageId,
    search_query: &'a str,
    surface_kind: PluginSurfaceKind,
    registry_access: CognitiveRegistryAccess,
}

async fn host_lifecycle_release_operation(
    context: &HostLifecycleContext<'_>,
    action: PluginOperationAction,
    version: &str,
    selected_surfaces: Vec<PluginSurfaceRef>,
    label: &str,
    prior_lock: Option<a3s_use_core::PluginPackageLock>,
) -> (
    PluginHostPlanRequest,
    a3s_use_core::PluginHostPlanResult,
    PluginHostApplyRequest,
) {
    let (candidate, package_lock) = if action == PluginOperationAction::Uninstall {
        (None, prior_lock)
    } else {
        let candidate = context
            .host
            .search_cognitive_packages(
                context.registry_access,
                Some("fixture"),
                &PluginCatalogSearch {
                    query: context.search_query.to_owned(),
                    kind: Some(context.surface_kind),
                    channel: Some(PluginReleaseChannel::Stable),
                    publisher: Some("acme".to_owned()),
                    category: None,
                    availability: None,
                    cursor: None,
                    limit: 20,
                },
            )
            .await
            .unwrap()
            .plugins
            .into_iter()
            .find(|candidate| {
                candidate.record.package_id == context.package_id.as_str()
                    && candidate.record.version == version
            })
            .unwrap_or_else(|| {
                panic!(
                    "Registry search omitted {} {version}",
                    context.package_id.as_str()
                )
            });
        let lock = context
            .host
            .resolve_cognitive_package_lock(context.registry_access, &candidate)
            .await
            .unwrap();
        (Some(candidate), Some(lock))
    };
    let request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: format!("plan:scope-kind:{label}"),
        assignment_generation: 1,
        capabilities_digest: context.capabilities_digest.to_owned(),
        scope: context.scope.clone(),
        action,
        package_id: context.package_id.clone(),
        candidate,
        package_lock,
        selected_surfaces,
    };
    let planned = context
        .host
        .plan(request.clone())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{0:?} {label} {action:?} planning failed: {error:?}",
                request.scope.scope_kind
            )
        });
    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: format!("apply:scope-kind:{label}"),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: planned.plan.plan.operation_id.clone(),
            plan_digest: planned.plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: planned.plan.plan.created_at_ms + 1,
        }),
    };
    (request, planned, apply)
}
