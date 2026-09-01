use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use a3s_use_core::{
    InstallationPackageSelection, LockedPluginPackage, OkfCapabilityProjection,
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt,
    PluginOperationAction, PluginPackageLockHost, PluginSurfaceKind, PluginSurfaceRef, UseError,
    UseResult, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionRegistry};

use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
use super::effect_port::{
    ControlEffectPortOutcome, ControlEffectRequestIdentity, ControlKnowledgeEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
};
use super::model::{
    ControlEffectIntent, ControlEffectKind, ControlEffectOwner, ControlEffectSubject,
    ControlPackageEffectAuthority,
};
use crate::okf_knowledge::{
    OkfKnowledgeAdapter, OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient,
    OkfKnowledgeSearchRequest, OkfKnowledgeSearchResponse, OkfKnowledgeStageRequest,
    SqliteOkfKnowledgeAdapter,
};
use crate::plugin_lifecycle::PluginLifecycleAction;

struct KnowledgeOwnerFixture {
    _temporary: tempfile::TempDir,
    paths: a3s_use_extension::ExtensionPaths,
    package_root: std::path::PathBuf,
    authority: ControlPackageEffectAuthority,
    bindings: OkfKnowledgeBindingStore,
    client: OkfKnowledgeClient,
    adapter: Arc<SqliteOkfKnowledgeAdapter>,
}

