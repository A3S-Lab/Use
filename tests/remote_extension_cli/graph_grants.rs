use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_use::cognitive_package::{
    CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider,
    CognitivePackageEnablementPreparation, CognitivePackageEnablementRequest,
    CognitivePackageHostManager, ReviewedCognitivePackageAuthorizationProvider,
    StandaloneCognitivePackageLifecycleFactory,
};
use a3s_use_core::{
    CatalogPlanningTarget, ExecutablePlanningSurface, PlanActor, PlanAuthority, PlanPolicyDecision,
    PlanScope, PlanScopeKind, PlanningSurfaceActivation, PluginDesiredState,
    PluginHostApplyRequest, PluginHostEnablementPlanRequest, PluginHostEnablementPlanStatus,
    PluginHostManager, PluginHostObservationRequest, PluginHostObservationStatus,
    PluginHostPlanRequest, PluginManagedScope, PluginOperationAction, PluginOperationConfirmation,
    PluginOperationPlan, PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPermissionCeiling, PluginPlanSource, PluginPlanningBundle, PluginSurfaceRef,
    PluginWorkspaceGrantChangeSet, ToolWorkloadClass, UseResult, PLUGIN_HOST_APPLY_REQUEST_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_PLAN_REQUEST_SCHEMA, PLUGIN_MANAGED_SCOPE_SCHEMA,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use a3s_use_extension::{
    RegistrySourceInput, RegistrySourceStore, StoredWorkspaceGrant, VerifiedTargetCachePolicy,
    WorkspaceGrantLifecyclePhase, WorkspaceGrantStore,
};
use async_trait::async_trait;

const POLICY_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MANAGED_SCOPE_ID: &str = "workspace:research";
const PERMISSIONS: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/permission-ceiling-v1.json");

#[derive(Debug)]
struct ConfirmAllPlans {
    authorization_count: Arc<AtomicUsize>,
}

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

#[tokio::test]
async fn production_host_manager_persists_plan_apply_replay_and_exact_fence() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        cognitive_tool_targets_version(
            temporary.path(),
            "acme/worker",
            "worker-host",
            "1.0.0",
            &target,
        ),
        61,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("host-home");
    let paths = ExtensionPaths::new(home.join("data"), home.join("state"));
    let sources = RegistrySourceStore::new(paths.clone());
    sources
        .add(RegistrySourceInput::new(
            "fixture",
            server.base_url(),
            &repository.root_sha256,
            None,
            VerifiedTargetCachePolicy::default(),
        ))
        .await
        .unwrap();
    let resolved = sources.resolve(Some("fixture")).await.unwrap();
    let lock = resolve_remote_package_lock(
        resolved.root(),
        resolved.dependencies(),
        "acme/worker",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let candidate = lock.package("acme/worker").unwrap().catalog.clone();
    let managed_scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA.to_string(),
        host_id: "host:node-01".to_string(),
        scope_id: MANAGED_SCOPE_ID.to_string(),
        authority_id: "cloud:control-plane".to_string(),
        fence_generation: 7,
        fence_digest: format!("sha256:{}", "f".repeat(64)),
    };
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let host = CognitivePackageHostManager::new(
        managed_scope.clone(),
        "use:test-build",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let selected_surfaces = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_string(),
    }];
    let plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_string(),
        request_id: "plan:worker:0001".to_string(),
        assignment_generation: 3,
        capabilities_digest: capabilities_digest.clone(),
        scope: managed_scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/worker".to_string()).unwrap(),
        candidate: Some(candidate),
        package_lock: Some(lock),
        selected_surfaces: selected_surfaces.clone(),
    };

    let planned = host.plan(plan_request.clone()).await.unwrap();
    assert!(!planned.replayed);
    assert_eq!(
        planned.plan.plan.packages[0]
            .after
            .as_ref()
            .unwrap()
            .release
            .surfaces
            .iter()
            .map(a3s_use_core::CatalogSurface::reference)
            .collect::<Vec<_>>(),
        selected_surfaces
    );
    let replayed_plan = host.plan(plan_request.clone()).await.unwrap();
    assert!(replayed_plan.replayed);
    assert_eq!(replayed_plan.plan, planned.plan);
    let mut conflicting_plan_request = plan_request.clone();
    conflicting_plan_request.assignment_generation += 1;
    let error = host.plan(conflicting_plan_request).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_store_conflict");

    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: planned.plan.plan.created_at_ms + 1,
    };
    let apply_request = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_string(),
        request_id: "apply:worker:0001".to_string(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: managed_scope.clone(),
        package_id: plan_request.package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmation: Some(confirmation),
    };
    let mut unconfirmed_apply = apply_request.clone();
    unconfirmed_apply.request_id = "apply:worker:unconfirmed".to_string();
    unconfirmed_apply.confirmation = None;
    let error = host.apply(unconfirmed_apply).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.plan_confirmation_mismatch");
    let applied = host.apply(apply_request.clone()).await.unwrap();
    assert!(!applied.replayed);
    assert_eq!(applied.state.selected_surfaces, selected_surfaces);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);

    let restarted = CognitivePackageHostManager::new(
        managed_scope.clone(),
        "use:test-build",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let replayed_apply = restarted.apply(apply_request.clone()).await.unwrap();
    assert!(replayed_apply.replayed);
    assert_eq!(
        replayed_apply.operation_result_digest,
        applied.operation_result_digest
    );

    let observe_request = PluginHostObservationRequest {
        schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_string(),
        request_id: "observe:worker:0001".to_string(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest,
        scope: managed_scope,
        package_id: plan_request.package_id,
    };
    let observed = restarted.observe(observe_request.clone()).await.unwrap();
    let PluginHostObservationStatus::Available { state } = observed.status else {
        panic!("the installed Host package must be observable");
    };
    assert_eq!(state.selected_surfaces, selected_surfaces);
    assert_eq!(state.desired, PluginDesiredState::Enabled);

    let disable_request = PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_string(),
        request_id: "plan:worker:disable:0001".to_string(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: observe_request.capabilities_digest.clone(),
        scope: observe_request.scope.clone(),
        package_id: observe_request.package_id.clone(),
        expected_package_generation: state.package_generation.unwrap(),
        enabled: false,
    };
    let planned_disable = restarted
        .plan_enablement(disable_request.clone())
        .await
        .unwrap();
    assert_eq!(
        planned_disable.status,
        PluginHostEnablementPlanStatus::Planned
    );
    assert_eq!(planned_disable.state, state);
    assert!(!planned_disable.replayed);
    let replayed_disable_plan = restarted
        .plan_enablement(disable_request.clone())
        .await
        .unwrap();
    assert!(replayed_disable_plan.replayed);
    assert_eq!(replayed_disable_plan.plan, planned_disable.plan);

    let disable_plan = planned_disable.plan.as_ref().unwrap();
    let disable_apply_request = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_string(),
        request_id: "apply:worker:disable:0001".to_string(),
        assignment_generation: disable_request.assignment_generation,
        capabilities_digest: disable_request.capabilities_digest.clone(),
        scope: disable_request.scope.clone(),
        package_id: disable_request.package_id.clone(),
        operation_id: disable_plan.plan.operation_id.clone(),
        plan_digest: disable_plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: disable_plan.plan.operation_id.clone(),
            plan_digest: disable_plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: disable_plan.plan.created_at_ms + 1,
        }),
    };
    let disabled = restarted
        .apply(disable_apply_request.clone())
        .await
        .unwrap();
    assert!(!disabled.replayed);
    assert_eq!(
        disabled.state.desired,
        PluginDesiredState::InstalledDisabled
    );
    assert!(
        disabled.state.package_generation.unwrap() > disable_request.expected_package_generation
    );

    let recovered = CognitivePackageHostManager::new(
        observe_request.scope.clone(),
        "use:test-build",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let replayed_disable = recovered.apply(disable_apply_request).await.unwrap();
    assert!(replayed_disable.replayed);
    assert_eq!(
        replayed_disable.operation_result_digest,
        disabled.operation_result_digest
    );

    let no_change = recovered
        .plan_enablement(PluginHostEnablementPlanRequest {
            schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_string(),
            request_id: "plan:worker:disable:0002".to_string(),
            assignment_generation: disable_request.assignment_generation,
            capabilities_digest: disable_request.capabilities_digest,
            scope: disable_request.scope,
            package_id: disable_request.package_id,
            expected_package_generation: disabled.state.package_generation.unwrap(),
            enabled: false,
        })
        .await
        .unwrap();
    assert_eq!(no_change.status, PluginHostEnablementPlanStatus::NoChange);
    assert!(no_change.plan.is_none());
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);

    let scope_digest = recovered.managed_scope().descriptor_digest().unwrap();
    let request_path = home
        .join("state/plugin-host-manager")
        .join(scope_digest.strip_prefix("sha256:").unwrap())
        .join("requests")
        .join(format!("{:x}.json", Sha256::digest(b"plan:worker:0001")));
    let mut stale = observe_request;
    stale.request_id = "observe:worker:stale".to_string();
    stale.scope.fence_generation -= 1;
    let error = recovered.observe(stale).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    let mut tampered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&request_path).unwrap()).unwrap();
    tampered["recordDigest"] = serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    std::fs::write(&request_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let error = recovered.apply(apply_request).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_store_invalid");
}

