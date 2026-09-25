use std::sync::Arc;

use a3s_use_core::CapabilityGatewayCatalog;
use a3s_use_extension::ExtensionPaths;
use async_trait::async_trait;

use super::aggregate_tests::fixtures::control_installation;
use super::dispatcher::SystemControlEffectClock;
use super::effect_owner::runtime::ControlRuntimeServiceReadinessPort;
use super::effect_port::{
    ControlCapabilityCatalogProjectionPort, ControlEffectFailure, ControlEffectPortOutcome,
    ControlFlowEffectPort, ControlSurfaceApplication, ControlSurfaceEffectRequest,
};
use super::filesystem::CONTROL_STORE_DATABASE_FILE;
use super::model::{ControlCapabilityEffectAuthority, ControlOperationStatus};
use super::production::{
    reject_legacy_authority_paths, ProductionControlHostDependencies, ProductionControlLifecycle,
};

struct RejectingFlow;

struct EmptyCatalogProjection;

#[async_trait]
impl ControlCapabilityCatalogProjectionPort for EmptyCatalogProjection {
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog> {
        ControlEffectPortOutcome::applied(
            CapabilityGatewayCatalog::new(
                authority.generation.snapshot.installation.clone(),
                authority.generation.capability.generation,
                Vec::new(),
            )
            .unwrap(),
        )
    }
}

#[async_trait]
impl ControlFlowEffectPort for RejectingFlow {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        ControlEffectPortOutcome::rejected(
            ControlEffectFailure::new(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "provider.flow_unavailable",
            )
            .unwrap(),
        )
    }
}

struct RejectingReadiness;

fn readiness_error() -> a3s_use_core::UseError {
    a3s_use_core::UseError::new(
        "provider.gateway_unavailable",
        "Gateway readiness is not configured in this production fixture.",
    )
}

#[async_trait]
impl ControlRuntimeServiceReadinessPort for RejectingReadiness {
    async fn bind_tool_service(
        &self,
        _surface: &a3s_use_extension::ToolSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<crate::plugin_runtime::RuntimeEndpointRef> {
        Err(readiness_error())
    }

    async fn bind_mcp_service(
        &self,
        _surface: &a3s_use_extension::PluginMcpSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<super::effect_owner::runtime::ControlRuntimeMcpReadiness> {
        Err(readiness_error())
    }

    async fn drain_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<()> {
        Err(readiness_error())
    }

    async fn remove_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<()> {
        Err(readiness_error())
    }
}

fn production_dependencies() -> ProductionControlHostDependencies {
    ProductionControlHostDependencies {
        runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
        runtime_readiness: Arc::new(RejectingReadiness),
        catalog_projection: Arc::new(EmptyCatalogProjection),
        flow: Arc::new(RejectingFlow),
        clock: Arc::new(SystemControlEffectClock),
    }
}

fn cognitive_authorization(
    reviewed: &super::model::ReviewedControlOperation,
) -> crate::cognitive_package::CognitivePackageAuthorizationEvidence {
    crate::cognitive_package::CognitivePackageAuthorizationEvidence {
        operation_confirmation: reviewed.authorization.operation_confirmation.clone(),
        grant_confirmations: reviewed.authorization.grant_confirmations.clone(),
    }
}

#[tokio::test]
async fn production_lifecycle_initializes_control_without_legacy_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    assert!(paths
        .installation_state_root()
        .join(CONTROL_STORE_DATABASE_FILE)
        .is_file());
    reject_legacy_authority_paths(&paths.installation_state_root()).unwrap();
    assert!(lifecycle.current_snapshot().await.unwrap().is_none());
}

#[tokio::test]
async fn production_lifecycle_rejects_legacy_authority_beside_control() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();
    std::fs::write(
        paths
            .installation_state_root()
            .join("installation-snapshot.json"),
        "{}",
    )
    .unwrap();

    let error = reject_legacy_authority_paths(&paths.installation_state_root()).unwrap_err();
    assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
}

#[tokio::test]
async fn production_apply_commits_sole_control_authority_without_legacy_files() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();
    let reviewed = super::aggregate_tests::fixtures::operation("operation:production-apply");

    // Commit authority first. Full effect drain against real Artifact-backed
    // owners requires package bytes; that matrix stays in owner/dispatcher
    // tests. This gate proves the production entry commits Control without
    // writing any legacy authority leaf.
    let generation = lifecycle
        .composition()
        .admit_and_commit_cognitive_package_operation_with_runtime_plans(
            &reviewed.envelope,
            &cognitive_authorization(&reviewed),
            None,
            reviewed.reviewed_at_ms,
            reviewed.reviewed_at_ms + 10,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(generation.snapshot.generation, 1);
    assert_eq!(
        lifecycle
            .current_snapshot()
            .await
            .unwrap()
            .expect("committed snapshot")
            .generation,
        1
    );
    reject_legacy_authority_paths(&paths.installation_state_root()).unwrap();
    let pending = lifecycle
        .composition()
        .store()
        .operation(reviewed.operation_id())
        .await
        .unwrap()
        .expect("committed operation");
    assert_eq!(pending.status, ControlOperationStatus::EffectsPending);
}

