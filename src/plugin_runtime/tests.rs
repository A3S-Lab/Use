use std::sync::atomic::Ordering;
use std::sync::Arc;

use a3s_runtime::contract::{NetworkMode, RuntimeLogStream, RuntimeUnitClass};
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{
    CatalogSurface, ExecutablePlanningSurface, NetworkEgressPermission, PlanActor,
    PlanPolicyDecision, PlannedPackageState, PlannedPluginRelease, PlanningArtifactRef,
    PlanningSurfaceActivation, PluginPermissionCeiling, PluginPlanningBundle, PluginReleaseChannel,
    PluginSurfaceKind, PluginSurfaceRef, PluginWorkspaceGrantProposal, ResourcePermissionCeiling,
    SurfacePermissionCeiling, ToolWorkloadClass, ToolWorkloadContract,
    WorkspaceGrantProposalAuthority, PLUGIN_PERMISSION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA,
};
use async_trait::async_trait;

use super::test_support::*;
use super::*;

struct StaticRuntimeFactory {
    provider_id: ProviderId,
    client: Arc<dyn RuntimeClient>,
}

#[async_trait]
impl RuntimeProviderFactory for StaticRuntimeFactory {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
        Ok(self.client.clone())
    }
}

#[test]
fn tool_task_plan_binds_invocation_and_release_semantics() {
    let descriptor = task_descriptor();
    let resolved = artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type);
    let invocation =
        RuntimeTaskInvocation::new("invoke-01", vec!["--format".into(), "json".into()]).unwrap();
    let first = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        resolved.clone(),
        invocation,
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let second = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        resolved,
        RuntimeTaskInvocation::new("invoke-02", vec!["--format".into(), "json".into()]).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();

    assert_eq!(first.spec().class, RuntimeUnitClass::Task);
    assert_eq!(
        first.spec().process.command,
        vec!["/usr/local/bin/example-tool"]
    );
    assert_eq!(first.spec().process.args, vec!["--format", "json"]);
    assert_eq!(first.spec().resources.execution_timeout_ms, Some(120_000));
    assert_eq!(first.spec().network.mode, NetworkMode::None);
    assert!(matches!(
        first.contract(),
        RuntimeSurfaceContract::ToolTask {
            command_name,
            json_output: true,
            ..
        } if command_name == "acme-convert"
    ));
    assert_ne!(first.spec().unit_id, second.spec().unit_id);
    assert_eq!(
        first.spec().semantics_profile_digest,
        second.spec().semantics_profile_digest
    );
    assert!(first
        .spec()
        .semantics_profile_digest
        .as_deref()
        .unwrap()
        .starts_with("sha256:"));
    assert!(first.spec().validate().is_ok());
}

#[test]
fn task_plan_rejects_unrepresentable_exit_code_semantics() {
    let mut descriptor = task_descriptor();
    let ToolWorkloadContract::Task {
        success_exit_codes, ..
    } = &mut descriptor.workload
    else {
        panic!("fixture should be a Task");
    };
    *success_exit_codes = vec![0, 2];
    let error = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.task_semantics_unsupported");
}

#[test]
fn service_plans_preserve_native_http_and_mcp_contracts() {
    let tool = service_descriptor();
    let tool_plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &tool,
        artifact(&tool.artifact.digest, &tool.artifact.media_type),
        policy(),
    )
    .unwrap();
    assert_eq!(tool_plan.spec().class, RuntimeUnitClass::Service);
    assert_eq!(tool_plan.spec().network.mode, NetworkMode::Service);
    assert_eq!(tool_plan.spec().network.ports[0].container_port, 8080);
    assert!(tool_plan.spec().process.command.is_empty());
    assert!(matches!(
        tool_plan.contract(),
        RuntimeSurfaceContract::ToolService { base_path, .. } if base_path == "/api"
    ));

    let mcp = mcp_descriptor();
    let mcp_plan = plan_mcp_service_release(
        context(PluginSurfaceKind::Mcp, "library"),
        &mcp_surface(),
        &mcp,
        artifact(&mcp.artifact.digest, &mcp.artifact.media_type),
        policy(),
    )
    .unwrap();
    assert_eq!(mcp_plan.spec().network.ports[0].container_port, 8080);
    assert!(matches!(
        mcp_plan.contract(),
        RuntimeSurfaceContract::McpService {
            endpoint_path,
            protocol_version,
            ..
        } if endpoint_path == "/mcp" && protocol_version == "2025-06-18"
    ));
}

