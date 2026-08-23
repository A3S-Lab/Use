use super::*;

use a3s_use::plugin_manager::PluginManagerService;
use a3s_use_core::{
    PluginHostEnablementPlanStatus, PluginManagerApplyPlanInput, PluginManagerInstallPlanInput,
    PluginManagerListInstalledInput, PluginManagerPackageScopeInput,
};

const MANAGER_ASSIGNMENT_GENERATION: u64 = 23;

#[tokio::test]
async fn shared_plugin_manager_replays_plans_and_preserves_stable_installed_pages() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-manager",
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        temporary.path(),
        "acme/helper",
        "helper-manager",
        "1.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 83, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("manager-home");
    let paths = ExtensionPaths::new(home.join("data"), home.join("state"));
    RegistrySourceStore::new(paths.clone())
        .add(RegistrySourceInput::new(
            "fixture",
            server.base_url(),
            &repository.root_sha256,
            None,
            VerifiedTargetCachePolicy::default(),
        ))
        .await
        .unwrap();
    let scope = managed_scope();
    let service = PluginManagerService::new(
        CognitivePackageHostManager::new(
            scope.clone(),
            "use:plugin-manager-integration",
            ExtensionRegistry::new(paths),
            Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
            Arc::new(ConfirmAllPlans {
                authorization_count: Arc::new(AtomicUsize::new(0)),
            }),
        )
        .unwrap(),
        MANAGER_ASSIGNMENT_GENERATION,
    )
    .unwrap();

    let worker_input = install_input("acme/worker");
    let worker_plan = service
        .plan_install(worker_input.clone(), CognitiveRegistryAccess::Refreshed)
        .await
        .unwrap();
    assert!(!worker_plan.replayed);
    let replayed = service
        .plan_install(worker_input, CognitiveRegistryAccess::Refreshed)
        .await
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.request_id, worker_plan.request_id);
    assert_eq!(replayed.plan, worker_plan.plan);

    let worker_apply = apply_input(&worker_plan.plan);
    assert_eq!(
        service.reviewed_plan(&worker_apply).await.unwrap().plan,
        worker_plan.plan
    );
    let mismatched = PluginManagerApplyPlanInput {
        operation_id: worker_apply.operation_id.clone(),
        plan_digest: format!("sha256:{}", "f".repeat(64)),
    };
    assert_eq!(
        service.reviewed_plan(&mismatched).await.unwrap_err().code,
        "use.plugin.host_plan_mismatch"
    );
    assert_eq!(
        service
            .apply_plan(worker_apply.clone(), None)
            .await
            .unwrap_err()
            .code,
        "use.plugin.plan_confirmation_mismatch"
    );
    service
        .apply_plan(worker_apply, Some(confirmation(&worker_plan.plan)))
        .await
        .unwrap();

    let helper_plan = service
        .plan_install(
            install_input("acme/helper"),
            CognitiveRegistryAccess::Refreshed,
        )
        .await
        .unwrap();
    service
        .apply_plan(
            apply_input(&helper_plan.plan),
            Some(confirmation(&helper_plan.plan)),
        )
        .await
        .unwrap();

    let first_page = service.list_installed(list_input(None, 1)).await.unwrap();
    assert_eq!(first_page.packages.len(), 1);
    let stale_cursor = first_page.next_cursor.unwrap();
    let installed = service.list_installed(list_input(None, 100)).await.unwrap();
    assert_eq!(
        installed
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/helper", "acme/worker"]
    );
    let first_state = &installed.packages[0].state;
    assert!(installed.packages.iter().all(|package| {
        package.state.capability_generation == first_state.capability_generation
            && package.state.capability_revision == first_state.capability_revision
    }));

    let scoped = package_scope("acme/worker");
    let disable = service.plan_disable(scoped.clone()).await.unwrap();
    assert_eq!(disable.status, PluginHostEnablementPlanStatus::Planned);
    let disable_plan = disable.plan.as_ref().unwrap();
    service
        .apply_plan(apply_input(disable_plan), Some(confirmation(disable_plan)))
        .await
        .unwrap();
    assert_eq!(
        service
            .list_installed(list_input(Some(stale_cursor), 1))
            .await
            .unwrap_err()
            .code,
        "use.plugin.manager_cursor_stale"
    );
    let no_change = service.plan_disable(scoped).await.unwrap();
    assert_eq!(no_change.status, PluginHostEnablementPlanStatus::NoChange);
    assert!(no_change.plan.is_none());
}

fn managed_scope() -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:plugin-manager-integration".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_owned(),
        authority_id: "user:plugin-manager-integration".to_owned(),
        fence_generation: MANAGER_ASSIGNMENT_GENERATION,
        fence_digest: format!("sha256:{}", "8".repeat(64)),
    }
}

fn install_input(package_id: &str) -> PluginManagerInstallPlanInput {
    PluginManagerInstallPlanInput {
        package_id: PluginPackageId::parse(package_id).unwrap(),
        registry_name: Some("fixture".to_owned()),
        version_requirement: Some("1.0.0".to_owned()),
        channel: Some(PluginReleaseChannel::Stable),
        surfaces: None,
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_owned(),
    }
}

fn package_scope(package_id: &str) -> PluginManagerPackageScopeInput {
    PluginManagerPackageScopeInput {
        package_id: PluginPackageId::parse(package_id).unwrap(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_owned(),
    }
}

fn list_input(cursor: Option<String>, limit: u16) -> PluginManagerListInstalledInput {
    PluginManagerListInstalledInput {
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_owned(),
        cursor,
        limit: Some(limit),
    }
}

fn apply_input(plan: &PluginOperationPlanEnvelope) -> PluginManagerApplyPlanInput {
    PluginManagerApplyPlanInput {
        operation_id: plan.plan.operation_id.clone(),
        plan_digest: plan.plan_digest.clone(),
    }
}

fn confirmation(plan: &PluginOperationPlanEnvelope) -> PluginOperationConfirmation {
    PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
        operation_id: plan.plan.operation_id.clone(),
        plan_digest: plan.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: plan.plan.created_at_ms,
    }
}