#[tokio::test]
async fn production_apply_commits_grants_without_legacy_grants_leaf() {
    use a3s_use_core::PluginOperationAction;
    use super::aggregate_tests::fixtures::transition;
    use super::aggregate_tests::grant_fixtures::reviewed_grant_operation;

    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    // Permissioned grant installs require Runtime plan publications on the
    // admit+commit-with-plans seam. Commit Grants through the Control store
    // transition (same durable authority) to prove no legacy `grants/` leaf.
    let reviewed = reviewed_grant_operation(
        "operation:production-grant-commit",
        PluginOperationAction::Install,
        None,
        None,
    );
    let store = lifecycle.composition().store();
    store.register_operation(reviewed.clone()).await.unwrap();
    let committed = store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    assert_eq!(committed.snapshot.generation, 1);
    assert_eq!(committed.grants.len(), 1);
    assert!(
        !paths.installation_state_root().join("grants").exists(),
        "Control Grant commit must not materialize the legacy grants/ leaf"
    );
    reject_legacy_authority_paths(&paths.installation_state_root()).unwrap();

    let observed = lifecycle
        .observe_stored_workspace_grant(
            &committed.grants[0].grant.scope_id,
            &committed.grants[0].grant.package_id,
            &committed.grants[0].grant.package_digest,
        )
        .await
        .unwrap();
    assert!(observed.is_some());
}

#[tokio::test]
async fn production_control_blocks_file_grant_store_from_creating_grants_leaf() {
    use a3s_use_extension::WorkspaceGrantStore;

    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    let store = WorkspaceGrantStore::from_extension_paths(&paths);
    let error = store
        .snapshot_scope("workspace", 1)
        .await
        .expect_err("file Grant store must not lock beside Control");
    assert_eq!(error.code, "use.plugin.grant_store.control_authority_required");
    assert!(
        !paths.installation_state_root().join("grants").exists(),
        "failed file Grant open must not materialize grants/"
    );
    reject_legacy_authority_paths(&paths.installation_state_root()).unwrap();
}

#[tokio::test]
async fn production_effects_pending_survives_restart_and_resumes_without_generation_inflation() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let reviewed = super::aggregate_tests::fixtures::operation("operation:production-resume");
    {
        let lifecycle =
            ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
                .unwrap();
        lifecycle.initialize().await.unwrap();
        lifecycle
            .composition()
            .admit_and_commit_cognitive_package_operation_with_runtime_plans(
                &reviewed.envelope,
                &cognitive_authorization(&reviewed),
                None,
                reviewed.reviewed_at_ms,
                reviewed.reviewed_at_ms + 10,
                &[],
            )
            .await
            .unwrap();
        let pending = lifecycle
            .composition()
            .store()
            .operation(reviewed.operation_id())
            .await
            .unwrap()
            .expect("committed operation");
        assert_eq!(pending.status, ControlOperationStatus::EffectsPending);
        assert_eq!(
            lifecycle
                .current_snapshot()
                .await
                .unwrap()
                .expect("committed snapshot")
                .generation,
            1
        );
    }

    // Process restart: reopen Control from the same installation root.
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    let pending = lifecycle
        .composition()
        .store()
        .effects_pending_operation()
        .await
        .unwrap()
        .expect("effects-pending must survive restart");
    assert_eq!(pending.reviewed.operation_id(), reviewed.operation_id());
    assert_eq!(pending.status, ControlOperationStatus::EffectsPending);
    assert_eq!(
        lifecycle
            .current_snapshot()
            .await
            .unwrap()
            .expect("snapshot survives restart")
            .generation,
        1
    );

    let maintenance = std::sync::Arc::new(
        a3s_use_extension::StateMaintenanceLock::new(paths.state_root())
            .acquire_shared()
            .await
            .unwrap(),
    );
    // Fixture ports have no Artifact-backed package bytes, so effect owners
    // defer. Resume must still target the exact abandoned operation and must
    // not invent a second generation while fail-closed.
    let error = lifecycle
        .resume_pending_effects(maintenance)
        .await
        .expect_err("fixture without package bytes cannot complete drain");
    assert_eq!(
        error.code,
        "use.control_store.production_activation_invalid"
    );
    assert_eq!(
        error
            .details
            .get("operation_id")
            .and_then(|value| value.as_str()),
        Some(reviewed.operation_id())
    );
    let still_pending = lifecycle
        .composition()
        .store()
        .effects_pending_operation()
        .await
        .unwrap()
        .expect("failed resume leaves EffectsPending");
    assert_eq!(
        still_pending.reviewed.operation_id(),
        reviewed.operation_id()
    );
    assert_eq!(still_pending.status, ControlOperationStatus::EffectsPending);
    assert_eq!(
        lifecycle
            .current_snapshot()
            .await
            .unwrap()
            .expect("snapshot after failed resume")
            .generation,
        1
    );
    reject_legacy_authority_paths(&paths.installation_state_root()).unwrap();
}

