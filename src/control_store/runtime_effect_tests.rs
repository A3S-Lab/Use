use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_runtime::contract::{NetworkMode, RuntimeObservation, RuntimeServiceEndpoint};
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{
    CatalogMcpTransport, CatalogSurface, InstallationId, InstallationKind,
    InstallationPackageSelection, LockedPluginPackage, McpReleaseDescriptor, PlanActor,
    PlanPolicyDecision, PluginCatalogRecord, PluginOperationAction, PluginPackageLockHost,
    PluginSurfaceKind, PluginSurfaceRef, PluginWorkspaceGrant, ToolReleaseDescriptor,
    ToolWorkloadClass, UseError, UseResult, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
    WorkspaceGrantAuthority, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};
use a3s_use_extension::{
    ExtensionLifecyclePackage, PluginMcpLaunch, PluginMcpSurface, ToolSurface, ToolWorkload,
};
use async_trait::async_trait;

use super::effect_owner::runtime::{
    ControlRuntimeEffectPort as RuntimeOwner, ControlRuntimeMcpReadiness,
    ControlRuntimeServiceReadinessPort,
};
use super::effect_port::{
    ControlEffectPortOutcome, ControlEffectRequestIdentity, ControlRuntimeApplication,
    ControlRuntimeEffectPort, ControlRuntimeEffectRequest, ControlSurfaceEffectAction,
    ControlSurfaceEffectRequest,
};
use super::model::{
    ControlEffectIntent, ControlEffectKind, ControlEffectOwner, ControlEffectSubject,
    ControlGrantSelection, ControlPackageEffectAuthority, ControlProviderSelection,
    ControlRuntimeBindingObservation, ControlRuntimeEffectAuthority,
};
use crate::plugin_lifecycle::PluginLifecycleAction;
use crate::plugin_runtime::test_support::{capabilities, policy, FakeRuntime};
use crate::plugin_runtime::{
    plan_mcp_service_release, plan_tool_service_release, plan_tool_task_release,
    RuntimeBindingStore, RuntimeEndpointRef, RuntimeMcpInitializeEvidence,
    RuntimeProviderAssignment, RuntimeProviderSelection, RuntimeProviderSelector,
    RuntimeServiceProvisioningReceipt, RuntimeSurfaceContext, RuntimeSurfaceContract,
    RuntimeSurfacePlan, RuntimeTaskInvocation,
};

mod authority;

#[derive(Debug, Clone, Copy)]
enum FixtureSurface {
    ToolTask,
    ToolService,
    McpService,
}

struct RuntimeOwnerFixture {
    _temporary: tempfile::TempDir,
    package_root: PathBuf,
    store: a3s_use_extension::ArtifactStore,
    bindings: RuntimeBindingStore,
    selection: RuntimeProviderSelection,
    runtime: Arc<FakeRuntime>,
    readiness: Arc<RecordingReadiness>,
    authority: ControlRuntimeEffectAuthority,
    surface: PluginSurfaceRef,
}

#[derive(Default)]
struct RecordingReadiness {
    fail_next_tool_bind: AtomicBool,
    tool_bind_count: AtomicUsize,
    mcp_bind_count: AtomicUsize,
    drain_count: AtomicUsize,
    remove_count: AtomicUsize,
    last_deadline_at_ms: AtomicU64,
}

struct StaticRuntimeFactory {
    provider_id: ProviderId,
    client: Arc<dyn RuntimeClient>,
}

#[async_trait]
impl RuntimeProviderFactory for StaticRuntimeFactory {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
        Ok(self.client.clone())
    }
}

