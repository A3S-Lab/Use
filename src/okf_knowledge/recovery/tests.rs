use std::path::{Path, PathBuf};
use std::sync::Arc;

use a3s_use_core::{PlanQualifiedSurfaceRef, PlanScopeKind, PluginSurfaceKind, PluginSurfaceRef};
use a3s_use_extension::{
    load_okf_bundle_files, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
};

use super::*;
use crate::okf_knowledge::{OkfKnowledgeClient, OkfKnowledgeStageRequest, OkfKnowledgeStageSpec};
use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleCheckpointOutcome, PluginLifecycleIntent,
    PluginLifecycleIntentSpec,
};

const RESTORE_CHILD_ROOT_ENV: &str = "A3S_USE_TEST_OKF_RESTORE_ROOT";
const RESTORE_CHILD_BACKUP_ENV: &str = "A3S_USE_TEST_OKF_RESTORE_BACKUP";
const RESTORE_CHILD_PLAN_DIGEST_ENV: &str = "A3S_USE_TEST_OKF_RESTORE_PLAN_DIGEST";
const RESTORE_CRASH_EXIT_CODE: i32 = 86;

#[tokio::test]
async fn restore_plan_binds_backup_live_database_and_complete_authority() {
    let fixture = RestoreFixture::complete().await;
    let backup = fixture.root.join("knowledge.a3s-okf-backup");
    fixture
        .adapter
        .backup(&fixture.scope, &backup)
        .await
        .unwrap();
    fixture.corrupt_derived_index();

    let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths);
    let plan = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    assert_eq!(plan.schema, OKF_KNOWLEDGE_RESTORE_PLAN_SCHEMA);
    assert_eq!(plan.scope, fixture.scope);
    assert_eq!(plan.status, OkfKnowledgeRestorePlanStatus::Required);
    assert_eq!(plan.retained_projections, 1);
    assert_eq!(plan.removed_tombstones, 0);
    assert_eq!(plan.selected_projections, 1);
    assert!(plan.database_before.is_some());
    assert!(plan.authority_digest.starts_with("sha256:"));
    assert!(plan.descriptor_digest().unwrap().starts_with("sha256:"));

    let replay = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    assert_eq!(replay, plan);
}

#[tokio::test]
async fn restore_plan_recovers_missing_binding_from_independent_authority() {
    let missing_binding = RestoreFixture::without_binding().await;
    let backup = missing_binding.root.join("missing-binding.a3s-okf-backup");
    missing_binding
        .adapter
        .backup(&missing_binding.scope, &backup)
        .await
        .unwrap();
    let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&missing_binding.paths);
    let plan = manager
        .plan_restore(&missing_binding.scope, &backup)
        .await
        .unwrap();
    assert_eq!(plan.status, OkfKnowledgeRestorePlanStatus::Required);
    assert_eq!(plan.missing_bindings, 1);
    let plan_digest = plan.descriptor_digest().unwrap();
    let result = manager
        .apply_restore(&missing_binding.scope, &backup, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.restored_bindings, 1);
    let bindings = manager
        .bindings
        .list_scope(&missing_binding.scope)
        .await
        .unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        bindings[0].observation.state,
        OkfKnowledgeObservedState::Promoted
    );
    missing_binding
        .adapter
        .audit(&missing_binding.scope)
        .await
        .unwrap();
}

#[tokio::test]
async fn restore_plan_rejects_nonterminal_or_missing_independent_authority() {
    let applying = RestoreFixture::without_binding_applying().await;
    let backup = applying.root.join("applying.a3s-okf-backup");
    applying
        .adapter
        .backup(&applying.scope, &backup)
        .await
        .unwrap();
    let error = OkfKnowledgeRecoveryManager::from_extension_paths(&applying.paths)
        .plan_restore(&applying.scope, &backup)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_lifecycle_active");

    let unpublished = RestoreFixture::without_binding_without_publication().await;
    let backup = unpublished
        .root
        .join("missing-binding-unpublished.a3s-okf-backup");
    unpublished
        .adapter
        .backup(&unpublished.scope, &backup)
        .await
        .unwrap();
    let error = OkfKnowledgeRecoveryManager::from_extension_paths(&unpublished.paths)
        .plan_restore(&unpublished.scope, &backup)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_registry_mismatch");
}

