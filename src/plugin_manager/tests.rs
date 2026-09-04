use std::sync::Arc;

use a3s_use_core::{
    PlanActor, PlanScope, PlanScopeKind, PluginManagedScope, PluginManagerOperationInput,
    PluginManagerOperationWatchInput, PluginOperationConfirmation, PLUGIN_MANAGED_SCOPE_SCHEMA_V2,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};
use a3s_use_extension::{ExtensionPaths, ExtensionRegistry};

use crate::cognitive_package::{
    CognitivePackageHostManager, StandaloneCognitivePackageAuthorizationProvider,
    StandaloneCognitivePackageLifecycleFactory,
};

use super::service::{decode_list_cursor, encode_list_cursor};
use super::PluginManagerService;

fn service() -> PluginManagerService {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.keep();
    let scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:plugin-manager-tests".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "workspace:plugin-manager-tests".to_owned(),
        authority_id: "user:plugin-manager-tests".to_owned(),
        fence_generation: 7,
        fence_digest: format!("sha256:{}", "7".repeat(64)),
    };
    let host = CognitivePackageHostManager::new(
        scope.clone(),
        "use:plugin-manager-tests",
        ExtensionRegistry::new(
            ExtensionPaths::new(home.join("data"), home.join("state"), scope.plan_scope()).unwrap(),
        ),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(StandaloneCognitivePackageAuthorizationProvider),
    )
    .unwrap();
    PluginManagerService::new(host, 7).unwrap()
}

#[test]
fn service_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PluginManagerService>();
}

#[test]
fn deterministic_request_identity_binds_the_exact_payload() {
    let service = service();
    let first = service.request_id("observe", &"acme/worker").unwrap();
    let replay = service.request_id("observe", &"acme/worker").unwrap();
    let different = service.request_id("observe", &"acme/other").unwrap();
    assert_eq!(first, replay);
    assert_ne!(first, different);
}

#[test]
fn manager_rejects_a_different_plan_scope() {
    let service = service();
    let error = service
        .verify_scope(&PlanScope {
            kind: PlanScopeKind::User,
            id: "user:current".to_owned(),
        })
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.manager_scope_mismatch");
}

#[test]
fn installed_list_cursor_rejects_a_stale_snapshot() {
    let first_digest = format!("sha256:{}", "a".repeat(64));
    let second_digest = format!("sha256:{}", "b".repeat(64));
    let cursor = encode_list_cursor(&first_digest, 2);
    assert_eq!(decode_list_cursor(&cursor, &first_digest, 3).unwrap(), 2);
    let error = decode_list_cursor(&cursor, &second_digest, 3).unwrap_err();
    assert_eq!(error.code, "use.plugin.manager_cursor_stale");
}

#[tokio::test]
async fn operation_controls_require_one_exact_durable_host_plan() {
    let service = service();
    let input = operation_input();

    let error = service.observe_operation(input.clone()).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_plan_missing");

    let error = service
        .watch_operation(PluginManagerOperationWatchInput {
            package_id: input.package_id.clone(),
            scope_kind: input.scope_kind,
            scope_id: input.scope_id.clone(),
            operation_id: input.operation_id.clone(),
            plan_digest: input.plan_digest.clone(),
            after_revision: None,
            timeout_ms: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.host_plan_missing");

    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
        operation_id: input.operation_id.clone(),
        plan_digest: input.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: 1,
    };
    let error = service
        .cancel_operation(input, Some(confirmation))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.host_plan_missing");
}

#[tokio::test]
async fn operation_watch_rejects_an_unbounded_timeout_before_host_access() {
    let service = service();
    let mut input = operation_input();
    input.scope_kind = PlanScopeKind::Workspace;
    let error = service
        .watch_operation(PluginManagerOperationWatchInput {
            package_id: input.package_id,
            scope_kind: input.scope_kind,
            scope_id: input.scope_id,
            operation_id: input.operation_id,
            plan_digest: input.plan_digest,
            after_revision: None,
            timeout_ms: a3s_use_core::MAX_PLUGIN_HOST_OPERATION_WATCH_TIMEOUT_MS + 1,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.manager_input_invalid");
}

fn operation_input() -> PluginManagerOperationInput {
    PluginManagerOperationInput {
        package_id: a3s_use_core::PluginPackageId::parse("acme/worker").unwrap(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "workspace:plugin-manager-tests".to_owned(),
        operation_id: "operation:install:01".to_owned(),
        plan_digest: format!("sha256:{}", "a".repeat(64)),
    }
}