#[async_trait]
impl ControlRuntimeServiceReadinessPort for RecordingReadiness {
    async fn bind_tool_service(
        &self,
        surface: &ToolSurface,
        plan: &RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        _runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeEndpointRef> {
        self.tool_bind_count.fetch_add(1, Ordering::SeqCst);
        self.last_deadline_at_ms
            .store(deadline_at_ms.unwrap_or_default(), Ordering::SeqCst);
        if self.fail_next_tool_bind.swap(false, Ordering::SeqCst) {
            return Err(UseError::new(
                "use.test.gateway_ambiguous",
                "The Gateway accepted the test route before its response was lost.",
            ));
        }
        RuntimeEndpointRef::parse(format!(
            "gateway:tool/{}/{}",
            surface.id,
            plan.context().generation()
        ))
    }

    async fn bind_mcp_service(
        &self,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        _runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<ControlRuntimeMcpReadiness> {
        self.mcp_bind_count.fetch_add(1, Ordering::SeqCst);
        self.last_deadline_at_ms
            .store(deadline_at_ms.unwrap_or_default(), Ordering::SeqCst);
        let RuntimeSurfaceContract::McpService {
            protocol_version, ..
        } = plan.contract()
        else {
            return Err(UseError::new(
                "use.test.runtime_contract_invalid",
                "The MCP readiness fixture received a non-MCP plan.",
            ));
        };
        Ok(ControlRuntimeMcpReadiness {
            endpoint: RuntimeEndpointRef::parse(format!(
                "gateway:mcp/{}/{}",
                surface.id,
                plan.context().generation()
            ))?,
            initialize: RuntimeMcpInitializeEvidence::new(
                protocol_version.clone(),
                observation.observed_at_ms,
            )?,
        })
    }

    async fn drain_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        self.drain_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn remove_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        self.remove_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn runtime_tool_service_replays_receipt_and_retires_without_artifact_reads() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    let owner = owner(&fixture);
    let prepare = request(&fixture, ControlSurfaceEffectAction::Prepare);

    let first = applied(&owner, &prepare).await;
    assert!(matches!(
        first.binding,
        Some(ControlRuntimeBindingObservation::Service { .. })
    ));
    assert_eq!(fixture.runtime.apply_count(), 1);
    assert_eq!(fixture.readiness.tool_bind_count.load(Ordering::SeqCst), 1);

    std::fs::write(
        fixture.package_root.join("releases/index-tool-v1.json"),
        b"tampered after committed Runtime receipt",
    )
    .unwrap();
    let mut retry = prepare.clone();
    retry.surface.identity.attempt = 2;
    retry.surface.identity.deadline_at_ms = 30_000;
    assert_eq!(applied(&owner, &retry).await, first);
    assert_eq!(fixture.runtime.apply_count(), 1);
    assert_eq!(fixture.readiness.tool_bind_count.load(Ordering::SeqCst), 1);

    let stop = applied(&owner, &request(&fixture, ControlSurfaceEffectAction::Stop)).await;
    assert!(stop.binding.is_none());
    assert_eq!(fixture.runtime.stop_count(), 1);
    assert_eq!(fixture.readiness.drain_count.load(Ordering::SeqCst), 1);

    let remove = applied(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Remove),
    )
    .await;
    assert!(remove.binding.is_none());
    assert_eq!(fixture.runtime.stop_count(), 1);
    assert_eq!(fixture.runtime.remove_count(), 1);
    assert_eq!(fixture.readiness.drain_count.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.readiness.remove_count.load(Ordering::SeqCst), 1);
    let qualified = fixture.selection.surfaces()[0].plan().surface();
    assert!(fixture
        .bindings
        .get_generation(&installation(), &qualified, 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn runtime_tool_task_prepares_and_replays_a_receipt_without_starting_a_unit() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolTask).await;
    let owner = owner(&fixture);
    let prepare = request(&fixture, ControlSurfaceEffectAction::Prepare);

    let first = applied(&owner, &prepare).await;
    assert_eq!(first.binding, Some(ControlRuntimeBindingObservation::Task));
    assert_eq!(fixture.runtime.apply_count(), 0);

    std::fs::write(
        fixture.package_root.join("releases/task.json"),
        b"tampered after committed Runtime Task receipt",
    )
    .unwrap();
    assert_eq!(applied(&owner, &prepare).await, first);
    assert_eq!(fixture.runtime.apply_count(), 0);

    applied(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Remove),
    )
    .await;
    assert_eq!(fixture.runtime.stop_count(), 0);
    assert_eq!(fixture.runtime.remove_count(), 0);
    assert_eq!(fixture.readiness.drain_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_tool_service_reconciles_ambiguous_gateway_bind_without_reapplying() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    fixture
        .readiness
        .fail_next_tool_bind
        .store(true, Ordering::SeqCst);
    let owner = owner(&fixture);
    let prepare = request(&fixture, ControlSurfaceEffectAction::Prepare);

    let first = ControlRuntimeEffectPort::apply_surface(&owner, &prepare).await;
    let ControlEffectPortOutcome::Unknown(failure) = first else {
        panic!("an ambiguous Gateway response must remain unknown");
    };
    assert_eq!(failure.error_code, "use.test.gateway_ambiguous");
    assert_eq!(fixture.runtime.apply_count(), 1);

    let application = applied(&owner, &prepare).await;
    assert!(application.binding.is_some());
    assert_eq!(fixture.runtime.apply_count(), 1);
    assert_eq!(fixture.readiness.tool_bind_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runtime_mcp_service_requires_initialize_evidence_and_removes_exact_binding() {
    let fixture = runtime_owner_fixture(FixtureSurface::McpService).await;
    let owner = owner(&fixture);

    let application = applied(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    assert!(matches!(
        application.binding,
        Some(ControlRuntimeBindingObservation::Service { .. })
    ));
    assert_eq!(fixture.runtime.apply_count(), 1);
    assert_eq!(fixture.readiness.mcp_bind_count.load(Ordering::SeqCst), 1);

    applied(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Remove),
    )
    .await;
    assert_eq!(fixture.runtime.stop_count(), 1);
    assert_eq!(fixture.runtime.remove_count(), 1);
    assert_eq!(fixture.readiness.drain_count.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.readiness.remove_count.load(Ordering::SeqCst), 1);
}

fn owner(fixture: &RuntimeOwnerFixture) -> RuntimeOwner {
    RuntimeOwner::new(
        fixture.store.clone(),
        fixture.selection.clone(),
        fixture.bindings.clone(),
        fixture.readiness.clone(),
    )
}

async fn applied(
    owner: &RuntimeOwner,
    request: &ControlRuntimeEffectRequest,
) -> ControlRuntimeApplication {
    match ControlRuntimeEffectPort::apply_surface(owner, request).await {
        ControlEffectPortOutcome::Applied(application) => application,
        ControlEffectPortOutcome::Deferred(failure)
        | ControlEffectPortOutcome::Rejected(failure)
        | ControlEffectPortOutcome::Unknown(failure) => {
            panic!(
                "the Runtime owner fixture must apply: {}",
                failure.error_code
            )
        }
    }
}

fn request(
    fixture: &RuntimeOwnerFixture,
    action: ControlSurfaceEffectAction,
) -> ControlRuntimeEffectRequest {
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
    let effect_kind = match action {
        ControlSurfaceEffectAction::Prepare => ControlEffectKind::SurfacePrepare,
        ControlSurfaceEffectAction::Stop => ControlEffectKind::SurfaceStop,
        ControlSurfaceEffectAction::Remove => ControlEffectKind::SurfaceRemove,
    };
    let package = &fixture.authority.package;
    let package_id = package.package.package_id().to_string();
    let package_digest = package
        .package
        .package
        .catalog
        .record
        .package
        .sha256
        .clone()
        .unwrap();
    let manifest_digest = package
        .package
        .package
        .catalog
        .record
        .package
        .manifest_sha256
        .clone()
        .unwrap();
    let plan_digest = digest('1');
    let owner = ControlEffectOwner::RuntimeProvider {
        provider_id: fixture
            .authority
            .provider_selection
            .evidence
            .provider_id
            .clone(),
        selection_digest: fixture
            .authority
            .provider_selection
            .selection_digest
            .clone(),
    };
    let intent = ControlEffectIntent::new(
        0,
        installation(),
        plan_digest.clone(),
        operation_action,
        package.installation_generation,
        ControlEffectSubject::Surface {
            package_id: package_id.clone(),
            lifecycle_generation: package.lifecycle_generation,
            package_digest: package_digest.clone(),
            manifest_digest: manifest_digest.clone(),
            action: lifecycle_action,
            surface: fixture.surface.clone(),
        },
        owner,
        effect_kind,
        true,
    )
    .unwrap();
    ControlRuntimeEffectRequest {
        surface: ControlSurfaceEffectRequest {
            identity: ControlEffectRequestIdentity {
                operation_id: package.generation_operation_id.clone(),
                installation: installation(),
                plan_digest,
                operation_action,
                installation_generation: package.installation_generation,
                sequence: 0,
                idempotency_key: intent.idempotency_key,
                required: true,
                attempt: 1,
                deadline_at_ms: 20_000,
            },
            authority: package.clone(),
            package_id,
            lifecycle_generation: package.lifecycle_generation,
            package_digest,
            manifest_digest,
            lifecycle_action,
            surface: fixture.surface.clone(),
            action,
        },
        authority: fixture.authority.clone(),
        provider_id: fixture
            .authority
            .provider_selection
            .evidence
            .provider_id
            .clone(),
        selection_digest: fixture
            .authority
            .provider_selection
            .selection_digest
            .clone(),
    }
}

async fn runtime_owner_fixture(surface_kind: FixtureSurface) -> RuntimeOwnerFixture {
    let temporary = tempfile::tempdir().unwrap();
    let source = match surface_kind {
        FixtureSurface::ToolTask => {
            let source = temporary.path().join("managed-task-package");
            write_release_task_package(&source).await;
            source
        }
        FixtureSurface::ToolService | FixtureSurface::McpService => {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("crates/extension/fixtures/packages/plugin-v3/package")
        }
    };
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/research", &source, true)
        .await
        .unwrap();
    let catalog = verified_catalog(&candidate);
    let surface = match surface_kind {
        FixtureSurface::ToolTask => PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_string(),
        },
        FixtureSurface::ToolService => PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "index".to_string(),
        },
        FixtureSurface::McpService => PluginSurfaceRef {
            kind: PluginSurfaceKind::Mcp,
            id: "library".to_string(),
        },
    };
    let paths = a3s_use_extension::ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation(),
    )
    .unwrap();
    let store = paths.artifact_store();
    let admission = store.acquire_reference_admission().await.unwrap();
    store
        .admit_prepared_package(&admission, &candidate)
        .await
        .unwrap();
    drop(admission);
    let package_root = store
        .expanded_package_path(candidate.package_digest())
        .unwrap();

    let selected_surfaces = catalog
        .record
        .resolve_surfaces(std::slice::from_ref(&surface))
        .unwrap()
        .into_iter()
        .map(|catalog_surface| catalog_surface.reference())
        .collect::<Vec<_>>();
    let permissions = catalog
        .selected_state(&selected_surfaces)
        .unwrap()
        .permissions;
    let permission_ceiling_digest = catalog.record.permission_ceiling_digest.clone();
    let permissions_digest = permissions.descriptor_digest().unwrap();
    let grant = PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: installation().id,
        package_id: candidate.package_id().to_string(),
        package_digest: candidate.package_digest().to_string(),
        permission_ceiling_digest,
        permissions_digest,
        permissions,
        authority: WorkspaceGrantAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('8'),
            confirmation_digest: Some(digest('7')),
        },
        granted_at_ms: 100,
        expires_at_ms: None,
    };
    let grant_digest = grant.descriptor_digest().unwrap();
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
    let plan = runtime_plan(&candidate, surface_kind, &surface, &grant_digest);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan), true));
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: runtime.clone(),
        }))
        .unwrap();
    let selection = RuntimeProviderSelector::new(&registry)
        .select(
            vec![plan.clone()],
            vec![RuntimeProviderAssignment::new(plan.surface(), "test-runtime").unwrap()],
        )
        .await
        .unwrap();
    let provider_selection =
        ControlProviderSelection::from_evidence(selection.surfaces()[0].provider().clone())
            .unwrap();
    let grant_proposal_digest = selection.surfaces()[0]
        .plan()
        .context()
        .grant_digest()
        .to_owned();
    let package_authority = ControlPackageEffectAuthority {
        generation_operation_id: "operation:runtime-owner".to_string(),
        installation_generation: 1,
        snapshot_digest: digest('3'),
        committed_at_ms: 1_000,
        host: PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
        package,
        lifecycle_generation: 1,
        grant: Some(ControlGrantSelection {
            grant,
            grant_digest,
            receipt_revision: 1,
        }),
    };
    RuntimeOwnerFixture {
        package_root,
        store,
        bindings: RuntimeBindingStore::from_extension_paths(&paths),
        selection,
        runtime,
        readiness: Arc::new(RecordingReadiness::default()),
        authority: ControlRuntimeEffectAuthority {
            package: package_authority,
            provider_selection,
            grant_proposal_digest: Some(grant_proposal_digest.into_boxed_str()),
        },
        surface,
        _temporary: temporary,
    }
}

