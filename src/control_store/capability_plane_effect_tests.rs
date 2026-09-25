use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(feature = "mcp")]
use crate::plugin_runtime::RuntimeServiceBindingReceipt;
#[cfg(feature = "mcp")]
use a3s_runtime::contract::{
    HealthCheckKind, IsolationLevel, NetworkMode, ResourceControl, RuntimeCapabilities,
    RuntimeFeature, RuntimeObservation, RuntimeServiceEndpoint, RuntimeUnitClass,
};
#[cfg(not(feature = "mcp"))]
use a3s_runtime::contract::{RuntimeObservation, RuntimeServiceEndpoint};
#[cfg(feature = "mcp")]
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{
    CapabilityConsumerExtension, CapabilityDescriptionProof,
    CapabilityDescriptionSignatureAlgorithm, CapabilityDescriptionSignaturePayload,
    CapabilityDescriptor, CapabilityDescriptorKind, CapabilityGatewayCatalog,
    CapabilityPublicationEvidence, CapabilityToolAnnotations, InvocationRef, PluginOperationAction,
    PluginPackageId, PluginSurfaceKind, PluginSurfaceRef, SignedCapabilityDescription,
};
#[cfg(feature = "mcp")]
use a3s_use_core::{
    CatalogMcpTransport, CatalogSurface, PlanEnforcementProfile, PlanQualifiedSurfaceRef,
    PlanScope, PlanScopeKind, PlannedProviderEvidence, PluginCatalogRecord, PluginPackageLock,
    PluginPackageLockHost, PluginPackageResolver, ToolWorkloadClass, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{CapabilityDescriptionTrustKey, CapabilityDescriptionTrustStore};
#[cfg(feature = "mcp")]
use a3s_use_extension::{
    ExtensionLifecyclePackage, ExtensionPaths, PluginMcpLaunch, PluginMcpSurface, ToolSurface,
    ToolWorkload,
};
use ring::signature::{Ed25519KeyPair, KeyPair};

#[cfg(feature = "mcp")]
use crate::capability_gateway::{
    CapabilityGatewayCompositionOptions, CapabilityGatewayExternalLease,
    CapabilityGatewayHttpConfig, CapabilityGatewayInvocation, CapabilityGatewayInvocationProvider,
    CapabilityGatewayMcpServer, CapabilityGatewayRequestContext, CapabilityGatewaySessionFactory,
};
#[cfg(feature = "mcp")]
use crate::control_store::effect_owner::capability_plane::ControlCapabilityGatewayInvocationFactory;
#[cfg(feature = "mcp")]
use rmcp::model::{CallToolRequestParam, ResourceContents};
#[cfg(feature = "mcp")]
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
#[cfg(feature = "mcp")]
use rmcp::{ClientHandler, ServiceExt};
#[cfg(feature = "mcp")]
use tokio_util::sync::CancellationToken;

use super::aggregate_tests::fixtures::{
    apply_all_effects, claim, control_installation, digest, initialized_store, observation,
    operation, operation_at, projected_transition, transition,
};
use super::dispatcher::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    ControlEffectDispatcher, ControlEffectPorts, SystemControlEffectClock,
};
use super::effect_owner::capability_plane::{
    ControlCapabilityDescriptorProjection, ControlCapabilityDescriptorSnapshot,
    ControlCapabilityDescriptorSnapshotKey, ControlCapabilityDescriptorSnapshotRestoreVerification,
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilityPlaneEffectPort,
    ControlCapabilitySignerPolicy,
};
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
#[cfg(feature = "mcp")]
use super::effect_owner::runtime::ControlRuntimeServiceReadinessPort;
use super::effect_owner::static_surface::ControlStaticSurfaceEffectPort;
use super::effect_port::{
    ControlCapabilityCatalogProjectionPort, ControlEffectPortOutcome, ControlFlowEffectPort,
    ControlRuntimeApplication, ControlRuntimeEffectPort, ControlRuntimeEffectRequest,
    ControlSurfaceApplication, ControlSurfaceEffectRequest,
};
use super::knowledge_effect_test_support::{knowledge_owner_fixture_for, KnowledgeOwnerFixture};
use super::model::{
    ControlAppliedEffectEvidence, ControlCapabilityEffectAuthority, ControlEffectAuthority,
    ControlEffectOutcome, ControlEffectOwner, ControlEffectStatus, ControlProjectionHistory,
    ReviewedControlOperation,
};
#[cfg(feature = "mcp")]
use super::production::{ProductionControlHostDependencies, ProductionControlLifecycle};
use super::ControlStore;
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;
#[cfg(feature = "mcp")]
use crate::cognitive_package::{
    CognitivePackageAuthorizationEvidence, PlannedWorkspaceGrantOperation,
};
#[cfg(feature = "mcp")]
use crate::plugin_runtime::test_support::{artifact, task_descriptor, task_surface, FakeRuntime};
#[cfg(feature = "mcp")]
use crate::plugin_runtime::{
    plan_tool_task_release, runtime_capabilities_digest, RuntimeSurfaceContext, RuntimeSurfacePlan,
    RuntimeSurfacePlanKey, RuntimeSurfacePlanPublication, RuntimeTaskInvocation,
};

struct EmptyCatalogProjection;

struct UnauthorizedCatalogProjection;

struct ExactResourceCatalogProjection;

#[cfg(feature = "mcp")]
/// Grant Tool fixture projector that admits descriptors through the same
/// `ControlCapabilityDescriptorProjection` gate used by Durable/SignedDurable
/// production hosts, instead of bypassing schema/attestation checks.
struct StrictToolCatalogProjection;

struct OptionalResourceCatalogProjection;

#[cfg(feature = "mcp")]
#[derive(Debug, Default)]
struct EmptyGatewayProvider;

#[cfg(feature = "mcp")]
struct DrainLeaseMarker(Arc<std::sync::atomic::AtomicBool>);

#[cfg(feature = "mcp")]
impl CapabilityGatewayExternalLease for DrainLeaseMarker {
    fn matches_gateway_session(
        &self,
        _key: &crate::capability_gateway::CapabilityGatewaySessionKey,
    ) -> bool {
        false
    }
}