#[tokio::test]
async fn restore_plan_rejects_staged_or_unpublished_projection_authority() {
    let staged = RestoreFixture::staged().await;
    let backup = staged.root.join("staged.a3s-okf-backup");
    staged.adapter.backup(&staged.scope, &backup).await.unwrap();
    let error = OkfKnowledgeRecoveryManager::from_extension_paths(&staged.paths)
        .plan_restore(&staged.scope, &backup)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_nonterminal");

    let unpublished = RestoreFixture::complete_without_publication().await;
    let backup = unpublished.root.join("unpublished.a3s-okf-backup");
    unpublished
        .adapter
        .backup(&unpublished.scope, &backup)
        .await
        .unwrap();
    let error = OkfKnowledgeRecoveryManager::from_extension_paths(&unpublished.paths)
        .plan_restore(&unpublished.scope, &backup)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_registry_mismatch");
}

#[tokio::test]
async fn restore_plan_rejects_a_conflicting_current_binding() {
    let fixture = RestoreFixture::complete().await;
    let backup = fixture.root.join("conflicting-binding.a3s-okf-backup");
    fixture
        .adapter
        .backup(&fixture.scope, &backup)
        .await
        .unwrap();
    let store = OkfKnowledgeBindingStore::from_extension_paths(&fixture.paths);
    let mut binding = store
        .list_scope(&fixture.scope)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    binding.observation.observed_at_ms += 1;
    binding.validate().unwrap();
    let scope_digest = format!("{:x}", Sha256::digest(fixture.scope.id.as_bytes()));
    let path = store
        .root()
        .join("workspace")
        .join(scope_digest)
        .join("acme")
        .join("knowledge")
        .join("okf-domain-knowledge")
        .join(format!("{:020}.json", binding.receipt.generation));
    std::fs::write(path, serde_json::to_vec_pretty(&binding).unwrap()).unwrap();

    let error = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths)
        .plan_restore(&fixture.scope, &backup)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_binding_conflict");
}

#[tokio::test]
async fn restore_apply_publishes_verified_database_and_terminal_replay_is_read_only() {
    let fixture = RestoreFixture::complete().await;
    let backup = fixture.root.join("apply.a3s-okf-backup");
    fixture
        .adapter
        .backup(&fixture.scope, &backup)
        .await
        .unwrap();
    fixture.corrupt_derived_index();

    let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths);
    let plan = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    assert_eq!(plan.status, OkfKnowledgeRestorePlanStatus::Required);
    let plan_digest = plan.descriptor_digest().unwrap();
    let result = manager
        .apply_restore(&fixture.scope, &backup, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.plan_digest, plan_digest);
    assert!(result.preserved_prior_files >= 1);
    assert_eq!(result.database_after.sha256, plan.backup.database_sha256);
    fixture.adapter.audit(&fixture.scope).await.unwrap();

    let same_id_other_kind = a3s_use_core::PlanScope {
        kind: PlanScopeKind::User,
        id: fixture.scope.id.clone(),
    };
    let isolated = manager
        .diagnose_restores(&same_id_other_kind)
        .await
        .unwrap();
    assert!(isolated.active.is_none());
    assert!(isolated.operations.is_empty());
    assert_eq!(isolated.retained_operation_directories, 0);

    let no_change = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    assert_eq!(no_change.status, OkfKnowledgeRestorePlanStatus::NoChange);
    let no_change_digest = no_change.descriptor_digest().unwrap();
    let no_change_result = manager
        .apply_restore(&fixture.scope, &backup, &no_change_digest)
        .await
        .unwrap();
    assert!(!no_change_result.changed);
    assert_eq!(
        no_change_result.database_before,
        Some(no_change_result.database_after)
    );

    let paths = manager
        .operations
        .paths(&fixture.scope, &plan_digest)
        .unwrap();
    assert!(paths.prior_database.is_file());
    assert!(!fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
    let database = scope_database_path(&fixture.adapter, &fixture.scope);
    let before_replay = [
        std::fs::read(&paths.journal).unwrap(),
        std::fs::read(&paths.prior_database).unwrap(),
        std::fs::read(&database).unwrap(),
    ];

    std::fs::remove_file(&backup).unwrap();
    let replay = manager
        .apply_restore(&fixture.scope, &backup, &plan_digest)
        .await
        .unwrap();
    assert_eq!(replay, result);
    let after_replay = [
        std::fs::read(&paths.journal).unwrap(),
        std::fs::read(&paths.prior_database).unwrap(),
        std::fs::read(&database).unwrap(),
    ];
    assert_eq!(after_replay, before_replay);
}

