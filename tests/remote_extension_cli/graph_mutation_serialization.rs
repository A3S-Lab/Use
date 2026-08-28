use super::*;

use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_use::cognitive_package::{
    CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider,
    ReviewedCognitivePackageAuthorizationProvider, StandaloneCognitivePackageAuthorizationProvider,
};
use a3s_use_core::{
    PlanAuthority, PlanPolicyDecision, PluginOperationConfirmation, PluginOperationPlan,
    PluginOperationPlanBinding, PluginOperationPlanDraft, PluginOperationPlanEnvelope,
    PluginWorkspaceGrantChangeSet, UseError, UseResult, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};
use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};

#[derive(Debug)]
struct PausingAuthorization {
    entered: mpsc::UnboundedSender<()>,
    release: Arc<Notify>,
}

#[derive(Debug)]
pub(super) struct CountingReviewedAuthorization {
    pub(super) reviewed: ReviewedCognitivePackageAuthorizationProvider,
    pub(super) authorization_count: Arc<AtomicUsize>,
}

#[async_trait]
impl CognitivePackageAuthorizationProvider for CountingReviewedAuthorization {
    fn name(&self) -> &'static str {
        "integration-counting-reviewed-authorization"
    }

    fn reviewed_plan(&self) -> Option<&PluginOperationPlanEnvelope> {
        self.reviewed.reviewed_plan()
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        self.reviewed.bind_authority(draft)
    }

    fn bind_operation(
        &self,
        draft: &PluginOperationPlanDraft,
        default_binding: PluginOperationPlanBinding,
    ) -> UseResult<PluginOperationPlanBinding> {
        self.reviewed.bind_operation(draft, default_binding)
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        self.reviewed.verify_authority(plan)
    }

    fn verify_plan(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        self.reviewed.verify_plan(envelope)
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        self.authorization_count.fetch_add(1, Ordering::SeqCst);
        self.reviewed.authorize(envelope, changes, now_ms).await
    }
}

#[async_trait]
impl CognitivePackageAuthorizationProvider for PausingAuthorization {
    fn name(&self) -> &'static str {
        "integration-pausing-authorization"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        StandaloneCognitivePackageAuthorizationProvider.bind_authority(draft)
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        StandaloneCognitivePackageAuthorizationProvider.verify_authority(plan)
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        self.entered.send(()).map_err(|_| {
            UseError::new(
                "test.plugin.authorization_barrier_closed",
                "The concurrent mutation authorization barrier closed unexpectedly.",
            )
        })?;
        self.release.notified().await;
        StandaloneCognitivePackageAuthorizationProvider
            .authorize(envelope, changes, now_ms)
            .await
    }
}