#[tokio::test]
async fn coordinated_backup_uses_control_export_as_package_graph_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    let destination = temporary.path().join("control.a3s-use-state-backup");
    let manager = crate::state_backup::StateBackupManager::new(paths.clone());
    let manifest = manager.backup(&destination).await.unwrap();
    assert!(manifest.entries.iter().any(|entry| {
        entry.path == super::CONTROL_STORE_EXPORT_BACKUP_PATH
            && entry.family == crate::state_backup::StateBackupFamily::PackageGraph
    }));
    assert!(!manifest.entries.iter().any(|entry| {
        entry.path == "installation-snapshot.json" || entry.path == "control.sqlite3"
    }));
    assert!(manifest.authority.registry_digest.starts_with("sha256:"));
    let verified = crate::state_backup::StateBackupManager::verify_backup(&destination)
        .await
        .unwrap();
    assert_eq!(verified, manifest);

    std::fs::write(
        paths
            .installation_state_root()
            .join("installation-snapshot.json"),
        b"legacy",
    )
    .unwrap();
    let error = manager
        .backup(temporary.path().join("dual.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
}

#[tokio::test]
async fn control_state_restore_plans_against_control_export_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    let backup_path = temporary.path().join("control-restore.a3s-use-state-backup");
    let backup = crate::state_backup::StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    assert!(backup.authority.packages.is_empty());
    assert!(backup.entries.iter().any(|entry| {
        entry.path == super::CONTROL_STORE_EXPORT_BACKUP_PATH
            && entry.sha256 == backup.authority.registry_digest
    }));

    let plan = crate::state_restore::StateRestoreManager::new(paths)
        .plan_restore(&backup_path)
        .await
        .unwrap();
    // Control export is archive-injected, so live inventory differs from the
    // backup leaf set; planning must still validate Control export authority
    // without reading legacy registry.json / extensions/ receipts.
    assert_eq!(plan.backup.authority, backup.authority);
    assert!(plan.authority_digest.starts_with("sha256:"));
}

#[tokio::test]
async fn coordinated_backup_inventory_admits_only_control_export_and_registered_owners() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    // generation-leases is still listed by installation_state_layout but is not a
    // registered Control payload owner live location.
    let unregistered = paths.installation_state_root().join("generation-leases");
    std::fs::create_dir_all(&unregistered).unwrap();
    std::fs::write(unregistered.join("lease.json"), b"{}").unwrap();

    let manager = crate::state_backup::StateBackupManager::new(paths.clone());
    let error = manager
        .backup(temporary.path().join("unregistered.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_layout_unsupported");

    std::fs::remove_dir_all(&unregistered).unwrap();
    let destination = temporary.path().join("owners.a3s-use-state-backup");
    let manifest = manager.backup(&destination).await.unwrap();
    assert!(manifest.entries.iter().all(|entry| {
        entry.path == super::CONTROL_STORE_EXPORT_BACKUP_PATH
            || crate::control_store::backup_admits_control_installation_path(&entry.path, false)
    }));
    assert!(manifest.entries.iter().any(|entry| {
        entry.path == super::CONTROL_STORE_EXPORT_BACKUP_PATH
            && entry.family == crate::state_backup::StateBackupFamily::PackageGraph
    }));
}

#[tokio::test]
async fn production_observe_grant_fails_closed_on_legacy_grants_leaf() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let lifecycle =
        ProductionControlLifecycle::from_extension_paths(&paths, production_dependencies())
            .unwrap();
    lifecycle.initialize().await.unwrap();

    assert!(lifecycle
        .observe_stored_workspace_grant(
            "workspace",
            "acme/root",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .await
        .unwrap()
        .is_none());

    let grants = paths.installation_state_root().join("grants");
    std::fs::create_dir_all(&grants).unwrap();
    std::fs::write(grants.join("poison.json"), b"{}").unwrap();
    let error = lifecycle
        .observe_stored_workspace_grant(
            "workspace",
            "acme/root",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
}