fn runtime_plan(
    candidate: &ExtensionLifecyclePackage,
    surface_kind: FixtureSurface,
    surface: &PluginSurfaceRef,
    grant_digest: &str,
) -> RuntimeSurfacePlan {
    let context = RuntimeSurfaceContext::new(
        candidate.package_id(),
        candidate.package_digest(),
        installation(),
        grant_digest,
        surface.clone(),
        1,
    )
    .unwrap();
    match surface_kind {
        FixtureSurface::ToolTask => {
            let tool = candidate
                .manifest()
                .tools
                .iter()
                .find(|tool| tool.id == surface.id)
                .unwrap();
            let ToolWorkload::Task(task) = &tool.workload else {
                panic!("the Tool fixture must be a Task");
            };
            let descriptor = ToolReleaseDescriptor::from_json(include_bytes!(
                "../../crates/core/fixtures/releases/tool-task-release-v1.json"
            ))
            .unwrap();
            plan_tool_task_release(
                context,
                task,
                &descriptor,
                crate::plugin_runtime::test_support::artifact(
                    &descriptor.artifact.digest,
                    &descriptor.artifact.media_type,
                ),
                RuntimeTaskInvocation::new("planning-template", Vec::new()).unwrap(),
                policy(),
                NetworkMode::None,
            )
            .unwrap()
        }
        FixtureSurface::ToolService => {
            let tool = candidate
                .manifest()
                .tools
                .iter()
                .find(|tool| tool.id == surface.id)
                .unwrap();
            let ToolWorkload::Service(service) = &tool.workload else {
                panic!("the Tool fixture must be a Service");
            };
            let descriptor = ToolReleaseDescriptor::from_json(include_bytes!(
                "../../crates/extension/fixtures/packages/plugin-v3/package/releases/index-tool-v1.json"
            ))
            .unwrap();
            plan_tool_service_release(
                context,
                service,
                &descriptor,
                crate::plugin_runtime::test_support::artifact(
                    &descriptor.artifact.digest,
                    &descriptor.artifact.media_type,
                ),
                policy(),
            )
            .unwrap()
        }
        FixtureSurface::McpService => {
            let mcp = candidate
                .manifest()
                .mcp_servers
                .iter()
                .find(|mcp| mcp.id == surface.id)
                .unwrap();
            let descriptor = McpReleaseDescriptor::from_json(include_bytes!(
                "../../crates/extension/fixtures/packages/plugin-v3/package/releases/library-mcp-v1.json"
            ))
            .unwrap();
            plan_mcp_service_release(
                context,
                mcp,
                &descriptor,
                crate::plugin_runtime::test_support::artifact(
                    &descriptor.artifact.digest,
                    &descriptor.artifact.media_type,
                ),
                policy(),
            )
            .unwrap()
        }
    }
}

