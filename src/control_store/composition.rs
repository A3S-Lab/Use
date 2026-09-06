//! One installation-scoped composition for the inactive Control Store kernel.
//!
//! The composition is deliberately narrower than a production cutover. It
//! proves the important ownership boundary, however: a Runtime effect port is
//! built from the host-owned durable plan source and an explicit provider
//! registry, while all other owner ports are assembled exactly once. A caller
//! cannot accidentally pair a process-local Runtime selection with a
//! committed Control dispatcher.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;
#[cfg(feature = "mcp")]
use std::time::Duration;

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{
    CapabilityGatewayCatalog, PluginOperationPlanEnvelope, PluginSurfaceRef, UseError, UseResult,
};
use a3s_use_extension::{
    ArtifactStore, ExtensionPaths, StateMaintenanceGuard, StateMaintenanceLock,
};
use async_trait::async_trait;

#[cfg(feature = "mcp")]
use crate::capability_gateway::{
    CapabilityGatewayCompositionOptions, CapabilityGatewayGenerationLeaseMode,
    CapabilityGatewayInvocationProvider, CapabilityGatewayMcpServer,
    CapabilityGatewaySessionFactory, CapabilityGatewaySessionKey,
    CapabilityGatewaySessionReplacement,
};

use super::dispatcher::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    ControlEffectPorts, ControlEffectRuntime,
};
use super::effect_owner::capability_plane::{
    ControlCapabilityDescriptorSnapshotKey, ControlCapabilityDescriptorSnapshotStore,
    ControlCapabilityPayloadRestoreCoordinator, ControlCapabilityPayloadRetentionCoordinator,
    ControlCapabilityPayloadRetentionPlan, ControlCapabilityPayloadRetentionResult,
    ControlCapabilityPlaneEffectPort, ControlCapabilitySnapshotLease,
};
#[cfg(feature = "mcp")]
use super::effect_owner::capability_plane::{
    ControlCapabilityGatewayInvocationFactory, ControlCapabilityGatewayInvocationResolver,
};
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
use super::effect_owner::runtime::{ControlRuntimeEffectPort, ControlRuntimeServiceReadinessPort};
use super::effect_owner::static_surface::ControlStaticSurfaceEffectPort;
use super::effect_port::{ControlCapabilityCatalogProjectionPort, ControlFlowEffectPort};
use super::model::{
    ControlEffectKind, ControlEffectOwner, ControlEffectSubject, ControlGeneration,
    ControlOperationRecord, ControlPublishedCapabilityCursor, ControlTransition,
    ReviewedControlOperation,
};
use super::operation_admission::reviewed_cognitive_package_operation;
use super::{ControlStore, ControlStoreMetadata};
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;
use crate::cognitive_package::{
    CognitivePackageAuthorizationEvidence, PlannedWorkspaceGrantOperation,
};
use crate::okf_knowledge::{
    OkfKnowledgeBindingStore, OkfKnowledgeClient, SqliteOkfKnowledgeAdapter,
};
use crate::plugin_lifecycle::PluginGraphCapabilityCutoverActivation;
use crate::plugin_runtime::{
    CommittedRuntimeSurfaceResolver, RuntimeBindingStore, RuntimeSurfacePlanPublication,
    RuntimeSurfacePlanStore,
};

const COMPOSITION_ERROR: &str = "use.control_store.composition_invalid";
const PUBLICATION_ERROR: &str = "use.control_store.runtime_plan_publication_invalid";
const CAPABILITY_RETENTION_CURSOR_ERROR: &str =
    "use.control.capability_payload_retention_cursor_stale";
const CAPABILITY_RETENTION_SNAPSHOT_ERROR: &str =
    "use.control.capability_payload_retention_snapshot_missing";

/// All dependencies needed to compose one inactive Control dispatcher.
///
/// Runtime and Flow/Gateway are host-owned boundaries. The remaining owners
/// are Use-owned adapters and are constructed from the same `ExtensionPaths`
/// and `ControlStore`, so they cannot silently drift to another installation.
pub(in crate::control_store) struct ControlEffectCompositionDependencies {
    pub(in crate::control_store) runtime_registry: Arc<RuntimeClientRegistry>,
    pub(in crate::control_store) runtime_readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
    pub(in crate::control_store) catalog_projection:
        Arc<dyn ControlCapabilityCatalogProjectionPort>,
    pub(in crate::control_store) flow: Arc<dyn ControlFlowEffectPort>,
    pub(in crate::control_store) clock: Arc<dyn ControlEffectClock>,
}

