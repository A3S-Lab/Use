use std::sync::atomic::Ordering;
use std::sync::Arc;

use a3s_runtime::contract::{NetworkMode, RuntimeLogStream, RuntimeUnitClass};
use a3s_use_core::{PluginSurfaceKind, ToolWorkloadContract};

use super::test_support::*;
use super::*;

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
