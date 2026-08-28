use std::path::{Path, PathBuf};

use super::*;

use a3s_use_core::{
    CatalogAvailability, CatalogMcpTransport, CatalogPlanningTarget, CatalogSurface,
    ExecutablePlanningSurface, PlanningSurfaceActivation, PluginPermissionCeiling,
    PluginSurfaceKind, ResourcePermissionCeiling, SurfacePermissionCeiling,
    PLUGIN_PERMISSION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
};

const PACKAGE_ID: &str = "acme/cognitive";
const ROUTE: &str = "cognitive";
const SCOPE_ID: &str = "shared:cognitive";

/// Exercise one remote package that contributes every schema-v3 surface. The
/// same Host plan/apply boundary is used for both managed scope kinds, and the
/// exact operation requests are replayed after reconstructing the Host.
#[test]
fn host_manager_replays_the_complete_six_surface_lifecycle() {
    const TEST_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

    std::thread::Builder::new()
        .name("six-surface-host-lifecycle".to_owned())
        .stack_size(TEST_THREAD_STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(host_manager_replays_the_complete_six_surface_lifecycle_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn host_manager_replays_the_complete_six_surface_lifecycle_scenario() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let compiler = fake_flow_compiler(temporary.path());
    let repository = TestRepository::with_targets(
        [
            cognitive_six_surface_targets(&temporary.path().join("v1"), "1.0.0", &target),
            cognitive_six_surface_targets(&temporary.path().join("v2"), "2.0.0", &target),
        ]
        .into_iter()
        .flatten()
        .collect(),
        141,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());

    for (scope_kind, label) in [
        (PlanScopeKind::User, "user"),
        (PlanScopeKind::Workspace, "workspace"),
    ] {
        let home = temporary.path().join(format!("{label}-six-surface-home"));
        let scope = managed_scope(scope_kind);
        let paths = managed_extension_paths(&home, &scope);
        RegistrySourceStore::new(use_paths(&home))
            .add(RegistrySourceInput::new(
                "fixture",
                server.base_url(),
                &repository.root_sha256,
                None,
                VerifiedTargetCachePolicy::default(),
            ))
            .await
            .unwrap();

        let host = six_surface_host(&scope, paths.clone(), &compiler);
        let capabilities_digest = host
            .capabilities()
            .await
            .unwrap()
            .descriptor_digest()
            .unwrap();
        let package_id = PluginPackageId::parse(PACKAGE_ID).unwrap();
        let selected_surfaces = six_surface_refs();

        let (install_request, install_plan, install_apply) = plan_operation(
            &host,
            &scope,
            &capabilities_digest,
            &package_id,
            PluginOperationAction::Install,
            "1.0.0",
            selected_surfaces.clone(),
            "install",
            None,
        )
        .await;
        assert_eq!(install_plan.plan.plan.scope.kind, scope_kind);
        assert_scope_fence_rejected(
            &host,
            &scope,
            &capabilities_digest,
            &package_id,
            &install_request,
            &install_apply,
            &install_plan,
        )
        .await;
        assert_eq!(
            install_plan.plan.plan.packages[0]
                .after
                .as_ref()
                .unwrap()
                .release
                .surfaces
                .iter()
                .map(|surface| surface.reference())
                .collect::<Vec<_>>(),
            selected_surfaces
        );
        assert_eq!(install_plan.plan.plan.packages[0].surfaces.len(), 6);
        let installed = host.apply(install_apply.clone()).await.unwrap();
        assert!(!installed.replayed);
        assert_ready_state(&installed.state, "1.0.0", &selected_surfaces);
        let first_generation = installed.state.package_generation.unwrap();
        assert_six_surface_assets(&host, &scope, &paths, "1.0.0").await;
        assert_operation_completed(
            &host,
            &scope,
            &capabilities_digest,
            &package_id,
            &install_plan,
            "observe:six:install",
            &installed.operation_result_digest,
        )
        .await;

        drop(host);
        let restarted = six_surface_host(&scope, paths.clone(), &compiler);
        let replayed_install = restarted.apply(install_apply.clone()).await.unwrap();
        assert!(replayed_install.replayed);
        assert_eq!(
            replayed_install.operation_result_digest,
            installed.operation_result_digest
        );
        assert_eq!(
            replayed_install.state.package_generation,
            installed.state.package_generation
        );
        assert_ready_state(&replayed_install.state, "1.0.0", &selected_surfaces);
        assert_six_surface_assets(&restarted, &scope, &paths, "1.0.0").await;

        let (upgrade_request, upgrade_plan, upgrade_apply) = plan_operation(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            PluginOperationAction::Upgrade,
            "2.0.0",
            selected_surfaces.clone(),
            "upgrade",
            install_request.package_lock.clone(),
        )
        .await;
        assert_scope_fence_rejected(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            &upgrade_request,
            &upgrade_apply,
            &upgrade_plan,
        )
        .await;
        let upgraded = restarted.apply(upgrade_apply.clone()).await.unwrap();
        assert!(!upgraded.replayed);
        assert_ready_state(&upgraded.state, "2.0.0", &selected_surfaces);
        assert!(upgraded
            .state
            .package_generation
            .is_some_and(|generation| generation > first_generation));
        assert_six_surface_assets(&restarted, &scope, &paths, "2.0.0").await;
        assert_operation_completed(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            &upgrade_plan,
            "observe:six:upgrade",
            &upgraded.operation_result_digest,
        )
        .await;

        drop(restarted);
        let restarted = six_surface_host(&scope, paths.clone(), &compiler);
        let replayed_upgrade = restarted.apply(upgrade_apply.clone()).await.unwrap();
        assert!(replayed_upgrade.replayed);
        assert_eq!(
            replayed_upgrade.operation_result_digest,
            upgraded.operation_result_digest
        );
        assert_eq!(
            replayed_upgrade.state.package_generation,
            upgraded.state.package_generation
        );
        assert_ready_state(&replayed_upgrade.state, "2.0.0", &selected_surfaces);
        assert_six_surface_assets(&restarted, &scope, &paths, "2.0.0").await;

        let (uninstall_request, uninstall_plan, uninstall_apply) = plan_operation(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            PluginOperationAction::Uninstall,
            "2.0.0",
            Vec::new(),
            "uninstall",
            upgrade_request.package_lock.clone(),
        )
        .await;
        assert_scope_fence_rejected(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            &uninstall_request,
            &uninstall_apply,
            &uninstall_plan,
        )
        .await;
        let uninstalled = restarted.apply(uninstall_apply.clone()).await.unwrap();
        assert!(!uninstalled.replayed);
        assert_eq!(uninstalled.state.desired, PluginDesiredState::Absent);
        assert_eq!(uninstalled.state.observed, PluginObservedState::Removed);
        assert!(uninstalled.state.version.is_none());
        assert_operation_completed(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            &uninstall_plan,
            "observe:six:uninstall",
            &uninstalled.operation_result_digest,
        )
        .await;

        drop(restarted);
        let restarted = six_surface_host(&scope, paths.clone(), &compiler);
        let replayed_uninstall = restarted.apply(uninstall_apply).await.unwrap();
        assert!(replayed_uninstall.replayed);
        assert_eq!(
            replayed_uninstall.operation_result_digest,
            uninstalled.operation_result_digest
        );
        assert_eq!(replayed_uninstall.state.desired, PluginDesiredState::Absent);
        assert_eq!(
            replayed_uninstall.state.observed,
            PluginObservedState::Removed
        );
    }
}

fn six_surface_refs() -> Vec<PluginSurfaceRef> {
    vec![
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "reason".to_owned(),
        },
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Mcp,
            id: "context".to_owned(),
        },
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain".to_owned(),
        },
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "reason".to_owned(),
        },
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "echo".to_owned(),
        },
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Ui,
            id: "reason".to_owned(),
        },
    ]
}