#[tokio::test]
async fn knowledge_prepare_replay_stop_and_remove_use_real_receipt_owned_projection() {
    let fixture = knowledge_owner_fixture().await;
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    );
    let prepare = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);

    let first = applied(&owner, &prepare).await;
    let second = applied(&owner, &prepare).await;
    let mut retried = prepare.clone();
    retried.identity.attempt = 2;
    retried.identity.deadline_at_ms = 30_000;
    let retried = applied(&owner, &retried).await;

    assert_eq!(first, second);
    assert_eq!(first, retried);
    let qualified = a3s_use_core::PlanQualifiedSurfaceRef {
        package_id: prepare.package_id.clone(),
        surface: prepare.surface.clone(),
    };
    let promoted = fixture
        .bindings
        .get(
            &prepare.identity.installation,
            &qualified,
            prepare.lifecycle_generation,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        promoted.observation.state,
        OkfKnowledgeObservedState::Promoted
    );
    let projection =
        OkfCapabilityProjection::from_promoted(&promoted.receipt, &promoted.observation).unwrap();
    assert_eq!(
        first.receipt_digest,
        promoted.observation.descriptor_digest().unwrap()
    );
    assert_eq!(
        first.materialization_digest.as_deref(),
        Some(projection.descriptor_digest().unwrap().as_str())
    );

    let search = fixture
        .client
        .search(
            &OkfKnowledgeSearchRequest::new(
                prepare.identity.installation.clone(),
                "runtime authority",
                5,
                vec![projection.clone()],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(!search.hits.is_empty());

    let stop = request(&fixture.authority, ControlSurfaceEffectAction::Stop);
    let first_stop = applied(&owner, &stop).await;
    let second_stop = applied(&owner, &stop).await;
    assert_eq!(first_stop, second_stop);
    assert!(first_stop.materialization_digest.is_none());
    assert!(!fixture
        .client
        .search(
            &OkfKnowledgeSearchRequest::new(
                prepare.identity.installation.clone(),
                "runtime authority",
                5,
                vec![projection.clone()],
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .hits
        .is_empty());

    let remove = request(&fixture.authority, ControlSurfaceEffectAction::Remove);
    let first_remove = applied(&owner, &remove).await;
    let second_remove = applied(&owner, &remove).await;
    assert_eq!(first_remove, second_remove);
    assert!(first_remove.materialization_digest.is_none());
    assert_eq!(
        fixture
            .bindings
            .get(
                &prepare.identity.installation,
                &qualified,
                prepare.lifecycle_generation,
            )
            .await
            .unwrap()
            .unwrap()
            .observation
            .state,
        OkfKnowledgeObservedState::Removed
    );
    let error = fixture
        .client
        .search(
            &OkfKnowledgeSearchRequest::new(
                prepare.identity.installation.clone(),
                "runtime authority",
                5,
                vec![projection],
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_projection_stale");
}

#[tokio::test]
async fn promoted_binding_replays_across_control_generations_without_artifact_access() {
    let fixture = knowledge_owner_fixture().await;
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    );
    let first = applied(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    std::fs::write(
        fixture
            .package_root
            .join("okf/domain-knowledge/concepts/runtime-boundary.md"),
        b"substituted after the receipt was retained",
    )
    .unwrap();
    let mut next_authority = fixture.authority.clone();
    next_authority.generation_operation_id = "operation:knowledge-reenable".to_string();
    next_authority.installation_generation = 2;
    next_authority.snapshot_digest = digest('5');
    next_authority.committed_at_ms = 2;

    let replay = applied(
        &owner,
        &request(&next_authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;

    assert_eq!(replay, first);
}

#[tokio::test]
async fn knowledge_prepare_rejects_artifact_tampering_before_external_effect() {
    let fixture = knowledge_owner_fixture().await;
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    );
    std::fs::write(
        fixture
            .package_root
            .join("okf/domain-knowledge/concepts/runtime-boundary.md"),
        b"substituted",
    )
    .unwrap();

    let outcome = ControlKnowledgeEffectPort::apply_surface(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;

    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("tampered immutable bytes must be rejected before Knowledge I/O");
    };
    assert_eq!(failure.error_code, "use.okf.bundle_invalid");
    assert!(!fixture.adapter.root().exists());
}

#[tokio::test]
async fn knowledge_prepare_defers_when_artifact_collection_proves_no_effect() {
    let fixture = knowledge_owner_fixture().await;
    let artifact_store = fixture.paths.artifact_store();
    let owner = ControlOkfKnowledgeEffectPort::new(
        artifact_store.clone(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    );
    let collection = artifact_store.acquire_collection().await.unwrap();

    let outcome = ControlKnowledgeEffectPort::apply_surface(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;

    drop(collection);
    let ControlEffectPortOutcome::Deferred(failure) = outcome else {
        panic!("Artifact contention must be a safe no-effect deferral");
    };
    assert_eq!(failure.error_code, "use.artifact_store.busy");
    assert!(!fixture.adapter.root().exists());
}

#[tokio::test]
async fn knowledge_owner_rejects_substituted_authority_before_external_effect() {
    let fixture = knowledge_owner_fixture().await;
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    );
    let mut wrong_package = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);
    let mut wrong_key = wrong_package.clone();
    let mut wrong_scope = wrong_package.clone();
    wrong_package.package_id = "acme/substituted".to_string();
    wrong_key.identity.idempotency_key = digest('9');
    wrong_scope.identity.installation =
        a3s_use_core::InstallationId::new(a3s_use_core::InstallationKind::User, "other").unwrap();

    for request in [wrong_package, wrong_key, wrong_scope] {
        let outcome = ControlKnowledgeEffectPort::apply_surface(&owner, &request).await;
        let ControlEffectPortOutcome::Rejected(failure) = outcome else {
            panic!("substituted Knowledge authority must fail before external I/O");
        };
        assert_eq!(
            failure.error_code,
            "use.control_store.knowledge_authority_invalid"
        );
    }
    assert!(!fixture.adapter.root().exists());
}

#[tokio::test]
async fn ambiguous_stage_is_unknown_and_same_key_reentry_converges() {
    let fixture = knowledge_owner_fixture().await;
    let ambiguous = Arc::new(AmbiguousStageAdapter {
        inner: (*fixture.adapter).clone(),
        fail_next_stage: AtomicBool::new(true),
        fail_next_promote: AtomicBool::new(false),
        fail_next_remove: AtomicBool::new(false),
    });
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        OkfKnowledgeClient::new(ambiguous),
        fixture.bindings.clone(),
    );
    let request = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);

    let first = ControlKnowledgeEffectPort::apply_surface(&owner, &request).await;

    let ControlEffectPortOutcome::Unknown(failure) = first else {
        panic!("an accepted stage followed by transport loss must remain unknown");
    };
    assert_eq!(failure.error_code, "use.test.knowledge_stage_ambiguous");
    assert!(fixture.adapter.root().exists());
    let qualified = a3s_use_core::PlanQualifiedSurfaceRef {
        package_id: request.package_id.clone(),
        surface: request.surface.clone(),
    };
    assert!(fixture
        .bindings
        .get(
            &request.identity.installation,
            &qualified,
            request.lifecycle_generation,
        )
        .await
        .unwrap()
        .is_none());

    let mut retry = request.clone();
    retry.identity.attempt = 2;
    retry.identity.deadline_at_ms = 30_000;
    let application = applied(&owner, &retry).await;
    assert!(application.materialization_digest.is_some());
    assert_eq!(
        fixture
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
            .unwrap()
            .unwrap()
            .observation
            .state,
        OkfKnowledgeObservedState::Promoted
    );
}

#[tokio::test]
async fn ambiguous_promotion_retains_staged_receipt_and_same_key_reentry_converges() {
    let fixture = knowledge_owner_fixture().await;
    let ambiguous = Arc::new(AmbiguousStageAdapter {
        inner: (*fixture.adapter).clone(),
        fail_next_stage: AtomicBool::new(false),
        fail_next_promote: AtomicBool::new(true),
        fail_next_remove: AtomicBool::new(false),
    });
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        OkfKnowledgeClient::new(ambiguous),
        fixture.bindings.clone(),
    );
    let request = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);
    let qualified = a3s_use_core::PlanQualifiedSurfaceRef {
        package_id: request.package_id.clone(),
        surface: request.surface.clone(),
    };

    let first = ControlKnowledgeEffectPort::apply_surface(&owner, &request).await;

    let ControlEffectPortOutcome::Unknown(failure) = first else {
        panic!("an accepted promotion followed by response loss must remain unknown");
    };
    assert_eq!(failure.error_code, "use.test.knowledge_promote_ambiguous");
    assert_eq!(
        fixture
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
            .unwrap()
            .unwrap()
            .observation
            .state,
        OkfKnowledgeObservedState::Staged
    );

    let mut retry = request.clone();
    retry.identity.attempt = 2;
    retry.identity.deadline_at_ms = 30_000;
    assert!(applied(&owner, &retry)
        .await
        .materialization_digest
        .is_some());
    assert_eq!(
        fixture
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
            .unwrap()
            .unwrap()
            .observation
            .state,
        OkfKnowledgeObservedState::Promoted
    );
}

#[tokio::test]
async fn ambiguous_removal_retains_receipt_and_same_key_reentry_converges() {
    let fixture = knowledge_owner_fixture().await;
    let initial = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    );
    applied(
        &initial,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    let ambiguous = Arc::new(AmbiguousStageAdapter {
        inner: (*fixture.adapter).clone(),
        fail_next_stage: AtomicBool::new(false),
        fail_next_promote: AtomicBool::new(false),
        fail_next_remove: AtomicBool::new(true),
    });
    let owner = ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        OkfKnowledgeClient::new(ambiguous),
        fixture.bindings.clone(),
    );
    let request = request(&fixture.authority, ControlSurfaceEffectAction::Remove);
    let qualified = a3s_use_core::PlanQualifiedSurfaceRef {
        package_id: request.package_id.clone(),
        surface: request.surface.clone(),
    };

    let first = ControlKnowledgeEffectPort::apply_surface(&owner, &request).await;

    let ControlEffectPortOutcome::Unknown(failure) = first else {
        panic!("an accepted removal followed by response loss must remain unknown");
    };
    assert_eq!(failure.error_code, "use.test.knowledge_remove_ambiguous");
    assert_eq!(
        fixture
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
            .unwrap()
            .unwrap()
            .observation
            .state,
        OkfKnowledgeObservedState::Promoted
    );

    let mut retry = request.clone();
    retry.identity.attempt = 2;
    retry.identity.deadline_at_ms = 30_000;
    assert!(applied(&owner, &retry)
        .await
        .materialization_digest
        .is_none());
    assert_eq!(
        fixture
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
            .unwrap()
            .unwrap()
            .observation
            .state,
        OkfKnowledgeObservedState::Removed
    );
}

struct AmbiguousStageAdapter {
    inner: SqliteOkfKnowledgeAdapter,
    fail_next_stage: AtomicBool,
    fail_next_promote: AtomicBool,
    fail_next_remove: AtomicBool,
}

#[async_trait::async_trait]
impl OkfKnowledgeAdapter for AmbiguousStageAdapter {
    async fn stage(&self, request: &OkfKnowledgeStageRequest) -> UseResult<OkfKnowledgeBinding> {
        let binding = self.inner.stage(request).await?;
        if self.fail_next_stage.swap(false, Ordering::SeqCst) {
            return Err(UseError::new(
                "use.test.knowledge_stage_ambiguous",
                "The stage was accepted before the simulated response was lost.",
            ));
        }
        Ok(binding)
    }

    async fn promote(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        let observation = self.inner.promote(receipt).await?;
        if self.fail_next_promote.swap(false, Ordering::SeqCst) {
            return Err(UseError::new(
                "use.test.knowledge_promote_ambiguous",
                "The promotion was accepted before the simulated response was lost.",
            ));
        }
        Ok(observation)
    }

    async fn observe(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        self.inner.observe(receipt).await
    }

    async fn remove(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        let observation = self.inner.remove(receipt).await?;
        if self.fail_next_remove.swap(false, Ordering::SeqCst) {
            return Err(UseError::new(
                "use.test.knowledge_remove_ambiguous",
                "The removal was accepted before the simulated response was lost.",
            ));
        }
        Ok(observation)
    }

    async fn search(
        &self,
        request: &OkfKnowledgeSearchRequest,
    ) -> UseResult<OkfKnowledgeSearchResponse> {
        self.inner.search(request).await
    }
}

async fn applied(
    owner: &ControlOkfKnowledgeEffectPort,
    request: &ControlSurfaceEffectRequest,
) -> ControlSurfaceApplication {
    let ControlEffectPortOutcome::Applied(application) =
        ControlKnowledgeEffectPort::apply_surface(owner, request).await
    else {
        panic!("the Knowledge owner must apply the valid fixture");
    };
    application
}

fn request(
    authority: &ControlPackageEffectAuthority,
    action: ControlSurfaceEffectAction,
) -> ControlSurfaceEffectRequest {
    let lifecycle_action = match action {
        ControlSurfaceEffectAction::Prepare => PluginLifecycleAction::Install,
        ControlSurfaceEffectAction::Stop => PluginLifecycleAction::Disable,
        ControlSurfaceEffectAction::Remove => PluginLifecycleAction::Uninstall,
    };
    let operation_action = match action {
        ControlSurfaceEffectAction::Prepare => PluginOperationAction::Install,
        ControlSurfaceEffectAction::Stop => PluginOperationAction::Disable,
        ControlSurfaceEffectAction::Remove => PluginOperationAction::Uninstall,
    };
    let surface = PluginSurfaceRef {
        kind: PluginSurfaceKind::Okf,
        id: "domain-knowledge".to_string(),
    };
    let package_id = authority.package.package_id().to_string();
    let package_digest = authority
        .package
        .package
        .catalog
        .record
        .package
        .sha256
        .clone()
        .unwrap();
    let manifest_digest = authority
        .package
        .package
        .catalog
        .record
        .package
        .manifest_sha256
        .clone()
        .unwrap();
    let installation = crate::test_installation();
    let plan_digest = digest('1');
    let intent = ControlEffectIntent::new(
        0,
        installation.clone(),
        plan_digest.clone(),
        operation_action,
        authority.installation_generation,
        ControlEffectSubject::Surface {
            package_id: package_id.clone(),
            lifecycle_generation: authority.lifecycle_generation,
            package_digest: package_digest.clone(),
            manifest_digest: manifest_digest.clone(),
            action: lifecycle_action,
            surface: surface.clone(),
        },
        ControlEffectOwner::KnowledgeHost,
        match action {
            ControlSurfaceEffectAction::Prepare => ControlEffectKind::SurfacePrepare,
            ControlSurfaceEffectAction::Stop => ControlEffectKind::SurfaceStop,
            ControlSurfaceEffectAction::Remove => ControlEffectKind::SurfaceRemove,
        },
        true,
    )
    .unwrap();
    ControlSurfaceEffectRequest {
        identity: ControlEffectRequestIdentity {
            operation_id: authority.generation_operation_id.clone(),
            installation,
            plan_digest,
            operation_action,
            installation_generation: authority.installation_generation,
            sequence: 0,
            idempotency_key: intent.idempotency_key,
            required: true,
            attempt: 1,
            deadline_at_ms: 20_000,
        },
        authority: authority.clone(),
        package_id,
        lifecycle_generation: authority.lifecycle_generation,
        package_digest,
        manifest_digest,
        lifecycle_action,
        surface,
        action,
    }
}

async fn knowledge_owner_fixture() -> KnowledgeOwnerFixture {
    let temporary = tempfile::tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/knowledge", &source, true)
        .await
        .unwrap();
    let catalog = verified_catalog(&candidate);
    let identity = ExtensionLifecycleIdentity::new(
        candidate.package_id(),
        candidate.package_digest(),
        candidate.manifest_digest(),
        1,
    )
    .unwrap();
    let paths = crate::test_extension_paths(temporary.path());
    let registry = ExtensionRegistry::new(paths.clone());
    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    let selected_surfaces = candidate
        .manifest()
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface)
        .collect();
    let package = InstallationPackageSelection::new(
        LockedPluginPackage {
            catalog,
            dependencies: Vec::new(),
        },
        1,
        true,
        selected_surfaces,
    )
    .unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(&paths));
    KnowledgeOwnerFixture {
        package_root: committed.extension.receipt.package_root,
        authority: ControlPackageEffectAuthority {
            generation_operation_id: "operation:knowledge-owner".to_string(),
            installation_generation: 1,
            snapshot_digest: digest('3'),
            committed_at_ms: 1,
            host: PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
            package,
            lifecycle_generation: 1,
            grant: None,
        },
        bindings: OkfKnowledgeBindingStore::from_extension_paths(&paths),
        client: OkfKnowledgeClient::new(adapter.clone()),
        adapter,
        paths,
        _temporary: temporary,
    }
}

fn verified_catalog(candidate: &ExtensionLifecyclePackage) -> VerifiedPluginCatalogRecord {
    let mut record = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    record.package.expanded_bytes = candidate.expanded_bytes();
    record.package.file_count = candidate.file_count();
    record.package.sha256 = Some(candidate.package_digest().to_string());
    record.package.manifest_sha256 = Some(candidate.manifest_digest().to_string());
    let provenance = VerifiedCatalogProvenance {
        registry_name: "fixture".to_string(),
        registry_url: "https://packages.example.test/catalog/".to_string(),
        root_sha256: digest('4'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