#[tokio::test]
async fn restore_apply_requires_exact_review_and_revalidates_authority() {
    let fixture = RestoreFixture::complete().await;
    let backup = fixture.root.join("reviewed.a3s-okf-backup");
    fixture
        .adapter
        .backup(&fixture.scope, &backup)
        .await
        .unwrap();
    fixture.corrupt_derived_index();
    let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths);
    let plan = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let error = manager
        .apply_restore(
            &fixture.scope,
            &backup,
            &format!("sha256:{}", "0".repeat(64)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_plan_mismatch");
    assert!(!fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());

    let scope_digest = format!("{:x}", Sha256::digest(fixture.scope.id.as_bytes()));
    let binding = OkfKnowledgeBindingStore::from_extension_paths(&fixture.paths)
        .root()
        .join("workspace")
        .join(scope_digest)
        .join("acme")
        .join("knowledge")
        .join("okf-domain-knowledge")
        .join(format!("{:020}.json", 7));
    std::fs::remove_file(binding).unwrap();
    let error = manager
        .apply_restore(&fixture.scope, &backup, &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_restore_plan_mismatch");
    assert!(!fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
}

#[tokio::test]
async fn restore_apply_recreates_a_missing_scope_database_from_exact_authority() {
    let fixture = RestoreFixture::complete().await;
    let backup = fixture.root.join("missing-database.a3s-okf-backup");
    fixture
        .adapter
        .backup(&fixture.scope, &backup)
        .await
        .unwrap();
    let database = scope_database_path(&fixture.adapter, &fixture.scope);
    std::fs::remove_file(&database).unwrap();
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = database.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            std::fs::remove_file(sidecar).unwrap();
        }
    }

    let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths);
    let plan = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    assert_eq!(plan.status, OkfKnowledgeRestorePlanStatus::Required);
    assert!(plan.database_before.is_none());
    let plan_digest = plan.descriptor_digest().unwrap();
    let result = manager
        .apply_restore(&fixture.scope, &backup, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.preserved_prior_files, 0);
    assert!(database.is_file());
    fixture.adapter.audit(&fixture.scope).await.unwrap();
}