#[cfg(feature = "mcp")]
impl Drop for DrainLeaseMarker {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl CapabilityGatewayInvocationProvider for EmptyGatewayProvider {
    async fn authorize(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: &serde_json::Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> a3s_use_core::UseResult<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: serde_json::Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> a3s_use_core::UseResult<serde_json::Value> {
        Ok(serde_json::json!({"ok": true}))
    }
}

#[cfg(feature = "mcp")]
#[derive(Debug, Default)]
struct RecordingControlInvocationFactory {
    opened: std::sync::Mutex<Vec<(String, u64, String)>>,
}

#[cfg(feature = "mcp")]
struct RecordingControlInvocation {
    invocation_ref: InvocationRef,
    resource_uri: String,
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl ControlCapabilityGatewayInvocationFactory for RecordingControlInvocationFactory {
    async fn open(
        &self,
        descriptor: &CapabilityDescriptor,
        _context: &CapabilityGatewayRequestContext,
        lease: &super::effect_owner::capability_plane::ControlCapabilitySnapshotLease,
    ) -> a3s_use_core::UseResult<Box<dyn CapabilityGatewayInvocation>> {
        self.opened
            .lock()
            .map_err(|_| {
                a3s_use_core::UseError::new(
                    "test.control_invocation_factory_poisoned",
                    "The test Control invocation factory lock was poisoned.",
                )
            })?
            .push((
                descriptor.package_id.to_string(),
                descriptor.generation,
                lease.cursor().catalog.digest.clone(),
            ));
        Ok(Box::new(RecordingControlInvocation {
            invocation_ref: descriptor.invocation_ref.clone(),
            resource_uri: descriptor
                .resource_uri()
                .map(|uri| uri.as_str().to_owned())
                .unwrap_or_default(),
        }))
    }
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl CapabilityGatewayInvocation for RecordingControlInvocation {
    async fn authorize(
        &self,
        _arguments: &serde_json::Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> a3s_use_core::UseResult<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        _arguments: serde_json::Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> a3s_use_core::UseResult<serde_json::Value> {
        Ok(serde_json::json!({
            "invocationRef": self.invocation_ref.as_str(),
        }))
    }

    async fn read_resource(
        &self,
        _context: &CapabilityGatewayRequestContext,
    ) -> a3s_use_core::UseResult<Vec<ResourceContents>> {
        Ok(vec![ResourceContents::text(
            "Control-bound resource",
            &self.resource_uri,
        )])
    }
}

#[cfg(feature = "mcp")]
struct CompositionReadiness;

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl ControlRuntimeServiceReadinessPort for CompositionReadiness {
    async fn bind_tool_service(
        &self,
        _surface: &ToolSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        _runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<crate::plugin_runtime::RuntimeEndpointRef> {
        Err(a3s_use_core::UseError::new(
            "provider.test_unavailable",
            "The composition test does not bind Runtime services.",
        ))
    }

    async fn bind_mcp_service(
        &self,
        _surface: &PluginMcpSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        _runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<super::effect_owner::runtime::ControlRuntimeMcpReadiness> {
        Err(a3s_use_core::UseError::new(
            "provider.test_unavailable",
            "The composition test does not bind Runtime services.",
        ))
    }

    async fn drain_service(
        &self,
        _receipt: &RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<()> {
        Err(a3s_use_core::UseError::new(
            "provider.test_unavailable",
            "The composition test does not drain Runtime services.",
        ))
    }

    async fn remove_service(
        &self,
        _receipt: &RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<()> {
        Err(a3s_use_core::UseError::new(
            "provider.test_unavailable",
            "The composition test does not remove Runtime services.",
        ))
    }
}

#[async_trait::async_trait]
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

#[async_trait::async_trait]
impl ControlCapabilityCatalogProjectionPort for ExactResourceCatalogProjection {
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog> {
        ControlEffectPortOutcome::applied(
            CapabilityGatewayCatalog::new(
                authority.generation.snapshot.installation.clone(),
                authority.generation.capability.generation,
                vec![exact_resource_descriptor(authority)],
            )
            .unwrap(),
        )
    }
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl ControlCapabilityCatalogProjectionPort for StrictToolCatalogProjection {
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog> {
        // Uninstall / empty generations project an empty catalog; Install and
        // Upgrade admit the convert Tool only when Runtime attestation binds.
        let Some(descriptor) = exact_tool_descriptor(authority) else {
            return ControlEffectPortOutcome::applied(
                CapabilityGatewayCatalog::new(
                    authority.generation.snapshot.installation.clone(),
                    authority.generation.capability.generation,
                    Vec::new(),
                )
                .unwrap(),
            );
        };
        let signer = "registry/acme";
        let package_id = descriptor.package_id.as_str().to_owned();
        let proof = CapabilityDescriptionProof::from_verified(descriptor, signer).unwrap();
        let projector = ControlCapabilityDescriptorProjection::new(
            vec![proof],
            signer_policy_for(&package_id, signer),
        )
        .unwrap();
        projector.project(authority).await
    }
}

#[async_trait::async_trait]
impl ControlCapabilityCatalogProjectionPort for OptionalResourceCatalogProjection {
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog> {
        let mut descriptor = exact_resource_descriptor(authority);
        descriptor.required_extensions = vec![CapabilityConsumerExtension::Flow];
        ControlEffectPortOutcome::applied(
            CapabilityGatewayCatalog::new(
                authority.generation.snapshot.installation.clone(),
                authority.generation.capability.generation,
                vec![descriptor],
            )
            .unwrap(),
        )
    }
}

#[async_trait::async_trait]
impl ControlCapabilityCatalogProjectionPort for UnauthorizedCatalogProjection {
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog> {
        let package = &authority.generation.snapshot.packages[0];
        let package_id = PluginPackageId::parse(package.package_id()).unwrap();
        let surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "unreviewed-tool".to_owned(),
        };
        let lifecycle_generation = authority.generation.package_lifecycles[0].lifecycle_generation;
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false
        });
        let descriptor = CapabilityDescriptor {
            schema: a3s_use_core::CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
            package_id: package_id.clone(),
            surface: surface.clone(),
            generation: lifecycle_generation,
            package_digest: package
                .package
                .catalog
                .record
                .package
                .sha256
                .clone()
                .unwrap(),
            manifest_digest: package
                .package
                .catalog
                .record
                .package
                .manifest_sha256
                .clone()
                .unwrap(),
            title: "Unreviewed Tool".to_owned(),
            description: "A valid descriptor outside committed surface authority.".to_owned(),
            invocation_ref: InvocationRef::derive(
                &package_id,
                &surface,
                lifecycle_generation,
                &digest('7'),
            )
            .unwrap(),
            artifact_ref: None,
            endpoint_ref: None,
            dependencies: Vec::new(),
            required_extensions: Vec::new(),
            publication: CapabilityPublicationEvidence {
                catalog_record_digest: package
                    .package
                    .catalog
                    .provenance
                    .catalog_record_digest
                    .clone(),
                signature_digest: digest('6'),
            },
            capability: CapabilityDescriptorKind::Tool {
                name: "unreviewed-tool".to_owned(),
                input_schema: schema.clone(),
                output_schema: schema,
                annotations: CapabilityToolAnnotations::new(false, false, false, false),
                runtime_descriptor_digest: None,
            },
        };
        ControlEffectPortOutcome::applied(
            CapabilityGatewayCatalog::new(
                authority.generation.snapshot.installation.clone(),
                authority.generation.capability.generation,
                vec![descriptor],
            )
            .unwrap(),
        )
    }
}

struct UnexpectedDynamicSurfacePort;

#[async_trait::async_trait]
impl ControlRuntimeEffectPort for UnexpectedDynamicSurfacePort {
    async fn apply_surface(
        &self,
        _request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        panic!("the Capability Plane fixture has no Runtime surface")
    }
}

#[async_trait::async_trait]
impl ControlFlowEffectPort for UnexpectedDynamicSurfacePort {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        panic!("the Capability Plane fixture has no Flow surface")
    }
}

struct InstalledCapabilityPlaneFixture {
    _owner_fixture: KnowledgeOwnerFixture,
    store: ControlStore,
    plane: Arc<ControlCapabilityPlaneEffectPort>,
    dispatcher: ControlEffectDispatcher,
    installed: ReviewedControlOperation,
}

#[tokio::test]
async fn published_cursor_exists_only_after_the_applied_capability_cutover() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:published-capability-cursor");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();

    assert!(store.published_capability().await.unwrap().is_none());

    apply_all_effects(&store, &reviewed, 100).await;
    let cursor = store.published_capability().await.unwrap().unwrap();
    assert_eq!(cursor.installation, control_installation());
    assert_eq!(cursor.installation_generation, 1);
    assert_eq!(cursor.capability_generation, 1);
    assert_eq!(cursor.catalog.installation, control_installation());
    assert_eq!(cursor.catalog.generation, 1);
    let effects = store.effects(reviewed.operation_id()).await.unwrap();
    let capability = effects
        .iter()
        .find(|effect| matches!(effect.intent.owner, ControlEffectOwner::CapabilityIndex))
        .unwrap();
    let ControlAppliedEffectEvidence::CapabilityIndex { receipt_digest, .. } =
        &capability.application.as_ref().unwrap().evidence
    else {
        panic!("the published capability effect must retain its Index receipt");
    };
    assert_eq!(&cursor.receipt_digest, receipt_digest);
    let ControlAppliedEffectEvidence::CapabilityIndex { catalog, .. } =
        &capability.application.as_ref().unwrap().evidence
    else {
        panic!("the published capability effect must retain its catalog binding");
    };
    assert_eq!(&cursor.catalog, catalog);
    assert_eq!(cursor.packages.len(), 1);
    assert_eq!(cursor.packages[0].package_id, "acme/knowledge");
    assert_eq!(cursor.packages[0].lifecycle_generation, 1);

    let mut duplicate_package = cursor.clone();
    let mut substituted_incarnation = duplicate_package.packages[0].clone();
    substituted_incarnation.lifecycle_generation = 2;
    duplicate_package.packages.push(substituted_incarnation);
    assert!(duplicate_package.validate().is_err());
}

#[tokio::test]
async fn real_surface_owners_publish_one_immutable_index_and_admit_its_exact_snapshot() {
    let fixture = installed_capability_plane("operation:capability-plane:install").await;
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();

    let lease = fixture
        .plane
        .acquire_published(&cursor)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(lease.cursor(), &cursor);
    assert_eq!(lease.package_count(), 1);
    assert_eq!(lease.catalog().generation(), cursor.catalog.generation);
    assert_eq!(
        lease.catalog().descriptor_digest().unwrap(),
        cursor.catalog.digest
    );
    assert_eq!(
        lease.document_receipt_digest().unwrap(),
        cursor.receipt_digest
    );
    assert_eq!(
        fixture
            .store
            .effects(fixture.installed.operation_id())
            .await
            .unwrap()
            .iter()
            .map(|effect| effect.status)
            .collect::<Vec<_>>(),
        vec![
            ControlEffectStatus::Applied,
            ControlEffectStatus::Applied,
            ControlEffectStatus::Applied,
        ]
    );
}

#[tokio::test]
async fn published_cursor_reopens_from_durable_control_after_restart() {
    let fixture = installed_capability_plane("operation:capability-plane:reopen").await;
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let paths = fixture._owner_fixture.paths.clone();

    // A fresh Control Store and Capability plane stand in for a restarted
    // host. No cursor is passed into the reopen operation: it must derive the
    // exact published authority from durable Control state.
    let reopened_store = ControlStore::from_extension_paths(&paths).unwrap();
    let reopened_catalogs = CapabilityGatewayCatalogStore::from_extension_paths(&paths);
    let reopened_plane = ControlCapabilityPlaneEffectPort::new(
        reopened_store,
        reopened_catalogs,
        Arc::new(EmptyCatalogProjection),
    )
    .unwrap();
    let lease = reopened_plane.reopen_published().await.unwrap().unwrap();

    assert_eq!(lease.cursor(), &cursor);
    assert_eq!(lease.package_count(), cursor.packages.len());
    assert_eq!(lease.catalog().generation(), cursor.catalog.generation);
    assert_eq!(
        lease.catalog().descriptor_digest().unwrap(),
        cursor.catalog.digest
    );
    assert_eq!(
        lease.document_receipt_digest().unwrap(),
        cursor.receipt_digest
    );
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn reopened_control_lease_seeds_a_lease_bound_gateway_session() {
    let fixture = installed_capability_plane("operation:capability-plane:gateway").await;
    let lease = fixture.plane.reopen_published().await.unwrap().unwrap();
    let generation = lease.cursor().capability_generation;
    let server =
        super::composition::ControlStoreRuntimeComposition::gateway_server_from_control_lease(
            lease,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .unwrap();

    assert_eq!(server.catalog().generation(), generation);
    assert!(server.has_generation_lease());
    let factory = CapabilityGatewaySessionFactory::new(server);
    assert!(factory.current().has_generation_lease());
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn control_gateway_binding_uses_source_publication_across_consumer_projection() {
    let fixture = installed_capability_plane_with_projection(
        "operation:capability-plane:gateway-source-binding",
        Arc::new(OptionalResourceCatalogProjection),
    )
    .await;
    let paths = fixture._owner_fixture.paths.clone();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            // Reopen reads the durable catalog payload; this projection
            // is used only for any later publication in this fixture.
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();

    let session = composition
        .reopen_published_capability_gateway(
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .unwrap();

    // Generic MCP omits the Flow-only descriptor from its visible view, but
    // the lifecycle key must still identify the complete durable publication.
    assert!(session.current().catalog().descriptors().is_empty());
    assert_eq!(session.current_key().unwrap().digest, cursor.catalog.digest);
    let reconciled = composition
        .reconcile_published_capability_gateway(
            &session,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        reconciled,
        Some(super::composition::ControlCapabilityGatewayReconciliation::Unchanged(_))
    ));

    let retained = composition
        .drain_and_retain_published_capability_gateway(
            &session,
            std::time::Duration::ZERO,
            &[],
            &[],
        )
        .await
        .unwrap();
    assert!(!retained.changed);
    assert_eq!(retained.catalog.retained_record_count, 1);
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn control_gateway_reconciliation_is_idempotent_for_the_current_cursor() {
    let fixture = installed_capability_plane("operation:capability-plane:gateway-reconcile").await;
    let paths = fixture._owner_fixture.paths.clone();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();

    let lease = fixture.plane.reopen_published().await.unwrap().unwrap();
    let factory = CapabilityGatewaySessionFactory::new(
        super::composition::ControlStoreRuntimeComposition::gateway_server_from_control_lease(
            lease,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .unwrap(),
    );
    let first = composition
        .reconcile_published_capability_gateway(
            &factory,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        first,
        Some(super::composition::ControlCapabilityGatewayReconciliation::Unchanged(_))
    ));
    let second = composition
        .reconcile_published_capability_gateway(
            &factory,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        second,
        Some(super::composition::ControlCapabilityGatewayReconciliation::Unchanged(_))
    ));

    let activation = composition.gateway_cutover_activation(
        factory,
        Arc::new(EmptyGatewayProvider),
        CapabilityGatewayCompositionOptions::default(),
    );
    let wrong_key = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let error = crate::plugin_lifecycle::PluginGraphCapabilityCutoverActivation::activate_capability_cutover(
        activation.as_ref(),
        wrong_key,
    )
    .await
    .expect_err("a replay from another graph operation must be rejected");
    assert_eq!(
        error.code,
        "use.control.capability_gateway_activation_key_mismatch"
    );
    let key = crate::plugin_lifecycle::operation_cutover_key(&fixture.installed.envelope).unwrap();
    crate::plugin_lifecycle::PluginGraphCapabilityCutoverActivation::activate_capability_cutover(
        activation.as_ref(),
        &key,
    )
    .await
    .unwrap();
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn control_gateway_reconciliation_swaps_from_the_prior_control_lease() {
    let fixture = installed_capability_plane("operation:capability-plane:gateway-upgrade").await;
    let paths = fixture._owner_fixture.paths.clone();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();

    let prior_cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let prior_lease = fixture.plane.reopen_published().await.unwrap().unwrap();
    let prior_server =
        super::composition::ControlStoreRuntimeComposition::gateway_server_from_control_lease(
            prior_lease,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .unwrap();
    let factory = CapabilityGatewaySessionFactory::new(prior_server.clone());
    let explicit_replace_factory = CapabilityGatewaySessionFactory::new(prior_server);
    let prior_key = factory.current_key().unwrap();

    let prior = fixture.store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&prior).unwrap();
    let upgrade = operation_at(
        "operation:capability-plane:gateway-upgrade-operation",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    fixture
        .store
        .register_operation(upgrade.clone())
        .await
        .unwrap();
    fixture
        .store
        .commit_transition(projected_transition(&upgrade, &prior, &history))
        .await
        .unwrap();

    // The fixture has no second package artifact. Mark the two preparation
    // effects applied so this test reaches the capability publication and
    // live-session reconciliation boundary.
    for sequence in 0..2_u32 {
        let now_ms = 200 + u64::from(sequence) * 20;
        let claim_token = format!("claim:capability-plane:gateway-upgrade:{sequence}");
        let claimed = fixture
            .store
            .claim_next_effect(claim(
                upgrade.operation_id(),
                &claim_token,
                now_ms,
                now_ms + 10,
                false,
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.intent.sequence, sequence);
        fixture
            .store
            .record_effect_observation(observation(
                upgrade.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence, 16).unwrap(),
                now_ms + 5,
            ))
            .await
            .unwrap();
    }
    assert_dispatch(
        &fixture.dispatcher,
        &upgrade,
        "claim:capability-plane:gateway-upgrade-cutover",
        2,
        1,
        ControlEffectOutcome::Applied,
        false,
    )
    .await;

    let target_cursor = fixture.store.published_capability().await.unwrap().unwrap();
    assert!(target_cursor.capability_generation > prior_cursor.capability_generation);
    let result = composition
        .reconcile_published_capability_gateway(
            &factory,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .expect("the target Control publication must be available");
    let super::composition::ControlCapabilityGatewayReconciliation::Replaced(replacement) = result
    else {
        panic!("a newer Control publication must replace the prior live endpoint");
    };
    assert_eq!(replacement.previous, prior_key);
    assert_eq!(
        replacement.current.generation,
        target_cursor.capability_generation
    );
    assert_eq!(replacement.current.revision, target_cursor.catalog.revision);
    assert_eq!(replacement.current.digest, target_cursor.catalog.digest);
    assert_eq!(factory.current_key().unwrap(), replacement.current);

    let explicit = composition
        .replace_published_capability_gateway(
            &explicit_replace_factory,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .expect("the confirmed target publication must replace the prior source");
    assert_eq!(explicit.previous, prior_key);
    assert_eq!(
        explicit.current.generation,
        target_cursor.capability_generation
    );
    assert_eq!(explicit.current.revision, target_cursor.catalog.revision);
    assert_eq!(explicit.current.digest, target_cursor.catalog.digest);
    assert_eq!(
        explicit_replace_factory.current_key().unwrap(),
        explicit.current
    );
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_cutover_reconcile_notifies_independent_client_list_changed() {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Debug, Clone, Default)]
    struct IndexNotifyClient {
        tools: Arc<AtomicUsize>,
        resources: Arc<AtomicUsize>,
        prompts: Arc<AtomicUsize>,
        notified: Arc<Notify>,
    }

    impl IndexNotifyClient {
        async fn wait_for_all(&self, expected: usize) {
            loop {
                if self.tools.load(Ordering::SeqCst) >= expected
                    && self.resources.load(Ordering::SeqCst) >= expected
                    && self.prompts.load(Ordering::SeqCst) >= expected
                {
                    return;
                }
                self.notified.notified().await;
            }
        }
    }

    impl ClientHandler for IndexNotifyClient {
        fn on_tool_list_changed(
            &self,
            _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
        ) -> impl Future<Output = ()> + Send + '_ {
            self.tools.fetch_add(1, Ordering::SeqCst);
            self.notified.notify_waiters();
            std::future::ready(())
        }

        fn on_resource_list_changed(
            &self,
            _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
        ) -> impl Future<Output = ()> + Send + '_ {
            self.resources.fetch_add(1, Ordering::SeqCst);
            self.notified.notify_waiters();
            std::future::ready(())
        }

        fn on_prompt_list_changed(
            &self,
            _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
        ) -> impl Future<Output = ()> + Send + '_ {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            self.notified.notify_waiters();
            std::future::ready(())
        }
    }

    let fixture = installed_capability_plane("operation:capability-plane:index-list-changed").await;
    let paths = fixture._owner_fixture.paths.clone();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();

    let prior_lease = fixture.plane.reopen_published().await.unwrap().unwrap();
    let factory = CapabilityGatewaySessionFactory::new(
        super::composition::ControlStoreRuntimeComposition::gateway_server_from_control_lease(
            prior_lease,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .unwrap(),
    );
    let prior_key = factory.current_key().unwrap();

    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let serving = factory.clone();
    let server_handle = tokio::spawn(async move {
        serving
            .live_server()
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let notification_client = IndexNotifyClient::default();
    let client = notification_client
        .clone()
        .serve(client_transport)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if factory.notification_hub().peer_count().await == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("independent client must register with the Index notification hub");

    let prior = fixture.store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&prior).unwrap();
    let upgrade = operation_at(
        "operation:capability-plane:index-list-changed-upgrade",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    fixture
        .store
        .register_operation(upgrade.clone())
        .await
        .unwrap();
    fixture
        .store
        .commit_transition(projected_transition(&upgrade, &prior, &history))
        .await
        .unwrap();
    for sequence in 0..2_u32 {
        let now_ms = 200 + u64::from(sequence) * 20;
        let claim_token = format!("claim:capability-plane:index-list-changed:{sequence}");
        let claimed = fixture
            .store
            .claim_next_effect(claim(
                upgrade.operation_id(),
                &claim_token,
                now_ms,
                now_ms + 10,
                false,
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.intent.sequence, sequence);
        fixture
            .store
            .record_effect_observation(observation(
                upgrade.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence, 16).unwrap(),
                now_ms + 5,
            ))
            .await
            .unwrap();
    }
    assert_dispatch(
        &fixture.dispatcher,
        &upgrade,
        "claim:capability-plane:index-list-changed-cutover",
        2,
        1,
        ControlEffectOutcome::Applied,
        false,
    )
    .await;

    let target_cursor = fixture.store.published_capability().await.unwrap().unwrap();
    assert!(target_cursor.capability_generation > prior_key.generation);
    let result = composition
        .reconcile_published_capability_gateway(
            &factory,
            Arc::new(EmptyGatewayProvider),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .expect("the target Control publication must be available");
    let super::composition::ControlCapabilityGatewayReconciliation::Replaced(replacement) = result
    else {
        panic!("Control Index cutover must replace the live Gateway publication");
    };
    assert_eq!(
        replacement
            .notification
            .as_ref()
            .map(|report| report.notified_peers),
        Some(1)
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        notification_client.wait_for_all(1),
    )
    .await
    .expect("independent client must observe tools/resources/prompts list_changed");
    assert_eq!(
        factory.current_key().unwrap().digest,
        target_cursor.catalog.digest
    );
    assert_eq!(
        factory.current_key().unwrap().generation,
        target_cursor.capability_generation
    );
    // Discovery still works on the same live connection after Index cutover.
    let _ = client.list_all_tools().await.unwrap();
    client.cancel().await.unwrap();
    drop(factory);
    let _ = server_handle.await;
}

#[tokio::test]
async fn published_cutover_key_follows_the_published_generation_not_current_generation() {
    let fixture =
        installed_capability_plane("operation:capability-plane:gateway-key-lineage").await;
    let prior = fixture.store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&prior).unwrap();

    // Enablement advances the installation generation but does not replace
    // the package-graph publication that still owns the live capability
    // cursor.  The callback key must remain bound to the published install.
    let disable = operation_at(
        "operation:capability-plane:gateway-key-disable",
        PluginOperationAction::Disable,
        prior.snapshot.generation,
        prior.capability.generation,
    );
    fixture
        .store
        .register_operation(disable.clone())
        .await
        .unwrap();
    fixture
        .store
        .commit_transition(projected_transition(&disable, &prior, &history))
        .await
        .unwrap();

    let expected =
        crate::plugin_lifecycle::operation_cutover_key(&fixture.installed.envelope).unwrap();
    assert_eq!(
        fixture
            .store
            .published_capability_cutover_key()
            .await
            .unwrap(),
        Some(expected)
    );
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn gateway_retention_refuses_to_drain_a_session_outside_the_published_cursor() {
    let fixture =
        installed_capability_plane("operation:capability-plane:gateway-drain-binding").await;
    let paths = fixture._owner_fixture.paths.clone();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();

    // The session belongs to the same installation but a different
    // publication generation. It must not be accepted as the endpoint
    // selected by the durable Control cursor.
    let unrelated = CapabilityGatewayCatalog::new(
        cursor.installation.clone(),
        cursor.capability_generation.saturating_add(1),
        Vec::new(),
    )
    .unwrap();
    let lease_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let session = CapabilityGatewaySessionFactory::new(
        CapabilityGatewayMcpServer::new(unrelated, Arc::new(EmptyGatewayProvider))
            .unwrap()
            .with_external_lease(Arc::new(DrainLeaseMarker(Arc::clone(&lease_dropped))))
            .unwrap(),
    );
    let error = composition
        .drain_and_retain_published_capability_gateway(
            &session,
            std::time::Duration::from_secs(1),
            &[],
            &[],
        )
        .await
        .expect_err("an unrelated endpoint must not be drained");
    assert_eq!(
        error.code,
        "use.control.capability_gateway_drain_binding_mismatch"
    );
    assert!(!lease_dropped.load(std::sync::atomic::Ordering::SeqCst));

    // A copied catalog identity is not enough either. The external lease
    // marker must be the one issued by the Control capability plane, or a
    // host could drain a different provider that happens to advertise the
    // same immutable bytes.
    let published_catalog = fixture
        .plane
        .reopen_published()
        .await
        .unwrap()
        .unwrap()
        .catalog()
        .clone();
    let forged_lease_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let forged_session = CapabilityGatewaySessionFactory::new(
        CapabilityGatewayMcpServer::new(published_catalog, Arc::new(EmptyGatewayProvider))
            .unwrap()
            .with_external_lease(Arc::new(DrainLeaseMarker(Arc::clone(
                &forged_lease_dropped,
            ))))
            .unwrap(),
    );
    let error = composition
        .drain_and_retain_published_capability_gateway(
            &forged_session,
            std::time::Duration::from_secs(1),
            &[],
            &[],
        )
        .await
        .expect_err("a copied catalog without the Control lease must not be drained");
    assert_eq!(
        error.code,
        "use.control.capability_gateway_drain_binding_mismatch"
    );
    assert!(!forged_lease_dropped.load(std::sync::atomic::Ordering::SeqCst));
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn control_gateway_invocation_resolves_only_exact_published_descriptors() {
    let fixture = installed_capability_plane_with_projection(
        "operation:capability-plane:gateway-invocation",
        Arc::new(ExactResourceCatalogProjection),
    )
    .await;
    let paths = fixture._owner_fixture.paths.clone();
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let descriptor_snapshot = ControlCapabilityDescriptorSnapshot::new(
        ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation,
            cursor.capability_generation,
            cursor.descriptor_digest.clone(),
        )
        .unwrap(),
        Vec::new(),
        ControlCapabilitySignerPolicy::new(BTreeMap::new()).unwrap(),
    )
    .unwrap();
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&paths)
        .publish(&descriptor_snapshot)
        .await
        .unwrap();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();

    let lease = fixture.plane.reopen_published().await.unwrap().unwrap();
    let descriptor = lease.catalog().descriptors()[0].clone();
    let factory = Arc::new(RecordingControlInvocationFactory::default());
    let provider = composition.gateway_invocation_provider(factory.clone());
    let context = CapabilityGatewayRequestContext::stdio();

    let contents = provider.read_resource(&descriptor, &context).await.unwrap();
    assert!(matches!(
        contents.as_slice(),
        [ResourceContents::TextResourceContents { uri, text, .. }]
            if uri == descriptor.resource_uri().unwrap().as_str()
                && text == "Control-bound resource"
    ));
    assert_eq!(
        factory.opened.lock().unwrap().as_slice(),
        &[(
            descriptor.package_id.to_string(),
            descriptor.generation,
            lease.cursor().catalog.digest.clone(),
        )]
    );

    let mut forged = descriptor.clone();
    forged.title = "substituted".to_owned();
    let error = provider
        .read_resource(&forged, &context)
        .await
        .expect_err("a forged descriptor must fail before opening provider state");
    assert_eq!(
        error.code,
        "use.control.capability_gateway_invocation_mismatch"
    );
    assert_eq!(factory.opened.lock().unwrap().len(), 1);

    let session = composition
        .reopen_published_capability_gateway_with_factory(
            factory.clone(),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .unwrap();
    let _activation = composition.gateway_cutover_activation_with_factory(
        session.clone(),
        factory,
        CapabilityGatewayCompositionOptions::default(),
    );

    // The composition-level retention entry point derives the currently
    // published catalog from Control instead of trusting a caller-selected
    // "current" pointer.  Drain the endpoint first so its shared
    // generation/maintenance lease no longer fences the destructive phase.
    drop(_activation);
    drop(lease);
    let result = composition
        .drain_and_retain_published_capability_gateway(
            &session,
            std::time::Duration::from_secs(1),
            &[],
            &[],
        )
        .await
        .unwrap();
    assert!(!result.changed);
    assert_eq!(result.catalog.retained_record_count, 1);
    assert_eq!(result.descriptor_snapshot.retained_record_count, 1);

    // A process-level lifecycle retry may repeat the combined boundary after
    // the session has already released its source lease.  The factory keeps a
    // one-shot proof of the exact externally bound endpoint, so the retry is
    // read-only instead of being rejected as an unrelated unleased catalog.
    let replay = composition
        .drain_and_retain_published_capability_gateway(
            &session,
            std::time::Duration::ZERO,
            &[],
            &[],
        )
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.catalog.retained_record_count, 1);
    assert_eq!(replay.descriptor_snapshot.retained_record_count, 1);

    // A plan that retains only a newer, independently published payload must
    // not be allowed to prune the catalog selected by the durable cursor.
    let extra_catalog = CapabilityGatewayCatalog::new(
        cursor.installation.clone(),
        cursor.capability_generation.saturating_add(1),
        Vec::new(),
    )
    .unwrap();
    let extra_catalog_publication = composition
        .catalog_store()
        .publish(&extra_catalog)
        .await
        .unwrap();
    let extra_snapshot = ControlCapabilityDescriptorSnapshot::new(
        ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation.saturating_add(1),
            cursor.capability_generation.saturating_add(1),
            digest('a'),
        )
        .unwrap(),
        Vec::new(),
        ControlCapabilitySignerPolicy::new(BTreeMap::new()).unwrap(),
    )
    .unwrap();
    let extra_snapshot_digest = extra_snapshot.digest().unwrap();
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&paths)
        .publish(&extra_snapshot)
        .await
        .unwrap();
    let unsafe_plan = composition
        .capability_payload_retention()
        .plan_retention(
            std::slice::from_ref(&extra_catalog_publication.digest),
            std::slice::from_ref(&extra_snapshot_digest),
        )
        .await
        .unwrap();
    let error = composition
        .apply_published_capability_payload_retention(
            &unsafe_plan,
            &unsafe_plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_payload_retention_cursor_stale"
    );
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn production_invocation_factory_requires_committed_control_grant() {
    let fixture = installed_capability_plane_with_projection(
        "operation:capability-plane:production-grant",
        Arc::new(ExactResourceCatalogProjection),
    )
    .await;
    let paths = fixture._owner_fixture.paths.clone();
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let descriptor_snapshot = ControlCapabilityDescriptorSnapshot::new(
        ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation,
            cursor.capability_generation,
            cursor.descriptor_digest.clone(),
        )
        .unwrap(),
        Vec::new(),
        ControlCapabilitySignerPolicy::new(BTreeMap::new()).unwrap(),
    )
    .unwrap();
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&paths)
        .publish(&descriptor_snapshot)
        .await
        .unwrap();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();

    let lease = fixture.plane.reopen_published().await.unwrap().unwrap();
    let descriptor = lease.catalog().descriptors()[0].clone();
    let provider = composition.production_gateway_invocation_provider();
    let context = CapabilityGatewayRequestContext::stdio();

    // Default capability-plane install fixtures commit no Grants. Production
    // open must fail closed before provider I/O.
    let generation = fixture.store.current_generation().await.unwrap().unwrap();
    assert!(generation.grants.is_empty());
    let error = provider
        .read_resource(&descriptor, &context)
        .await
        .expect_err("production factory requires a committed Control Grant");
    assert_eq!(error.code, "use.plugin.capability_gateway_forbidden");
}

#[cfg(feature = "mcp")]
#[derive(Clone, Debug, Default)]
struct IndependentGatewayClient;

#[cfg(feature = "mcp")]
impl ClientHandler for IndependentGatewayClient {}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_discovers_control_published_gateway_without_shared_package_fs() {
    let fixture = installed_capability_plane_with_projection(
        "operation:capability-plane:independent-client",
        Arc::new(ExactResourceCatalogProjection),
    )
    .await;
    let paths = fixture._owner_fixture.paths.clone();
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let descriptor_snapshot = ControlCapabilityDescriptorSnapshot::new(
        ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation,
            cursor.capability_generation,
            cursor.descriptor_digest.clone(),
        )
        .unwrap(),
        Vec::new(),
        ControlCapabilitySignerPolicy::new(BTreeMap::new()).unwrap(),
    )
    .unwrap();
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&paths)
        .publish(&descriptor_snapshot)
        .await
        .unwrap();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();

    let session = composition
        .reopen_published_capability_gateway(
            composition.production_gateway_invocation_provider(),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .expect("Control must reopen the published Gateway catalog");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    let server_handle = tokio::spawn(
        session.serve_streamable_http(
            listener,
            CapabilityGatewayHttpConfig::for_principal(
                "independent-client-token",
                "agent/independent-rust",
            )
            .unwrap(),
            shutdown.clone(),
        ),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header("independent-client-token"),
    );
    let client = IndependentGatewayClient.serve(transport).await.unwrap();
    let resources = client.list_all_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert!(resources[0].uri.contains("resource"));

    client.cancel().await.unwrap();
    shutdown.cancel();
    server_handle.await.unwrap().unwrap();
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_is_denied_with_wrong_gateway_token() {
    let fixture = installed_capability_plane_with_projection(
        "operation:capability-plane:independent-denied",
        Arc::new(ExactResourceCatalogProjection),
    )
    .await;
    let paths = fixture._owner_fixture.paths.clone();
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let descriptor_snapshot = ControlCapabilityDescriptorSnapshot::new(
        ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation,
            cursor.capability_generation,
            cursor.descriptor_digest.clone(),
        )
        .unwrap(),
        Vec::new(),
        ControlCapabilitySignerPolicy::new(BTreeMap::new()).unwrap(),
    )
    .unwrap();
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&paths)
        .publish(&descriptor_snapshot)
        .await
        .unwrap();
    let composition = super::composition::ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        super::composition::ControlEffectCompositionDependencies {
            runtime_registry: Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: Arc::new(CompositionReadiness),
            catalog_projection: Arc::new(EmptyCatalogProjection),
            flow: Arc::new(UnexpectedDynamicSurfacePort),
            clock: Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let session = composition
        .reopen_published_capability_gateway(
            composition.production_gateway_invocation_provider(),
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    let server_handle = tokio::spawn(
        session.serve_streamable_http(
            listener,
            CapabilityGatewayHttpConfig::for_principal("correct-token", "agent/independent-rust")
                .unwrap(),
            shutdown.clone(),
        ),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header("wrong-token"),
    );
    let error = IndependentGatewayClient
        .serve(transport)
        .await
        .expect_err("wrong bearer token must fail closed before discovery");
    let message = error.to_string().to_lowercase();
    assert!(
        message.contains("401")
            || message.contains("unauthorized")
            || message.contains("auth required"),
        "unexpected denial signal: {message}"
    );

    shutdown.cancel();
    server_handle.await.unwrap().unwrap();
}

#[cfg(feature = "mcp")]
struct GrantToolControlFixture {
    _temporary: tempfile::TempDir,
    paths: ExtensionPaths,
    runtime: Arc<FakeRuntime>,
}

#[cfg(feature = "mcp")]
impl GrantToolControlFixture {
    async fn install() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let installation = control_installation();
        let package_source = temporary.path().join("managed-task-package");
        write_grant_tool_task_package(&package_source).await;
        let candidate =
            ExtensionLifecyclePackage::prepare_local("acme/research", &package_source, true)
                .await
                .unwrap();
        let catalog = verified_grant_tool_catalog(&candidate);
        let package_lock = PluginPackageResolver::new(
            PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
        )
        .resolve(catalog, Vec::new())
        .unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation.clone(),
        )
        .unwrap();
        let artifact_admission = paths
            .artifact_store()
            .acquire_reference_admission()
            .await
            .unwrap();
        paths
            .artifact_store()
            .admit_prepared_package(&artifact_admission, &candidate)
            .await
            .unwrap();
        drop(artifact_admission);

        let surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_string(),
        };
        let placeholder_provider = PlannedProviderEvidence {
            surface: PlanQualifiedSurfaceRef {
                package_id: candidate.package_id().to_string(),
                surface: surface.clone(),
            },
            provider_id: "test-runtime".to_string(),
            provider_build_id: "build-1".to_string(),
            capability_digest: digest('6'),
            semantics_profile_digest: digest('7'),
            enforcement: PlanEnforcementProfile::Sandbox,
        };
        let proposal_seed = super::aggregate_tests::grant_fixtures::reviewed_grant_operation_for(
            &installation,
            "operation:capability-plane:grant-tool-invoke",
            PluginOperationAction::Install,
            None,
            None,
            Some(package_lock.clone()),
            Some(vec![placeholder_provider]),
        );
        let proposal = proposal_seed
            .authorization
            .grant_transition
            .as_ref()
            .and_then(|transition| transition.change_set.changes[0].after.as_ref())
            .expect("grant install must carry a proposal");
        let proposal_digest = proposal.descriptor_digest().unwrap();
        let plan = plan_tool_task_release(
            RuntimeSurfaceContext::new(
                proposal.package_id.clone(),
                proposal.package_digest.clone(),
                PlanScope {
                    kind: PlanScopeKind::Workspace,
                    id: proposal.scope_id.clone(),
                },
                proposal_digest.clone(),
                surface.clone(),
                1,
            )
            .unwrap(),
            &task_surface(),
            &schema_bearing_task_descriptor(),
            artifact(
                &schema_bearing_task_descriptor().artifact.digest,
                &schema_bearing_task_descriptor().artifact.media_type,
            ),
            RuntimeTaskInvocation::new("invoke", Vec::new()).unwrap(),
            grant_tool_task_policy(),
            NetworkMode::None,
        )
        .unwrap();
        assert!(
            plan.tool_schema_attestation().is_some(),
            "Grant Tool plans must carry Runtime schema attestation for strict admission"
        );
        let runtime_capabilities = grant_tool_runtime_capabilities(&plan);
        let runtime = Arc::new(FakeRuntime::new(runtime_capabilities.clone(), true));
        let provider_evidence = PlannedProviderEvidence {
            surface: PlanQualifiedSurfaceRef {
                package_id: candidate.package_id().to_string(),
                surface,
            },
            provider_id: "test-runtime".to_string(),
            provider_build_id: "build-1".to_string(),
            capability_digest: runtime_capabilities_digest(&runtime_capabilities).unwrap(),
            semantics_profile_digest: plan.spec().semantics_profile_digest.clone().unwrap(),
            enforcement: PlanEnforcementProfile::Sandbox,
        };
        let reviewed = super::aggregate_tests::grant_fixtures::reviewed_grant_operation_for(
            &installation,
            "operation:capability-plane:grant-tool-invoke",
            PluginOperationAction::Install,
            None,
            None,
            Some(package_lock),
            Some(vec![provider_evidence.clone()]),
        );
        assert_eq!(
            reviewed
                .authorization
                .grant_transition
                .as_ref()
                .and_then(|transition| transition.change_set.changes[0].after.as_ref())
                .unwrap()
                .descriptor_digest()
                .unwrap(),
            proposal_digest
        );
        let publication = RuntimeSurfacePlanPublication::new(
            RuntimeSurfacePlanKey::from_plan(&plan, &provider_evidence).unwrap(),
            plan,
        )
        .unwrap();

        let mut registry = RuntimeClientRegistry::new();
        registry
            .register(Arc::new(GrantToolRuntimeFactory {
                provider_id: ProviderId::parse("test-runtime").unwrap(),
                client: runtime.clone(),
            }))
            .unwrap();
        let lifecycle = ProductionControlLifecycle::from_extension_paths(
            &paths,
            ProductionControlHostDependencies::with_system_clock(
                Arc::new(registry),
                Arc::new(CompositionReadiness),
                Arc::new(StrictToolCatalogProjection),
                Arc::new(UnexpectedDynamicSurfacePort),
            ),
        )
        .unwrap();
        lifecycle.initialize().await.unwrap();
        let authorization = CognitivePackageAuthorizationEvidence {
            operation_confirmation: reviewed.authorization.operation_confirmation.clone(),
            grant_confirmations: reviewed.authorization.grant_confirmations.clone(),
        };
        let grants = PlannedWorkspaceGrantOperation {
            snapshot: reviewed
                .authorization
                .grant_transition
                .as_ref()
                .unwrap()
                .snapshot
                .clone(),
            change_set: reviewed
                .authorization
                .grant_transition
                .as_ref()
                .unwrap()
                .change_set
                .clone(),
            ceilings: Vec::new(),
        };
        let maintenance = Arc::new(
            a3s_use_extension::StateMaintenanceLock::new(paths.state_root())
                .acquire_shared()
                .await
                .unwrap(),
        );
        lifecycle
            .apply_reviewed_operation(
                &reviewed.envelope,
                &authorization,
                Some(&grants),
                reviewed.reviewed_at_ms,
                reviewed.reviewed_at_ms + 10,
                &[publication],
                maintenance,
            )
            .await
            .unwrap();
        let cursor = lifecycle
            .composition()
            .store()
            .published_capability()
            .await
            .unwrap()
            .unwrap();
        publish_grant_tool_descriptor_snapshot(&paths, &cursor).await;
        // Drop the admitting lifecycle; callers reopen from durable paths.
        drop(lifecycle);
        Self {
            _temporary: temporary,
            paths,
            runtime,
        }
    }

    fn reopen_lifecycle(&self) -> ProductionControlLifecycle {
        let mut registry = RuntimeClientRegistry::new();
        registry
            .register(Arc::new(GrantToolRuntimeFactory {
                provider_id: ProviderId::parse("test-runtime").unwrap(),
                client: self.runtime.clone(),
            }))
            .unwrap();
        let lifecycle = ProductionControlLifecycle::from_extension_paths(
            &self.paths,
            ProductionControlHostDependencies::with_system_clock(
                Arc::new(registry),
                Arc::new(CompositionReadiness),
                Arc::new(StrictToolCatalogProjection),
                Arc::new(UnexpectedDynamicSurfacePort),
            ),
        )
        .unwrap();
        // Reopen against already-initialized Control; initialize is idempotent
        // for an existing root.
        lifecycle
    }

    /// Replace the installed Grant Tool package with a newer local candidate
    /// under the same Control root, publishing a fresh Runtime plan + Grant.
    async fn apply_live_upgrade(&self, lifecycle: &ProductionControlLifecycle) {
        let prior = lifecycle
            .composition()
            .store()
            .current_generation()
            .await
            .unwrap()
            .expect("install must leave a generation");
        let package_source = self._temporary.path().join("managed-task-package-v2");
        write_grant_tool_task_package_at(
            &package_source,
            "2.1.0",
            "# Grant Tool Task fixture v2\n",
        )
        .await;
        let candidate =
            ExtensionLifecyclePackage::prepare_local("acme/research", &package_source, true)
                .await
                .unwrap();
        let catalog = verified_grant_tool_catalog(&candidate);
        let package_lock = PluginPackageResolver::new(
            PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
        )
        .resolve(catalog, Vec::new())
        .unwrap();
        let artifact_admission = self
            .paths
            .artifact_store()
            .acquire_reference_admission()
            .await
            .unwrap();
        self.paths
            .artifact_store()
            .admit_prepared_package(&artifact_admission, &candidate)
            .await
            .unwrap();
        drop(artifact_admission);

        let surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_string(),
        };
        let placeholder_provider = PlannedProviderEvidence {
            surface: PlanQualifiedSurfaceRef {
                package_id: candidate.package_id().to_string(),
                surface: surface.clone(),
            },
            provider_id: "test-runtime".to_string(),
            provider_build_id: "build-1".to_string(),
            capability_digest: digest('6'),
            semantics_profile_digest: digest('7'),
            enforcement: PlanEnforcementProfile::Sandbox,
        };
        let proposal_seed = super::aggregate_tests::grant_fixtures::reviewed_grant_operation_for(
            &prior.snapshot.installation,
            "operation:capability-plane:grant-tool-upgrade",
            PluginOperationAction::Upgrade,
            Some(&prior),
            None,
            Some(package_lock.clone()),
            Some(vec![placeholder_provider]),
        );
        let proposal = proposal_seed
            .authorization
            .grant_transition
            .as_ref()
            .and_then(|transition| transition.change_set.changes[0].after.as_ref())
            .expect("grant upgrade must carry a proposal");
        let proposal_digest = proposal.descriptor_digest().unwrap();
        // Upgrade allocates the next package lifecycle incarnation (install was 1).
        let lifecycle_generation = prior
            .package_lifecycles
            .iter()
            .map(|lifecycle| lifecycle.lifecycle_generation)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let plan = plan_tool_task_release(
            RuntimeSurfaceContext::new(
                proposal.package_id.clone(),
                proposal.package_digest.clone(),
                PlanScope {
                    kind: PlanScopeKind::Workspace,
                    id: proposal.scope_id.clone(),
                },
                proposal_digest.clone(),
                surface.clone(),
                lifecycle_generation,
            )
            .unwrap(),
            &task_surface(),
            &schema_bearing_task_descriptor(),
            artifact(
                &schema_bearing_task_descriptor().artifact.digest,
                &schema_bearing_task_descriptor().artifact.media_type,
            ),
            RuntimeTaskInvocation::new("invoke", Vec::new()).unwrap(),
            grant_tool_task_policy(),
            NetworkMode::None,
        )
        .unwrap();
        assert!(plan.tool_schema_attestation().is_some());
        // Reuse the fixture FakeRuntime capability digest so the upgraded
        // provider selection still joins the same provider lease.
        let runtime_capabilities = self.runtime.capabilities().await.unwrap();
        let provider_evidence = PlannedProviderEvidence {
            surface: PlanQualifiedSurfaceRef {
                package_id: candidate.package_id().to_string(),
                surface,
            },
            provider_id: "test-runtime".to_string(),
            provider_build_id: "build-1".to_string(),
            capability_digest: runtime_capabilities_digest(&runtime_capabilities).unwrap(),
            semantics_profile_digest: plan.spec().semantics_profile_digest.clone().unwrap(),
            enforcement: PlanEnforcementProfile::Sandbox,
        };
        let reviewed = super::aggregate_tests::grant_fixtures::reviewed_grant_operation_for(
            &prior.snapshot.installation,
            "operation:capability-plane:grant-tool-upgrade",
            PluginOperationAction::Upgrade,
            Some(&prior),
            None,
            Some(package_lock),
            Some(vec![provider_evidence.clone()]),
        );
        assert_eq!(
            reviewed
                .authorization
                .grant_transition
                .as_ref()
                .and_then(|transition| transition.change_set.changes[0].after.as_ref())
                .unwrap()
                .descriptor_digest()
                .unwrap(),
            proposal_digest
        );
        let publication = RuntimeSurfacePlanPublication::new(
            RuntimeSurfacePlanKey::from_plan(&plan, &provider_evidence).unwrap(),
            plan,
        )
        .unwrap();
        let authorization = CognitivePackageAuthorizationEvidence {
            operation_confirmation: reviewed.authorization.operation_confirmation.clone(),
            grant_confirmations: reviewed.authorization.grant_confirmations.clone(),
        };
        let grants = PlannedWorkspaceGrantOperation {
            snapshot: reviewed
                .authorization
                .grant_transition
                .as_ref()
                .unwrap()
                .snapshot
                .clone(),
            change_set: reviewed
                .authorization
                .grant_transition
                .as_ref()
                .unwrap()
                .change_set
                .clone(),
            ceilings: Vec::new(),
        };
        let maintenance = Arc::new(
            a3s_use_extension::StateMaintenanceLock::new(self.paths.state_root())
                .acquire_shared()
                .await
                .unwrap(),
        );
        lifecycle
            .apply_reviewed_operation(
                &reviewed.envelope,
                &authorization,
                Some(&grants),
                reviewed.reviewed_at_ms,
                reviewed.reviewed_at_ms + 10,
                &[publication],
                maintenance,
            )
            .await
            .unwrap();
        let cursor = lifecycle
            .composition()
            .store()
            .published_capability()
            .await
            .unwrap()
            .unwrap();
        publish_grant_tool_descriptor_snapshot(&self.paths, &cursor).await;
    }
}

#[cfg(feature = "mcp")]
async fn assert_independent_convert_tool_invoke(lifecycle: &ProductionControlLifecycle) {
    let session = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap()
        .expect("Control must reopen the Grant-backed Tool catalog");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    let server_handle = tokio::spawn(
        session.serve_streamable_http(
            listener,
            CapabilityGatewayHttpConfig::for_principal(
                "grant-tool-token",
                "agent/independent-rust",
            )
            .unwrap(),
            shutdown.clone(),
        ),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header("grant-tool-token"),
    );
    let client = IndependentGatewayClient.serve(transport).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "convert");
    let result = client
        .call_tool(CallToolRequestParam {
            name: "convert".into(),
            arguments: Some(
                serde_json::json!({ "args": [] })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        result.is_error,
        Some(false),
        "Tool Task invoke failed: {:?}",
        result.structured_content
    );
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("exitCode"))
            .and_then(|value| value.as_i64()),
        Some(0)
    );
    client.cancel().await.unwrap();
    shutdown.cancel();
    server_handle.await.unwrap().unwrap();
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_tool_publishes_signed_schema_bearing_descriptor_snapshot() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let cursor = lifecycle
        .composition()
        .store()
        .published_capability()
        .await
        .unwrap()
        .unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&fixture.paths);
    let key = ControlCapabilityDescriptorSnapshotKey::new(
        cursor.installation.clone(),
        cursor.installation_generation,
        cursor.capability_generation,
        cursor.descriptor_digest.clone(),
    )
    .unwrap();
    let snapshot = store
        .get(&key)
        .await
        .unwrap()
        .expect("Grant Tool install must publish a descriptor snapshot");
    assert!(
        snapshot.signed_descriptions().is_some(),
        "Grant Tool must retain signed envelopes, not proof-only snapshots"
    );
    let signer = "registry/acme";
    let (_, trust_store) = signed_descriptor_for(
        snapshot.proofs()[0].descriptor().clone(),
        signer,
        1_000,
        2_000,
    );
    let proofs = snapshot.reverify_signed(&trust_store, 1_500).unwrap();
    assert_eq!(proofs.len(), 1);
    match &proofs[0].descriptor().capability {
        CapabilityDescriptorKind::Tool {
            input_schema,
            output_schema,
            runtime_descriptor_digest: Some(runtime_digest),
            ..
        } => {
            assert_eq!(input_schema, &grant_tool_input_schema());
            assert_eq!(output_schema, &grant_tool_output_schema());
            assert!(runtime_digest.starts_with("sha256:"));
        }
        other => panic!("expected schema-bearing Tool descriptor, got {other:?}"),
    }
    let projector = ControlCapabilityDescriptorProjection::from_signed_snapshot_store_at(
        store,
        trust_store,
        1_500,
    )
    .unwrap();
    // Re-admit through the product SignedDurable projector against the same
    // published catalog identity the independent clients already invoke.
    let published = CapabilityGatewayCatalogStore::from_extension_paths(&fixture.paths)
        .get(&cursor.catalog.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        published.descriptors()[0].capability,
        proofs[0].descriptor().capability
    );
    drop(projector);
    assert_independent_convert_tool_invoke(&lifecycle).await;
    drop(lifecycle);