/// Installation-scoped composition of the Control Store, immutable Runtime
/// plan payload owner, and one typed post-commit dispatcher.
///
/// Construction is side-effect free apart from the bounded Control worker;
/// callers must invoke [`Self::initialize`] before committing or dispatching.
/// Runtime plan publication and the following Control commit are performed
/// with [`Self::commit_reviewed_operation_with_runtime_plans`], which retains
/// one installation-wide shared maintenance fence across both local
/// boundaries. The lifecycle-facing
/// [`Self::admit_and_commit_cognitive_package_operation_with_runtime_plans`]
/// entry point adds reviewed Plan admission to that same fenced sequence.
#[derive(Clone)]
pub(in crate::control_store) struct ControlStoreRuntimeComposition {
    store: ControlStore,
    plan_store: RuntimeSurfacePlanStore,
    catalog_store: CapabilityGatewayCatalogStore,
    capability_payload_restore: ControlCapabilityPayloadRestoreCoordinator,
    capability_payload_retention: ControlCapabilityPayloadRetentionCoordinator,
    capability_plane: Arc<ControlCapabilityPlaneEffectPort>,
    artifact_store: ArtifactStore,
    effects: ControlEffectRuntime,
}

/// Result of reconciling a live Gateway endpoint with the durable Control
/// publication.  An unchanged endpoint is reported separately so recovery
/// does not acquire a second package-generation lease or emit a redundant
/// list-change notification.
#[cfg(feature = "mcp")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) enum ControlCapabilityGatewayReconciliation {
    Unchanged(CapabilityGatewaySessionKey),
    Replaced(CapabilityGatewaySessionReplacement),
}

/// Lifecycle activation adapter that binds the graph coordinator's
/// post-publication hook to the Control-owned Gateway session factory.
///
/// The factory must already be seeded from Control authority.  Activation
/// then reopens the current durable cursor and swaps the endpoint before the
/// graph coordinator starts draining prior package generations.
#[cfg(feature = "mcp")]
#[derive(Clone)]
pub(in crate::control_store) struct ControlCapabilityGatewayCutoverActivation {
    composition: ControlStoreRuntimeComposition,
    factory: CapabilityGatewaySessionFactory,
    provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    options: CapabilityGatewayCompositionOptions,
}

impl std::fmt::Debug for ControlStoreRuntimeComposition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlStoreRuntimeComposition")
            .field("installation", &self.store.installation)
            .field("state_root", &self.store.state_root)
            .field("runtime_plan_root", &self.plan_store.root())
            .field("catalog_root", &self.catalog_store.root())
            .field(
                "capability_payload_restore",
                &self.capability_payload_restore,
            )
            .field(
                "capability_payload_retention",
                &self.capability_payload_retention,
            )
            .field("capability_plane", &self.capability_plane)
            .finish_non_exhaustive()
    }
}

impl ControlStoreRuntimeComposition {
    /// Compose Use-owned adapters and the committed-authority Runtime owner
    /// from one exact installation path set.
    pub(in crate::control_store) fn from_extension_paths(
        paths: &ExtensionPaths,
        dependencies: ControlEffectCompositionDependencies,
    ) -> UseResult<Self> {
        let store = ControlStore::from_extension_paths(paths)?;
        let plan_store = RuntimeSurfacePlanStore::from_extension_paths(paths);
        if store.installation != *plan_store.installation()
            || store.state_root != plan_store.state_root()
        {
            return Err(UseError::new(
                COMPOSITION_ERROR,
                "The Control Store and Runtime plan store do not share one installation root.",
            ));
        }
        let catalog_store = CapabilityGatewayCatalogStore::from_extension_paths(paths);
        let descriptor_snapshot_store =
            ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths);
        let capability_payload_restore = ControlCapabilityPayloadRestoreCoordinator::new(
            catalog_store.clone(),
            descriptor_snapshot_store.clone(),
        )?;
        let capability_payload_retention = ControlCapabilityPayloadRetentionCoordinator::new(
            catalog_store.clone(),
            descriptor_snapshot_store,
        )?;