fn managed_scope(kind: PlanScopeKind) -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:six-surface-lifecycle".to_owned(),
        scope_kind: kind,
        scope_id: SCOPE_ID.to_owned(),
        authority_id: "six-surface:lifecycle".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "f".repeat(64)),
    }
}

fn six_surface_host(
    scope: &PluginManagedScope,
    paths: ExtensionPaths,
    compiler: &Path,
) -> CognitivePackageHostManager {
    let lifecycle =
        StandaloneCognitivePackageLifecycleFactory::with_flow_compiler(compiler).unwrap();
    CognitivePackageHostManager::new(
        scope.clone(),
        "use:six-surface-lifecycle",
        ExtensionRegistry::new(paths),
        Arc::new(lifecycle),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn plan_operation(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    package_id: &PluginPackageId,
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
        let candidate = host
            .search_cognitive_packages(
                CognitiveRegistryAccess::Refreshed,
                Some("fixture"),
                &PluginCatalogSearch {
                    query: ROUTE.to_owned(),
                    kind: None,
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
            .find(|candidate| candidate.record.version == version)
            .unwrap_or_else(|| panic!("Registry search omitted {PACKAGE_ID} {version}"));
        let lock = host
            .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
            .await
            .unwrap();
        (Some(candidate), Some(lock))
    };
    let request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: format!("plan:six-surface:{label}"),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.to_owned(),
        scope: scope.clone(),
        action,
        package_id: package_id.clone(),
        candidate,
        package_lock,
        selected_surfaces,
    };
    let planned = host
        .plan(request.clone())
        .await
        .unwrap_or_else(|error| panic!("{action:?} planning failed: {error:?}"));
    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: format!("apply:six-surface:{label}"),
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

fn assert_ready_state(
    state: &a3s_use_core::PluginHostPackageState,
    version: &str,
    selected_surfaces: &[PluginSurfaceRef],
) {
    assert_eq!(state.version.as_deref(), Some(version));
    assert_eq!(state.desired, PluginDesiredState::Enabled);
    assert_eq!(state.observed, PluginObservedState::Ready);
    assert_eq!(state.selected_surfaces, selected_surfaces);
    assert!(state
        .package_generation
        .is_some_and(|generation| generation > 0));
}

async fn assert_scope_fence_rejected(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    package_id: &PluginPackageId,
    request: &PluginHostPlanRequest,
    apply: &PluginHostApplyRequest,
    planned: &a3s_use_core::PluginHostPlanResult,
) {
    let wrong_scope = opposite_scope(scope);
    let mut wrong_plan = request.clone();
    wrong_plan.scope = wrong_scope.clone();
    assert_eq!(
        host.plan(wrong_plan).await.unwrap_err().code,
        "use.plugin.managed_scope_fence_mismatch"
    );

    let mut wrong_apply = apply.clone();
    wrong_apply.scope = wrong_scope.clone();
    assert_eq!(
        host.apply(wrong_apply).await.unwrap_err().code,
        "use.plugin.managed_scope_fence_mismatch"
    );

    let wrong_observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:six:wrong-scope".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.to_owned(),
        scope: wrong_scope,
        package_id: package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
    };
    assert_eq!(
        host.observe_operation(wrong_observation)
            .await
            .unwrap_err()
            .code,
        "use.plugin.managed_scope_fence_mismatch"
    );
}

fn opposite_scope(scope: &PluginManagedScope) -> PluginManagedScope {
    let mut opposite = scope.clone();
    opposite.scope_kind = match scope.scope_kind {
        PlanScopeKind::User => PlanScopeKind::Workspace,
        PlanScopeKind::Workspace => PlanScopeKind::User,
    };
    opposite
}

async fn assert_six_surface_assets(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    paths: &ExtensionPaths,
    version: &str,
) {
    let registry = ExtensionRegistry::new(paths.clone());
    let lease = registry
        .acquire_published_route(ROUTE)
        .await
        .unwrap()
        .expect("the complete package route must be published");
    let extension = lease.extension();
    assert_eq!(extension.receipt.package_id, PACKAGE_ID);
    assert_eq!(extension.receipt.version, version);
    assert_eq!(extension.receipt.selected_surfaces, six_surface_refs());
    assert_eq!(extension.manifest.tools.len(), 1);
    assert_eq!(extension.manifest.mcp_servers.len(), 1);
    assert_eq!(extension.manifest.okf.len(), 1);
    assert_eq!(extension.manifest.flows.len(), 1);
    assert_eq!(extension.manifest.skills.len(), 1);
    assert_eq!(extension.manifest.ui.len(), 1);
    let planning = extension
        .plan_ready_planning_bundle()
        .unwrap()
        .expect("executable planning evidence must be retained");
    assert_eq!(
        planning
            .surfaces
            .iter()
            .map(ExecutablePlanningSurface::reference)
            .collect::<Vec<_>>(),
        vec![
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Mcp,
                id: "context".to_owned(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "echo".to_owned(),
            },
        ]
    );

    let root = &extension.receipt.package_root;
    assert!(root.join("tools/echo").is_file());
    assert!(root.join("mcp/context").is_file());
    assert!(root.join("flows/reason.ts").is_file());
    assert!(root.join("skills/reason/SKILL.md").is_file());
    assert!(root.join("ui/reason/index.html").is_file());
    assert!(root.join("okf/domain/concepts/lifecycle.md").is_file());

    #[cfg(unix)]
    {
        let tool = Command::new(root.join("tools/echo")).output().unwrap();
        assert!(tool.status.success());
        assert!(String::from_utf8_lossy(&tool.stdout).contains("cognitive package ready"));
        let mcp = Command::new(root.join("mcp/context"))
            .arg("--stdio")
            .output()
            .unwrap();
        assert!(mcp.status.success());
        assert!(String::from_utf8_lossy(&mcp.stdout).contains("stdio MCP fixture"));
    }

    let capability = host
        .acquire_cognitive_capability(scope, PACKAGE_ID, "domain")
        .await
        .unwrap()
        .expect("the exact OKF capability must be callable");
    let search = capability
        .knowledge()
        .search("atomic cognitive package", 4)
        .await
        .unwrap();
    assert!(!search.hits.is_empty());
    assert_eq!(
        capability.evidence().lifecycle_generation,
        extension.receipt.lifecycle_generation.unwrap()
    );
}

fn cognitive_six_surface_targets(
    fixture_root: &Path,
    version: &str,
    target: &str,
) -> Vec<TestTarget> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-cognitive/package");
    let package_root = fixture_root.join("package");
    copy_fixture_tree(&source, &package_root);
    let manifest_path = package_root.join("a3s-use-extension.acl");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap().replace(
        "version        = \"1.0.0\"",
        &format!("version        = \"{version}\""),
    );
    std::fs::write(&manifest_path, &manifest).unwrap();
    let parsed = a3s_use_extension::ExtensionManifest::parse_acl(&manifest).unwrap();
    parsed.validate_package_root(&package_root).unwrap();

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let package_sha256 = format!("sha256:{}", fingerprint.0);
    let manifest_sha256 = format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()));
    let archive_sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    let permissions = native_permission_ceiling();
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    catalog.package_id = PACKAGE_ID.to_owned();
    catalog.display_name = format!("Complete Cognitive Package {version}");
    catalog.description = "A complete six-surface cognitive package fixture.".to_owned();
    catalog.publisher = "acme".to_owned();
    catalog.keywords = vec!["cognitive".to_owned(), "fixture".to_owned()];
    catalog.categories = vec!["cognitive".to_owned()];
    catalog.version = version.to_owned();
    catalog.channel = PluginReleaseChannel::Stable;
    catalog.requires_use = ">=0.3.0, <0.4.0".to_owned();
    catalog.dependencies.clear();
    catalog.target = target.to_owned();
    catalog.surfaces = parsed
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| CatalogSurface {
            kind: surface.surface.kind,
            id: surface.surface.id.clone(),
            optional: surface.optional,
            workload: parsed
                .tools
                .iter()
                .find(|tool| tool.id == surface.surface.id)
                .map(|tool| match tool.workload {
                    a3s_use_extension::ToolWorkload::Task(_) => ToolWorkloadClass::Task,
                    a3s_use_extension::ToolWorkload::Service(_) => ToolWorkloadClass::Service,
                }),
            mcp_transport: parsed
                .mcp_servers
                .iter()
                .find(|mcp| mcp.id == surface.surface.id)
                .map(|mcp| match mcp.launch {
                    a3s_use_extension::PluginMcpLaunch::Stdio { .. } => CatalogMcpTransport::Stdio,
                    a3s_use_extension::PluginMcpLaunch::StreamableHttp { .. } => {
                        CatalogMcpTransport::StreamableHttp
                    }
                }),
            mcp_tool_count: None,
            okf_bundle: parsed
                .okf
                .iter()
                .find(|okf| okf.id == surface.surface.id)
                .map(|okf| okf.bundle.clone()),
            requires: surface.dependencies,
        })
        .collect();
    catalog.permission_ceiling = permissions;
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    let archive_name = format!(
        "extensions/{PACKAGE_ID}/{version}/stable/{target}/{ROUTE}-{version}-{target}.tar.gz"
    );
    catalog.archive.target_name = archive_name.clone();
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = archive_sha256.clone();
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(package_sha256.clone());
    catalog.package.manifest_sha256 = Some(manifest_sha256.clone());
    catalog.license = "MIT".to_owned();
    catalog.repository = "https://github.com/acme/cognitive".to_owned();
    catalog.availability = CatalogAvailability::Available;

    let planning = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: PACKAGE_ID.to_owned(),
        version: version.to_owned(),
        channel: PluginReleaseChannel::Stable,
        target: target.to_owned(),
        archive_sha256,
        package_sha256,
        manifest_sha256,
        permission_ceiling_digest: catalog.permission_ceiling_digest.clone(),
        surfaces: vec![
            ExecutablePlanningSurface::McpStdio {
                id: "context".to_owned(),
                activation: PlanningSurfaceActivation::Lazy,
                executable: "mcp/context".to_owned(),
                args: vec!["--stdio".to_owned()],
            },
            ExecutablePlanningSurface::ToolTaskNative {
                id: "echo".to_owned(),
                activation: PlanningSurfaceActivation::Lazy,
                executable: "tools/echo".to_owned(),
                command: "acme-cognitive-echo".to_owned(),
                json_output: true,
                timeout_ms: 30_000,
            },
        ],
    };
    let planning_bytes = planning.canonical_bytes().unwrap();
    let planning_name =
        format!("extensions/{PACKAGE_ID}/{version}/stable/{target}/planning-v1.json");
    catalog.planning = Some(CatalogPlanningTarget {
        target_name: planning_name.clone(),
        length: planning_bytes.len() as u64,
        sha256: format!("sha256:{:x}", Sha256::digest(&planning_bytes)),
    });
    catalog.validate().unwrap();

    vec![
        TestTarget {
            target_name: archive_name,
            custom: Some(serde_json::to_value(catalog).unwrap()),
            archive,
        },
        TestTarget::raw(planning_name, planning_bytes),
    ]
}