    // Product hosts inject Registry trust material into the signed catalog
    // projector. Construction must succeed against the published signed
    // snapshot root (invoke still uses the durable catalog payload).
    let (_, trust_store_for_host) =
        signed_descriptor_for(published.descriptors()[0].clone(), signer, 1_000, 2_000);
    let dependencies = ProductionControlHostDependencies::standalone_with_signed_catalog(
        &fixture.paths,
        Arc::new(RuntimeClientRegistry::new()),
        trust_store_for_host,
        None,
    )
    .expect("signed catalog production dependencies must compose");
    let _ = ProductionControlLifecycle::from_extension_paths(&fixture.paths, dependencies)
        .expect("signed catalog production lifecycle must compose");
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_invokes_tool_task_under_committed_control_grant() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    assert_eq!(
        lifecycle
            .composition()
            .store()
            .current_generation()
            .await
            .unwrap()
            .unwrap()
            .grants
            .len(),
        1
    );
    assert_independent_convert_tool_invoke(&lifecycle).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_invokes_grant_tool_after_control_process_restart() {
    let fixture = GrantToolControlFixture::install().await;
    // Model process restart: new lifecycle composition over the same durable
    // Control root, FakeRuntime registry, and published catalog/receipts.
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    assert!(lifecycle
        .composition()
        .store()
        .published_capability()
        .await
        .unwrap()
        .is_some());
    assert_independent_convert_tool_invoke(&lifecycle).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_grant_tool_denied_for_foreign_installation_scope() {
    let fixture = GrantToolControlFixture::install().await;
    // Peer installation with its own Control root must not observe or serve the
    // Grant Tool publication committed under workspace-01.
    let foreign_temporary = tempfile::tempdir().unwrap();
    let foreign_installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::Workspace,
        "workspace-99",
    )
    .unwrap();
    let foreign_paths = ExtensionPaths::new(
        foreign_temporary.path().join("data"),
        foreign_temporary.path().join("state"),
        foreign_installation,
    )
    .unwrap();
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(GrantToolRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: fixture.runtime.clone(),
        }))
        .unwrap();
    let foreign = ProductionControlLifecycle::from_extension_paths(
        &foreign_paths,
        ProductionControlHostDependencies::with_system_clock(
            Arc::new(registry),
            Arc::new(CompositionReadiness),
            Arc::new(StrictToolCatalogProjection),
            Arc::new(UnexpectedDynamicSurfacePort),
        ),
    )
    .unwrap();
    foreign.initialize().await.unwrap();
    assert!(
        foreign
            .composition()
            .store()
            .published_capability()
            .await
            .unwrap()
            .is_none(),
        "a foreign installation scope must not observe another scope's published cursor"
    );
    let opened = foreign
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap();
    assert!(
        opened.is_none(),
        "foreign installation scope must fail closed before serving Grant Tool invoke"
    );
    // The original scope remains serveable after the foreign probe.
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    assert_independent_convert_tool_invoke(&lifecycle).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_grant_tool_fails_closed_after_uninstall() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let prior = lifecycle
        .composition()
        .store()
        .current_generation()
        .await
        .unwrap()
        .expect("install must leave a generation");
    let uninstall = super::aggregate_tests::grant_fixtures::reviewed_grant_operation_for(
        &prior.snapshot.installation,
        "operation:capability-plane:grant-tool-uninstall",
        PluginOperationAction::Uninstall,
        Some(&prior),
        None,
        None,
        None,
    );
    let authorization = CognitivePackageAuthorizationEvidence {
        operation_confirmation: uninstall.authorization.operation_confirmation.clone(),
        grant_confirmations: uninstall.authorization.grant_confirmations.clone(),
    };
    let grants = uninstall
        .authorization
        .grant_transition
        .as_ref()
        .map(|transition| PlannedWorkspaceGrantOperation {
            snapshot: transition.snapshot.clone(),
            change_set: transition.change_set.clone(),
            ceilings: Vec::new(),
        });
    let maintenance = Arc::new(
        a3s_use_extension::StateMaintenanceLock::new(fixture.paths.state_root())
            .acquire_shared()
            .await
            .unwrap(),
    );
    lifecycle
        .apply_reviewed_operation(
            &uninstall.envelope,
            &authorization,
            grants.as_ref(),
            uninstall.reviewed_at_ms,
            uninstall.reviewed_at_ms + 10,
            &[],
            maintenance,
        )
        .await
        .unwrap();
    let after = lifecycle
        .composition()
        .store()
        .current_generation()
        .await
        .unwrap()
        .expect("uninstall must commit");
    assert!(after.grants.is_empty());
    // Catalog may still reopen empty, but Grant-backed Tool invoke must not
    // succeed without a committed Grant.
    let session = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap();
    if let Some(session) = session {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let shutdown = CancellationToken::new();
        let server_handle = tokio::spawn(
            session.serve_streamable_http(
                listener,
                CapabilityGatewayHttpConfig::for_principal(
                    "grant-tool-token",
                    "agent/independent-rust",
                )
                .unwrap(),
                shutdown.clone(),
            ),
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
                .auth_header("grant-tool-token"),
        );
        let client = IndependentGatewayClient.serve(transport).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        assert!(
            tools.is_empty(),
            "uninstall must retire Grant Tool discovery"
        );
        client.cancel().await.unwrap();
        shutdown.cancel();
        server_handle.await.unwrap().unwrap();
    }
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_invokes_grant_tool_after_live_upgrade() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let prior_generation = lifecycle
        .composition()
        .store()
        .published_capability()
        .await
        .unwrap()
        .unwrap()
        .capability_generation;
    fixture.apply_live_upgrade(&lifecycle).await;
    let after = lifecycle
        .composition()
        .store()
        .published_capability()
        .await
        .unwrap()
        .unwrap();
    assert!(
        after.capability_generation > prior_generation,
        "live upgrade must advance the published capability generation"
    );
    assert_eq!(
        lifecycle
            .composition()
            .store()
            .current_generation()
            .await
            .unwrap()
            .unwrap()
            .grants
            .len(),
        1
    );
    assert_independent_convert_tool_invoke(&lifecycle).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_rust_client_grant_tool_prior_generation_drains_on_live_upgrade() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let prior_session = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap()
        .expect("gen1 Grant Tool catalog must open");
    let prior_key = prior_session.current_key().unwrap();
    // Package-generation leases retained by the gen1 Gateway session fence
    // Upgrade Remove/Prepare drain. Prior-generation drain is the host
    // releasing that endpoint before the Replace cutover can complete.
    drop(prior_session);
    fixture.apply_live_upgrade(&lifecycle).await;
    let target = lifecycle
        .composition()
        .store()
        .published_capability()
        .await
        .unwrap()
        .unwrap();
    assert!(
        target.capability_generation > prior_key.generation,
        "drained prior generation must be replaced by a newer published catalog"
    );
    let replacement = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap()
        .expect("upgraded Grant Tool catalog must open");
    assert_ne!(
        replacement.current_key().unwrap(),
        prior_key,
        "post-drain Gateway session must not reuse the prior publication key"
    );
    assert_eq!(
        replacement.current_key().unwrap().digest,
        target.catalog.digest
    );
    assert_independent_convert_tool_invoke(&lifecycle).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_retained_gateway_cutover_activates_during_upgrade_drain() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let options = CapabilityGatewayCompositionOptions::default();
    let session = lifecycle
        .open_published_capability_gateway(options.clone())
        .await
        .unwrap()
        .expect("gen1 Grant Tool catalog must open");
    let prior_key = session.current_key().unwrap();
    // Retain the live session across upgrade: production drain must activate
    // cutover after CapabilityCutover so prior-generation leases release before
    // Remove/Prepare (without requiring the host to drop the endpoint first).
    lifecycle.attach_retained_gateway_cutover(session.clone(), options);
    fixture.apply_live_upgrade(&lifecycle).await;
    let next_key = session.current_key().unwrap();
    assert_ne!(
        next_key, prior_key,
        "retained Gateway must swap to the upgraded Control publication during drain"
    );
    assert!(
        next_key.generation > prior_key.generation,
        "cutover activation must advance the live session generation"
    );
    lifecycle.clear_retained_gateway_cutover();
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_gateway_reconcile_is_unchanged_for_the_current_grant_tool_publication() {
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let session = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap()
        .expect("Grant Tool catalog must open");
    let key = session.current_key().unwrap();
    let reconciled = lifecycle
        .reconcile_published_capability_gateway(
            &session,
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
        .unwrap()
        .expect("current publication must reconcile");
    assert!(matches!(
        reconciled,
        super::composition::ControlCapabilityGatewayReconciliation::Unchanged(_)
    ));
    assert_eq!(session.current_key().unwrap(), key);
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_cognitive_package_manager_gateway_face_after_grant_tool_install() {
    use std::time::Duration;

    use a3s_use_extension::ExtensionRegistry;
    use tokio_util::sync::CancellationToken;

    use crate::cognitive_package::CognitivePackageManager;

    let fixture = GrantToolControlFixture::install().await;
    // Public embedding face only — no private ProductionControlLifecycle.
    let manager =
        CognitivePackageManager::new(ExtensionRegistry::new(fixture.paths.clone())).unwrap();
    let options = CapabilityGatewayCompositionOptions::default();
    let session = manager
        .open_published_capability_gateway(options.clone())
        .await
        .unwrap()
        .expect("Grant Tool catalog must open through the public manager face");
    let key = session.current_key().unwrap();
    let _activation = manager
        .gateway_cutover_activation(session.clone(), options.clone())
        .await
        .expect("public cutover activation must be constructible for a retained session");
    let shutdown = CancellationToken::new();
    let watch_manager = manager.clone();
    let watch_session = session.clone();
    let watch_shutdown = shutdown.clone();
    let watch = tokio::spawn(async move {
        watch_manager
            .watch_and_reconcile_published_capability_gateway(
                &watch_session,
                options,
                &watch_shutdown,
                Duration::from_millis(20),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(session.current_key().unwrap(), key);
    shutdown.cancel();
    watch.await.unwrap().unwrap();
    manager
        .drain_and_retain_published_capability_gateway(&session, Duration::from_secs(1))
        .await
        .expect("public drain must succeed for the Control-selected publication");
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_retained_gateway_watch_reconciles_then_drains_on_shutdown() {
    use tokio_util::sync::CancellationToken;

    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let session = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap()
        .expect("Grant Tool catalog must open");
    let key = session.current_key().unwrap();
    let shutdown = CancellationToken::new();
    let watch_lifecycle = lifecycle.clone();
    let watch_session = session.clone();
    let watch_shutdown = shutdown.clone();
    let watch = tokio::spawn(async move {
        watch_lifecycle
            .watch_and_reconcile_published_capability_gateway(
                &watch_session,
                CapabilityGatewayCompositionOptions::default(),
                &watch_shutdown,
                std::time::Duration::from_millis(20),
            )
            .await
    });
    // One poll tick must observe Unchanged for the current publication.
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert_eq!(session.current_key().unwrap(), key);
    shutdown.cancel();
    watch.await.unwrap().unwrap();
    let retention = lifecycle
        .drain_and_retain_published_capability_gateway(
            &session,
            std::time::Duration::from_secs(1),
            &[],
            &[],
        )
        .await
        .expect("shutdown drain must retain the Control-selected payloads");
    assert!(retention.catalog.retained_record_count >= 1);
}

#[cfg(feature = "mcp")]
fn independent_clients_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/independent_clients")
}

#[cfg(feature = "mcp")]
fn ensure_typescript_client_deps() {
    let ts_root = independent_clients_root().join("ts");
    if ts_root
        .join("node_modules")
        .join("@modelcontextprotocol")
        .exists()
    {
        return;
    }
    let status = std::process::Command::new("npm")
        .args(["install", "--no-fund", "--no-audit"])
        .current_dir(&ts_root)
        .status()
        .expect("npm must be available to install the independent TypeScript MCP client");
    assert!(
        status.success(),
        "npm install failed for independent TS client"
    );
}

#[cfg(feature = "mcp")]
fn ensure_python_client_deps() {
    let requirements = independent_clients_root().join("python/requirements.txt");
    let mut command = python_command();
    let status = command
        .args([
            "-m",
            "pip",
            "install",
            "-r",
            requirements.to_str().unwrap(),
            "--quiet",
        ])
        .status()
        .expect("Python must be available to install the independent MCP client");
    assert!(
        status.success(),
        "pip install failed for independent Python MCP client"
    );
}

#[cfg(feature = "mcp")]
fn python_command() -> std::process::Command {
    for (executable, prefix) in [
        ("python3", &[][..]),
        ("python", &[][..]),
        ("py", &["-3"][..]),
    ] {
        let mut probe = std::process::Command::new(executable);
        probe.args(prefix).arg("--version");
        if probe.output().is_ok_and(|output| output.status.success()) {
            let mut command = std::process::Command::new(executable);
            command.args(prefix);
            return command;
        }
    }
    panic!("Python 3 is required for the independent Python MCP client");
}

#[cfg(feature = "mcp")]
async fn assert_external_client_invokes_convert(
    lifecycle: &ProductionControlLifecycle,
    launch: impl FnOnce(&str, &str) -> std::process::Output + Send + 'static,
) {
    let session = lifecycle
        .open_published_capability_gateway(CapabilityGatewayCompositionOptions::default())
        .await
        .unwrap()
        .expect("Control must reopen the Grant-backed Tool catalog");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    let server_handle = tokio::spawn(
        session.serve_streamable_http(
            listener,
            CapabilityGatewayHttpConfig::for_principal(
                "grant-tool-token",
                "agent/independent-external",
            )
            .unwrap(),
            shutdown.clone(),
        ),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let endpoint = format!("http://127.0.0.1:{port}/mcp");
    let output = tokio::task::spawn_blocking(move || launch(&endpoint, "grant-tool-token"))
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "independent client failed: status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    shutdown.cancel();
    server_handle.await.unwrap().unwrap();
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_typescript_client_invokes_tool_task_under_committed_control_grant() {
    ensure_typescript_client_deps();
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let script = independent_clients_root().join("ts/call_convert.mjs");
    assert_external_client_invokes_convert(&lifecycle, move |endpoint, token| {
        std::process::Command::new("node")
            .arg(&script)
            .env("A3S_GATEWAY_ENDPOINT", endpoint)
            .env("A3S_GATEWAY_TOKEN", token)
            .output()
            .expect("node must launch the independent TypeScript MCP client")
    })
    .await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_python_client_invokes_tool_task_under_committed_control_grant() {
    ensure_python_client_deps();
    let fixture = GrantToolControlFixture::install().await;
    let lifecycle = fixture.reopen_lifecycle();
    lifecycle.initialize().await.unwrap();
    let script = independent_clients_root().join("python/call_convert.py");
    assert_external_client_invokes_convert(&lifecycle, move |endpoint, token| {
        let mut command = python_command();
        command
            .arg(&script)
            .env("A3S_GATEWAY_ENDPOINT", endpoint)
            .env("A3S_GATEWAY_TOKEN", token)
            .output()
            .expect("Python must launch the independent MCP client")
    })
    .await;
}

#[tokio::test]
async fn catalog_projection_cannot_publish_an_unreviewed_surface() {
    let installation = control_installation();
    let (owner_fixture, artifact_admission) =
        knowledge_owner_fixture_for(installation.clone()).await;
    let store = ControlStore::from_extension_paths(&owner_fixture.paths).unwrap();
    store.initialize().await.unwrap();
    let installed = operation("operation:capability-plane:unauthorized-catalog");
    store.register_operation(installed.clone()).await.unwrap();
    store
        .commit_transition(transition(installation, &installed))
        .await
        .unwrap();
    drop(artifact_admission);

    let catalogs = CapabilityGatewayCatalogStore::from_extension_paths(&owner_fixture.paths);
    let plane = Arc::new(
        ControlCapabilityPlaneEffectPort::new(
            store.clone(),
            catalogs.clone(),
            Arc::new(UnauthorizedCatalogProjection),
        )
        .unwrap(),
    );
    let knowledge = Arc::new(ControlOkfKnowledgeEffectPort::new(
        owner_fixture.paths.artifact_store(),
        owner_fixture.client.clone(),
        owner_fixture.bindings.clone(),
    ));
    let static_surfaces = Arc::new(ControlStaticSurfaceEffectPort::new(
        owner_fixture.paths.artifact_store(),
    ));
    let unexpected = Arc::new(UnexpectedDynamicSurfacePort);
    let dispatcher = ControlEffectDispatcher::new(
        store.clone(),
        ControlEffectPorts::new(
            plane.clone(),
            plane,
            unexpected.clone(),
            unexpected,
            knowledge,
            static_surfaces.clone(),
            static_surfaces,
        ),
        Arc::new(SystemControlEffectClock),
    );
    for sequence in 0..2_u32 {
        assert_dispatch(
            &dispatcher,
            &installed,
            &format!("claim:capability-plane:unauthorized:{sequence}"),
            sequence,
            1,
            ControlEffectOutcome::Applied,
            false,
        )
        .await;
    }
    assert_dispatch(
        &dispatcher,
        &installed,
        "claim:capability-plane:unauthorized:cutover",
        2,
        1,
        ControlEffectOutcome::Rejected,
        false,
    )
    .await;

    assert!(store.published_capability().await.unwrap().is_none());
    assert!(catalogs.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn strict_descriptor_projection_binds_a_description_to_committed_owner_evidence() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:descriptor-projector").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-projector",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let proof = CapabilityDescriptionProof::from_verified(descriptor.clone(), signer).unwrap();
    let projector = ControlCapabilityDescriptorProjection::new(
        vec![proof.clone()],
        signer_policy_for(descriptor.package_id.as_str(), signer),
    )
    .unwrap();
    ControlCapabilityPlaneEffectPort::with_verified_descriptions(
        fixture.store.clone(),
        CapabilityGatewayCatalogStore::from_extension_paths(&fixture._owner_fixture.paths),
        vec![proof],
        signer_policy_for(descriptor.package_id.as_str(), signer),
    )
    .unwrap();

    let first = projector.project_catalog(&authority).unwrap();
    let second = projector.project_catalog(&authority).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.descriptors, vec![descriptor.clone()]);
    assert_eq!(
        first.descriptor_digest().unwrap(),
        second.descriptor_digest().unwrap()
    );
    assert!(matches!(
        projector.project(&authority).await,
        ControlEffectPortOutcome::Applied(_)
    ));
}

#[tokio::test]
async fn strict_descriptor_projection_rejects_route_and_dependency_substitution() {
    let fixture = prepared_capability_plane("operation:capability-plane:descriptor-tamper").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-tamper",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let policy = signer_policy_for(descriptor.package_id.as_str(), signer);

    let mut route_tampered = descriptor.clone();
    route_tampered.invocation_ref = InvocationRef::derive(
        &route_tampered.package_id,
        &route_tampered.surface,
        route_tampered.generation,
        &digest('0'),
    )
    .unwrap();
    let route_projector = ControlCapabilityDescriptorProjection::new(
        vec![CapabilityDescriptionProof::from_verified(route_tampered, signer).unwrap()],
        policy.clone(),
    )
    .unwrap();
    let route_error = route_projector.project_catalog(&authority).unwrap_err();
    assert_eq!(
        route_error.code,
        "use.control_store.capability_descriptor_projection_invalid"
    );

    let mut dependency_tampered = descriptor.clone();
    dependency_tampered.dependencies = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Skill,
        id: "substituted-dependency".to_owned(),
    }];
    let dependency_projector = ControlCapabilityDescriptorProjection::new(
        vec![CapabilityDescriptionProof::from_verified(dependency_tampered, signer).unwrap()],
        policy,
    )
    .unwrap();
    let dependency_error = dependency_projector
        .project_catalog(&authority)
        .unwrap_err();
    assert_eq!(
        dependency_error.code,
        "use.control_store.capability_descriptor_projection_invalid"
    );

    let mut forged_authority = authority.clone();
    let super::model::ControlEffectSubject::Surface { package_digest, .. } =
        &mut forged_authority.materializations[0].intent.subject
    else {
        panic!("the fixture's first materialization must be a surface");
    };
    *package_digest = digest('0');
    let forged_projector = ControlCapabilityDescriptorProjection::new(
        vec![CapabilityDescriptionProof::from_verified(descriptor, signer).unwrap()],
        signer_policy_for("acme/knowledge", signer),
    )
    .unwrap();
    assert!(forged_projector.project_catalog(&forged_authority).is_err());
}

#[tokio::test]
async fn strict_descriptor_projection_requires_the_package_signer_allowlist() {
    let fixture = prepared_capability_plane("operation:capability-plane:descriptor-signer").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-signer",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let projector = ControlCapabilityDescriptorProjection::new(
        vec![CapabilityDescriptionProof::from_verified(
            descriptor.clone(),
            "registry/unauthorized",
        )
        .unwrap()],
        signer_policy_for(descriptor.package_id.as_str(), "registry/acme"),
    )
    .unwrap();

    let first = projector.project(&authority).await;
    let second = projector.project(&authority).await;
    let (first_failure, second_failure) = match (first, second) {
        (ControlEffectPortOutcome::Rejected(first), ControlEffectPortOutcome::Rejected(second)) => {
            (first, second)
        }
        _ => panic!("an unauthorized signer must be rejected before publication"),
    };
    assert_eq!(
        first_failure.error_code,
        "use.control_store.capability_descriptor_projection_invalid"
    );
    assert_eq!(first_failure, second_failure);
}

#[tokio::test]
async fn descriptor_snapshot_store_replays_the_exact_proof_set_after_restart() {
    let fixture = prepared_capability_plane("operation:capability-plane:descriptor-snapshot").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let proof = CapabilityDescriptionProof::from_verified(descriptor.clone(), signer).unwrap();
    let policy = signer_policy_for(descriptor.package_id.as_str(), signer);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let snapshot =
        ControlCapabilityDescriptorSnapshot::new(key.clone(), vec![proof], policy).unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let first_publication = store.publish(&snapshot).await.unwrap();
    first_publication.validate().unwrap();

    // Reconstruct both store and projector to model a process restart. The
    // result must come from the immutable snapshot, not a live Registry view.
    let reopened = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    assert_eq!(reopened.get(&key).await.unwrap(), Some(snapshot.clone()));
    assert_eq!(reopened.get(&key).await.unwrap(), Some(snapshot.clone()));
    let _plane = ControlCapabilityPlaneEffectPort::with_descriptor_snapshot_store(
        fixture.store.clone(),
        CapabilityGatewayCatalogStore::from_extension_paths(&fixture._owner_fixture.paths),
        reopened.clone(),
    )
    .unwrap();
    let projector = ControlCapabilityDescriptorProjection::from_snapshot_store(reopened).unwrap();
    let ControlEffectPortOutcome::Applied(catalog) = projector.project(&authority).await else {
        panic!("the exact durable proof snapshot must project");
    };
    assert_eq!(catalog.descriptors, vec![descriptor]);
}

#[tokio::test]
async fn descriptor_snapshot_retention_is_plan_bound_and_idempotent() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:descriptor-snapshot-retention").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot-retention",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let proof = CapabilityDescriptionProof::from_verified(descriptor.clone(), signer).unwrap();
    let policy = signer_policy_for(descriptor.package_id.as_str(), signer);
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );

    // Two keys deliberately point at the same verified evidence. Their
    // content-addressed records still differ because the committed Control
    // identity is part of each immutable snapshot.
    let key_one = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let key_two = ControlCapabilityDescriptorSnapshotKey::new(
        key_one.installation.clone(),
        key_one.installation_generation + 1,
        key_one.capability_generation + 1,
        digest('a'),
    )
    .unwrap();
    let snapshot_one = ControlCapabilityDescriptorSnapshot::new(
        key_one.clone(),
        vec![proof.clone()],
        policy.clone(),
    )
    .unwrap();
    let snapshot_two =
        ControlCapabilityDescriptorSnapshot::new(key_two.clone(), vec![proof], policy).unwrap();
    let publication_one = store.publish(&snapshot_one).await.unwrap();
    let publication_two = store.publish(&snapshot_two).await.unwrap();

    let plan = store
        .plan_retention(std::slice::from_ref(&publication_two.snapshot_digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    assert_eq!(plan.before_record_count, 2);
    assert_eq!(plan.retain.len(), 1);
    assert_eq!(plan.remove.len(), 1);
    assert_eq!(plan.retain[0].digest, publication_two.snapshot_digest);
    assert_eq!(plan.remove[0].digest, publication_one.snapshot_digest);

    let result = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(result.changed);
    assert_eq!(result.plan_digest, plan_digest);
    assert_eq!(result.removed.len(), 1);
    assert_eq!(result.removed[0].digest, publication_one.snapshot_digest);
    assert_eq!(result.retained_record_count, 1);
    assert_eq!(store.get(&key_one).await.unwrap(), None);
    assert_eq!(store.get(&key_two).await.unwrap(), Some(snapshot_two));
    assert!(store.recover_retention().await.unwrap().is_none());

    // Reapplying the exact reviewed plan is a no-op, which makes callers safe
    // to retry after an acknowledged response or a transport interruption.
    let replay = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(!replay.changed);
    assert!(replay.removed.is_empty());
    assert_eq!(replay.retained_record_count, 1);
}

#[tokio::test]
async fn descriptor_snapshot_clean_restore_publishes_exact_set_and_replays() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:descriptor-snapshot-clean-restore")
            .await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot-clean-restore",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let proof = CapabilityDescriptionProof::from_verified(descriptor, signer).unwrap();
    let policy = signer_policy_for(proof.descriptor.package_id.as_str(), signer);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let snapshot =
        ControlCapabilityDescriptorSnapshot::new(key.clone(), vec![proof], policy).unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let plan = store
        .plan_clean_restore(std::slice::from_ref(&snapshot))
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let result = store
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&snapshot),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.restored_record_count, 1);
    assert_eq!(store.get(&key).await.unwrap(), Some(snapshot.clone()));

    let replay = store
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&snapshot),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.plan_digest, result.plan_digest);
}

#[tokio::test]
async fn descriptor_snapshot_clean_restore_refuses_existing_owner_state() {
    let fixture = prepared_capability_plane(
        "operation:capability-plane:descriptor-snapshot-clean-restore-conflict",
    )
    .await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot-clean-restore-conflict",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let proof = CapabilityDescriptionProof::from_verified(descriptor, signer).unwrap();
    let policy = signer_policy_for(proof.descriptor.package_id.as_str(), signer);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let existing =
        ControlCapabilityDescriptorSnapshot::new(key, vec![proof.clone()], policy.clone()).unwrap();
    let requested_key = ControlCapabilityDescriptorSnapshotKey::new(
        authority.generation.snapshot.installation.clone(),
        authority.generation.snapshot.generation + 1,
        authority.generation.capability.generation + 1,
        digest('a'),
    )
    .unwrap();
    let requested =
        ControlCapabilityDescriptorSnapshot::new(requested_key, vec![proof], policy).unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    store.publish(&existing).await.unwrap();
    let plan = store
        .plan_clean_restore(std::slice::from_ref(&requested))
        .unwrap();
    let error = store
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&requested),
            &plan.descriptor_digest().unwrap(),
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_descriptor_snapshot_restore_target_not_empty"
    );
    assert_eq!(store.keys().await.unwrap().len(), 1);
}

