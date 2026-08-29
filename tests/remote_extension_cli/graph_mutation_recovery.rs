use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_use::cognitive_package::ReviewedCognitivePackageAuthorizationProvider;
use a3s_use_core::{
    PlanPolicyDecision, PluginOperationConfirmation, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};

use super::graph_mutation_serialization::CountingReviewedAuthorization;

#[tokio::test]
async fn interrupted_graph_durably_blocks_enablement_admission_until_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let installed_target = cognitive_skill_target(
        temp.path(),
        "acme/installed",
        "installed",
        Vec::new(),
        &target,
    );
    let waiting_target =
        cognitive_skill_target(temp.path(), "acme/waiting", "waiting", Vec::new(), &target);
    let repository =
        TestRepository::with_targets(vec![installed_target, waiting_target], 27, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let registry = ExtensionRegistry::new(extension_paths(&home));
    let manager = CognitivePackageManager::new(registry.clone()).unwrap();
    manager
        .install_remote(
            &trusted,
            &[],
            "acme/installed",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();

    let waiting_lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/waiting",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let waiting_lock_digest = waiting_lock.descriptor_digest().unwrap();
    let waiting_plan = manager
        .prepare_install_remote(
            &trusted,
            &[],
            "acme/waiting",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &waiting_lock_digest,
        )
        .await
        .unwrap();
    let waiting_confirmation = (waiting_plan.plan.authority.decision == PlanPolicyDecision::Ask)
        .then(|| PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: waiting_plan.plan.operation_id.clone(),
            plan_digest: waiting_plan.plan_digest.clone(),
            confirmed_by: waiting_plan.plan.authority.actor,
            confirmed_at_ms: waiting_plan.plan.created_at_ms + 1,
        });

    let observed = manager.observe_package("acme/installed").await.unwrap();
    let disable = CognitivePackageEnablementRequest::new(
        "enablement:disable:blocked-by-graph",
        "acme/installed",
        observed.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let mut planned_disable = manager.plan_enablement(&disable).await.unwrap();
    let disable_plan = planned_disable.plan.take().unwrap();
    let disable_confirmation = (disable_plan.plan.authority.decision == PlanPolicyDecision::Ask)
        .then(|| PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: disable_plan.plan.operation_id.clone(),
            plan_digest: disable_plan.plan_digest.clone(),
            confirmed_by: disable_plan.plan.authority.actor,
            confirmed_at_ms: disable_plan.plan.created_at_ms + 1,
        });

    let authorization_count = Arc::new(AtomicUsize::new(0));
    let reviewed =
        ReviewedCognitivePackageAuthorizationProvider::new(waiting_plan, waiting_confirmation)
            .unwrap();
    let applying = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(CountingReviewedAuthorization {
            reviewed,
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let registry_lock = exclusive_lock(&scoped_state(&home, "extensions/.registry.lock"));
    let interrupted = applying
        .install_cached(
            &trusted,
            &[],
            "acme/waiting",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&waiting_lock_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let blocked = manager
        .apply_enablement(&disable, disable_plan, disable_confirmation)
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "use.plugin.package_graph_busy");
    assert!(registry.get("acme/waiting").await.unwrap().is_none());
    assert!(registry
        .get("acme/installed")
        .await
        .unwrap()
        .unwrap()
        .enabled());

    applying
        .install_cached(
            &trusted,
            &[],
            "acme/waiting",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&waiting_lock_digest),
        )
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert!(registry.get("acme/waiting").await.unwrap().is_some());
}

#[tokio::test]
async fn interrupted_enablement_durably_blocks_graph_admission_until_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let installed_target = cognitive_skill_target(
        temp.path(),
        "acme/installed",
        "installed",
        Vec::new(),
        &target,
    );
    let waiting_target =
        cognitive_skill_target(temp.path(), "acme/waiting", "waiting", Vec::new(), &target);
    let repository =
        TestRepository::with_targets(vec![installed_target, waiting_target], 25, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let registry = ExtensionRegistry::new(extension_paths(&home));
    let manager = CognitivePackageManager::new(registry.clone()).unwrap();
    manager
        .install_remote(
            &trusted,
            &[],
            "acme/installed",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();

    let waiting_lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/waiting",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let waiting_lock_digest = waiting_lock.descriptor_digest().unwrap();
    let waiting_plan = manager
        .prepare_install_remote(
            &trusted,
            &[],
            "acme/waiting",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &waiting_lock_digest,
        )
        .await
        .unwrap();

    let observed = manager.observe_package("acme/installed").await.unwrap();
    let disable = CognitivePackageEnablementRequest::new(
        "enablement:disable:durable-owner",
        "acme/installed",
        observed.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let mut planned_disable = manager.plan_enablement(&disable).await.unwrap();
    let disable_plan = planned_disable.plan.take().unwrap();
    let disable_confirmation = (disable_plan.plan.authority.decision == PlanPolicyDecision::Ask)
        .then(|| PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: disable_plan.plan.operation_id.clone(),
            plan_digest: disable_plan.plan_digest.clone(),
            confirmed_by: disable_plan.plan.authority.actor,
            confirmed_at_ms: disable_plan.plan.created_at_ms + 1,
        });
    let registry_lock = exclusive_lock(&scoped_state(&home, "extensions/.registry.lock"));
    let interrupted = manager
        .apply_enablement(&disable, disable_plan.clone(), disable_confirmation.clone())
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let waiting_confirmation = (waiting_plan.plan.authority.decision == PlanPolicyDecision::Ask)
        .then(|| PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: waiting_plan.plan.operation_id.clone(),
            plan_digest: waiting_plan.plan_digest.clone(),
            confirmed_by: waiting_plan.plan.authority.actor,
            confirmed_at_ms: waiting_plan.plan.created_at_ms + 1,
        });
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let reviewed =
        ReviewedCognitivePackageAuthorizationProvider::new(waiting_plan, waiting_confirmation)
            .unwrap();
    let applying = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(CountingReviewedAuthorization {
            reviewed,
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let blocked = applying
        .install_cached(
            &trusted,
            &[],
            "acme/waiting",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&waiting_lock_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "use.plugin.package_graph_busy");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);
    assert!(registry.get("acme/waiting").await.unwrap().is_none());
    assert!(registry
        .get("acme/installed")
        .await
        .unwrap()
        .unwrap()
        .enabled());

    let recovered = manager
        .apply_enablement(&disable, disable_plan, disable_confirmation)
        .await
        .unwrap();
    assert_eq!(
        recovered.state.desired,
        PluginDesiredState::InstalledDisabled
    );
}