#[tokio::test]
async fn every_restore_checkpoint_recovers_after_process_exit() {
    for checkpoint in [
        "marker-active",
        "planned",
        "staged",
        "bindings-restored",
        "prior-wal-moved",
        "prior-shm-moved",
        "prior-database-moved",
        "prior-moved",
        "published",
        "completed",
    ] {
        let fixture = RestoreFixture::complete().await;
        let backup = fixture
            .root
            .join(format!("checkpoint-{checkpoint}.a3s-okf-backup"));
        fixture
            .adapter
            .backup(&fixture.scope, &backup)
            .await
            .unwrap();
        if checkpoint.starts_with("prior-") && checkpoint != "prior-moved" {
            fixture.corrupt_with_sidecars();
        } else {
            fixture.corrupt_derived_index();
        }
        let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths);
        let plan = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
        let plan_digest = plan.descriptor_digest().unwrap();
        let registry_before = manager.registry.snapshot().await.unwrap();

        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .arg("restore_checkpoint_crash_child")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(RESTORE_CHILD_ROOT_ENV, &fixture.root)
            .env(RESTORE_CHILD_BACKUP_ENV, &backup)
            .env(RESTORE_CHILD_PLAN_DIGEST_ENV, &plan_digest)
            .env(RESTORE_CRASH_CHECKPOINT_ENV, checkpoint)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(RESTORE_CRASH_EXIT_CODE),
            "restore child did not exit at {checkpoint}: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let evidence_before_diagnostic = snapshot_regular_files(fixture.paths.state_root());
        let diagnostic = manager.diagnose_restores(&fixture.scope).await.unwrap();
        assert_eq!(
            snapshot_regular_files(fixture.paths.state_root()),
            evidence_before_diagnostic,
            "restore-status changed durable state at {checkpoint}"
        );
        assert_eq!(diagnostic.schema, OKF_KNOWLEDGE_RESTORE_DIAGNOSTIC_SCHEMA);
        assert_eq!(diagnostic.scope, fixture.scope);
        assert_eq!(diagnostic.retention_limit, 32);
        assert_eq!(diagnostic.retained_operation_directories, 1);
        assert_eq!(
            diagnostic.unrecorded_operation_directories,
            usize::from(checkpoint == "marker-active")
        );
        assert_eq!(
            diagnostic.operations.len(),
            usize::from(checkpoint != "marker-active")
        );
        assert_eq!(
            diagnostic.retention_remaining + diagnostic.retained_operation_directories,
            diagnostic.retention_limit
        );
        let active = diagnostic.active.as_ref().unwrap();
        assert_eq!(active.scope, fixture.scope);
        assert_eq!(active.plan_digest, plan_digest);
        assert_eq!(
            active.status,
            match checkpoint {
                "marker-active" | "planned" => {
                    OkfKnowledgeRestoreOperationDiagnosticStatus::Planned
                }
                "staged" => OkfKnowledgeRestoreOperationDiagnosticStatus::Staged,
                "bindings-restored"
                | "prior-wal-moved"
                | "prior-shm-moved"
                | "prior-database-moved" => {
                    OkfKnowledgeRestoreOperationDiagnosticStatus::BindingsRestored
                }
                "prior-moved" => OkfKnowledgeRestoreOperationDiagnosticStatus::PriorMoved,
                "published" => OkfKnowledgeRestoreOperationDiagnosticStatus::Published,
                "completed" => OkfKnowledgeRestoreOperationDiagnosticStatus::Completed,
                _ => unreachable!(),
            }
        );

        let blocked = fixture.adapter.audit(&fixture.scope).await.unwrap_err();
        assert_eq!(blocked.code, "use.state.maintenance_restore_active");
        let blocked = manager
            .plan_restore(&fixture.scope, &backup)
            .await
            .unwrap_err();
        assert_eq!(blocked.code, "use.okf.knowledge_restore_in_progress");
        let binding_store = OkfKnowledgeBindingStore::from_extension_paths(&fixture.paths);
        let binding = binding_store
            .list_scope(&fixture.scope)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let blocked = binding_store.put(&binding).await.unwrap_err();
        assert_eq!(blocked.code, "use.state.maintenance_restore_active");

        let installed = manager
            .registry
            .get("acme/knowledge")
            .await
            .unwrap()
            .unwrap();
        let identity = ExtensionLifecycleIdentity::new(
            &installed.receipt.package_id,
            format!(
                "sha256:{}",
                installed.receipt.package_sha256.as_deref().unwrap()
            ),
            format!("sha256:{}", installed.receipt.manifest_sha256),
            installed.receipt.lifecycle_generation.unwrap(),
        )
        .unwrap();
        let blocked = manager
            .registry
            .hide_lifecycle_package(&identity)
            .await
            .unwrap_err();
        assert_eq!(blocked.code, "use.state.maintenance_restore_active");

        let lifecycle = manager
            .lifecycle
            .load_active(&fixture.scope, "acme/knowledge")
            .await
            .unwrap()
            .unwrap();
        let blocked = manager
            .lifecycle
            .record_failure(
                &lifecycle.intent,
                &lifecycle.intent.checkpoints[0].idempotency_key,
                "use.plugin.restore_test_blocked",
                format!("sha256:{}", "9".repeat(64)),
                30_000,
            )
            .await
            .unwrap_err();
        assert_eq!(blocked.code, "use.state.maintenance_restore_active");
        #[cfg(unix)]
        if checkpoint == "planned" {
            use std::os::unix::fs::symlink;

            let operation_paths = manager
                .operations
                .paths(&fixture.scope, &plan_digest)
                .unwrap();
            symlink(&backup, &operation_paths.candidate).unwrap();
            let error = manager
                .apply_restore(&fixture.scope, &backup, &plan_digest)
                .await
                .unwrap_err();
            assert_eq!(error.code, "use.okf.knowledge_restore_filesystem_invalid");
            std::fs::remove_file(&operation_paths.candidate).unwrap();
        }
        if !matches!(checkpoint, "marker-active" | "planned") {
            std::fs::remove_file(&backup).unwrap();
        }
        let result = manager
            .apply_restore(&fixture.scope, &backup, &plan_digest)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "failed to recover checkpoint {checkpoint}: {} ({})",
                    error.message, error.code
                )
            });
        assert!(result.changed);
        assert_eq!(result.plan_digest, plan_digest);
        assert_eq!(manager.registry.snapshot().await.unwrap(), registry_before);
        fixture.adapter.audit(&fixture.scope).await.unwrap();
        assert!(!fixture
            .paths
            .state_root()
            .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
            .exists());

        let operation = manager
            .operations
            .load(&fixture.scope, &plan_digest)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(operation.status, RestoreOperationStatus::Completed);
        let diagnostic = manager.diagnose_restores(&fixture.scope).await.unwrap();
        assert!(diagnostic.active.is_none());
        assert_eq!(diagnostic.operations.len(), 1);
        assert_eq!(diagnostic.operations[0].plan_digest, plan_digest);
        assert_eq!(
            diagnostic.operations[0].status,
            OkfKnowledgeRestoreOperationDiagnosticStatus::Completed
        );
        assert_eq!(diagnostic.retained_operation_directories, 1);
        assert_eq!(diagnostic.retention_remaining, 31);
    }
}