#[tokio::test]
async fn signed_descriptor_snapshot_clean_restore_requires_current_trust_verification() {
    let fixture = prepared_capability_plane(
        "operation:capability-plane:signed-descriptor-snapshot-clean-restore",
    )
    .await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:signed-descriptor-snapshot-clean-restore",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let (signed, trust_store) = signed_descriptor_for(descriptor, signer, 1_000, 2_000);
    let policy = signer_policy_for(signed.descriptor().package_id.as_str(), signer);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let snapshot = ControlCapabilityDescriptorSnapshot::new_signed(
        key,
        vec![signed],
        policy,
        &trust_store,
        1_500,
    )
    .unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let plan = store
        .plan_clean_restore(std::slice::from_ref(&snapshot))
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let error = store
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&snapshot),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_descriptor_snapshot_restore_invalid"
    );
    assert!(store.keys().await.unwrap().is_empty());

    let result = store
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&snapshot),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::Signed {
                trust_store: &trust_store,
                now_unix_seconds: 1_500,
            },
        )
        .await
        .unwrap();
    assert!(result.changed);
}

#[tokio::test]
async fn descriptor_snapshot_retention_rejects_a_stale_inventory() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:descriptor-snapshot-retention-stale")
            .await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot-retention-stale",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let proof = CapabilityDescriptionProof::from_verified(descriptor.clone(), signer).unwrap();
    let policy = signer_policy_for(descriptor.package_id.as_str(), signer);
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let key_one = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let key_two = ControlCapabilityDescriptorSnapshotKey::new(
        key_one.installation.clone(),
        key_one.installation_generation + 1,
        key_one.capability_generation + 1,
        digest('a'),
    )
    .unwrap();
    let key_three = ControlCapabilityDescriptorSnapshotKey::new(
        key_one.installation.clone(),
        key_one.installation_generation + 2,
        key_one.capability_generation + 2,
        digest('b'),
    )
    .unwrap();
    let snapshot_one = ControlCapabilityDescriptorSnapshot::new(
        key_one.clone(),
        vec![proof.clone()],
        policy.clone(),
    )
    .unwrap();
    let snapshot_two = ControlCapabilityDescriptorSnapshot::new(
        key_two.clone(),
        vec![proof.clone()],
        policy.clone(),
    )
    .unwrap();
    let snapshot_three =
        ControlCapabilityDescriptorSnapshot::new(key_three.clone(), vec![proof], policy).unwrap();
    let publication_one = store.publish(&snapshot_one).await.unwrap();
    let publication_two = store.publish(&snapshot_two).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&publication_two.snapshot_digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    // A new immutable record invalidates the reviewed inventory. No record is
    // removed, and the exact stale-plan error remains retryable by replanning.
    store.publish(&snapshot_three).await.unwrap();
    let error = store
        .apply_retention(&plan, &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_descriptor_snapshot_retention_stale"
    );
    assert_eq!(store.get(&key_one).await.unwrap(), Some(snapshot_one));
    assert_eq!(store.get(&key_two).await.unwrap(), Some(snapshot_two));
    assert_eq!(store.get(&key_three).await.unwrap(), Some(snapshot_three));
    assert_eq!(
        publication_one.key.installation,
        publication_two.key.installation
    );
}

