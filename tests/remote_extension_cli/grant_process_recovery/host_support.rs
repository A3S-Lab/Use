use super::*;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use a3s_use::cognitive_package::{CognitivePackageHostManager, CognitiveRegistryAccess};
use a3s_use_core::{
    PlanScopeKind, PluginHostApplyRequest, PluginHostEnablementPlanRequest,
    PluginHostEnablementPlanStatus, PluginHostManager, PluginHostPlanRequest, PluginHostPlanResult,
    PluginManagedScope, PluginOperationAction, PluginOperationConfirmation, PluginPackageId,
    PluginPackageLock, PluginSurfaceKind, PluginSurfaceRef, PLUGIN_HOST_APPLY_REQUEST_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_PLAN_REQUEST_SCHEMA,
    PLUGIN_MANAGED_SCOPE_SCHEMA_V2, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};
use a3s_use_extension::{
    PluginCatalogSearch, RegistrySourceInput, RegistrySourceStore, VerifiedTargetCachePolicy,
};

const HOST_CHILD_HOME_ENV: &str = "A3S_USE_TEST_HOST_GRANT_PROCESS_HOME";
const HOST_CHILD_APPLY_REQUEST_ENV: &str = "A3S_USE_TEST_HOST_GRANT_PROCESS_APPLY_REQUEST";
const HOST_CHILD_AUTHORIZATION_MARKER_ENV: &str = "A3S_USE_TEST_HOST_GRANT_PROCESS_AUTH_MARKER";
const HOST_BUILD_ID: &str = "use:grant-process-host";
const HOST_ASSIGNMENT_GENERATION: u64 = 19;

pub(super) async fn configure_host_registry(
    home: &Path,
    server: &TestServer,
    repository: &TestRepository,
) {
    RegistrySourceStore::new(ExtensionPaths::new(home.join("data"), home.join("state")))
        .add(RegistrySourceInput::new(
            "fixture",
            server.base_url(),
            &repository.root_sha256,
            None,
            VerifiedTargetCachePolicy::default(),
        ))
        .await
        .unwrap();
}

pub(super) async fn plan_host_release_apply(
    host: &CognitivePackageHostManager,
    action: PluginOperationAction,
    version: &str,
    plan_request_id: &str,
    apply_request_id: &str,
) -> (PluginHostApplyRequest, PluginPackageLock) {
    assert!(matches!(
        action,
        PluginOperationAction::Install | PluginOperationAction::Upgrade
    ));
    let candidate = host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            Some("fixture"),
            &PluginCatalogSearch {
                query: "worker".to_owned(),
                kind: Some(PluginSurfaceKind::Tool),
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
        .unwrap_or_else(|| panic!("Registry search omitted acme/worker {version}"));
    let lock = host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
        .await
        .unwrap();
    let capabilities = host.capabilities().await.unwrap();
    let request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: plan_request_id.to_owned(),
        assignment_generation: HOST_ASSIGNMENT_GENERATION,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: managed_host_scope(),
        action,
        package_id: PluginPackageId::parse(PACKAGE_ID).unwrap(),
        candidate: Some(candidate),
        package_lock: Some(lock.clone()),
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        }],
    };
    let planned = host.plan(request.clone()).await.unwrap();
    (
        host_apply_request(&request, &planned, apply_request_id),
        lock,
    )
}

pub(super) async fn plan_host_uninstall_apply(
    host: &CognitivePackageHostManager,
    lock: &PluginPackageLock,
    plan_request_id: &str,
    apply_request_id: &str,
) -> PluginHostApplyRequest {
    let capabilities = host.capabilities().await.unwrap();
    let request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: plan_request_id.to_owned(),
        assignment_generation: HOST_ASSIGNMENT_GENERATION,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: managed_host_scope(),
        action: PluginOperationAction::Uninstall,
        package_id: PluginPackageId::parse(PACKAGE_ID).unwrap(),
        candidate: None,
        package_lock: Some(lock.clone()),
        selected_surfaces: Vec::new(),
    };
    let planned = host.plan(request.clone()).await.unwrap();
    host_apply_request(&request, &planned, apply_request_id)
}

pub(super) async fn plan_host_enablement_apply(
    host: &CognitivePackageHostManager,
    expected_package_generation: u64,
    enabled: bool,
    plan_request_id: &str,
    apply_request_id: &str,
) -> PluginHostApplyRequest {
    let capabilities = host.capabilities().await.unwrap();
    let request = PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: plan_request_id.to_owned(),
        assignment_generation: HOST_ASSIGNMENT_GENERATION,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: managed_host_scope(),
        package_id: PluginPackageId::parse(PACKAGE_ID).unwrap(),
        expected_package_generation,
        enabled,
    };
    let planned = host.plan_enablement(request.clone()).await.unwrap();
    assert_eq!(planned.status, PluginHostEnablementPlanStatus::Planned);
    let envelope = planned.plan.as_ref().unwrap();
    PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: apply_request_id.to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest,
        scope: request.scope,
        package_id: request.package_id,
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: envelope.plan.created_at_ms + 1,
        }),
    }
}

fn host_apply_request(
    request: &PluginHostPlanRequest,
    planned: &PluginHostPlanResult,
    request_id: &str,
) -> PluginHostApplyRequest {
    PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: request_id.to_owned(),
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
    }
}

#[tokio::test]
#[ignore = "subprocess helper for Host protocol Graph/Grant interruption"]
async fn managed_host_graph_apply_child() {
    let Some(home) = std::env::var_os(HOST_CHILD_HOME_ENV).map(PathBuf::from) else {
        return;
    };
    let apply_request_path = PathBuf::from(std::env::var_os(HOST_CHILD_APPLY_REQUEST_ENV).unwrap());
    let authorization_marker =
        PathBuf::from(std::env::var_os(HOST_CHILD_AUTHORIZATION_MARKER_ENV).unwrap());
    let request =
        PluginHostApplyRequest::from_json(&tokio::fs::read(apply_request_path).await.unwrap())
            .unwrap();
    host_manager(&home, &authorization_marker)
        .apply(request)
        .await
        .unwrap();
}

pub(super) fn managed_host_scope() -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:grant-process".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_owned(),
        authority_id: "test:user".to_owned(),
        fence_generation: HOST_ASSIGNMENT_GENERATION,
        fence_digest: format!("sha256:{}", "c".repeat(64)),
    }
}

pub(super) fn host_manager(
    home: &Path,
    authorization_marker: &Path,
) -> CognitivePackageHostManager {
    CognitivePackageHostManager::new(
        managed_host_scope(),
        HOST_BUILD_ID,
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state"))),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ProcessAuthorization {
            marker: authorization_marker.to_owned(),
            allow_authorization: false,
        }),
    )
    .unwrap()
}

pub(super) fn spawn_host_apply_child(
    home: &Path,
    apply_request_path: &Path,
    authorization_marker: &Path,
) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("managed_host_graph_apply_child")
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(HOST_CHILD_HOME_ENV, home)
        .env(HOST_CHILD_APPLY_REQUEST_ENV, apply_request_path)
        .env(HOST_CHILD_AUTHORIZATION_MARKER_ENV, authorization_marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