#[tokio::test]
async fn missing_binding_restore_recovers_after_binding_file_process_exit() {
    let fixture = RestoreFixture::without_binding().await;
    let backup = fixture.root.join("binding-file-exit.a3s-okf-backup");
    fixture
        .adapter
        .backup(&fixture.scope, &backup)
        .await
        .unwrap();
    let manager = OkfKnowledgeRecoveryManager::from_extension_paths(&fixture.paths);
    let plan = manager.plan_restore(&fixture.scope, &backup).await.unwrap();
    assert_eq!(plan.missing_bindings, 1);
    let plan_digest = plan.descriptor_digest().unwrap();

    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .arg("restore_checkpoint_crash_child")
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(RESTORE_CHILD_ROOT_ENV, &fixture.root)
        .env(RESTORE_CHILD_BACKUP_ENV, &backup)
        .env(RESTORE_CHILD_PLAN_DIGEST_ENV, &plan_digest)
        .env(RESTORE_CRASH_CHECKPOINT_ENV, "binding-file-restored")
        .output()
        .await
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(RESTORE_CRASH_EXIT_CODE),
        "binding recovery child did not exit: status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        manager
            .bindings
            .list_scope(&fixture.scope)
            .await
            .unwrap()
            .len(),
        1
    );
    let diagnostic = manager.diagnose_restores(&fixture.scope).await.unwrap();
    let active = diagnostic.active.unwrap();
    assert_eq!(
        active.status,
        OkfKnowledgeRestoreOperationDiagnosticStatus::Staged
    );
    assert_eq!(active.missing_bindings, 1);

    std::fs::remove_file(&backup).unwrap();
    let result = manager
        .apply_restore(&fixture.scope, &backup, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.restored_bindings, 1);
    fixture.adapter.audit(&fixture.scope).await.unwrap();
    assert!(!fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
}

#[tokio::test]
#[ignore = "subprocess helper for Knowledge restore checkpoint crash injection"]
async fn restore_checkpoint_crash_child() {
    let Some(root) = std::env::var_os(RESTORE_CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let backup = PathBuf::from(
        std::env::var_os(RESTORE_CHILD_BACKUP_ENV)
            .expect("restore checkpoint child backup is missing"),
    );
    let plan_digest = std::env::var(RESTORE_CHILD_PLAN_DIGEST_ENV)
        .expect("restore checkpoint child plan digest is missing");
    let paths = a3s_use_extension::ExtensionPaths::new(root.join("data"), root.join("state"));
    let scope = a3s_use_core::PlanScope {
        kind: PlanScopeKind::Workspace,
        id: "restore-workspace".to_owned(),
    };
    let outcome = OkfKnowledgeRecoveryManager::from_extension_paths(&paths)
        .apply_restore(&scope, backup, &plan_digest)
        .await;
    panic!("restore checkpoint child completed without exiting: {outcome:?}");
}

struct RestoreFixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    paths: a3s_use_extension::ExtensionPaths,
    scope: a3s_use_core::PlanScope,
    adapter: Arc<SqliteOkfKnowledgeAdapter>,
}

impl RestoreFixture {
    async fn complete() -> Self {
        Self::build(true, true, true, true).await
    }

    async fn without_binding() -> Self {
        Self::build(false, true, true, true).await
    }

    async fn without_binding_without_publication() -> Self {
        Self::build(false, true, true, false).await
    }

    async fn without_binding_applying() -> Self {
        Self::build(false, true, false, true).await
    }

    async fn staged() -> Self {
        Self::build(true, false, true, true).await
    }

    async fn complete_without_publication() -> Self {
        Self::build(true, true, true, false).await
    }

