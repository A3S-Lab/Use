use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use a3s_use_core::{
    InstallationId, InstallationPackageSelection, LockedPluginPackage, OkfKnowledgeObservation,
    OkfProjectionReceipt, PluginOperationAction, PluginPackageLockHost, PluginSurfaceKind,
    PluginSurfaceRef, UseError, UseResult, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
};
use a3s_use_extension::ExtensionLifecyclePackage;

use super::dispatcher::ControlEffectPorts;
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
use super::effect_port::{
    ControlCapabilityCutoverRequest, ControlCapabilityIndexEffectPort, ControlEffectPortOutcome,
    ControlEffectRequestIdentity, ControlFlowEffectPort, ControlInvocationDrainRequest,
    ControlInvocationLeaseEffectPort, ControlKnowledgeEffectPort, ControlReceiptApplication,
    ControlRuntimeApplication, ControlRuntimeEffectPort, ControlRuntimeEffectRequest,
    ControlSkillEffectPort, ControlSurfaceApplication, ControlSurfaceEffectAction,
    ControlSurfaceEffectRequest, ControlUiEffectPort,
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

pub(super) struct KnowledgeOwnerFixture {
    _temporary: tempfile::TempDir,
    pub(super) paths: a3s_use_extension::ExtensionPaths,
    pub(super) package_root: std::path::PathBuf,
    pub(super) authority: ControlPackageEffectAuthority,
    pub(super) bindings: OkfKnowledgeBindingStore,
    pub(super) client: OkfKnowledgeClient,
    pub(super) adapter: Arc<SqliteOkfKnowledgeAdapter>,
}

pub(super) struct AmbiguousStageAdapter {
    pub(super) inner: SqliteOkfKnowledgeAdapter,
    pub(super) fail_next_stage: AtomicBool,
    pub(super) fail_next_promote: AtomicBool,
    pub(super) fail_next_remove: AtomicBool,
}

struct UnexpectedEffectPort;

fn unexpected_effect<T>() -> ControlEffectPortOutcome<T> {
    panic!("the committed Knowledge fixture must not route to another effect owner")
}

#[async_trait::async_trait]
impl ControlCapabilityIndexEffectPort for UnexpectedEffectPort {
    async fn cutover(
        &self,
        _request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        unexpected_effect()
    }
}

#[async_trait::async_trait]
impl ControlInvocationLeaseEffectPort for UnexpectedEffectPort {
    async fn drain(
        &self,
        _request: &ControlInvocationDrainRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        unexpected_effect()
    }
}

#[async_trait::async_trait]
impl ControlRuntimeEffectPort for UnexpectedEffectPort {
    async fn apply_surface(
        &self,
        _request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        unexpected_effect()
    }
}

#[async_trait::async_trait]
impl ControlFlowEffectPort for UnexpectedEffectPort {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        unexpected_effect()
    }
}

#[async_trait::async_trait]
impl ControlSkillEffectPort for UnexpectedEffectPort {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        unexpected_effect()
    }
}

#[async_trait::async_trait]
impl ControlUiEffectPort for UnexpectedEffectPort {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        unexpected_effect()
    }
}

pub(super) fn control_ports_with_knowledge(
    knowledge: Arc<dyn ControlKnowledgeEffectPort>,
) -> ControlEffectPorts {
    let unexpected = Arc::new(UnexpectedEffectPort);
    ControlEffectPorts::new(
        unexpected.clone(),
        unexpected.clone(),
        unexpected.clone(),
        unexpected.clone(),
        knowledge,
        unexpected.clone(),
        unexpected,
    )
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

pub(super) async fn applied(
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

pub(super) fn request(
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

pub(super) async fn knowledge_owner_fixture() -> KnowledgeOwnerFixture {
    let (fixture, artifact_admission) =
        knowledge_owner_fixture_for(crate::test_installation()).await;
    drop(artifact_admission);
    fixture
}

pub(super) async fn knowledge_owner_fixture_for(
    installation: InstallationId,
) -> (
    KnowledgeOwnerFixture,
    a3s_use_extension::ArtifactReferenceAdmission,
) {
    let temporary = tempfile::tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/knowledge", &source, true)
        .await
        .unwrap();
    let catalog = verified_catalog(&candidate);
    let paths = a3s_use_extension::ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation,
    )
    .unwrap();
    let artifact_store = paths.artifact_store();
    let artifact_admission = artifact_store.acquire_reference_admission().await.unwrap();
    artifact_store
        .admit_prepared_package(&artifact_admission, &candidate)
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
    (
        KnowledgeOwnerFixture {
            package_root: artifact_store
                .expanded_package_path(candidate.package_digest())
                .unwrap(),
            authority: ControlPackageEffectAuthority {
                generation_operation_id: "operation:knowledge-owner".to_string(),
                installation_generation: 1,
                snapshot_digest: digest('3'),
                committed_at_ms: 1,
                host: PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION"))
                    .unwrap(),
                package,
                lifecycle_generation: 1,
                grant: None,
            },
            bindings: OkfKnowledgeBindingStore::from_extension_paths(&paths),
            client: OkfKnowledgeClient::new(adapter.clone()),
            adapter,
            paths,
            _temporary: temporary,
        },
        artifact_admission,
    )
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

pub(super) fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