#[tokio::test]
async fn signed_descriptor_snapshot_replays_only_after_current_policy_verification() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:signed-descriptor-snapshot").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:signed-descriptor-snapshot",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let (signed, trust_store) = signed_descriptor_for(descriptor.clone(), signer, 1_000, 2_000);
    let policy = signer_policy_for(descriptor.package_id.as_str(), signer);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );

    // Admission verifies the signature before a durable record is created.
    let publication = store
        .publish_signed(key.clone(), vec![signed], policy, &trust_store, 1_500)
        .await
        .unwrap();
    publication.validate().unwrap();
    assert!(publication.signed_description_set_digest.is_some());

    // The coordinated state inventory recognizes the descriptor snapshot as
    // a durable Capability Gateway payload. The owner-specific byte validator
    // is exercised by the archive scanner; this path check also protects the
    // public restore-plan classification from accidental downgrades.
    let snapshot_hex = publication.snapshot_digest.strip_prefix("sha256:").unwrap();
    let snapshot_path = format!("capability-gateway/descriptor-snapshots/{snapshot_hex}.json");
    assert_eq!(
        crate::state_backup::validate_state_backup_entry_path(
            crate::state_backup::StateBackupRoot::State,
            &snapshot_path,
        )
        .unwrap(),
        crate::state_backup::StateBackupFamily::CapabilityPayloads
    );
    let snapshot_bytes = tokio::fs::read(
        fixture
            ._owner_fixture
            .paths
            .installation_state_root()
            .join(&snapshot_path),
    )
    .await
    .unwrap();
    crate::control_store::validate_capability_descriptor_snapshot_backup_bytes(
        &snapshot_bytes,
        fixture._owner_fixture.paths.installation(),
        &publication.snapshot_digest,
    )
    .unwrap();

    // A reconstructed projector ignores the retained proof projection and
    // verifies the canonical signed envelope against the current policy.
    let reopened = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let snapshot = reopened.get(&key).await.unwrap().unwrap();
    assert!(snapshot.signed_descriptions().is_some());
    let projector = ControlCapabilityDescriptorProjection::from_signed_snapshot_store_at(
        reopened.clone(),
        trust_store.clone(),
        1_500,
    )
    .unwrap();
    let ControlEffectPortOutcome::Applied(catalog) = projector.project(&authority).await else {
        panic!("the cryptographically verified snapshot must project");
    };
    assert_eq!(catalog.descriptors, vec![descriptor]);

    // Expiry is checked at replay time, not only when the file was written.
    let expired = ControlCapabilityDescriptorProjection::from_signed_snapshot_store_at(
        reopened.clone(),
        trust_store.clone(),
        2_000,
    )
    .unwrap();
    assert!(matches!(
        expired.project(&authority).await,
        ControlEffectPortOutcome::Rejected(_)
    ));

    // Revocation in the current trust policy also prevents projection.
    let mut revoked_key = trust_store.keys()[0].clone();
    revoked_key.revoked_at_unix_seconds = Some(1_600);
    let revoked_store = CapabilityDescriptionTrustStore::new(vec![revoked_key]).unwrap();
    let revoked = ControlCapabilityDescriptorProjection::from_signed_snapshot_store_at(
        reopened,
        revoked_store,
        1_700,
    )
    .unwrap();
    assert!(matches!(
        revoked.project(&authority).await,
        ControlEffectPortOutcome::Rejected(_)
    ));
}