    async fn build(
        persist_binding: bool,
        promote: bool,
        complete_journal: bool,
        publish: bool,
    ) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().to_path_buf();
        let paths = a3s_use_extension::ExtensionPaths::new(root.join("data"), root.join("state"));
        let registry = a3s_use_extension::ExtensionRegistry::new(paths.clone());
        let package_root = fixture_package_root();
        let candidate =
            ExtensionLifecyclePackage::prepare_local("acme/knowledge", &package_root, true)
                .await
                .unwrap();
        let generation = 7;
        let identity = ExtensionLifecycleIdentity::new(
            candidate.package_id(),
            candidate.package_digest(),
            candidate.manifest_digest(),
            generation,
        )
        .unwrap();
        let scope = a3s_use_core::PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "restore-workspace".to_owned(),
        };
        let surface = candidate.manifest().okf[0].clone();
        let files = load_okf_bundle_files(&surface, &package_root)
            .await
            .unwrap();
        let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(&paths));
        let client = OkfKnowledgeClient::new(adapter.clone());
        let staged = client
            .stage(
                OkfKnowledgeStageRequest::new(
                    OkfKnowledgeStageSpec {
                        operation_id: "restore-fixture-install".to_owned(),
                        scope: scope.clone(),
                        surface: PlanQualifiedSurfaceRef {
                            package_id: candidate.package_id().to_owned(),
                            surface: PluginSurfaceRef {
                                kind: PluginSurfaceKind::Okf,
                                id: surface.id.clone(),
                            },
                        },
                        generation,
                        package_digest: candidate.package_digest().to_owned(),
                        manifest_digest: candidate.manifest_digest().to_owned(),
                        bundle: surface.bundle,
                    },
                    files,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let binding = if promote {
            client.promote(&staged.receipt).await.unwrap()
        } else {
            staged
        };
        if persist_binding {
            OkfKnowledgeBindingStore::from_extension_paths(&paths)
                .put(&binding)
                .await
                .unwrap();
        }

        registry
            .commit_lifecycle_package(&identity, &candidate)
            .await
            .unwrap();
        if publish {
            let cutover = format!("sha256:{}", "7".repeat(64));
            registry
                .publish_lifecycle_package_with_durable_cutover(&identity, &cutover)
                .await
                .unwrap();
            registry.complete_lifecycle_cutover(&cutover).await.unwrap();
        }

        let intent = PluginLifecycleIntent::from_manifest(
            PluginLifecycleIntentSpec {
                operation_id: "restore-fixture-install".to_owned(),
                plan_digest: format!("sha256:{}", "8".repeat(64)),
                scope: scope.clone(),
                package_id: candidate.package_id().to_owned(),
                package_digest: candidate.package_digest().to_owned(),
                manifest_digest: candidate.manifest_digest().to_owned(),
                generation,
                action: PluginLifecycleAction::Install,
                retained_ui_state_surfaces: Vec::new(),
            },
            candidate.manifest(),
        )
        .unwrap();
        let journal =
            crate::plugin_lifecycle::PluginLifecycleJournalStore::from_extension_paths(&paths);
        journal.begin(&intent).await.unwrap();
        if complete_journal {
            for (index, checkpoint) in intent.checkpoints.iter().enumerate() {
                journal
                    .record_checkpoint(
                        &intent,
                        &checkpoint.idempotency_key,
                        PluginLifecycleCheckpointOutcome::Applied,
                        format!("sha256:{:064x}", index + 1),
                        None,
                        10_000 + u64::try_from(index).unwrap(),
                    )
                    .await
                    .unwrap();
            }
            journal.complete(&intent, 20_000).await.unwrap();
        }

        Self {
            _temporary: temporary,
            root,
            paths,
            scope,
            adapter,
        }
    }

    fn corrupt_derived_index(&self) {
        let path = scope_database_path(&self.adapter, &self.scope);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute("DELETE FROM knowledge_documents_fts", [])
            .unwrap();
    }

    fn corrupt_with_sidecars(&self) {
        let path = scope_database_path(&self.adapter, &self.scope);
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let mut shm = path.as_os_str().to_os_string();
        shm.push("-shm");
        std::fs::write(PathBuf::from(wal), b"retained invalid WAL evidence").unwrap();
        std::fs::write(PathBuf::from(shm), b"retained invalid SHM evidence").unwrap();
    }
}

fn scope_database_path(
    adapter: &SqliteOkfKnowledgeAdapter,
    scope: &a3s_use_core::PlanScope,
) -> PathBuf {
    adapter
        .scope_directory(scope)
        .unwrap()
        .join("knowledge.sqlite3")
}

fn fixture_package_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package")
}

fn snapshot_regular_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, files);
            } else if metadata.is_file() {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                ));
            } else {
                panic!("unexpected restore test state entry: {}", path.display());
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}