#[test]
fn release_plan_rejects_artifact_substitution() {
    let descriptor = service_descriptor();
    let error = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(DIGEST_A, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.artifact_mismatch");
}

#[tokio::test]
async fn explicit_provider_evidence_is_rechecked_without_fallback() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities.clone(), true));
    let client = PluginRuntimeClient::new(runtime);
    let binding = client.prepare_task(&plan, &provider).await.unwrap();
    assert_eq!(binding.provider_id, "test-runtime");
    assert_eq!(binding.artifact_digest, plan.spec().artifact.digest);

    let mut changed = capabilities;
    changed.provider_build = "build-2".to_string();
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(changed, true)));
    let error = client.prepare_task(&plan, &provider).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");
}

#[tokio::test]
async fn explicit_provider_assignments_resolve_sorted_evidence_without_fallback() {
    let task_descriptor = task_descriptor();
    let task = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &task_descriptor,
        artifact(
            &task_descriptor.artifact.digest,
            &task_descriptor.artifact.media_type,
        ),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let service_descriptor = service_descriptor();
    let service = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &service_descriptor,
        artifact(
            &service_descriptor.artifact.digest,
            &service_descriptor.artifact.media_type,
        ),
        policy(),
    )
    .unwrap();
    let mut runtime_capabilities = capabilities(&task);
    if !runtime_capabilities
        .artifact_media_types
        .contains(&service.spec().artifact.media_type)
    {
        runtime_capabilities
            .artifact_media_types
            .push(service.spec().artifact.media_type.clone());
    }
    let runtime = Arc::new(FakeRuntime::new(runtime_capabilities, true));
    let provider_id = ProviderId::parse("test-runtime").unwrap();
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id,
            client: runtime,
        }))
        .unwrap();
    let selector = RuntimeProviderSelector::new(&registry);

    let selected = selector
        .select(
            vec![service.clone(), task.clone()],
            vec![
                RuntimeProviderAssignment::new(service.surface(), "test-runtime").unwrap(),
                RuntimeProviderAssignment::new(task.surface(), "test-runtime").unwrap(),
            ],
        )
        .await
        .unwrap();

    let evidence = selected.provider_evidence();
    assert_eq!(evidence.len(), 2);
    assert_eq!(evidence[0].surface.surface.id, "convert");
    assert_eq!(evidence[1].surface.surface.id, "index");
    assert!(evidence
        .iter()
        .all(|provider| provider.provider_id == "test-runtime"));
    selected.surfaces()[0]
        .client()
        .verify_plan(
            selected.surfaces()[0].plan(),
            selected.surfaces()[0].provider(),
        )
        .await
        .unwrap();

    let missing = selector
        .select(
            vec![task.clone()],
            vec![RuntimeProviderAssignment::new(task.surface(), "missing-runtime").unwrap()],
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code, "use.plugin.runtime.provider_unavailable");

    let incomplete = selector
        .select(vec![task.clone()], Vec::new())
        .await
        .unwrap_err();
    assert_eq!(
        incomplete.code,
        "use.plugin.runtime.provider_assignment_invalid"
    );

    let duplicate = selector
        .select(
            vec![task.clone()],
            vec![
                RuntimeProviderAssignment::new(task.surface(), "test-runtime").unwrap(),
                RuntimeProviderAssignment::new(task.surface(), "test-runtime").unwrap(),
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(
        duplicate.code,
        "use.plugin.runtime.provider_assignment_invalid"
    );
}

#[tokio::test]
async fn task_binding_invokes_native_argv_and_captures_separate_output_streams() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", vec!["--input".into(), "paper.pdf".into()])
            .unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true).with_logs(vec![
        log_chunk(RuntimeLogStream::Stdout, 1, "stdout-1", "{\"ok\":true}\n"),
        log_chunk(RuntimeLogStream::Stderr, 1, "stderr-1", "diagnostic\n"),
    ]));
    let client = PluginRuntimeClient::new(runtime.clone());
    let binding = client.prepare_task(&plan, &provider).await.unwrap();
    let result = client
        .invoke_task(&plan, &binding, "invoke-request-01", Some(9_999_999))
        .await
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "{\"ok\":true}\n");
    assert_eq!(result.stderr, "diagnostic\n");
    assert!(!result.truncated);
    assert_eq!(
        plan.spec().process.args,
        vec!["--input".to_string(), "paper.pdf".to_string()]
    );
}