#[tokio::test]
async fn signed_descriptor_snapshot_rejects_invalid_envelope_before_writing() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:signed-descriptor-snapshot-invalid")
            .await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:signed-descriptor-snapshot-invalid",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let (mut signed, trust_store) = signed_descriptor_for(descriptor.clone(), signer, 1_000, 2_000);
    signed.signature.replace_range(0..2, "00");
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let error = store
        .publish_signed(
            key,
            vec![signed],
            signer_policy_for(descriptor.package_id.as_str(), signer),
            &trust_store,
            1_500,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.capability_description_untrusted");
    assert!(store.keys().await.unwrap().is_empty());
}

#[tokio::test]
async fn legacy_descriptor_projector_cannot_downgrade_a_signed_snapshot() {
    let fixture = prepared_capability_plane(
        "operation:capability-plane:signed-descriptor-snapshot-downgrade",
    )
    .await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:signed-descriptor-snapshot-downgrade",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let (signed, trust_store) = signed_descriptor_for(descriptor.clone(), signer, 1_000, 2_000);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let snapshot = ControlCapabilityDescriptorSnapshot::new_signed(
        key,
        vec![signed],
        signer_policy_for(descriptor.package_id.as_str(), signer),
        &trust_store,
        1_500,
    )
    .unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    store.publish(&snapshot).await.unwrap();
    let legacy = ControlCapabilityDescriptorProjection::from_snapshot_store(store).unwrap();
    assert!(matches!(
        legacy.project(&authority).await,
        ControlEffectPortOutcome::Rejected(_)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn descriptor_snapshot_store_accepts_an_ancestor_path_alias() {
    use std::os::unix::fs::symlink;

    let logical_parent = tempfile::tempdir().unwrap();
    let physical_parent = tempfile::tempdir().unwrap();
    let alias = logical_parent.path().join("state-alias");
    symlink(physical_parent.path(), &alias).unwrap();
    let state_root = alias.join("installation");
    tokio::fs::create_dir_all(physical_parent.path().join("installation"))
        .await
        .unwrap();

    let store =
        ControlCapabilityDescriptorSnapshotStore::new(state_root, control_installation()).unwrap();

    assert!(store.keys().await.unwrap().is_empty());
}

#[tokio::test]
async fn durable_descriptor_projection_defers_when_its_snapshot_is_not_yet_published() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:descriptor-snapshot-missing").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot-missing",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    let projector = ControlCapabilityDescriptorProjection::from_snapshot_store(store).unwrap();
    let first = projector.project(&authority).await;
    let second = projector.project(&authority).await;
    let (first_failure, second_failure) = match (first, second) {
        (ControlEffectPortOutcome::Deferred(first), ControlEffectPortOutcome::Deferred(second)) => {
            (first, second)
        }
        _ => panic!("a missing immutable snapshot must remain safely retryable"),
    };
    assert_eq!(first_failure, second_failure);
    assert_eq!(
        first_failure.error_code,
        "use.control_store.capability_descriptor_projection_invalid"
    );
}

#[tokio::test]
async fn descriptor_snapshot_store_rejects_replacement_and_tampering() {
    let fixture =
        prepared_capability_plane("operation:capability-plane:descriptor-snapshot-tamper").await;
    let claimed = fixture
        .store
        .claim_next_effect(claim(
            fixture.installed.operation_id(),
            "claim:capability-plane:descriptor-snapshot-tamper",
            100,
            110,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the third effect must carry Capability Index authority");
    };
    let descriptor = exact_resource_descriptor(&authority);
    let signer = "registry/acme";
    let policy = signer_policy_for(descriptor.package_id.as_str(), signer);
    let key = ControlCapabilityDescriptorSnapshotKey::from_authority(&authority).unwrap();
    let proof = CapabilityDescriptionProof::from_verified(descriptor.clone(), signer).unwrap();
    let snapshot =
        ControlCapabilityDescriptorSnapshot::new(key.clone(), vec![proof], policy.clone()).unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(
        &fixture._owner_fixture.paths,
    );
    store.publish(&snapshot).await.unwrap();

    let mut substituted = descriptor;
    substituted.title = "substituted after publication".to_owned();
    let replacement = ControlCapabilityDescriptorSnapshot::new(
        key.clone(),
        vec![CapabilityDescriptionProof::from_verified(substituted, signer).unwrap()],
        policy,
    )
    .unwrap();
    let error = store.publish(&replacement).await.unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_descriptor_snapshot_conflict"
    );

    let snapshot_digest = snapshot.digest().unwrap();
    let path = fixture
        ._owner_fixture
        .paths
        .installation_state_root()
        .join("capability-gateway")
        .join("descriptor-snapshots")
        .join(format!(
            "{}.json",
            snapshot_digest.strip_prefix("sha256:").unwrap()
        ));
    let mut bytes = std::fs::read(&path).unwrap();
    let index = bytes.len() / 2;
    bytes[index] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let tamper = store.get(&key).await.unwrap_err();
    assert_eq!(
        tamper.code,
        "use.control.capability_descriptor_snapshot_conflict"
    );
}

#[tokio::test]
async fn capability_plane_rejects_a_catalog_owner_from_another_state_root() {
    let (temporary, store) = initialized_store().await;
    let foreign = CapabilityGatewayCatalogStore::new(
        temporary.path().join("foreign-state"),
        control_installation(),
    )
    .unwrap();

    let error =
        ControlCapabilityPlaneEffectPort::new(store, foreign, Arc::new(EmptyCatalogProjection))
            .unwrap_err();

    assert_eq!(error.code, "use.control.capability_catalog_binding_invalid");
}

#[tokio::test]
async fn published_lease_requires_the_exact_bound_catalog_payload() {
    let fixture = installed_capability_plane("operation:capability-plane:missing-catalog").await;
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let path = catalog_path(
        &fixture._owner_fixture.paths.installation_state_root(),
        &cursor.catalog.digest,
    );
    tokio::fs::remove_file(path).await.unwrap();

    let error = fixture.plane.acquire_published(&cursor).await.unwrap_err();

    assert_eq!(error.code, "use.control.capability_catalog_binding_invalid");
}

#[tokio::test]
async fn published_lease_rejects_tampered_bound_catalog_payload() {
    let fixture = installed_capability_plane("operation:capability-plane:tampered-catalog").await;
    let cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let path = catalog_path(
        &fixture._owner_fixture.paths.installation_state_root(),
        &cursor.catalog.digest,
    );
    tokio::fs::write(path, b"{}").await.unwrap();

    let error = fixture.plane.acquire_published(&cursor).await.unwrap_err();

    assert_eq!(
        error.code,
        "use.plugin.capability_gateway_catalog_store_conflict"
    );
}

#[tokio::test]
async fn index_failure_after_catalog_publication_is_unknown_not_safe_rejection() {
    let fixture = prepared_capability_plane("operation:capability-plane:index-failure").await;
    // Force the later Index owner phase to fail after the catalog payload has
    // been accepted. The owner must not misclassify that accepted payload as
    // a proven no-effect rejection or deferral.
    let index_root = fixture
        ._owner_fixture
        .paths
        .installation_state_root()
        .join("capability-index");
    tokio::fs::write(&index_root, b"not-a-directory")
        .await
        .unwrap();

    assert_dispatch(
        &fixture.dispatcher,
        &fixture.installed,
        "claim:capability-plane:index-failure:cutover",
        2,
        1,
        ControlEffectOutcome::Unknown,
        false,
    )
    .await;
    assert_eq!(
        fixture
            .store
            .effects(fixture.installed.operation_id())
            .await
            .unwrap()[2]
            .error_code
            .as_deref(),
        Some("use.control.capability_index_path_invalid")
    );
    assert!(fixture
        .store
        .published_capability()
        .await
        .unwrap()
        .is_none());
    assert!(
        !CapabilityGatewayCatalogStore::from_extension_paths(&fixture._owner_fixture.paths)
            .list()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn published_snapshot_lease_blocks_prior_generation_drain_until_the_call_releases_it() {
    let fixture = installed_capability_plane("operation:capability-plane:drain-install").await;
    let prior_cursor = fixture.store.published_capability().await.unwrap().unwrap();
    let lease = fixture
        .plane
        .acquire_published(&prior_cursor)
        .await
        .unwrap()
        .unwrap();
    let prior = fixture.store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&prior).unwrap();
    let upgrade = operation_at(
        "operation:capability-plane:drain-upgrade",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    fixture
        .store
        .register_operation(upgrade.clone())
        .await
        .unwrap();
    fixture
        .store
        .commit_transition(projected_transition(&upgrade, &prior, &history))
        .await
        .unwrap();

    // The generic upgrade fixture intentionally has no second package artifact.
    // Record its already-qualified preparation evidence so this test isolates
    // the Capability Index publication and invocation-drain boundary.
    for sequence in 0..2_u32 {
        let now_ms = 200 + u64::from(sequence) * 20;
        let claim_token = format!("claim:capability-plane:prepare:{sequence}");
        let claimed = fixture
            .store
            .claim_next_effect(claim(
                upgrade.operation_id(),
                &claim_token,
                now_ms,
                now_ms + 10,
                false,
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.intent.sequence, sequence);
        fixture
            .store
            .record_effect_observation(observation(
                upgrade.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence, 16).unwrap(),
                now_ms + 5,
            ))
            .await
            .unwrap();
    }

    assert_dispatch(
        &fixture.dispatcher,
        &upgrade,
        "claim:capability-plane:upgrade-cutover",
        2,
        1,
        ControlEffectOutcome::Applied,
        false,
    )
    .await;
    assert!(fixture
        .plane
        .acquire_published(&prior_cursor)
        .await
        .unwrap()
        .is_none());

    assert_dispatch(
        &fixture.dispatcher,
        &upgrade,
        "claim:capability-plane:upgrade-drain-busy",
        3,
        1,
        ControlEffectOutcome::Deferred,
        false,
    )
    .await;
    let effects = fixture.store.effects(upgrade.operation_id()).await.unwrap();
    assert_eq!(
        effects[3].error_code.as_deref(),
        Some("use.control.invocation_generation_busy")
    );

    drop(lease);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert_dispatch(
        &fixture.dispatcher,
        &upgrade,
        "claim:capability-plane:upgrade-drain-retry",
        3,
        2,
        ControlEffectOutcome::Applied,
        false,
    )
    .await;
}

async fn installed_capability_plane(operation_id: &str) -> InstalledCapabilityPlaneFixture {
    installed_capability_plane_with_projection(operation_id, Arc::new(EmptyCatalogProjection)).await
}

async fn prepared_capability_plane(operation_id: &str) -> InstalledCapabilityPlaneFixture {
    prepared_capability_plane_with_projection(operation_id, Arc::new(EmptyCatalogProjection)).await
}

async fn installed_capability_plane_with_projection(
    operation_id: &str,
    projection: Arc<dyn ControlCapabilityCatalogProjectionPort>,
) -> InstalledCapabilityPlaneFixture {
    let fixture = prepared_capability_plane_with_projection(operation_id, projection).await;
    assert_dispatch(
        &fixture.dispatcher,
        &fixture.installed,
        "claim:capability-plane:install:2",
        2,
        1,
        ControlEffectOutcome::Applied,
        false,
    )
    .await;
    fixture
        .store
        .complete_operation(
            fixture.installed.operation_id(),
            fixture.installed.plan_digest(),
            &digest('f'),
            SystemControlEffectClock.now_ms().unwrap(),
        )
        .await
        .unwrap();
    fixture
}

async fn prepared_capability_plane_with_projection(
    operation_id: &str,
    projection: Arc<dyn ControlCapabilityCatalogProjectionPort>,
) -> InstalledCapabilityPlaneFixture {
    let installation = control_installation();
    let (owner_fixture, artifact_admission) =
        knowledge_owner_fixture_for(installation.clone()).await;
    let store = ControlStore::from_extension_paths(&owner_fixture.paths).unwrap();
    store.initialize().await.unwrap();
    let installed = operation(operation_id);
    store.register_operation(installed.clone()).await.unwrap();
    store
        .commit_transition(transition(installation, &installed))
        .await
        .unwrap();
    drop(artifact_admission);

    let plane = Arc::new(
        ControlCapabilityPlaneEffectPort::new(
            store.clone(),
            CapabilityGatewayCatalogStore::from_extension_paths(&owner_fixture.paths),
            projection,
        )
        .unwrap(),
    );
    let knowledge = Arc::new(ControlOkfKnowledgeEffectPort::new(
        owner_fixture.paths.artifact_store(),
        owner_fixture.client.clone(),
        owner_fixture.bindings.clone(),
    ));
    let static_surfaces = Arc::new(ControlStaticSurfaceEffectPort::new(
        owner_fixture.paths.artifact_store(),
    ));
    let unexpected = Arc::new(UnexpectedDynamicSurfacePort);
    let ports = ControlEffectPorts::new(
        plane.clone(),
        plane.clone(),
        unexpected.clone(),
        unexpected,
        knowledge,
        static_surfaces.clone(),
        static_surfaces,
    );
    let dispatcher =
        ControlEffectDispatcher::new(store.clone(), ports, Arc::new(SystemControlEffectClock));
    for sequence in 0..2_u32 {
        assert_dispatch(
            &dispatcher,
            &installed,
            &format!("claim:capability-plane:install:{sequence}"),
            sequence,
            1,
            ControlEffectOutcome::Applied,
            false,
        )
        .await;
    }
    InstalledCapabilityPlaneFixture {
        _owner_fixture: owner_fixture,
        store,
        plane,
        dispatcher,
        installed,
    }
}

#[allow(clippy::too_many_arguments)]
async fn assert_dispatch(
    dispatcher: &ControlEffectDispatcher,
    operation: &ReviewedControlOperation,
    claim_token: &str,
    sequence: u32,
    attempt: u32,
    expected_outcome: ControlEffectOutcome,
    explicit_reconciliation: bool,
) {
    let result = dispatcher
        .dispatch_next(ControlEffectDispatchRequest {
            operation_id: operation.operation_id().to_string(),
            worker_id: "worker:capability-plane".to_string(),
            claim_token: claim_token.to_string(),
            lease_duration_ms: 10_000,
            provider_timeout_ms: 5_000,
            deferred_retry_delay_ms: 1,
            explicit_reconciliation,
        })
        .await
        .unwrap();
    match &result {
        ControlEffectDispatchResult::Observed {
            sequence: observed_sequence,
            attempt: observed_attempt,
            outcome,
            observation_changed: true,
            ..
        } if *observed_sequence == sequence
            && *observed_attempt == attempt
            && *outcome == expected_outcome => {}
        other => panic!(
            "capability-plane dispatch mismatch for claim '{claim_token}': \
             expected Observed(sequence={sequence}, attempt={attempt}, \
             outcome={expected_outcome:?}, observation_changed=true), got {other:?}"
        ),
    }
}

#[cfg(feature = "mcp")]
fn exact_tool_descriptor(
    authority: &ControlCapabilityEffectAuthority,
) -> Option<CapabilityDescriptor> {
    let (package_id, surface) = authority
        .materializations
        .iter()
        .find_map(|materialization| {
            let super::model::ControlEffectSubject::Surface {
                package_id,
                surface,
                ..
            } = &materialization.intent.subject
            else {
                return None;
            };
            (surface.kind == PluginSurfaceKind::Tool && surface.id == "convert")
                .then(|| (package_id.clone(), surface.clone()))
        })?;
    let package = authority
        .generation
        .snapshot
        .package_selection(&package_id)?;
    let lifecycle_generation = authority
        .generation
        .package_lifecycles
        .iter()
        .find(|lifecycle| lifecycle.package_id == package_id)?
        .lifecycle_generation;
    let route = ControlCapabilityDescriptorProjection::route_binding(
        authority,
        &PluginPackageId::parse(package_id.clone()).unwrap(),
        &surface,
    )
    .unwrap();
    let attestation = route.runtime_schema_attestation.as_ref()?;
    let catalog_surface = package
        .package
        .catalog
        .record
        .surfaces
        .iter()
        .find(|candidate| candidate.reference() == surface)
        .expect("the descriptor surface must be in the package catalog");
    let input_schema = grant_tool_input_schema();
    let output_schema = grant_tool_output_schema();
    Some(CapabilityDescriptor {
        schema: a3s_use_core::CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        package_id: PluginPackageId::parse(package_id).unwrap(),
        surface: surface.clone(),
        generation: lifecycle_generation,
        package_digest: package
            .package
            .catalog
            .record
            .package
            .sha256
            .clone()
            .unwrap(),
        manifest_digest: package
            .package
            .catalog
            .record
            .package
            .manifest_sha256
            .clone()
            .unwrap(),
        title: "Convert Tool".to_owned(),
        description: "A Tool Task projected from committed Grant + Runtime evidence.".to_owned(),
        invocation_ref: route.invocation_ref.clone(),
        artifact_ref: route.artifact_ref.clone(),
        endpoint_ref: route.endpoint_ref.clone(),
        dependencies: catalog_surface.requires.clone(),
        required_extensions: Vec::new(),
        publication: CapabilityPublicationEvidence {
            catalog_record_digest: package
                .package
                .catalog
                .provenance
                .catalog_record_digest
                .clone(),
            signature_digest: digest('e'),
        },
        capability: CapabilityDescriptorKind::Tool {
            name: surface.id.clone(),
            input_schema,
            output_schema,
            annotations: CapabilityToolAnnotations::new(false, false, false, false),
            runtime_descriptor_digest: Some(attestation.descriptor_digest.clone()),
        },
    })
}

#[cfg(feature = "mcp")]
fn grant_tool_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "args": {
                "type": "array",
                "items": { "type": "string" }
            },
            "invocationId": { "type": "string" }
        },
        "additionalProperties": false
    })
}