fn verified_catalog(candidate: &ExtensionLifecyclePackage) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-v3.json"
    ))
    .unwrap();
    let manifest = candidate.manifest();
    record.dependencies = manifest.dependencies.clone();
    record.surfaces = manifest
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| {
            let workload = manifest
                .tools
                .iter()
                .find(|tool| {
                    surface.surface.kind == PluginSurfaceKind::Tool && tool.id == surface.surface.id
                })
                .map(|tool| match &tool.workload {
                    ToolWorkload::Task(_) => ToolWorkloadClass::Task,
                    ToolWorkload::Service(_) => ToolWorkloadClass::Service,
                });
            let mcp_transport = manifest
                .mcp_servers
                .iter()
                .find(|mcp| {
                    surface.surface.kind == PluginSurfaceKind::Mcp && mcp.id == surface.surface.id
                })
                .map(|mcp| match &mcp.launch {
                    PluginMcpLaunch::Stdio { .. } => CatalogMcpTransport::Stdio,
                    PluginMcpLaunch::StreamableHttp { .. } => CatalogMcpTransport::StreamableHttp,
                });
            CatalogSurface {
                kind: surface.surface.kind,
                id: surface.surface.id,
                optional: surface.optional,
                workload,
                mcp_transport,
                mcp_tool_count: None,
                okf_bundle: None,
                requires: surface.dependencies,
            }
        })
        .collect();
    record.permission_ceiling.surfaces.retain(|permission| {
        record
            .surfaces
            .iter()
            .any(|surface| surface.reference() == permission.surface)
    });
    if manifest
        .mcp_servers
        .iter()
        .any(|surface| surface.id == "local-library")
    {
        let mut local_library = record
            .permission_ceiling
            .surfaces
            .iter()
            .find(|permission| {
                permission.surface.kind == PluginSurfaceKind::Mcp
                    && permission.surface.id == "library"
            })
            .cloned()
            .unwrap();
        local_library.surface.id = "local-library".to_string();
        local_library.native_execution = true;
        local_library.private_service = false;
        record.permission_ceiling.surfaces.push(local_library);
    }
    record
        .permission_ceiling
        .surfaces
        .sort_by(|left, right| left.surface.cmp(&right.surface));
    record.permission_ceiling_digest = record.permission_ceiling.descriptor_digest().unwrap();
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

fn installation() -> InstallationId {
    InstallationId::new(InstallationKind::Workspace, "workspace-01").unwrap()
}

async fn write_release_task_package(root: &std::path::Path) {
    tokio::fs::create_dir_all(root.join("releases"))
        .await
        .unwrap();
    tokio::fs::write(root.join("README.md"), "# Managed Runtime Task fixture\n")
        .await
        .unwrap();
    tokio::fs::write(
        root.join("releases/task.json"),
        include_bytes!("../../crates/core/fixtures/releases/tool-task-release-v1.json"),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.join("a3s-use-extension.acl"),
        r#"extension "acme/research" {
  schema_version = 3
  version        = "2.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["execute"]

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    release     = "releases/task.json"
    command     = "acme-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }
}
"#,
    )
    .await
    .unwrap();
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