#[tokio::test]
async fn unsupported_in_memory_capture_is_rejected_before_task_apply() {
    let mut descriptor = task_descriptor();
    let ToolWorkloadContract::Task {
        max_stdout_bytes, ..
    } = &mut descriptor.workload
    else {
        panic!("fixture should be a Task");
    };
    *max_stdout_bytes = 16 * 1024 * 1024 + 1;
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let client = PluginRuntimeClient::new(runtime.clone());

    let error = client.prepare_task(&plan, &provider).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.capture_unsupported");
    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ambiguous_task_apply_failure_attempts_exact_cleanup() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true).with_apply_failure());
    let client = PluginRuntimeClient::new(runtime.clone());
    let binding = client.prepare_task(&plan, &provider).await.unwrap();

    let error = client
        .invoke_task(&plan, &binding, "invoke-01", Some(9_999_999))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.operation_failed");
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn healthy_service_activation_requires_an_opaque_endpoint_binding() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let client = PluginRuntimeClient::new(runtime.clone());
    let activation = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap();
    let receipt = activation
        .into_tool_service_receipt(RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap())
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(receipt.schema, RUNTIME_SERVICE_BINDING_SCHEMA);
    assert_eq!(receipt.endpoint_ref.as_str(), "gateway:workspace-01/index");
    assert_eq!(receipt.provider_build_id, "build-1");
    assert!(RuntimeEndpointRef::parse("https://user:token@example.com").is_err());
    assert!(!serde_json::to_string(&receipt).unwrap().contains("token"));
}

#[tokio::test]
async fn service_binding_is_not_published_before_runtime_convergence() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(capabilities, false)));

    let error = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.not_converged");
}

#[tokio::test]
async fn mcp_service_binding_requires_matching_initialize_evidence() {
    let descriptor = mcp_descriptor();
    let plan = plan_mcp_service_release(
        context(PluginSurfaceKind::Mcp, "library"),
        &mcp_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(capabilities, true)));
    let activation = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap();
    let endpoint = RuntimeEndpointRef::parse("gateway:workspace-01/library").unwrap();

    assert!(activation
        .clone()
        .into_tool_service_receipt(endpoint.clone())
        .is_err());
    let wrong_protocol = RuntimeMcpInitializeEvidence::new("2024-11-05", 1_001).unwrap();
    assert!(activation
        .clone()
        .into_mcp_service_receipt(endpoint.clone(), wrong_protocol)
        .is_err());
    let initialize = RuntimeMcpInitializeEvidence::new("2025-06-18", 1_001).unwrap();
    let receipt = activation
        .into_mcp_service_receipt(endpoint, initialize)
        .unwrap();
    assert!(matches!(
        receipt.readiness,
        RuntimeServiceReadinessEvidence::McpInitialized { .. }
    ));
}