#[cfg(feature = "mcp")]
fn grant_tool_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "exitCode": { "type": "integer" },
            "stdout": { "type": "string" },
            "stderr": { "type": "string" },
            "truncated": { "type": "boolean" }
        },
        "required": ["exitCode", "stderr", "stdout", "truncated"],
        "additionalProperties": false
    })
}

#[cfg(feature = "mcp")]
fn schema_bearing_task_descriptor() -> a3s_use_core::ToolReleaseDescriptor {
    let mut descriptor = task_descriptor();
    descriptor.input_schema = Some(grant_tool_input_schema());
    descriptor.output_schema = Some(grant_tool_output_schema());
    descriptor
}

#[cfg(feature = "mcp")]
async fn publish_grant_tool_descriptor_snapshot(
    paths: &ExtensionPaths,
    cursor: &super::model::ControlPublishedCapabilityCursor,
) {
    let catalog = CapabilityGatewayCatalogStore::from_extension_paths(paths)
        .get(&cursor.catalog.digest)
        .await
        .unwrap()
        .expect("Grant Tool cutover must publish a catalog before the descriptor snapshot");
    let signer = "registry/acme";
    let key = ControlCapabilityDescriptorSnapshotKey::new(
        cursor.installation.clone(),
        cursor.installation_generation,
        cursor.capability_generation,
        cursor.descriptor_digest.clone(),
    )
    .unwrap();
    let store = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths);
    if catalog.descriptors().is_empty() {
        store
            .publish(
                &ControlCapabilityDescriptorSnapshot::new(
                    key,
                    Vec::new(),
                    ControlCapabilitySignerPolicy::new(BTreeMap::new()).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        return;
    }
    let now_unix_seconds = 1_500;
    let mut signed_descriptions = Vec::with_capacity(catalog.descriptors().len());
    let mut trust_store = None;
    for descriptor in catalog.descriptors() {
        let (signed, store_for_descriptor) =
            signed_descriptor_for(descriptor.clone(), signer, 1_000, 2_000);
        trust_store = Some(store_for_descriptor);
        signed_descriptions.push(signed);
    }
    let policy = signer_policy_for(catalog.descriptors()[0].package_id.as_str(), signer);
    store
        .publish_signed(
            key,
            signed_descriptions,
            policy,
            trust_store.as_ref().unwrap(),
            now_unix_seconds,
        )
        .await
        .unwrap();
}

#[cfg(feature = "mcp")]
fn grant_tool_task_policy() -> crate::plugin_runtime::RuntimeWorkloadPolicy {
    use crate::plugin_runtime::{RuntimeResourcePolicy, RuntimeWorkloadPolicy};
    RuntimeWorkloadPolicy {
        isolation: IsolationLevel::Sandbox,
        resources: RuntimeResourcePolicy {
            cpu_millis: 500,
            memory_bytes: 256 * 1024 * 1024,
            pids: 64,
            ephemeral_storage_bytes: Some(512 * 1024 * 1024),
        },
        mounts: Vec::new(),
        secrets: Vec::new(),
        non_secret_environment: std::collections::BTreeMap::from([(
            "A3S_PLUGIN_MODE".to_string(),
            "managed".to_string(),
        )]),
        working_directory: None,
    }
}

#[cfg(feature = "mcp")]
fn grant_tool_runtime_capabilities(plan: &RuntimeSurfacePlan) -> RuntimeCapabilities {
    RuntimeCapabilities {
        schema: RuntimeCapabilities::SCHEMA.to_string(),
        provider_id: ProviderId::parse("test-runtime").unwrap(),
        provider_build: "build-1".to_string(),
        unit_classes: vec![RuntimeUnitClass::Task, RuntimeUnitClass::Service],
        artifact_media_types: vec![plan.spec().artifact.media_type.clone()],
        isolation_levels: vec![IsolationLevel::Sandbox, IsolationLevel::Container],
        network_modes: vec![NetworkMode::None, NetworkMode::Service],
        mount_kinds: Vec::new(),
        health_check_kinds: vec![HealthCheckKind::Http],
        resource_controls: vec![
            ResourceControl::Cpu,
            ResourceControl::Memory,
            ResourceControl::Pids,
            ResourceControl::EphemeralStorage,
            ResourceControl::ExecutionTimeout,
        ],
        features: vec![
            RuntimeFeature::DurableIdentity,
            RuntimeFeature::ServiceTcp,
            RuntimeFeature::Logs,
            RuntimeFeature::Stop,
            RuntimeFeature::Remove,
        ],
    }
}

#[cfg(feature = "mcp")]
struct GrantToolRuntimeFactory {
    provider_id: ProviderId,
    client: Arc<dyn RuntimeClient>,
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl RuntimeProviderFactory for GrantToolRuntimeFactory {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
        Ok(self.client.clone())
    }
}

#[cfg(feature = "mcp")]
fn verified_grant_tool_catalog(
    candidate: &ExtensionLifecyclePackage,
) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-v3.json"
    ))
    .unwrap();
    let manifest = candidate.manifest();
    let prior_version = record.version.clone();
    record.package_id = manifest.package_id.clone();
    record.version = manifest.version.clone();
    if prior_version != record.version {
        record.archive.target_name = record
            .archive
            .target_name
            .replace(
                &format!("/{prior_version}/"),
                &format!("/{}/", record.version),
            )
            .replace(
                &format!("-{prior_version}-"),
                &format!("-{}-", record.version),
            );
        if let Some(planning) = &mut record.planning {
            planning.target_name = planning.target_name.replace(
                &format!("/{prior_version}/"),
                &format!("/{}/", record.version),
            );
        }
    }
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