#[tokio::test]
async fn permission_grants_follow_install_upgrade_uninstall_and_survive_replay() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v1",
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v2",
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 53, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("home");
    let registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let extension_registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let managed_scope = PlanScope {
        kind: PlanScopeKind::Workspace,
        id: MANAGED_SCOPE_ID.to_string(),
    };
    let manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        extension_registry.clone(),
        managed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    assert_eq!(manager.scope(), &managed_scope);

    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    let interrupted = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let wrong_scope_manager = CognitivePackageManager::with_scope_lifecycle_and_authorization(
        extension_registry.clone(),
        MANAGED_SCOPE_ID,
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let scope_error = wrong_scope_manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(scope_error.code, "use.plugin.package_graph_busy");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);

    let pending_path = home.join("state/operations/package-graphs/install/acme/worker.json");
    let pending_bytes = std::fs::read(&pending_path).unwrap();
    let pending: serde_json::Value = serde_json::from_slice(&pending_bytes).unwrap();
    assert_eq!(pending["envelope"]["plan"]["scope"]["kind"], "workspace");
    assert_eq!(pending["envelope"]["plan"]["scope"]["id"], MANAGED_SCOPE_ID);
    let mut tampered = Vec::new();

    let mut missing_resolved = pending.clone();
    missing_resolved["authorization"]
        .as_object_mut()
        .unwrap()
        .remove("resolvedGrants");
    tampered.push((
        "missing resolved Grant",
        "use.plugin.package_authorization_invalid",
        missing_resolved,
    ));

    let mut changed_confirmation = pending.clone();
    let confirmed_at = changed_confirmation["authorization"]["operationConfirmation"]
        ["confirmedAtMs"]
        .as_u64()
        .unwrap();
    changed_confirmation["authorization"]["operationConfirmation"]["confirmedAtMs"] =
        serde_json::json!(confirmed_at + 1);
    tampered.push((
        "changed operation confirmation",
        "use.plugin.plan_confirmation_mismatch",
        changed_confirmation,
    ));

    let mut changed_snapshot = pending.clone();
    changed_snapshot["authorization"]["grantSnapshot"]["stateRevision"] = serde_json::json!(999);
    tampered.push((
        "changed Grant snapshot",
        "use.plugin.package_authorization_invalid",
        changed_snapshot,
    ));

    let mut changed_change_set = pending.clone();
    changed_change_set["authorization"]["grantChangeSet"]["stateRevision"] = serde_json::json!(999);
    tampered.push((
        "changed Grant change set",
        "use.plugin.grant_changes_plan_mismatch",
        changed_change_set,
    ));

    let mut changed_ceiling = pending.clone();
    changed_ceiling["authorization"]["grantCeilings"][0]["packageDigest"] =
        serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    tampered.push((
        "changed signed ceiling",
        "use.plugin.package_authorization_invalid",
        changed_ceiling,
    ));

    let mut legacy_permission_operation = pending;
    legacy_permission_operation["schema"] =
        serde_json::json!("a3s.use.pending-package-graph-operation.v1");
    tampered.push((
        "permission-bearing legacy pending schema",
        "use.plugin.package_graph_store_invalid",
        legacy_permission_operation,
    ));

    for (case, expected_code, value) in tampered {
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let error = manager
            .install_remote(
                &registry,
                &[],
                "acme/worker",
                Some("1.0.0"),
                PluginReleaseChannel::Stable,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected_code, "unexpected error for {case}");
        assert_eq!(
            authorization_count.load(Ordering::SeqCst),
            1,
            "tampered pending evidence must not trigger reauthorization: {case}"
        );
    }
    std::fs::write(&pending_path, &pending_bytes).unwrap();

    let installed = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(installed.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    let install_plan = installed.plan.as_ref().unwrap();
    assert_eq!(install_plan.plan.authority, test_authority());
    assert_eq!(install_plan.plan.scope, managed_scope);
    assert_eq!(install_plan.plan.workspace_impacts.len(), 1);
    let first_state = install_plan.plan.packages[0].after.as_ref().unwrap();
    assert_granted(
        &home,
        MANAGED_SCOPE_ID,
        &first_state.release.package_sha256,
        &first_state.permissions,
    )
    .await;

    let upgrade_lock = resolve_remote_package_lock(
        &registry,
        &[],
        "acme/worker",
        Some("2.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let upgrade_lock_digest = upgrade_lock.descriptor_digest().unwrap();
    let prepared_upgrade = manager
        .prepare_upgrade_remote(
            &registry,
            &[],
            "acme/worker",
            Some("2.0.0"),
            PluginReleaseChannel::Stable,
            &upgrade_lock_digest,
        )
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        manager
            .installed_package_lock("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        installed.package_lock_digest
    );
    assert_eq!(
        manager
            .prepare_upgrade_remote(
                &registry,
                &[],
                "acme/worker",
                Some("2.0.0"),
                PluginReleaseChannel::Stable,
                &upgrade_lock_digest,
            )
            .await
            .unwrap(),
        prepared_upgrade
    );
    let upgraded = manager
        .upgrade_remote(
            &registry,
            &[],
            "acme/worker",
            Some("2.0.0"),
            PluginReleaseChannel::Stable,
            Some(&upgrade_lock_digest),
        )
        .await
        .unwrap();
    assert!(upgraded.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 2);
    let upgrade_plan = upgraded.plan.as_ref().unwrap();
    assert_eq!(upgrade_plan, &prepared_upgrade);
    assert_eq!(upgrade_plan.plan.scope, managed_scope);
    let transition = &upgrade_plan.plan.packages[0];
    let prior = transition.before.as_ref().unwrap();
    let candidate = transition.after.as_ref().unwrap();
    assert_revoked(&home, MANAGED_SCOPE_ID, &prior.release.package_sha256).await;
    assert_granted(
        &home,
        MANAGED_SCOPE_ID,
        &candidate.release.package_sha256,
        &candidate.permissions,
    )
    .await;

    let uninstall_lock_digest = upgraded.package_lock_digest.clone();
    let prepared_uninstall = manager
        .prepare_uninstall("acme/worker", &uninstall_lock_digest)
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        manager
            .installed_package_lock("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        uninstall_lock_digest
    );
    assert_granted(
        &home,
        MANAGED_SCOPE_ID,
        &candidate.release.package_sha256,
        &candidate.permissions,
    )
    .await;
    assert_eq!(
        manager
            .prepare_uninstall("acme/worker", &uninstall_lock_digest)
            .await
            .unwrap(),
        prepared_uninstall
    );
    let uninstalled = manager.uninstall("acme/worker").await.unwrap();
    assert!(uninstalled.changed);
    assert_eq!(uninstalled.plan, prepared_uninstall);
    assert_eq!(uninstalled.plan.plan.scope, managed_scope);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 3);
    assert_revoked(&home, MANAGED_SCOPE_ID, &candidate.release.package_sha256).await;
    assert!(!home
        .join("state/operations/package-graphs/install/acme/worker.json")
        .exists());
    assert!(!home
        .join("state/operations/package-graphs/upgrade/acme/worker.json")
        .exists());
    assert!(!home
        .join("state/operations/package-graphs/uninstall/acme/worker.json")
        .exists());
}

#[tokio::test]
async fn permission_bearing_enablement_cuts_over_grants_and_recovers_after_cutover() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let targets = cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v1",
        "1.0.0",
        &target,
    );
    let repository = TestRepository::with_targets(targets, 59, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("home");
    let registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let extension_registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let manager = CognitivePackageManager::with_authorization(
        extension_registry.clone(),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    let installed = extension_registry
        .get("acme/worker")
        .await
        .unwrap()
        .unwrap();
    let lifecycle_generation = installed.receipt.lifecycle_generation.unwrap();
    assert!(installed.receipt.planning_bundle.is_some());
    assert!(installed.plan_ready_planning_bundle().unwrap().is_some());
    let install_plan = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(!install_plan.changed);
    let catalog = installed.plan_ready_catalog().unwrap();
    let package_digest = catalog.record.package.sha256.clone().unwrap();
    let permissions = catalog.record.permission_ceiling.clone();
    assert_granted(&home, &manager.scope().id, &package_digest, &permissions).await;

    let state = manager.observe_package("acme/worker").await.unwrap();
    let request = CognitivePackageEnablementRequest::new(
        "enablement:worker:disable:0001",
        "acme/worker",
        state.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let planned = manager.plan_enablement(&request).await.unwrap();
    assert_eq!(
        planned.status,
        CognitivePackageEnablementPlanStatus::Planned
    );
    let planned_envelope = planned.plan.as_ref().unwrap();
    assert_eq!(planned_envelope.plan.operation_id, request.operation_id);
    assert_eq!(planned_envelope.plan.action, PluginOperationAction::Disable);
    assert_eq!(
        planned_envelope.plan.schema,
        a3s_use_core::PLUGIN_OPERATION_PLAN_SCHEMA_V4
    );
    assert_eq!(
        planned_envelope.plan.authority.decision,
        PlanPolicyDecision::Ask
    );
    assert!(planned_envelope.plan.authority.confirmation_required);
    assert!(planned.result.is_none());
    assert_eq!(planned.state, state);
    assert!(
        extension_registry
            .get("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );
    assert_granted(&home, &manager.scope().id, &package_digest, &permissions).await;

    let confirmation_required = manager
        .apply_enablement(&request, planned_envelope.clone(), None)
        .await
        .unwrap_err();
    assert_eq!(
        confirmation_required.code,
        "use.plugin.package_reviewed_authorization_invalid"
    );
    assert!(
        extension_registry
            .get("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );
    assert_granted(&home, &manager.scope().id, &package_digest, &permissions).await;
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);

    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks/acme/worker")
            .join(format!("{lifecycle_generation:020}.lock")),
    );
    let interrupted_manager = manager.clone();
    let interrupted_request = request.clone();
    let interrupted_plan = planned_envelope.clone();
    let interrupted_confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: interrupted_plan.plan.operation_id.clone(),
        plan_digest: interrupted_plan.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: interrupted_plan.plan.created_at_ms + 1,
    };
    let interrupted = tokio::spawn(async move {
        interrupted_manager
            .apply_enablement(
                &interrupted_request,
                interrupted_plan,
                Some(interrupted_confirmation),
            )
            .await
    });

    let grant_store = WorkspaceGrantStore::new(home.join("state"));
    let mut reached_cutover_drain = false;
    let mut disable_cutover_generation = None;
    for _ in 0..500 {
        let hidden = extension_registry
            .get("acme/worker")
            .await
            .unwrap()
            .is_some_and(|extension| !extension.receipt.enabled);
        let cutover_committed = grant_store
            .observe_change_set(&request.operation_id)
            .await
            .unwrap()
            .is_some_and(|journal| journal.phase == WorkspaceGrantLifecyclePhase::CutoverCommitted);
        if hidden && cutover_committed {
            reached_cutover_drain = true;
            disable_cutover_generation =
                Some(extension_registry.snapshot().await.unwrap().generation);
            break;
        }
        if interrupted.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !reached_cutover_drain {
        FileExt::unlock(&route_lock).unwrap();
        drop(route_lock);
        let outcome = interrupted.await;
        panic!("disable did not reach the cutover-before-drain checkpoint: {outcome:?}");
    }
    assert_granted(&home, &manager.scope().id, &package_digest, &permissions).await;
    assert!(extension_registry
        .find_published_route("worker-v1")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        extension_registry
            .snapshot()
            .await
            .unwrap()
            .pending_cutovers
            .len(),
        1
    );

    interrupted.abort();
    let _ = interrupted.await;
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);

    let restarted = CognitivePackageManager::with_authorization(
        extension_registry.clone(),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let disabled = apply_planned_enablement(&restarted, &request)
        .await
        .unwrap();
    assert!(disabled.changed);
    assert!(!disabled.replayed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        extension_registry.snapshot().await.unwrap().generation,
        disable_cutover_generation.unwrap()
    );
    assert!(extension_registry
        .snapshot()
        .await
        .unwrap()
        .pending_cutovers
        .is_empty());
    assert_revoked(&home, &restarted.scope().id, &package_digest).await;
    assert_eq!(
        grant_store
            .observe_change_set(&request.operation_id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        WorkspaceGrantLifecyclePhase::Completed
    );

    let replayed = apply_planned_enablement(&restarted, &request)
        .await
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);

    let enable = CognitivePackageEnablementRequest::new(
        "enablement:worker:enable:0002",
        "acme/worker",
        disabled.state.package_generation.unwrap(),
        true,
    )
    .unwrap();
    let prepared = restarted.prepare_enablement(&enable).await.unwrap();
    let CognitivePackageEnablementPreparation::Draft(prepared) = prepared else {
        panic!("re-enable must produce a provider-neutral draft");
    };
    assert!(prepared.planning_bundles.contains_key("acme/worker"));
    assert_eq!(
        prepared.installed_generations.get("acme/worker"),
        Some(&lifecycle_generation)
    );
    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    assert_eq!(
        apply_planned_enablement(&restarted, &enable)
            .await
            .unwrap_err()
            .code,
        "use.extension.busy"
    );
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        grant_store
            .observe_change_set(&enable.operation_id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        WorkspaceGrantLifecyclePhase::Prepared
    );
    assert!(extension_registry
        .find_published_route("worker-v1")
        .await
        .unwrap()
        .is_none());
    assert_granted(&home, &restarted.scope().id, &package_digest, &permissions).await;
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let enabled = apply_planned_enablement(&restarted, &enable).await.unwrap();
    assert!(enabled.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        extension_registry.snapshot().await.unwrap().generation,
        disable_cutover_generation.unwrap() + 1
    );
    assert!(extension_registry
        .snapshot()
        .await
        .unwrap()
        .pending_cutovers
        .is_empty());
    assert_eq!(
        grant_store
            .observe_change_set(&enable.operation_id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        WorkspaceGrantLifecyclePhase::Completed
    );
    assert_granted(&home, &restarted.scope().id, &package_digest, &permissions).await;
    assert!(extension_registry
        .find_published_route("worker-v1")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn reviewed_host_plan_reproduces_exact_signed_lock_and_grant_in_a_clean_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        cognitive_tool_targets_version(
            temporary.path(),
            "acme/worker",
            "worker-reviewed",
            "1.0.0",
            &target,
        ),
        59,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());

    let source_home = temporary.path().join("source-home");
    let source_registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        source_home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let reviewed_scope = PlanScope {
        kind: PlanScopeKind::Workspace,
        id: MANAGED_SCOPE_ID.to_string(),
    };
    let source_extension_registry = ExtensionRegistry::new(ExtensionPaths::new(
        source_home.join("data"),
        source_home.join("state"),
    ));
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let source_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        source_extension_registry.clone(),
        reviewed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let package_lock = resolve_remote_package_lock(
        &source_registry,
        &[],
        "acme/worker",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let expected_lock_digest = package_lock.descriptor_digest().unwrap();
    let reviewed = source_manager
        .prepare_install_remote(
            &source_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &expected_lock_digest,
        )
        .await
        .unwrap();
    let replayed_plan = source_manager
        .prepare_install_remote(
            &source_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &expected_lock_digest,
        )
        .await
        .unwrap();
    assert_eq!(replayed_plan, reviewed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);
    assert!(source_manager
        .installed_package_lock("acme/worker")
        .await
        .unwrap()
        .is_none());
    assert!(source_extension_registry
        .get("acme/worker")
        .await
        .unwrap()
        .is_none());
    assert!(reviewed.package_lock.is_some());
    assert_eq!(reviewed.plan.scope, reviewed_scope);
    assert_eq!(reviewed.plan.workspace_impacts.len(), 1);
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: reviewed.plan.operation_id.clone(),
        plan_digest: reviewed.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: reviewed.plan.created_at_ms + 1,
    };
    assert_eq!(
        reviewed
            .package_lock
            .as_ref()
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        expected_lock_digest
    );

    let target_home = temporary.path().join("target-home");
    let target_registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        target_home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let target_extension_registry = ExtensionRegistry::new(ExtensionPaths::new(
        target_home.join("data"),
        target_home.join("state"),
    ));
    let target_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        target_extension_registry.clone(),
        reviewed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(
            ReviewedCognitivePackageAuthorizationProvider::new(
                reviewed.clone(),
                Some(confirmation.clone()),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let registry_lock = exclusive_lock(&target_home.join("state/extensions/.registry.lock"));
    let interrupted = target_manager
        .install_remote(
            &target_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&expected_lock_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let mut drifted = reviewed.clone();
    let drifted_lock = drifted.package_lock.as_mut().unwrap();
    drifted_lock.packages[0]
        .catalog
        .provenance
        .timestamp_version += 1;
    let drifted_provenance = drifted_lock.packages[0].catalog.provenance.clone();
    let PluginPlanSource::Registry { provenance, .. } =
        drifted.plan.packages[0].source.as_mut().unwrap()
    else {
        panic!("reviewed signed package plan must retain Registry provenance");
    };
    *provenance = drifted_provenance;
    drifted.plan.package_lock_digest = Some(drifted_lock.descriptor_digest().unwrap());
    drifted.plan_digest = drifted.plan.descriptor_digest().unwrap();
    drifted.validate().unwrap();
    let drifted_confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: drifted.plan.operation_id.clone(),
        plan_digest: drifted.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: confirmation.confirmed_at_ms,
    };
    let drifted_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        target_extension_registry.clone(),
        reviewed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(
            ReviewedCognitivePackageAuthorizationProvider::new(drifted, Some(drifted_confirmation))
                .unwrap(),
        ),
    )
    .unwrap();
    let replay_error = drifted_manager
        .install_remote(
            &target_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&expected_lock_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(
        replay_error.code,
        "use.plugin.package_reviewed_plan_mismatch"
    );

    let replay_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        target_extension_registry,
        reviewed_scope,
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(
            ReviewedCognitivePackageAuthorizationProvider::new(
                reviewed.clone(),
                Some(confirmation),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let target_result = replay_manager
        .install_remote(
            &target_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&expected_lock_digest),
        )
        .await
        .unwrap();

    assert!(target_result.changed);
    assert_eq!(target_result.plan.as_ref(), Some(&reviewed));
    let installed = reviewed.plan.packages[0].after.as_ref().unwrap();
    assert_granted(
        &target_home,
        MANAGED_SCOPE_ID,
        &installed.release.package_sha256,
        &installed.permissions,
    )
    .await;
}

fn cognitive_tool_targets_version(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    target: &str,
) -> Vec<TestTarget> {
    let package_root = fixture_root.join("packages").join(route);
    std::fs::create_dir_all(package_root.join("tools/convert/bin")).unwrap();
    let manifest = format!(
        "extension \"{package_id}\" {{\n  schema_version = 3\n  version = \"{version}\"\n  route = \"{route}\"\n  requires_use = \">=0.3.0, <0.4.0\"\n  actions = [\"read\", \"execute\"]\n\n  repository {{\n    url = \"https://github.com/acme/worker\"\n    revision = \"0123456789abcdef0123456789abcdef01234567\"\n  }}\n\n  tool \"convert\" {{\n    workload = \"task\"\n    interface = \"cli\"\n    executable = \"tools/convert/bin/convert\"\n    command = \"acme-worker-convert\"\n    json_output = true\n    interactive = false\n    timeout_ms = 120000\n    activation = \"lazy\"\n    optional = false\n  }}\n}}\n"
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
    catalog.dependencies.clear();
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
    scope_id: &str,
    package_digest: &str,
    ceiling: &PluginPermissionCeiling,
) {
    let record = WorkspaceGrantStore::new(home.join("state"))
        .observe(scope_id, "acme/worker", package_digest)
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

async fn assert_revoked(home: &std::path::Path, scope_id: &str, package_digest: &str) {
    let record = WorkspaceGrantStore::new(home.join("state"))
        .observe(scope_id, "acme/worker", package_digest)
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