#[tokio::test]
async fn service_binding_is_live_observed_then_drained_and_removed_exactly() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let client = PluginRuntimeClient::new(runtime.clone());
    let receipt = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap()
        .into_tool_service_receipt(RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap())
        .unwrap();
    let binding = RuntimeBindingReceipt::Service(receipt.clone());

    let observed = client.observe_binding(&binding).await.unwrap();
    assert_eq!(observed.state, RuntimeBindingObservedState::Healthy);
    assert!(observed.observation.is_some());

    runtime.set_service_health_revision(1_200, 1_100);
    let removal = client
        .drain_remove_service(
            &receipt,
            "operation-01-stop",
            "operation-01-remove",
            Some(9_999_999),
        )
        .await
        .unwrap();
    assert!(!removal.already_absent);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);

    let missing = client.observe_binding(&binding).await.unwrap();
    assert_eq!(missing.state, RuntimeBindingObservedState::Missing);
    assert_eq!(missing.last_generation, Some(7));
}

#[tokio::test]
async fn service_restart_makes_the_old_endpoint_binding_stale() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let client = PluginRuntimeClient::new(runtime.clone());
    let receipt = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap()
        .into_tool_service_receipt(RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap())
        .unwrap();
    runtime.restart_service(1_050, 1_100);

    let observed = client
        .observe_binding(&RuntimeBindingReceipt::Service(receipt))
        .await
        .unwrap();
    assert_eq!(observed.state, RuntimeBindingObservedState::Stale);
}

#[tokio::test]
async fn service_health_revision_cannot_regress_or_exceed_its_observation() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let client = PluginRuntimeClient::new(runtime.clone());
    let receipt = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap()
        .into_tool_service_receipt(RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap())
        .unwrap();

    runtime.set_service_health_revision(999, 1_100);
    let regressed = client
        .observe_binding(&RuntimeBindingReceipt::Service(receipt.clone()))
        .await
        .unwrap();
    assert_eq!(regressed.state, RuntimeBindingObservedState::Stale);

    runtime.set_service_health_revision(1_200, 1_100);
    let invalid = client
        .observe_binding(&RuntimeBindingReceipt::Service(receipt))
        .await
        .unwrap_err();
    assert_eq!(invalid.code, "use.plugin.runtime.contract_invalid");

    let mut invalid_activation = client
        .apply_service(&plan, &provider, "operation-02", Some(9_999_999))
        .await
        .unwrap();
    invalid_activation
        .observation
        .health
        .as_mut()
        .unwrap()
        .checked_at_ms = 1_200;
    let invalid_receipt = invalid_activation
        .into_tool_service_receipt(RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap())
        .unwrap_err();
    assert_eq!(invalid_receipt.code, "use.plugin.runtime.input_invalid");
}

#[tokio::test]
async fn planning_bundle_selects_only_the_explicit_capable_provider() {
    let (bundle, package, proposal) = runtime_bundle_inputs(false);
    let plans = plan_runtime_bundle(&bundle, &package, &proposal, 8).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(
        plans[0].context().grant_digest(),
        proposal.descriptor_digest().unwrap()
    );
    assert_eq!(
        plans[0].spec().resources.ephemeral_storage_bytes,
        Some(512 * 1024 * 1024)
    );

    let runtime_capabilities = capabilities(&plans[0]);
    let runtime = Arc::new(FakeRuntime::new(runtime_capabilities, true));
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: runtime,
        }))
        .unwrap();
    let selection = RuntimeProviderSelector::new(&registry)
        .select(
            plans.clone(),
            vec![RuntimeProviderAssignment::new(plans[0].surface(), "test-runtime").unwrap()],
        )
        .await
        .unwrap();

    assert_eq!(selection.surfaces().len(), 1);
    assert_eq!(selection.provider_evidence()[0].provider_id, "test-runtime");
    assert_eq!(
        selection.provider_evidence()[0].semantics_profile_digest,
        plans[0].spec().semantics_profile_digest.clone().unwrap()
    );
}