fn native_permission_ceiling() -> PluginPermissionCeiling {
    let task_resources = || ResourcePermissionCeiling {
        cpu_millis: 100,
        memory_bytes: 1_048_576,
        pids: 4,
        ephemeral_storage_bytes: 1_048_576,
        task_timeout_ms: Some(30_000),
        max_stdout_bytes: Some(1_048_576),
        max_stderr_bytes: Some(1_048_576),
    };
    let mcp_resources = || ResourcePermissionCeiling {
        cpu_millis: 100,
        memory_bytes: 1_048_576,
        pids: 4,
        ephemeral_storage_bytes: 1_048_576,
        task_timeout_ms: None,
        max_stdout_bytes: None,
        max_stderr_bytes: None,
    };
    let ceiling = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_owned(),
        surfaces: vec![
            SurfacePermissionCeiling {
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Mcp,
                    id: "context".to_owned(),
                },
                native_execution: true,
                child_process: false,
                filesystem: Vec::new(),
                network_egress: Vec::new(),
                private_service: false,
                secrets: Vec::new(),
                resources: Some(mcp_resources()),
                ui_http: Vec::new(),
            },
            SurfacePermissionCeiling {
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Tool,
                    id: "echo".to_owned(),
                },
                native_execution: true,
                child_process: false,
                filesystem: Vec::new(),
                network_egress: Vec::new(),
                private_service: false,
                secrets: Vec::new(),
                resources: Some(task_resources()),
                ui_http: Vec::new(),
            },
        ],
    };
    ceiling.validate().unwrap();
    ceiling
}