        let artifact_store = paths.artifact_store();
        let runtime_source = Arc::new(plan_store.clone());
        let runtime_resolver = Arc::new(CommittedRuntimeSurfaceResolver::new(
            runtime_source,
            dependencies.runtime_registry,
        ));
        let runtime = Arc::new(ControlRuntimeEffectPort::with_resolver(
            artifact_store.clone(),
            runtime_resolver,
            RuntimeBindingStore::from_extension_paths(paths),
            dependencies.runtime_readiness,
        ));
        let capability = Arc::new(ControlCapabilityPlaneEffectPort::new(
            store.clone(),
            catalog_store.clone(),
            dependencies.catalog_projection,
        )?);
        let knowledge = Arc::new(ControlOkfKnowledgeEffectPort::new(
            artifact_store.clone(),
            OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(
                paths,
            ))),
            OkfKnowledgeBindingStore::from_extension_paths(paths),
        ));
        let static_surface = Arc::new(ControlStaticSurfaceEffectPort::new(artifact_store.clone()));
        let ports = ControlEffectPorts::new(
            capability.clone(),
            capability.clone(),
            runtime,
            dependencies.flow,
            knowledge,
            static_surface.clone(),
            static_surface,
        );
        let effects = ControlEffectRuntime::compose(store.clone(), ports, dependencies.clock);
        Ok(Self {
            store,
            plan_store,
            catalog_store,
            capability_payload_restore,
            capability_payload_retention,
            capability_plane: capability,
            artifact_store,
            effects,
        })
    }

    pub(in crate::control_store) fn store(&self) -> &ControlStore {
        &self.store
    }

    pub(in crate::control_store) fn plan_store(&self) -> &RuntimeSurfacePlanStore {
        &self.plan_store
    }

    pub(in crate::control_store) fn catalog_store(&self) -> &CapabilityGatewayCatalogStore {
        &self.catalog_store
    }

    /// Return the maintenance-fenced coordinator for the immutable Capability
    /// catalog and descriptor-snapshot payload owners.
    #[allow(dead_code)]
    pub(in crate::control_store) fn capability_payload_restore(
        &self,
    ) -> &ControlCapabilityPayloadRestoreCoordinator {
        &self.capability_payload_restore
    }

    /// Return the maintenance-fenced coordinator for immutable Capability
    /// catalog and descriptor-snapshot retention.
    #[allow(dead_code)]
    pub(in crate::control_store) fn capability_payload_retention(
        &self,
    ) -> &ControlCapabilityPayloadRetentionCoordinator {
        &self.capability_payload_retention
    }

    /// Resume a cross-owner Capability payload retention operation left by a
    /// process interruption. The durable coordinator journal supplies the
    /// exact reviewed plan; callers cannot substitute a new one during
    /// recovery.
    pub(in crate::control_store) async fn recover_capability_payload_retention(
        &self,
    ) -> UseResult<Option<ControlCapabilityPayloadRetentionResult>> {
        self.capability_payload_retention.recover_retention().await
    }

    /// Build a retention plan whose protected set starts with the exact
    /// currently published Control capability payloads.
    ///
    /// The caller may add digests for an independently managed rollback or a
    /// non-Control endpoint.  The durable cursor is always included by this
    /// boundary; when descriptor snapshots exist, the snapshot keyed by that
    /// cursor is included as well.  A cursor race during planning is reported
    /// so lifecycle code can refresh instead of reviewing a stale plan.
    pub(in crate::control_store) async fn plan_published_capability_payload_retention(
        &self,
        additional_catalog_retain_digests: &[String],
        additional_descriptor_snapshot_retain_digests: &[String],
    ) -> UseResult<ControlCapabilityPayloadRetentionPlan> {
        let before = self.store.published_capability().await?;
        let (catalog_retain, descriptor_snapshot_retain) = self
            .published_payload_retain_digests(
                before.as_ref(),
                additional_catalog_retain_digests,
                additional_descriptor_snapshot_retain_digests,
            )
            .await?;
        let catalog_retain = catalog_retain.into_iter().collect::<Vec<_>>();
        let descriptor_snapshot_retain = descriptor_snapshot_retain.into_iter().collect::<Vec<_>>();
        let plan = self
            .capability_payload_retention
            .plan_retention(&catalog_retain, &descriptor_snapshot_retain)
            .await?;
        let after = self.store.published_capability().await?;
        if after != before {
            return Err(UseError::new(
                CAPABILITY_RETENTION_CURSOR_ERROR,
                "The published Control capability changed while its payload retention plan was being built.",
            ));
        }
        Ok(plan)
    }

    /// Apply a reviewed published-payload retention plan without allowing a
    /// concurrent Control cutover or live snapshot lease to invalidate its
    /// protected set.
    ///
    /// The exclusive maintenance guard is acquired before rereading Control
    /// authority and is held through both owner deletions.  This is the
    /// lifecycle-safe entry point; direct owner plans remain useful for
    /// compatibility stores but cannot provide this authority check.
    pub(in crate::control_store) async fn apply_published_capability_payload_retention(
        &self,
        plan: &ControlCapabilityPayloadRetentionPlan,
        expected_plan_digest: &str,
    ) -> UseResult<ControlCapabilityPayloadRetentionResult> {
        plan.validate()?;
        let maintenance = StateMaintenanceLock::new(&self.store.state_root)
            .acquire_exclusive()
            .await?;
        let cursor = self
            .store
            .published_capability_under_maintenance(&maintenance)
            .await?;
        if let Some(cursor) = cursor.as_ref() {
            ensure_published_payloads_are_retained(cursor, plan)?;
        }
        self.capability_payload_retention
            .apply_retention_with_exclusive_maintenance(plan, expected_plan_digest, &maintenance)
            .await
    }

    /// Drain a live Control-bound Gateway endpoint and then retain exactly the
    /// payloads selected by the durable Control cursor.  This is the shutdown
    /// path for hosts that are retiring an endpoint rather than replacing it:
    /// the session must release its shared generation lease before the
    /// exclusive owner-retention fence can be acquired.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn drain_and_retain_published_capability_gateway(
        &self,
        session: &CapabilityGatewaySessionFactory,
        drain_timeout: Duration,
        additional_catalog_retain_digests: &[String],
        additional_descriptor_snapshot_retain_digests: &[String],
    ) -> UseResult<ControlCapabilityPayloadRetentionResult> {
        session.drain(drain_timeout).await?;
        let plan = self
            .plan_published_capability_payload_retention(
                additional_catalog_retain_digests,
                additional_descriptor_snapshot_retain_digests,
            )
            .await?;
        let digest = plan.descriptor_digest()?;
        self.apply_published_capability_payload_retention(&plan, &digest)
            .await
    }

    async fn published_payload_retain_digests(
        &self,
        cursor: Option<&ControlPublishedCapabilityCursor>,
        additional_catalog_retain_digests: &[String],
        additional_descriptor_snapshot_retain_digests: &[String],
    ) -> UseResult<(BTreeSet<String>, BTreeSet<String>)> {
        let mut catalog_retain = additional_catalog_retain_digests
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut descriptor_snapshot_retain = additional_descriptor_snapshot_retain_digests
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let Some(cursor) = cursor else {
            return Ok((catalog_retain, descriptor_snapshot_retain));
        };
        cursor.validate()?;
        catalog_retain.insert(cursor.catalog.digest.clone());

        let key = ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation,
            cursor.capability_generation,
            cursor.descriptor_digest.clone(),
        )?;
        let snapshots = self
            .capability_payload_retention
            .descriptor_snapshot_store()
            .keys()
            .await?;
        if snapshots.is_empty() {
            return Ok((catalog_retain, descriptor_snapshot_retain));
        }
        if !snapshots.iter().any(|candidate| candidate == &key) {
            return Err(UseError::new(
                CAPABILITY_RETENTION_SNAPSHOT_ERROR,
                "The published Control capability has no matching descriptor proof snapshot.",
            ));
        }
        let snapshot = self
            .capability_payload_retention
            .descriptor_snapshot_store()
            .get(&key)
            .await?
            .ok_or_else(|| {
                UseError::new(
                    CAPABILITY_RETENTION_SNAPSHOT_ERROR,
                    "The published Control descriptor proof snapshot disappeared while retention was being planned.",
                )
            })?;
        descriptor_snapshot_retain.insert(snapshot.digest()?);
        Ok((catalog_retain, descriptor_snapshot_retain))
    }

    /// Reopen the exact published Capability snapshot from durable Control
    /// authority after a host restart. The returned lease keeps the complete
    /// package-generation set alive for the caller's accepted call set.
    pub(in crate::control_store) async fn reopen_published_capability(
        &self,
    ) -> UseResult<Option<ControlCapabilitySnapshotLease>> {
        self.capability_plane.reopen_published().await
    }

    /// Reconstruct a live Gateway endpoint from the durable published Control
    /// cursor after a host restart. The returned session factory retains the
    /// exact Control generation lease in every cloned immutable server.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn reopen_published_capability_gateway(
        &self,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionFactory>> {
        let Some(lease) = self.capability_plane.reopen_published().await? else {
            return Ok(None);
        };
        let server = Self::gateway_server_from_control_lease(lease, provider, options)?;
        Ok(Some(CapabilityGatewaySessionFactory::new(server)))
    }

    /// Reconstruct a live Gateway whose opaque invocation provider is bound to
    /// the same durable Control publication as the session lease. This helper
    /// keeps resolver and endpoint construction together so a host cannot
    /// accidentally pair a Control catalog with a Registry-backed resolver.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn reopen_published_capability_gateway_with_factory(
        &self,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionFactory>> {
        let provider = self.gateway_invocation_provider(factory);
        self.reopen_published_capability_gateway(provider, options)
            .await
    }

    /// Replace an existing live Gateway endpoint from the current durable
    /// Control publication. A missing or raced publication returns `None` so
    /// the host can retry after refreshing its lifecycle view; the factory
    /// itself retains the old server until the new lease-backed server swaps.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn replace_published_capability_gateway(
        &self,
        factory: &CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionReplacement>> {
        let Some(lease) = self.capability_plane.reopen_published().await? else {
            return Ok(None);
        };
        let server = Self::gateway_server_from_control_lease(lease, provider, options)?;
        Ok(Some(factory.replace(server).await?))
    }

    /// Replace a live Gateway from the current durable Control publication
    /// while constructing its resolver from the same Control authority.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn replace_published_capability_gateway_with_factory(
        &self,
        session: &CapabilityGatewaySessionFactory,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionReplacement>> {
        let provider = self.gateway_invocation_provider(factory);
        self.replace_published_capability_gateway(session, provider, options)
            .await
    }

    /// Reconcile a live Control-bound Gateway endpoint with the durable
    /// publication, without replacing an endpoint that already serves the
    /// exact same immutable catalog.  A newer in-memory endpoint is rejected
    /// rather than silently moving the durable authority backwards; callers
    /// can retry after refreshing their Control view.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn reconcile_published_capability_gateway(
        &self,
        factory: &CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<ControlCapabilityGatewayReconciliation>> {
        let Some(cursor) = self.store.published_capability().await? else {
            return Ok(None);
        };
        let expected = gateway_session_key_from_cursor(&cursor)?;
        let current = factory.current_key()?;
        if factory.current().generation_lease_mode()
            != CapabilityGatewayGenerationLeaseMode::External
        {
            return Err(UseError::new(
                "use.control.capability_gateway_activation_invalid",
                "The live Gateway endpoint is not bound to the Control generation lease authority.",
            ));
        }
        if current == expected {
            return Ok(Some(ControlCapabilityGatewayReconciliation::Unchanged(
                current,
            )));
        }
        if current.generation > expected.generation {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_stale",
                "The durable Control publication is older than the live Gateway endpoint.",
            ));
        }

        let Some(lease) = self.capability_plane.reopen_published().await? else {
            return Ok(None);
        };
        let server = Self::gateway_server_from_control_lease(lease, provider, options)?;
        let next = gateway_session_key(server.catalog())?;
        let current = factory.current_key()?;
        if current == next {
            return Ok(Some(ControlCapabilityGatewayReconciliation::Unchanged(
                current,
            )));
        }
        if current.generation > next.generation {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_stale",
                "The durable Control publication is older than the live Gateway endpoint.",
            ));
        }
        Ok(Some(ControlCapabilityGatewayReconciliation::Replaced(
            factory.replace(server).await?,
        )))
    }

    /// Build the graph-lifecycle activation adapter for a Gateway factory
    /// that was seeded from this composition's Control authority.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_cutover_activation(
        &self,
        factory: CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> Arc<dyn PluginGraphCapabilityCutoverActivation> {
        Arc::new(ControlCapabilityGatewayCutoverActivation {
            composition: self.clone(),
            factory,
            provider,
            options,
        })
    }

    /// Build a lifecycle activation hook whose resolver and session factory
    /// are both derived from this Control composition. Keeping this helper at
    /// the boundary prevents a host from attaching a Registry-backed provider
    /// to a Control-leased endpoint by accident.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_cutover_activation_with_factory(
        &self,
        session: CapabilityGatewaySessionFactory,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> Arc<dyn PluginGraphCapabilityCutoverActivation> {
        let provider = self.gateway_invocation_provider(factory);
        self.gateway_cutover_activation(session, provider, options)
    }

    /// Build the host provider that resolves opaque references through this
    /// composition's exact Control publication and generation lease.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_invocation_provider(
        &self,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
    ) -> Arc<dyn CapabilityGatewayInvocationProvider> {
        Arc::new(
            crate::capability_gateway::CapabilityGatewayResolvedProvider::new(Arc::new(
                ControlCapabilityGatewayInvocationResolver::new(
                    Arc::clone(&self.capability_plane),
                    factory,
                ),
            )),
        )
    }

    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_server_from_control_lease(
        lease: ControlCapabilitySnapshotLease,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<CapabilityGatewayMcpServer> {
        let CapabilityGatewayCompositionOptions {
            negotiation,
            limits,
        } = options;
        let catalog = lease.catalog().clone();
        let projected = catalog.for_consumer(&negotiation)?;
        lease.validate_gateway_catalog(&projected)?;
        let lease: Arc<dyn crate::capability_gateway::CapabilityGatewayExternalLease> =
            Arc::new(lease);
        let server = CapabilityGatewayMcpServer::with_consumer_negotiation_and_limits(
            catalog,
            provider,
            negotiation,
            limits,
        )?;
        server.with_external_lease(lease)
    }

    pub(in crate::control_store) async fn initialize(&self) -> UseResult<ControlStoreMetadata> {
        self.store.initialize().await
    }

    /// Persist one exact reviewed cognitive-package operation before any
    /// external effect or immutable Runtime plan publication.
    ///
    /// The Plan owns the intended target state and capability cursors. This
    /// boundary derives their prior values instead of accepting caller-selected
    /// generations, then delegates the compare-and-set to the Control Store.
    pub(in crate::control_store) async fn register_cognitive_package_operation(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        authorization: &CognitivePackageAuthorizationEvidence,
        grants: Option<&PlannedWorkspaceGrantOperation>,
        reviewed_at_ms: u64,
    ) -> UseResult<ControlOperationRecord> {
        let reviewed =
            reviewed_cognitive_package_operation(envelope, authorization, grants, reviewed_at_ms)?;
        self.store.register_operation(reviewed).await
    }

    /// Admit and commit one cognitive-package lifecycle operation with its
    /// Runtime plan payloads under one installation-wide shared fence.
    ///
    /// The reviewed Plan and authorization evidence are converted first. The
    /// held fence then orders the durable boundaries as: Control registration,
    /// transition projection, immutable Runtime publication, and Control
    /// commit. No SQLite transaction is held across Runtime plan I/O, and a
    /// failed publication leaves the reviewed operation available for an exact
    /// retry or explicit cancellation.
    pub(in crate::control_store) async fn admit_and_commit_cognitive_package_operation_with_runtime_plans(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        authorization: &CognitivePackageAuthorizationEvidence,
        grants: Option<&PlannedWorkspaceGrantOperation>,
        reviewed_at_ms: u64,
        committed_at_ms: u64,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<ControlGeneration> {
        let reviewed =
            reviewed_cognitive_package_operation(envelope, authorization, grants, reviewed_at_ms)?;
        let operation_id = reviewed.operation_id().to_string();
        // Reference admission must cover registration as well as publication:
        // the reviewed transition may retain package/artifact references while
        // the external plan bytes are being published.
        let _artifact_admission = self.artifact_store.acquire_reference_admission().await?;
        let maintenance = StateMaintenanceLock::new(&self.store.state_root)
            .acquire_shared()
            .await?;
        self.store
            .register_operation_under_maintenance(reviewed)
            .await?;
        self.commit_registered_operation_under_maintenance(
            &maintenance,
            &operation_id,
            committed_at_ms,
            publications,
        )
        .await
    }

    pub(in crate::control_store) async fn dispatch_next(
        &self,
        request: ControlEffectDispatchRequest,
    ) -> UseResult<ControlEffectDispatchResult> {
        self.effects.dispatch_next(request).await
    }

    /// Publish Runtime payloads independently when the caller has not yet
    /// assembled a Control transition. This remains monotonic and idempotent;
    /// production lifecycle code should prefer the combined method below.
    pub(in crate::control_store) async fn publish_runtime_plans(
        &self,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<crate::plugin_runtime::RuntimeSurfacePlanPublishResult> {
        // `RuntimeSurfacePlanStore::from_extension_paths` carries the same
        // global Artifact Store and acquires reference admission before its
        // installation fence. Keep this entry point thin so the lock order is
        // defined in one place for every standalone publication caller.
        self.plan_store.publish(publications).await
    }

    /// Publish the exact new Runtime plan payloads before committing their
    /// Control transition while retaining one installation-wide shared fence.
    ///
    /// Publication is intentionally monotonic: if the database commit fails,
    /// immutable, unreferenced plan records may remain for bounded later
    /// collection, but a committed Runtime effect can never point at a record
    /// that was not durably published first. The effect inventory check rejects
    /// missing or extra target `SurfacePrepare` publications before any bytes
    /// are written.
    /// Derive the exact transition from a registered reviewed operation and
    /// commit it with its immutable Runtime payloads. This is the preferred
    /// production entry point: callers provide only the operation identity,
    /// commit timestamp, and host-produced plan payloads; graph, Grant,
    /// provider, capability, and effect fields are projected by the Control
    /// Store itself.
    pub(in crate::control_store) async fn commit_reviewed_operation_with_runtime_plans(
        &self,
        operation_id: &str,
        committed_at_ms: u64,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<ControlGeneration> {
        // The projected Control transition retains package and Runtime
        // artifact references. Reference admission is therefore the outer
        // boundary; the installation maintenance fence is nested beneath it
        // and remains held through plan publication plus the authority CAS.
        let _artifact_admission = self.artifact_store.acquire_reference_admission().await?;
        let _maintenance = StateMaintenanceLock::new(&self.store.state_root)
            .acquire_shared()
            .await?;
        self.commit_registered_operation_under_maintenance(
            &_maintenance,
            operation_id,
            committed_at_ms,
            publications,
        )
        .await
    }

    async fn commit_registered_operation_under_maintenance(
        &self,
        _maintenance: &StateMaintenanceGuard,
        operation_id: &str,
        committed_at_ms: u64,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<ControlGeneration> {
        let (reviewed, transition) = self
            .store
            .project_transition_under_maintenance(operation_id, committed_at_ms)
            .await?;
        validate_runtime_publications(&transition, publications)?;
        validate_runtime_publication_authority(&reviewed, publications)?;
        self.plan_store
            .publish_under_maintenance(_maintenance, publications)
            .await?;
        self.store
            .commit_transition_under_maintenance(transition)
            .await
    }
}

#[cfg(feature = "mcp")]
#[async_trait]
impl PluginGraphCapabilityCutoverActivation for ControlCapabilityGatewayCutoverActivation {
    async fn activate_capability_cutover(&self, _idempotency_key: &str) -> UseResult<()> {
        self.composition
            .reconcile_published_capability_gateway(
                &self.factory,
                Arc::clone(&self.provider),
                self.options.clone(),
            )
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.control.capability_gateway_publication_missing",
                    "The lifecycle cutover has no durable Control Gateway publication to activate.",
                )
            })?;
        Ok(())
    }
}

fn ensure_published_payloads_are_retained(
    cursor: &ControlPublishedCapabilityCursor,
    plan: &ControlCapabilityPayloadRetentionPlan,
) -> UseResult<()> {
    cursor.validate()?;
    let catalog_is_retained = plan.catalog_plan.retain.iter().any(|entry| {
        entry.digest == cursor.catalog.digest
            && entry.generation == cursor.catalog.generation
            && entry.revision == cursor.catalog.revision
    });
    if !catalog_is_retained {
        return Err(UseError::new(
            CAPABILITY_RETENTION_CURSOR_ERROR,
            "The reviewed retention plan would remove the catalog selected by the published Control cursor.",
        ));
    }

    // A proof snapshot is optional for the legacy proof-only projector.  Once
    // this owner has any records, however, a published cursor must retain the
    // exact key-derived record rather than silently pruning the evidence that
    // would be needed to reconstruct its descriptor projection.
    if plan.descriptor_snapshot_plan.before_record_count > 0 {
        let key = ControlCapabilityDescriptorSnapshotKey::new(
            cursor.installation.clone(),
            cursor.installation_generation,
            cursor.capability_generation,
            cursor.descriptor_digest.clone(),
        )?;
        let key_digest = key.digest()?;
        let snapshot_is_retained = plan.descriptor_snapshot_plan.retain.iter().any(|entry| {
            entry.key_digest == key_digest
                && entry.installation_generation == cursor.installation_generation
                && entry.capability_generation == cursor.capability_generation
        });
        if !snapshot_is_retained {
            return Err(UseError::new(
                CAPABILITY_RETENTION_SNAPSHOT_ERROR,
                "The reviewed retention plan would remove the descriptor proof snapshot selected by the published Control cursor.",
            ));
        }
    }
    Ok(())
}

#[cfg(feature = "mcp")]
fn gateway_session_key_from_cursor(
    cursor: &ControlPublishedCapabilityCursor,
) -> UseResult<CapabilityGatewaySessionKey> {
    cursor.validate()?;
    Ok(CapabilityGatewaySessionKey {
        installation: cursor.installation.clone(),
        generation: cursor.catalog.generation,
        revision: cursor.catalog.revision.clone(),
        digest: cursor.catalog.digest.clone(),
    })
}

#[cfg(feature = "mcp")]
fn gateway_session_key(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<CapabilityGatewaySessionKey> {
    catalog.validate()?;
    Ok(CapabilityGatewaySessionKey {
        installation: catalog.installation().clone(),
        generation: catalog.generation(),
        revision: catalog.revision().to_owned(),
        digest: catalog.descriptor_digest()?,
    })
}

/// Validate the publication set against the target Runtime prepare inventory.
///
/// The transition is the only source of desired-state authority. A publication
/// is merely an immutable payload, so this check binds every target Runtime
/// effect to one exact package/surface/provider identity and rejects extras.
/// The plan's authorization digest is checked again by the committed resolver
/// from Control-derived evidence at effect claim time.
pub(in crate::control_store) fn validate_runtime_publications(
    transition: &ControlTransition,
    publications: &[RuntimeSurfacePlanPublication],
) -> UseResult<()> {
    let expected = transition
        .effects
        .iter()
        .filter_map(|effect| runtime_prepare_identity(effect, transition.snapshot.generation))
        .collect::<Vec<_>>();
    let expected_set = expected.iter().cloned().collect::<BTreeSet<_>>();
    if expected_set.len() != expected.len() || expected_set.len() != publications.len() {
        return Err(UseError::new(
            PUBLICATION_ERROR,
            "Runtime plan publications must exactly cover target Runtime prepare effects.",
        ));
    }

    let mut seen = BTreeSet::new();
    for publication in publications {
        publication.key.validate()?;
        if publication.key.scope != transition.snapshot.installation {
            return Err(UseError::new(
                PUBLICATION_ERROR,
                "A Runtime plan publication belongs to another installation.",
            ));
        }
        let identity = RuntimePublicationIdentity::from_key(&publication.key);
        if !expected_set.contains(&identity) || !seen.insert(identity) {
            return Err(UseError::new(
                PUBLICATION_ERROR,
                "A Runtime plan publication does not match one unique target effect.",
            ));
        }
        // Re-run the pair validation here even though the public constructor
        // already does so; callers may have deserialized or cloned the value
        // across an internal boundary.
        RuntimeSurfacePlanPublication::new(publication.key.clone(), publication.plan.clone())?;
    }
    Ok(())
}

/// Bind each published plan's authorization digest to the exact reviewed
/// Grant proposal that will be projected for its package. The finalized Grant
/// digest is intentionally different: Runtime planning is bound to the stable
/// pre-confirmation proposal, while the Control authority derives that same
/// proposal from the immutable reviewed operation at claim time.
pub(in crate::control_store) fn validate_runtime_publication_authority(
    reviewed: &ReviewedControlOperation,
    publications: &[RuntimeSurfacePlanPublication],
) -> UseResult<()> {
    let proposals = reviewed
        .authorization
        .grant_transition
        .as_ref()
        .map(|transition| {
            transition
                .change_set
                .changes
                .iter()
                .filter_map(|change| change.after.as_ref())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for publication in publications {
        let expected = proposals.iter().find(|proposal| {
            proposal.package_id == publication.plan.context().package_id()
                && proposal.package_digest == publication.plan.context().package_digest()
                && proposal.scope_id == publication.plan.context().scope().id
        });
        let Some(expected) = expected else {
            return Err(UseError::new(
                PUBLICATION_ERROR,
                "A Runtime plan has no exact reviewed Grant proposal authority.",
            ));
        };
        let expected_digest = expected.descriptor_digest().map_err(|error| {
            UseError::new(
                PUBLICATION_ERROR,
                format!("The reviewed Runtime Grant proposal is not canonical: {error}"),
            )
        })?;
        if publication.plan.context().grant_digest() != expected_digest
            || publication.key.grant_digest.as_deref() != Some(expected_digest.as_str())
        {
            return Err(UseError::new(
                PUBLICATION_ERROR,
                "A Runtime plan authorization digest differs from the reviewed Grant proposal.",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RuntimePublicationIdentity {
    package_id: String,
    package_digest: String,
    surface: PluginSurfaceRef,
    lifecycle_generation: u64,
    provider_id: String,
    selection_digest: String,
}

impl RuntimePublicationIdentity {
    fn from_key(key: &crate::plugin_runtime::RuntimeSurfacePlanKey) -> Self {
        Self {
            package_id: key.package_id.clone(),
            package_digest: key.package_digest.clone(),
            surface: key.surface.surface.clone(),
            lifecycle_generation: key.generation,
            provider_id: key.provider_id.clone(),
            selection_digest: key.selection_digest.clone(),
        }
    }
}

fn runtime_prepare_identity(
    effect: &super::model::ControlEffectIntent,
    target_generation: u64,
) -> Option<RuntimePublicationIdentity> {
    if effect.kind != ControlEffectKind::SurfacePrepare {
        return None;
    }
    let ControlEffectSubject::Surface {
        package_id,
        lifecycle_generation,
        package_digest,
        surface,
        ..
    } = &effect.subject
    else {
        return None;
    };
    if effect.installation_generation != target_generation {
        return None;
    }
    let ControlEffectOwner::RuntimeProvider {
        provider_id,
        selection_digest,
    } = &effect.owner
    else {
        return None;
    };
    Some(RuntimePublicationIdentity {
        package_id: package_id.clone(),
        package_digest: package_digest.clone(),
        surface: surface.clone(),
        lifecycle_generation: *lifecycle_generation,
        provider_id: provider_id.clone(),
        selection_digest: selection_digest.clone(),
    })
}