#[tokio::test]
async fn different_roots_with_a_shared_dependency_serialize_before_authorization() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let dependency = cognitive_skill_target(
        temp.path(),
        "acme/dependency",
        "dependency",
        Vec::new(),
        &target,
    );
    let first = cognitive_skill_target(
        temp.path(),
        "acme/first",
        "first",
        vec![PluginPackageDependency::new("acme/dependency", "^1.0.0").unwrap()],
        &target,
    );
    let second = cognitive_skill_target(
        temp.path(),
        "acme/second",
        "second",
        vec![PluginPackageDependency::new("acme/dependency", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, second, dependency], 17, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let registry = ExtensionRegistry::new(extension_paths(&home));
    let initial = CognitivePackageManager::new(registry.clone()).unwrap();
    initial
        .install_remote(
            &trusted,
            &[],
            "acme/first",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();

    let (install_entered_tx, mut install_entered_rx) = mpsc::unbounded_channel();
    let install_release = Arc::new(Notify::new());
    let install_manager = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(PausingAuthorization {
            entered: install_entered_tx,
            release: install_release.clone(),
        }),
    )
    .unwrap();
    let install_registry = trusted.clone();
    let install = tokio::spawn(async move {
        install_manager
            .install_remote(
                &install_registry,
                &[],
                "acme/second",
                Some("1.0.0"),
                PluginReleaseChannel::Stable,
                None,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), install_entered_rx.recv())
        .await
        .expect("install did not reach its deterministic authorization barrier")
        .expect("install authorization barrier closed");

    let (uninstall_entered_tx, mut uninstall_entered_rx) = mpsc::unbounded_channel();
    let uninstall_release = Arc::new(Notify::new());
    let uninstall_manager = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(PausingAuthorization {
            entered: uninstall_entered_tx,
            release: uninstall_release.clone(),
        }),
    )
    .unwrap();
    let uninstall = tokio::spawn(async move { uninstall_manager.uninstall("acme/first").await });

    assert!(
        tokio::time::timeout(Duration::from_millis(250), uninstall_entered_rx.recv())
            .await
            .is_err(),
        "a second root mutation reached authorization before the active installation mutation completed"
    );

    install_release.notify_one();
    let installed = tokio::time::timeout(Duration::from_secs(10), install)
        .await
        .expect("serialized install timed out")
        .expect("serialized install task failed")
        .unwrap();
    assert_eq!(installed.retained_packages, ["acme/dependency"]);

    tokio::time::timeout(Duration::from_secs(5), uninstall_entered_rx.recv())
        .await
        .expect("uninstall did not resume after the installation mutation completed")
        .expect("uninstall authorization barrier closed");
    uninstall_release.notify_one();
    let removed = tokio::time::timeout(Duration::from_secs(10), uninstall)
        .await
        .expect("serialized uninstall timed out")
        .expect("serialized uninstall task failed")
        .unwrap();

    assert_eq!(removed.removed_packages, ["acme/first"]);
    assert_eq!(removed.retained_packages, ["acme/dependency"]);
    assert!(registry.get("acme/first").await.unwrap().is_none());
    assert!(registry.get("acme/second").await.unwrap().is_some());
    assert!(registry.get("acme/dependency").await.unwrap().is_some());
    let restarted = CognitivePackageManager::new(registry).unwrap();
    assert!(restarted
        .installed_package_lock("acme/second")
        .await
        .unwrap()
        .is_some());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn separate_cli_processes_leave_a_linearizable_shared_dependency_graph() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let dependency = cognitive_skill_target(
        temp.path(),
        "acme/dependency",
        "dependency",
        Vec::new(),
        &target,
    );
    let first = cognitive_skill_target(
        temp.path(),
        "acme/first",
        "first",
        vec![PluginPackageDependency::new("acme/dependency", "^1.0.0").unwrap()],
        &target,
    );
    let second = cognitive_skill_target(
        temp.path(),
        "acme/second",
        "second",
        vec![PluginPackageDependency::new("acme/dependency", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, second, dependency], 23, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let initial = cognitive_registry_install(&server, &repository, &home, "acme/first", &[]);
    assert!(initial.status.success(), "{initial:?}");

    let mutation_lock = exclusive_lock(&scoped_state(&home, ".installation-mutation.lock"));
    let mut install = tokio::process::Command::new(binary())
        .args([
            "install",
            "acme/second",
            "--registry-name",
            "fixture",
            "--version",
            "1.0.0",
            "--json",
        ])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut uninstall = tokio::process::Command::new(binary())
        .args(["uninstall", "acme/first", "--yes", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(install.try_wait().unwrap().is_none());
    assert!(uninstall.try_wait().unwrap().is_none());

    FileExt::unlock(&mutation_lock).unwrap();
    drop(mutation_lock);
    let (installed, uninstalled) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(install.wait_with_output(), uninstall.wait_with_output())
    })
    .await
    .expect("serialized CLI graph mutations timed out");
    let installed = installed.unwrap();
    let uninstalled = uninstalled.unwrap();
    let install_succeeded = installed.status.success();
    let uninstall_succeeded = uninstalled.status.success();
    assert!(
        install_succeeded || uninstall_succeeded,
        "at least one serialized mutation must commit: install={installed:?}, uninstall={uninstalled:?}",
    );
    for rejected in [&installed, &uninstalled]
        .into_iter()
        .filter(|output| !output.status.success())
    {
        let error = json(rejected);
        assert!(
            matches!(
                error["error"]["code"].as_str(),
                Some(
                    "use.plugin.package_graph_busy"
                        | "use.plugin.package_generation_changed"
                        | "use.extension.registry_cutover_conflict"
                )
            ),
            "a losing reviewed plan must fail as an explicit concurrency conflict: {error}",
        );
    }

    let registry = ExtensionRegistry::new(extension_paths(&home));
    let first_exists = registry.get("acme/first").await.unwrap().is_some();
    let second_exists = registry.get("acme/second").await.unwrap().is_some();
    let dependency_exists = registry.get("acme/dependency").await.unwrap().is_some();
    assert_eq!(first_exists, !uninstall_succeeded);
    assert_eq!(second_exists, install_succeeded);
    assert_eq!(dependency_exists, first_exists || second_exists);
    let restarted = CognitivePackageManager::new(registry).unwrap();
    assert_eq!(
        restarted
            .installed_package_lock("acme/first")
            .await
            .unwrap()
            .is_some(),
        first_exists,
    );
    let second_lock = restarted
        .installed_package_lock("acme/second")
        .await
        .unwrap();
    assert_eq!(second_lock.is_some(), second_exists);
    if let Some(second_lock) = second_lock {
        assert!(second_lock.package("acme/dependency").is_some());
    }
}

#[tokio::test]
async fn a_stale_reviewed_graph_fails_before_authorization_or_installation_effects() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let reviewed_target = cognitive_skill_target(
        temp.path(),
        "acme/reviewed",
        "reviewed",
        Vec::new(),
        &target,
    );
    let intervening_target = cognitive_skill_target(
        temp.path(),
        "acme/intervening",
        "intervening",
        Vec::new(),
        &target,
    );
    let repository =
        TestRepository::with_targets(vec![reviewed_target, intervening_target], 19, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let registry = ExtensionRegistry::new(extension_paths(&home));
    let planner = CognitivePackageManager::new(registry.clone()).unwrap();
    let reviewed_lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/reviewed",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let reviewed_lock_digest = reviewed_lock.descriptor_digest().unwrap();
    let reviewed_plan = planner
        .prepare_install_remote(
            &trusted,
            &[],
            "acme/reviewed",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &reviewed_lock_digest,
        )
        .await
        .unwrap();
    let reviewed_generation = reviewed_plan.plan.state.capability_generation;

    planner
        .install_remote(
            &trusted,
            &[],
            "acme/intervening",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    let snapshot_before_apply = registry.snapshot().await.unwrap();
    assert!(snapshot_before_apply.generation > reviewed_generation);

    let confirmation =
        (reviewed_plan.plan.authority.decision == PlanPolicyDecision::Ask).then(|| {
            PluginOperationConfirmation {
                schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
                operation_id: reviewed_plan.plan.operation_id.clone(),
                plan_digest: reviewed_plan.plan_digest.clone(),
                confirmed_by: reviewed_plan.plan.authority.actor,
                confirmed_at_ms: reviewed_plan.plan.created_at_ms + 1,
            }
        });
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let reviewed =
        ReviewedCognitivePackageAuthorizationProvider::new(reviewed_plan, confirmation).unwrap();
    let applying = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(CountingReviewedAuthorization {
            reviewed,
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let error = applying
        .install_cached(
            &trusted,
            &[],
            "acme/reviewed",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&reviewed_lock_digest),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.package_generation_changed");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before_apply);
    assert!(registry.get("acme/reviewed").await.unwrap().is_none());
    assert!(registry.get("acme/intervening").await.unwrap().is_some());
}

#[tokio::test]
async fn uninstall_revalidates_a_dependency_adopted_as_a_root_without_generation_change() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let dependency = cognitive_skill_target(
        temp.path(),
        "acme/dependency",
        "dependency",
        Vec::new(),
        &target,
    );
    let owner = cognitive_skill_target(
        temp.path(),
        "acme/owner",
        "owner",
        vec![PluginPackageDependency::new("acme/dependency", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![owner, dependency], 21, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let registry = ExtensionRegistry::new(extension_paths(&home));
    let planner = CognitivePackageManager::new(registry.clone()).unwrap();
    let installed = planner
        .install_remote(
            &trusted,
            &[],
            "acme/owner",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    let reviewed_plan = planner
        .prepare_uninstall("acme/owner", &installed.package_lock_digest)
        .await
        .unwrap();
    let reviewed_generation = reviewed_plan.plan.state.capability_generation;

    let adopted = planner
        .install_remote(
            &trusted,
            &[],
            "acme/dependency",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(!adopted.changed);
    assert_eq!(
        registry.snapshot().await.unwrap().generation,
        reviewed_generation,
        "root adoption must not be mistaken for a capability cutover"
    );

    let confirmation =
        (reviewed_plan.plan.authority.decision == PlanPolicyDecision::Ask).then(|| {
            PluginOperationConfirmation {
                schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
                operation_id: reviewed_plan.plan.operation_id.clone(),
                plan_digest: reviewed_plan.plan_digest.clone(),
                confirmed_by: reviewed_plan.plan.authority.actor,
                confirmed_at_ms: reviewed_plan.plan.created_at_ms + 1,
            }
        });
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let reviewed =
        ReviewedCognitivePackageAuthorizationProvider::new(reviewed_plan, confirmation).unwrap();
    let applying = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(CountingReviewedAuthorization {
            reviewed,
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let error = applying.uninstall("acme/owner").await.unwrap_err();

    assert_eq!(error.code, "use.plugin.package_generation_changed");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);
    assert!(registry.get("acme/owner").await.unwrap().is_some());
    assert!(registry.get("acme/dependency").await.unwrap().is_some());
    assert!(planner
        .installed_package_lock("acme/owner")
        .await
        .unwrap()
        .is_some());
    assert!(planner
        .installed_package_lock("acme/dependency")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn upgrade_revalidates_a_dependency_adopted_as_a_root_without_generation_change() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first_root = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/root",
        "root",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/dependency", "^1.0.0").unwrap()],
        &target,
    );
    let dependency = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/dependency",
        "dependency",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let next_root = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/root",
        "root",
        "1.1.0",
        Vec::new(),
        &target,
    );
    let first_repository = TestRepository::with_targets(vec![first_root, dependency], 29, FUTURE);
    let next_repository = TestRepository::with_targets(vec![next_root], 31, FUTURE);
    let first_server = TestServer::start(first_repository.routes.clone());
    let next_server = TestServer::start(next_repository.routes.clone());
    let home = temp.path().join("home");
    let first_registry = TrustedRegistry::new(
        "first",
        first_server.base_url(),
        &first_repository.root_sha256,
        None,
        home.join("state/remote-registries/first"),
    )
    .unwrap();
    let next_registry = TrustedRegistry::new(
        "next",
        next_server.base_url(),
        &next_repository.root_sha256,
        None,
        home.join("state/remote-registries/next"),
    )
    .unwrap();
    let registry = ExtensionRegistry::new(extension_paths(&home));
    let planner = CognitivePackageManager::new(registry.clone()).unwrap();
    planner
        .install_remote(
            &first_registry,
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();

    let candidate_lock = resolve_remote_package_lock(
        &next_registry,
        &[],
        "acme/root",
        Some("1.1.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let candidate_lock_digest = candidate_lock.descriptor_digest().unwrap();
    let reviewed_plan = planner
        .prepare_upgrade_remote(
            &next_registry,
            &[],
            "acme/root",
            Some("1.1.0"),
            PluginReleaseChannel::Stable,
            &candidate_lock_digest,
        )
        .await
        .unwrap();
    let reviewed_generation = reviewed_plan.plan.state.capability_generation;

    let adopted = planner
        .install_remote(
            &first_registry,
            &[],
            "acme/dependency",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(!adopted.changed);
    assert_eq!(
        registry.snapshot().await.unwrap().generation,
        reviewed_generation,
        "root adoption must not be mistaken for a capability cutover"
    );

    let confirmation =
        (reviewed_plan.plan.authority.decision == PlanPolicyDecision::Ask).then(|| {
            PluginOperationConfirmation {
                schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
                operation_id: reviewed_plan.plan.operation_id.clone(),
                plan_digest: reviewed_plan.plan_digest.clone(),
                confirmed_by: reviewed_plan.plan.authority.actor,
                confirmed_at_ms: reviewed_plan.plan.created_at_ms + 1,
            }
        });
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let reviewed =
        ReviewedCognitivePackageAuthorizationProvider::new(reviewed_plan, confirmation).unwrap();
    let applying = CognitivePackageManager::with_authorization(
        registry.clone(),
        Arc::new(CountingReviewedAuthorization {
            reviewed,
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let error = applying
        .upgrade_cached(
            &next_registry,
            &[],
            "acme/root",
            Some("1.1.0"),
            PluginReleaseChannel::Stable,
            Some(&candidate_lock_digest),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.package_generation_changed");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        registry
            .get("acme/root")
            .await
            .unwrap()
            .unwrap()
            .manifest
            .version,
        "1.0.0"
    );
    assert!(registry.get("acme/dependency").await.unwrap().is_some());
    assert!(planner
        .installed_package_lock("acme/root")
        .await
        .unwrap()
        .is_some());
    assert!(planner
        .installed_package_lock("acme/dependency")
        .await
        .unwrap()
        .is_some());
}
