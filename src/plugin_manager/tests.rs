use std::sync::Arc;

use a3s_use_core::{PlanScope, PlanScopeKind, PluginManagedScope, PLUGIN_MANAGED_SCOPE_SCHEMA_V2};
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