#[cfg(feature = "mcp")]
async fn write_grant_tool_task_package(root: &std::path::Path) {
    write_grant_tool_task_package_at(root, "2.0.0", "# Grant Tool Task fixture\n").await;
}

#[cfg(feature = "mcp")]
async fn write_grant_tool_task_package_at(root: &std::path::Path, version: &str, readme: &str) {
    tokio::fs::create_dir_all(root.join("releases"))
        .await
        .unwrap();
    tokio::fs::write(root.join("README.md"), readme)
        .await
        .unwrap();
    tokio::fs::write(
        root.join("releases/task.json"),
        serde_json::to_vec_pretty(&schema_bearing_task_descriptor()).unwrap(),
    )
    .await
    .unwrap();
    let acl = format!(
        r#"extension "acme/research" {{
  schema_version = 3
  version        = "{version}"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["execute"]

  repository {{
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }}

  tool "convert" {{
    workload    = "task"
    interface   = "cli"
    release     = "releases/task.json"
    command     = "acme-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }}
}}
"#
    );
    tokio::fs::write(root.join("a3s-use-extension.acl"), acl)
        .await
        .unwrap();
}

fn exact_resource_descriptor(authority: &ControlCapabilityEffectAuthority) -> CapabilityDescriptor {
    let (package_id, surface) = authority
        .materializations
        .iter()
        .find_map(|materialization| {
            let super::model::ControlEffectSubject::Surface {
                package_id,
                surface,
                ..
            } = &materialization.intent.subject
            else {
                return None;
            };
            (surface.kind == PluginSurfaceKind::Okf).then(|| (package_id.clone(), surface.clone()))
        })
        .expect("the capability fixture must prepare an OKF surface");
    let package = authority
        .generation
        .snapshot
        .package_selection(&package_id)
        .expect("the descriptor package must be selected");
    let lifecycle_generation = authority
        .generation
        .package_lifecycles
        .iter()
        .find(|lifecycle| lifecycle.package_id == package_id)
        .expect("the descriptor package must have a lifecycle")
        .lifecycle_generation;
    let route = ControlCapabilityDescriptorProjection::route_binding(
        authority,
        &PluginPackageId::parse(package_id.clone()).unwrap(),
        &surface,
    )
    .unwrap();
    let catalog_surface = package
        .package
        .catalog
        .record
        .surfaces
        .iter()
        .find(|candidate| candidate.reference() == surface)
        .expect("the descriptor surface must be in the package catalog");
    CapabilityDescriptor {
        schema: a3s_use_core::CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        package_id: PluginPackageId::parse(package_id).unwrap(),
        surface,
        generation: lifecycle_generation,
        package_digest: package
            .package
            .catalog
            .record
            .package
            .sha256
            .clone()
            .unwrap(),
        manifest_digest: package
            .package
            .catalog
            .record
            .package
            .manifest_sha256
            .clone()
            .unwrap(),
        title: "A3S Knowledge Resource".to_owned(),
        description: "A resource projected from committed OKF evidence.".to_owned(),
        invocation_ref: route.invocation_ref.clone(),
        artifact_ref: route.artifact_ref.clone(),
        endpoint_ref: route.endpoint_ref.clone(),
        dependencies: catalog_surface.requires.clone(),
        required_extensions: Vec::new(),
        publication: CapabilityPublicationEvidence {
            catalog_record_digest: package
                .package
                .catalog
                .provenance
                .catalog_record_digest
                .clone(),
            signature_digest: digest('e'),
        },
        capability: CapabilityDescriptorKind::Resource {
            name: "domain-knowledge".to_owned(),
            uri: route.resource_ref,
            mime_type: Some("text/plain".to_owned()),
            size: Some(1),
        },
    }
}

fn signer_policy_for(package_id: &str, signer: &str) -> ControlCapabilitySignerPolicy {
    let mut package_signers = BTreeMap::new();
    package_signers.insert(package_id.to_owned(), BTreeSet::from([signer.to_owned()]));
    ControlCapabilitySignerPolicy::new(package_signers).unwrap()
}

fn signed_descriptor_for(
    descriptor: CapabilityDescriptor,
    signer: &str,
    issued_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
) -> (SignedCapabilityDescription, CapabilityDescriptionTrustStore) {
    let key_pair = Ed25519KeyPair::from_seed_unchecked(&[9_u8; 32]).unwrap();
    let key_id = "registry/acme/descriptor";
    let payload = CapabilityDescriptionSignaturePayload::new(
        descriptor,
        signer,
        key_id,
        CapabilityDescriptionSignatureAlgorithm::Ed25519,
        issued_at_unix_seconds,
        expires_at_unix_seconds,
    )
    .unwrap();
    let signature = key_pair.sign(&payload.canonical_bytes().unwrap());
    let signed = SignedCapabilityDescription::from_parts(
        payload,
        signature
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    let key = CapabilityDescriptionTrustKey::new(
        key_id,
        signer,
        CapabilityDescriptionSignatureAlgorithm::Ed25519,
        key_pair
            .public_key()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        issued_at_unix_seconds.saturating_sub(100),
        expires_at_unix_seconds.saturating_add(100),
        None,
    )
    .unwrap();
    (
        signed,
        CapabilityDescriptionTrustStore::new(vec![key]).unwrap(),
    )
}

fn catalog_path(state_root: &std::path::Path, digest: &str) -> std::path::PathBuf {
    let hex = digest.strip_prefix("sha256:").unwrap();
    state_root
        .join("capability-gateway")
        .join("catalogs")
        .join("sha256")
        .join(&hex[..2])
        .join(format!("{hex}.json"))
}