fn fake_flow_compiler(root: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("a3s-flow-native-compiler");
        std::fs::write(
            &path,
            r#"#!/bin/sh
set -eu
[ "$1" = "compile" ]
shift
output=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    output="$1"
  fi
  shift
done
[ -n "$output" ]
printf '#!/bin/sh\nexit 0\n' > "$output"
chmod +x "$output"
"#,
        )
        .unwrap();
        let mut mode = std::fs::metadata(&path).unwrap().permissions();
        mode.set_mode(0o755);
        std::fs::set_permissions(&path, mode).unwrap();
        path
    }
    #[cfg(windows)]
    {
        let path = root.join("a3s-flow-native-compiler.cmd");
        std::fs::write(
            &path,
            "@echo off\r\n\
setlocal EnableExtensions\r\n\
if /I not \"%~1\"==\"compile\" exit /b 2\r\n\
shift\r\n\
set \"output=\"\r\n\
:parse\r\n\
if \"%~1\"==\"\" goto done\r\n\
if /I \"%~1\"==\"-o\" goto output\r\n\
shift\r\n\
goto parse\r\n\
:output\r\n\
shift\r\n\
set \"output=%~1\"\r\n\
shift\r\n\
goto parse\r\n\
:done\r\n\
if not defined output exit /b 3\r\n\
> \"%output%\" echo @echo off\r\n\
>> \"%output%\" echo exit /b 0\r\n\
exit /b 0\r\n",
        )
        .unwrap();
        path
    }
}

async fn assert_operation_completed(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    package_id: &PluginPackageId,
    planned: &a3s_use_core::PluginHostPlanResult,
    request_id: &str,
    result_digest: &str,
) {
    let observed = host
        .observe_operation(PluginHostOperationObservationRequest {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: request_id.to_owned(),
            assignment_generation: 1,
            capabilities_digest: capabilities_digest.to_owned(),
            scope: scope.clone(),
            package_id: package_id.clone(),
            operation_id: planned.plan.plan.operation_id.clone(),
            plan_digest: planned.plan.plan_digest.clone(),
        })
        .await
        .unwrap();
    assert_eq!(observed.status.phase, PluginHostOperationPhase::Completed);
    assert_eq!(
        observed.status.operation_result_digest.as_deref(),
        Some(result_digest)
    );
}