#[test]
fn planning_bundle_fails_closed_on_unrepresentable_egress_authority() {
    let (bundle, package, proposal) = runtime_bundle_inputs(true);
    let error = plan_runtime_bundle(&bundle, &package, &proposal, 8).unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.authorization_unsupported");
}

fn runtime_bundle_inputs(
    with_egress: bool,
) -> (
    PluginPlanningBundle,
    PlannedPackageState,
    PluginWorkspaceGrantProposal,
) {
    let descriptor = service_descriptor();
    let mut permission = SurfacePermissionCeiling {
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "index".to_owned(),
        },
        native_execution: false,
        child_process: false,
        filesystem: Vec::new(),
        network_egress: Vec::new(),
        private_service: true,
        secrets: Vec::new(),
        resources: Some(ResourcePermissionCeiling {
            cpu_millis: 500,
            memory_bytes: 256 * 1024 * 1024,
            pids: 64,
            ephemeral_storage_bytes: 512 * 1024 * 1024,
            task_timeout_ms: None,
            max_stdout_bytes: None,
            max_stderr_bytes: None,
        }),
        ui_http: Vec::new(),
    };
    if with_egress {
        permission.network_egress.push(NetworkEgressPermission {
            host: "api.example.com".to_owned(),
            ports: vec![443],
        });
    }
    let permissions = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_owned(),
        surfaces: vec![permission],
    };
    let permission_digest = permissions.descriptor_digest().unwrap();
    let catalog_surface = CatalogSurface {
        kind: PluginSurfaceKind::Tool,
        id: "index".to_owned(),
        optional: false,
        workload: Some(ToolWorkloadClass::Service),
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    };
    let package = PlannedPackageState {
        release: PlannedPluginRelease {
            package_id: "acme/research".to_owned(),
            version: "2.0.0".to_owned(),
            channel: PluginReleaseChannel::Stable,
            target: "linux-x86_64".to_owned(),
            package_sha256: DIGEST_A.to_owned(),
            manifest_sha256:
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
            permission_ceiling_digest: permission_digest.clone(),
            surfaces: vec![catalog_surface],
        },
        permissions: permissions.clone(),
    };
    let bundle = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: package.release.package_id.clone(),
        version: package.release.version.clone(),
        channel: package.release.channel,
        target: package.release.target.clone(),
        archive_sha256: "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
            .to_owned(),
        package_sha256: package.release.package_sha256.clone(),
        manifest_sha256: package.release.manifest_sha256.clone(),
        permission_ceiling_digest: permission_digest.clone(),
        surfaces: vec![ExecutablePlanningSurface::ToolService {
            id: "index".to_owned(),
            activation: PlanningSurfaceActivation::Eager,
            base_path: "/api".to_owned(),
            artifact: PlanningArtifactRef {
                uri: format!(
                    "oci://registry.example/acme/research-index@{}",
                    descriptor.artifact.digest
                ),
                digest: descriptor.artifact.digest.clone(),
                media_type: descriptor.artifact.media_type.clone(),
            },
            descriptor,
        }],
    };
    let proposal = PluginWorkspaceGrantProposal {
        schema: PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA.to_owned(),
        operation_id: "operation-01".to_owned(),
        scope_id: "workspace-01".to_owned(),
        package_id: package.release.package_id.clone(),
        package_digest: package.release.package_sha256.clone(),
        permission_ceiling_digest: permission_digest,
        permissions_digest: permissions.descriptor_digest().unwrap(),
        permissions,
        authority: WorkspaceGrantProposalAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Allow,
            policy_digest:
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned(),
        },
        created_at_ms: 1_000,
        apply_expires_at_ms: 2_000,
        grant_expires_at_ms: None,
    };
    (bundle, package, proposal)
}
