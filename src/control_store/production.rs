//! Production Control Store lifecycle entry for the A2 authority cutover.
//!
//! This module is the only intended production constructor for
//! [`ControlStoreRuntimeComposition`]. Callers must keep ADR-003 invariants:
//! one installation opens either Control or legacy files, never both; clean
//! state only; no dual write; no legacy fallback read.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{InstallationSnapshot, PluginOperationPlanEnvelope, UseError, UseResult};
use a3s_use_extension::{
    CapabilityDescriptionTrustStore, ExtensionPaths, PluginMcpSurface, ToolSurface,
    VerifiedCapabilityDescriptionTrustStore,
};
use sha2::{Digest, Sha256};
use tokio::time::sleep;

use super::composition::{ControlEffectCompositionDependencies, ControlStoreRuntimeComposition};
use super::dispatcher::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    SystemControlEffectClock,
};
use super::effect_owner::capability_plane::{
    ControlCapabilityDescriptorProjection, ControlCapabilityDescriptorSnapshot,
    ControlCapabilityDescriptorSnapshotKey, ControlCapabilityDescriptorSnapshotStore,
    ControlCapabilitySignerPolicy,
};
use super::effect_owner::flow::ControlA3sFlowEffectPort;
use super::effect_owner::runtime::{
    ControlRuntimeMcpReadiness, ControlRuntimeServiceReadinessPort,
};
use super::effect_port::{
    ControlCapabilityCatalogProjectionPort, ControlEffectFailure, ControlEffectPortOutcome,
    ControlFlowEffectPort, ControlSurfaceApplication, ControlSurfaceEffectRequest,
};
use super::filesystem::CONTROL_STORE_DATABASE_FILE;
use super::model::{ControlEffectOutcome, ControlOperationStatus};
use crate::cognitive_package::{
    CognitivePackageAuthorizationEvidence, PlannedWorkspaceGrantOperation,
};
#[cfg(feature = "mcp")]
use crate::plugin_lifecycle::{operation_cutover_key, PluginGraphCapabilityCutoverActivation};
use crate::plugin_runtime::{
    RuntimeEndpointRef, RuntimeMcpInitializeEvidence, RuntimeSurfaceContract,
    RuntimeSurfacePlanPublication,
};
use async_trait::async_trait;
#[cfg(feature = "mcp")]
use std::sync::Mutex;

const PRODUCTION_ERROR: &str = "use.control_store.production_activation_invalid";
const LEGACY_AUTHORITY_ERROR: &str = "use.control_store.legacy_state_unsupported";
#[cfg(feature = "mcp")]
const GATEWAY_RECONCILE_POLL: Duration = Duration::from_millis(500);
#[cfg(feature = "mcp")]
const GATEWAY_SHUTDOWN_DRAIN: Duration = Duration::from_secs(30);

/// Observed Control operation phase for Host and manager readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlObservedOperationPhase {
    InFlight,
    Completed,
    Cancelled,
    Rejected,
}

/// Provider-neutral Control operation observation (no legacy file authority).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ControlObservedOperation {
    pub envelope: PluginOperationPlanEnvelope,
    pub phase: ControlObservedOperationPhase,
    pub completed_at_ms: Option<u64>,
    pub result_digest: Option<String>,
}

impl ControlObservedOperation {
    pub fn matches_envelope(&self, envelope: &PluginOperationPlanEnvelope) -> bool {
        &self.envelope == envelope
    }
}

/// Legacy mutable-authority leaves that must stay absent under Control.
///
/// Kept in sync with `docs/control-store-cutover.acl` authority `legacy_paths`.
const LEGACY_AUTHORITY_PATHS: &[&str] = &[
    "bindings/flow",
    "bindings/knowledge",
    "bindings/runtime",
    "extension-generations",
    "extensions",
    "grants",
    "installation-snapshot.json",
    "operations/package-graphs",
    "operations/plugins",
    "package-enablement",
    "registry.json",
];

const MAX_EFFECT_DISPATCH_ITERATIONS: usize = 64;
const MAX_DEFERRED_WAIT: Duration = Duration::from_secs(5);
const MAX_DEFERRED_STREAK: usize = 8;

/// Host-owned ports required to compose one production Control lifecycle.
pub(crate) struct ProductionControlHostDependencies {
    pub(crate) runtime_registry: Arc<RuntimeClientRegistry>,
    pub(crate) runtime_readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
    pub(crate) catalog_projection: Arc<dyn ControlCapabilityCatalogProjectionPort>,
    pub(crate) flow: Arc<dyn ControlFlowEffectPort>,
    pub(crate) clock: Arc<dyn ControlEffectClock>,
}

impl ProductionControlHostDependencies {
    /// Compose with the system clock when the host does not inject time.
    pub(crate) fn with_system_clock(
        runtime_registry: Arc<RuntimeClientRegistry>,
        runtime_readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
        catalog_projection: Arc<dyn ControlCapabilityCatalogProjectionPort>,
        flow: Arc<dyn ControlFlowEffectPort>,
    ) -> Self {
        Self {
            runtime_registry,
            runtime_readiness,
            catalog_projection,
            flow,
            clock: Arc::new(SystemControlEffectClock),
        }
    }

    /// Standalone defaults with an injected Runtime Service readiness port.
    ///
    /// Managed hosts that own live Gateway/Runtime Service bindings pass their
    /// [`ControlRuntimeServiceReadinessPort`] here so Control effects bind real
    /// endpoints instead of opaque `gateway:` placeholders. The composition
    /// still wraps the port with endpoint-route recording when MCP is enabled.
    pub(crate) fn with_injected_runtime_readiness(
        paths: &ExtensionPaths,
        runtime_registry: Arc<RuntimeClientRegistry>,
        runtime_readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
        flow_compiler: Option<&Path>,
    ) -> UseResult<Self> {
        let flow: Arc<dyn ControlFlowEffectPort> = match flow_compiler {
            Some(compiler) => Arc::new(ControlA3sFlowEffectPort::new(
                paths.artifact_store(),
                compiler,
                paths
                    .use_paths()
                    .data_root()
                    .join("artifacts")
                    .join("flow-native-ts"),
            )?),
            None => Arc::new(StandaloneRejectingFlow),
        };
        let snapshots = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths);
        let catalog_projection = Arc::new(
            ControlCapabilityDescriptorProjection::from_snapshot_store(snapshots)?,
        );
        Ok(Self::with_system_clock(
            runtime_registry,
            runtime_readiness,
            catalog_projection,
            flow,
        ))
    }

    /// Standalone defaults: restart-stable descriptor-snapshot catalog
    /// projection and opaque Gateway endpoint minting. Live loopback routes
    /// are recorded beside those opaque identities by composition. When
    /// `flow_compiler` is set, compose the Control A3S Flow owner; otherwise
    /// Flow surfaces fail closed until a host injects one.
    pub(crate) fn standalone(
        paths: &ExtensionPaths,
        runtime_registry: Arc<RuntimeClientRegistry>,
        flow_compiler: Option<&Path>,
    ) -> UseResult<Self> {
        let flow: Arc<dyn ControlFlowEffectPort> = match flow_compiler {
            Some(compiler) => Arc::new(ControlA3sFlowEffectPort::new(
                paths.artifact_store(),
                compiler,
                paths
                    .use_paths()
                    .data_root()
                    .join("artifacts")
                    .join("flow-native-ts"),
            )?),
            None => Arc::new(StandaloneRejectingFlow),
        };
        let snapshots = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths);
        let catalog_projection = Arc::new(
            ControlCapabilityDescriptorProjection::from_snapshot_store(snapshots)?,
        );
        Ok(Self::with_system_clock(
            runtime_registry,
            Arc::new(StandaloneOpaqueBindingReadiness),
            catalog_projection,
            flow,
        ))
    }

    /// Standalone defaults that re-verify signed descriptor snapshots on every
    /// catalog projection. Hosts must supply the Registry/TUF-derived trust
    /// store; unsigned proof-only snapshots are rejected by this projector.
    pub(crate) fn standalone_with_signed_catalog(
        paths: &ExtensionPaths,
        runtime_registry: Arc<RuntimeClientRegistry>,
        trust_store: CapabilityDescriptionTrustStore,
        flow_compiler: Option<&Path>,
    ) -> UseResult<Self> {
        let flow: Arc<dyn ControlFlowEffectPort> = match flow_compiler {
            Some(compiler) => Arc::new(ControlA3sFlowEffectPort::new(
                paths.artifact_store(),
                compiler,
                paths
                    .use_paths()
                    .data_root()
                    .join("artifacts")
                    .join("flow-native-ts"),
            )?),
            None => Arc::new(StandaloneRejectingFlow),
        };
        let snapshots = ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths);
        let catalog_projection = Arc::new(
            ControlCapabilityDescriptorProjection::from_signed_snapshot_store(
                snapshots,
                trust_store,
            )?,
        );
        Ok(Self::with_system_clock(
            runtime_registry,
            Arc::new(StandaloneOpaqueBindingReadiness),
            catalog_projection,
            flow,
        ))
    }

    /// Product path: inject a trust store previously loaded from the signed
    /// Registry/TUF target `capability/description-trust-store-v1.json`.
    pub(crate) fn standalone_with_signed_catalog_from_registry(
        paths: &ExtensionPaths,
        runtime_registry: Arc<RuntimeClientRegistry>,
        trust: VerifiedCapabilityDescriptionTrustStore,
        flow_compiler: Option<&Path>,
    ) -> UseResult<Self> {
        trust.validate()?;
        Self::standalone_with_signed_catalog(
            paths,
            runtime_registry,
            trust.into_store(),
            flow_compiler,
        )
    }
}

/// Production lifecycle face over the Control Store composition.
///
/// Construction is side-effect free. [`Self::initialize`] creates the database
/// only after rejecting every legacy authority leaf.
#[derive(Clone)]
pub(crate) struct ProductionControlLifecycle {
    composition: ControlStoreRuntimeComposition,
    /// Host-retained Gateway cutover activation for same-process graph apply.
    ///
    /// When set, production effect drain activates the live endpoint after the
    /// durable CapabilityCutover observation and before prior-generation
    /// Remove/Prepare effects that require those package leases.
    #[cfg(feature = "mcp")]
    retained_gateway_cutover:
        Arc<Mutex<Option<Arc<dyn PluginGraphCapabilityCutoverActivation>>>>,
}

impl std::fmt::Debug for ProductionControlLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProductionControlLifecycle")
            .field("composition", &self.composition)
            .finish()
    }
}

impl ProductionControlLifecycle {
    /// Compose one production Control lifecycle for an exact installation.
    pub(crate) fn from_extension_paths(
        paths: &ExtensionPaths,
        dependencies: ProductionControlHostDependencies,
    ) -> UseResult<Self> {
        let composition = ControlStoreRuntimeComposition::from_extension_paths(
            paths,
            ControlEffectCompositionDependencies {
                runtime_registry: dependencies.runtime_registry,
                runtime_readiness: dependencies.runtime_readiness,
                catalog_projection: dependencies.catalog_projection,
                flow: dependencies.flow,
                clock: dependencies.clock,
            },
        )?;
        Ok(Self {
            composition,
            #[cfg(feature = "mcp")]
            retained_gateway_cutover: Arc::new(Mutex::new(None)),
        })
    }

    /// Initialize Control as the sole mutable authority for a clean root.
    pub(crate) async fn initialize(&self) -> UseResult<()> {
        self.composition.initialize().await?;
        reject_legacy_authority_paths(self.composition.store().state_root.as_path())?;
        Ok(())
    }

    pub(crate) fn composition(&self) -> &ControlStoreRuntimeComposition {
        &self.composition
    }

    /// Admit, commit, and drain every outbox effect for one reviewed operation.
    ///
    /// This is the production mutation seam that replaces file-store graph
    /// apply plus `InstallationSnapshotStore` writes. Callers must not write
    /// any legacy authority path before or after this method.
    ///
    /// `maintenance` must be the caller's shared fence for this installation.
    /// Drain reuses it instead of re-locking `.maintenance.lock` (nested shared
    /// file locks deadlock on Windows).
    pub(crate) async fn apply_reviewed_operation(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        authorization: &CognitivePackageAuthorizationEvidence,
        grants: Option<&PlannedWorkspaceGrantOperation>,
        reviewed_at_ms: u64,
        committed_at_ms: u64,
        publications: &[RuntimeSurfacePlanPublication],
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<InstallationSnapshot> {
        if !maintenance.is_shared_for(self.composition.store().state_root.as_path()) {
            return Err(UseError::new(
                PRODUCTION_ERROR,
                "Control production apply requires the caller's shared maintenance fence for this installation.",
            ));
        }
        reject_legacy_authority_paths(self.composition.store().state_root.as_path())?;
        // An abandoned EffectsPending for a *different* operation must finish
        // before a new commit. Same-operation recovery stays on admit replay +
        // drain so completion identity is not doubled.
        if let Some(pending) = self.composition.store().effects_pending_operation().await? {
            let pending_id = pending.reviewed.operation_id();
            if pending_id != envelope.plan.operation_id.as_str() {
                self.drain_effects(pending_id, maintenance.clone()).await?;
            }
        }
        let generation = self
            .composition
            .admit_and_commit_cognitive_package_operation_with_runtime_plans(
                envelope,
                authorization,
                grants,
                reviewed_at_ms,
                committed_at_ms,
                publications,
            )
            .await?;
        // Durable catalog projection requires the exact unsigned (or signed)
        // descriptor snapshot before Capability Index cutover. Seed an empty
        // proof set only when the published Gateway catalog is empty (skill-
        // only / catalog-empty). Non-empty Tool/MCP catalogs leave the
        // snapshot absent until the host stages proofs.
        self.ensure_unsigned_descriptor_snapshot_for_generation(&generation)
            .await?;
        self.drain_effects(envelope.plan.operation_id.as_str(), maintenance)
            .await?;
        reject_legacy_authority_paths(self.composition.store().state_root.as_path())?;
        Ok(generation.snapshot)
    }

    /// Resume the sole EffectsPending operation after process restart.
    ///
    /// Returns immediately when no EffectsPending record exists. Generation
    /// cursors are not advanced; only external-effect drain and operation
    /// completion run.
    pub(crate) async fn resume_pending_effects(
        &self,
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<Option<String>> {
        if !maintenance.is_shared_for(self.composition.store().state_root.as_path()) {
            return Err(UseError::new(
                PRODUCTION_ERROR,
                "Control pending-effect resume requires the caller's shared maintenance fence.",
            ));
        }
        reject_legacy_authority_paths(self.composition.store().state_root.as_path())?;
        let Some(pending) = self.composition.store().effects_pending_operation().await? else {
            return Ok(None);
        };
        let operation_id = pending.reviewed.operation_id().to_string();
        if let Some(generation) = self.composition.store().current_generation().await? {
            self.ensure_unsigned_descriptor_snapshot_for_generation(&generation)
                .await?;
        }
        self.drain_effects(&operation_id, maintenance).await?;
        reject_legacy_authority_paths(self.composition.store().state_root.as_path())?;
        Ok(Some(operation_id))
    }

    /// Publish a restart-stable empty proof snapshot for the committed
    /// capability identity when none exists yet **and** the published
    /// Gateway catalog has no descriptors that require proofs.
    ///
    /// Skill-only / catalog-empty generations project an empty Durable catalog
    /// and need this seed so cutover does not defer forever. Non-empty
    /// Tool/MCP publications must stage signed (or exact) proofs before drain;
    /// seeding an empty snapshot for those identities would conflict with the
    /// later proof publish.
    async fn ensure_unsigned_descriptor_snapshot_for_generation(
        &self,
        generation: &super::model::ControlGeneration,
    ) -> UseResult<()> {
        if generation.capability.generation == 0 {
            return Ok(());
        }
        let key = ControlCapabilityDescriptorSnapshotKey::new(
            generation.snapshot.installation.clone(),
            generation.snapshot.generation,
            generation.capability.generation,
            generation.capability.descriptor_digest.clone(),
        )?;
        let store = ControlCapabilityDescriptorSnapshotStore::new(
            self.composition.store().state_root.clone(),
            self.composition.store().installation.clone(),
        )?;
        if store.get(&key).await?.is_some() {
            return Ok(());
        }
        let Some(cursor) = self.composition.store().published_capability().await? else {
            return Ok(());
        };
        if cursor.capability_generation != generation.capability.generation
            || cursor.descriptor_digest != generation.capability.descriptor_digest
        {
            return Ok(());
        }
        let Some(catalog) = self
            .composition
            .catalog_store()
            .get(&cursor.catalog.digest)
            .await?
        else {
            return Ok(());
        };
        if !catalog.descriptors().is_empty() {
            // Tools/MCP must stage proofs; do not occupy the identity with an
            // empty seed that would conflict with a later signed publish.
            return Ok(());
        }
        let snapshot = ControlCapabilityDescriptorSnapshot::new(
            key,
            Vec::new(),
            ControlCapabilitySignerPolicy::new(std::collections::BTreeMap::new())?,
        )?;
        store.publish(&snapshot).await?;
        Ok(())
    }

    /// Read the committed installation snapshot, if any generation exists.
    pub(crate) async fn current_snapshot(&self) -> UseResult<Option<InstallationSnapshot>> {
        Ok(self
            .composition
            .store()
            .current_generation()
            .await?
            .map(|generation| generation.snapshot))
    }

    /// Reopen the durable published Capability Gateway with the production
    /// Grant + Runtime invocation join and composition-owned live endpoint
    /// routes. Returns `None` when no catalog has been published yet.
    #[cfg(feature = "mcp")]
    pub(crate) async fn open_published_capability_gateway(
        &self,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<crate::capability_gateway::CapabilityGatewaySessionFactory>> {
        let provider = self.composition.production_gateway_invocation_provider();
        self.composition
            .reopen_published_capability_gateway(provider, options)
            .await
    }

    /// Reconcile a live Gateway session factory to the durable published
    /// Control catalog after lifecycle cutover. Hosts that retain a long-lived
    /// session must call this (after releasing prior-generation leases) so
    /// clients observe `list_changed` against the replacement publication.
    #[cfg(feature = "mcp")]
    pub(crate) async fn reconcile_published_capability_gateway(
        &self,
        session: &crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<super::composition::ControlCapabilityGatewayReconciliation>> {
        let provider = self.composition.production_gateway_invocation_provider();
        self.composition
            .reconcile_published_capability_gateway(session, provider, options)
            .await
    }

    /// Serve the durable published Capability Gateway over stdio for the
    /// standalone product CLI. Fails closed when no catalog is published.
    #[cfg(feature = "mcp")]
    pub(crate) async fn serve_published_capability_gateway_stdio(
        &self,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<()> {
        let Some(factory) = self.open_published_capability_gateway(options).await? else {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_missing",
                "No published Control Capability Gateway catalog is available for this installation.",
            ));
        };
        factory.serve_stdio().await
    }

    /// Serve a long-lived HTTP Capability Gateway that stays bound to Control.
    ///
    /// While the endpoint is up, this watches the durable published cursor and
    /// reconciles the retained session so clients observe `list_changed` when
    /// another process advances the publication. On shutdown it drains the
    /// session and retains only the Control-selected payloads.
    #[cfg(feature = "mcp")]
    pub(crate) async fn serve_published_capability_gateway_streamable_http(
        &self,
        listener: tokio::net::TcpListener,
        config: crate::capability_gateway::CapabilityGatewayHttpConfig,
        shutdown: tokio_util::sync::CancellationToken,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<()> {
        let Some(factory) = self.open_published_capability_gateway(options.clone()).await? else {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_missing",
                "No published Control Capability Gateway catalog is available for this installation.",
            ));
        };
        let reconcile_shutdown = shutdown.clone();
        let reconcile_lifecycle = self.clone();
        let reconcile_factory = factory.clone();
        let reconcile_options = options;
        let reconcile_task = tokio::spawn(async move {
            reconcile_lifecycle
                .watch_and_reconcile_published_capability_gateway(
                    &reconcile_factory,
                    reconcile_options,
                    &reconcile_shutdown,
                    GATEWAY_RECONCILE_POLL,
                )
                .await
        });
        let serve_result = factory
            .clone()
            .serve_streamable_http(listener, config, shutdown.clone())
            .await;
        // Stop the watcher whether serve ended via signal or transport error.
        shutdown.cancel();
        let reconcile_result = match reconcile_task.await {
            Ok(result) => result,
            Err(error) => Err(UseError::new(
                PRODUCTION_ERROR,
                format!("Capability Gateway reconcile watcher failed: {error}"),
            )),
        };
        let drain_result = self
            .drain_and_retain_published_capability_gateway(
                &factory,
                GATEWAY_SHUTDOWN_DRAIN,
                &[],
                &[],
            )
            .await;
        serve_result?;
        reconcile_result?;
        drain_result.map(|_| ())
    }

    /// Poll Control and reconcile one retained Gateway session until shutdown.
    ///
    /// Embedding hosts that retain a live session outside the product HTTP
    /// serve path can run this beside their own transport. Same-process graph
    /// apply should prefer [`Self::gateway_cutover_activation`] instead.
    #[cfg(feature = "mcp")]
    pub(crate) async fn watch_and_reconcile_published_capability_gateway(
        &self,
        session: &crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
        shutdown: &tokio_util::sync::CancellationToken,
        poll_interval: Duration,
    ) -> UseResult<()> {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => return Ok(()),
                _ = sleep(poll_interval) => {
                    match self
                        .reconcile_published_capability_gateway(session, options.clone())
                        .await
                    {
                        Ok(Some(_)) | Ok(None) => {}
                        Err(error)
                            if error.code
                                == "use.control.capability_gateway_publication_missing" =>
                        {
                            // Brief window while another process replaces the
                            // cursor; keep the endpoint on the prior lease.
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }

    /// Build the graph-lifecycle cutover activation hook for one live Gateway
    /// session factory seeded from this Control publication.
    ///
    /// Prefer [`Self::attach_retained_gateway_cutover`] for Control production
    /// apply: drain activates after CapabilityCutover before prior-generation
    /// Remove/Prepare. Long-lived hosts that still drive
    /// [`crate::plugin_lifecycle::PluginPackageGraphLifecycleCoordinator`] may
    /// attach the returned activation via
    /// [`crate::plugin_lifecycle::PluginPackageGraphLifecycleCoordinator::with_capability_cutover_activation`].
    /// Product CLI reopen paths that open a fresh session after apply do not
    /// need this hook. Product HTTP serve attaches reconcile via
    /// [`Self::serve_published_capability_gateway_streamable_http`] instead.
    #[cfg(feature = "mcp")]
    pub(crate) fn gateway_cutover_activation(
        &self,
        session: crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> std::sync::Arc<dyn crate::plugin_lifecycle::PluginGraphCapabilityCutoverActivation> {
        let provider = self.composition.production_gateway_invocation_provider();
        self.composition
            .gateway_cutover_activation(session, provider, options)
    }

    /// Retain a live Gateway session for subsequent Control production applies.
    ///
    /// While attached, effect drain activates this session after the durable
    /// CapabilityCutover observation so prior-generation leases release before
    /// Remove/Prepare. Clear with [`Self::clear_retained_gateway_cutover`] when
    /// the host drops the session.
    #[cfg(feature = "mcp")]
    pub(crate) fn attach_retained_gateway_cutover(
        &self,
        session: crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) {
        let activation = self.gateway_cutover_activation(session, options);
        *self
            .retained_gateway_cutover
            .lock()
            .expect("retained Gateway cutover lock") = Some(activation);
    }

    /// Drop the retained Gateway cutover attachment.
    #[cfg(feature = "mcp")]
    pub(crate) fn clear_retained_gateway_cutover(&self) {
        *self
            .retained_gateway_cutover
            .lock()
            .expect("retained Gateway cutover lock") = None;
    }

    /// Drain one live Control-bound Gateway session and retain only the
    /// payloads selected by the durable Control cursor.
    ///
    /// Hosts that are retiring an endpoint (shutdown or controlled replace)
    /// must call this after releasing prior-generation package leases so
    /// exclusive payload retention can proceed. The product HTTP serve path
    /// invokes this automatically on shutdown.
    #[cfg(feature = "mcp")]
    pub(crate) async fn drain_and_retain_published_capability_gateway(
        &self,
        session: &crate::capability_gateway::CapabilityGatewaySessionFactory,
        drain_timeout: std::time::Duration,
        additional_catalog_retain_digests: &[String],
        additional_descriptor_snapshot_retain_digests: &[String],
    ) -> UseResult<super::effect_owner::capability_plane::ControlCapabilityPayloadRetentionResult> {
        self.composition
            .drain_and_retain_published_capability_gateway(
                session,
                drain_timeout,
                additional_catalog_retain_digests,
                additional_descriptor_snapshot_retain_digests,
            )
            .await
    }

    /// Host-facing drain that hides the private retention result type.
    #[cfg(feature = "mcp")]
    pub(crate) async fn drain_published_capability_gateway_for_host(
        &self,
        session: &crate::capability_gateway::CapabilityGatewaySessionFactory,
        drain_timeout: std::time::Duration,
    ) -> UseResult<()> {
        self.drain_and_retain_published_capability_gateway(session, drain_timeout, &[], &[])
            .await
            .map(|_| ())
    }

    /// Project committed Control Grants into the planner-facing snapshot shape.
    ///
    /// `state_revision` is the target Plan revision (current installation
    /// generation + 1). Grant rows themselves come from the committed Control
    /// generation so upgrade/uninstall can retire exact prior Grants without
    /// reading the legacy `grants/` leaf.
    pub(crate) async fn planned_grant_snapshot(
        &self,
        scope_id: &str,
        state_revision: u64,
    ) -> UseResult<a3s_use_core::PluginWorkspaceGrantSnapshot> {
        use a3s_use_core::{
            PluginWorkspaceGrantSnapshot, WorkspaceGrantEvidence,
            PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
        };

        let grants = match self.composition.store().current_generation().await? {
            Some(generation) => generation
                .grants
                .into_iter()
                .map(|selection| WorkspaceGrantEvidence {
                    package_id: selection.grant.package_id.clone(),
                    package_digest: selection.grant.package_digest.clone(),
                    receipt_revision: selection.receipt_revision,
                    grant_digest: selection.grant_digest.clone(),
                })
                .collect(),
            None => Vec::new(),
        };
        let snapshot = PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
            scope_id: scope_id.to_owned(),
            state_revision,
            grants,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Observe one retained Workspace Grant from the committed Control
    /// generation as the file-store `StoredWorkspaceGrant` shape used by
    /// Knowledge restore validation. Never reads the legacy `grants/` leaf.
    pub(crate) async fn observe_stored_workspace_grant(
        &self,
        scope_id: &str,
        package_id: &str,
        package_digest: &str,
    ) -> UseResult<Option<a3s_use_extension::StoredWorkspaceGrant>> {
        use a3s_use_extension::{
            StoredWorkspaceGrant, WorkspaceGrantReceipt, WORKSPACE_GRANT_RECEIPT_SCHEMA,
        };

        reject_legacy_authority_paths(self.composition.store().state_root.as_path())?;
        let Some(generation) = self.composition.store().current_generation().await? else {
            return Ok(None);
        };
        let Some(selection) = generation.grants.into_iter().find(|selection| {
            selection.grant.scope_id == scope_id
                && selection.grant.package_id == package_id
                && selection.grant.package_digest == package_digest
        }) else {
            return Ok(None);
        };
        Ok(Some(StoredWorkspaceGrant::Granted(WorkspaceGrantReceipt {
            schema: WORKSPACE_GRANT_RECEIPT_SCHEMA.to_string(),
            revision: selection.receipt_revision,
            grant_digest: selection.grant_digest,
            grant: selection.grant,
        })))
    }

    /// Observe one Control operation by id for Host admission and status.
    pub(crate) async fn observe_operation(
        &self,
        operation_id: &str,
    ) -> UseResult<Option<ControlObservedOperation>> {
        let Some(record) = self.composition.store().operation(operation_id).await? else {
            return Ok(None);
        };
        let phase = match record.status {
            ControlOperationStatus::Reviewed | ControlOperationStatus::EffectsPending => {
                ControlObservedOperationPhase::InFlight
            }
            ControlOperationStatus::Completed => ControlObservedOperationPhase::Completed,
            ControlOperationStatus::Cancelled => ControlObservedOperationPhase::Cancelled,
            ControlOperationStatus::Rejected => ControlObservedOperationPhase::Rejected,
        };
        Ok(Some(ControlObservedOperation {
            envelope: record.reviewed.envelope,
            phase,
            completed_at_ms: record.completed_at_ms,
            result_digest: record.result_digest,
        }))
    }

    async fn drain_effects(
        &self,
        operation_id: &str,
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<()> {
        let operation = self
            .composition
            .store()
            .operation(operation_id)
            .await?
            .ok_or_else(|| {
                UseError::new(
                    PRODUCTION_ERROR,
                    "The committed Control operation disappeared before effect drain.",
                )
                .with_detail("operation_id", operation_id)
            })?;
        let plan_digest = operation.reviewed.plan_digest().to_string();
        #[cfg(feature = "mcp")]
        let graph_cutover_key = match operation.reviewed.action() {
            a3s_use_core::PluginOperationAction::Install
            | a3s_use_core::PluginOperationAction::Upgrade
            | a3s_use_core::PluginOperationAction::Uninstall => {
                Some(operation_cutover_key(&operation.reviewed.envelope)?)
            }
            a3s_use_core::PluginOperationAction::Enable
            | a3s_use_core::PluginOperationAction::Disable => None,
        };
        #[cfg(feature = "mcp")]
        let mut gateway_activated = false;
        let mut deferred_streak = 0usize;
        for sequence in 0..MAX_EFFECT_DISPATCH_ITERATIONS {
            let result = self
                .composition
                .dispatch_next_with_shared_fence(
                    ControlEffectDispatchRequest {
                        operation_id: operation_id.to_string(),
                        worker_id: format!("worker:production:{operation_id}"),
                        claim_token: format!("claim:production:{operation_id}:{sequence}"),
                        lease_duration_ms: 60_000,
                        provider_timeout_ms: 5_000,
                        deferred_retry_delay_ms: 1_000,
                        explicit_reconciliation: false,
                    },
                    maintenance.clone(),
                )
                .await?;
            match result {
                ControlEffectDispatchResult::Idle => {
                    let completed_at_ms = SystemControlEffectClock.now_ms()?;
                    let result_digest =
                        production_completion_digest(operation_id, &plan_digest, completed_at_ms);
                    self.composition
                        .store()
                        .complete_operation(
                            operation_id,
                            &plan_digest,
                            &result_digest,
                            completed_at_ms,
                        )
                        .await?;
                    return Ok(());
                }
                ControlEffectDispatchResult::Observed {
                    outcome: ControlEffectOutcome::Deferred,
                    retry_not_before_ms,
                    error_code,
                    idempotency_key,
                    sequence,
                    ..
                } => {
                    deferred_streak = deferred_streak.saturating_add(1);
                    if deferred_streak > MAX_DEFERRED_STREAK {
                        let mut error = UseError::new(
                            PRODUCTION_ERROR,
                            "Control effect drain deferred repeatedly without progress.",
                        )
                        .with_detail("operation_id", operation_id)
                        .with_detail("deferred_streak", deferred_streak.to_string())
                        .with_detail("effect_sequence", sequence.to_string())
                        .with_detail("idempotency_key", idempotency_key);
                        if let Some(error_code) = error_code {
                            error = error.with_detail("error_code", error_code);
                        }
                        return Err(error);
                    }
                    let Some(retry_not_before_ms) = retry_not_before_ms else {
                        return Err(UseError::new(
                            PRODUCTION_ERROR,
                            "A deferred Control effect omitted its durable not-before time.",
                        )
                        .with_detail("operation_id", operation_id));
                    };
                    let now_ms = SystemControlEffectClock.now_ms()?;
                    if retry_not_before_ms > now_ms {
                        let wait_ms = retry_not_before_ms - now_ms;
                        let wait = Duration::from_millis(wait_ms.min(u64::from(u32::MAX)));
                        if wait > MAX_DEFERRED_WAIT {
                            return Err(UseError::new(
                                PRODUCTION_ERROR,
                                "A deferred Control effect exceeded the production wait bound.",
                            )
                            .with_detail("operation_id", operation_id)
                            .with_detail("wait_ms", wait_ms.to_string()));
                        }
                        sleep(wait).await;
                    }
                }
                ControlEffectDispatchResult::Observed {
                    outcome: ControlEffectOutcome::Unknown,
                    ..
                } => {
                    return Err(UseError::new(
                        PRODUCTION_ERROR,
                        "A Control external effect left an unknown outcome during production drain.",
                    )
                    .with_detail("operation_id", operation_id));
                }
                ControlEffectDispatchResult::Observed {
                    outcome: ControlEffectOutcome::Rejected,
                    idempotency_key,
                    sequence,
                    error_code,
                    ..
                } => {
                    deferred_streak = 0;
                    let status = self
                        .composition
                        .store()
                        .operation(operation_id)
                        .await?
                        .map(|record| record.status);
                    if matches!(status, Some(ControlOperationStatus::Rejected)) {
                        let mut error = UseError::new(
                            PRODUCTION_ERROR,
                            "A required Control effect rejected the operation during production drain.",
                        )
                        .with_detail("operation_id", operation_id)
                        .with_detail("effect_sequence", sequence.to_string())
                        .with_detail("idempotency_key", idempotency_key);
                        if let Some(error_code) = error_code {
                            error = error.with_detail("error_code", error_code);
                        }
                        return Err(error);
                    }
                    continue;
                }
                ControlEffectDispatchResult::Observed {
                    outcome: ControlEffectOutcome::Applied,
                    ..
                } => {
                    deferred_streak = 0;
                    #[cfg(feature = "mcp")]
                    if !gateway_activated {
                        if let Some(cutover_key) = graph_cutover_key.as_deref() {
                            self.activate_retained_gateway_after_capability_cutover(
                                cutover_key,
                            )
                            .await?;
                            // Only mark activated once the durable publication
                            // matches this operation; earlier Applied effects
                            // (SurfacePrepare) leave the prior cursor.
                            if self
                                .retained_gateway_matches_cutover(cutover_key)
                                .await?
                            {
                                gateway_activated = true;
                            }
                        }
                    }
                    continue;
                }
            }
        }
        Err(UseError::new(
            PRODUCTION_ERROR,
            "Control effect dispatch exceeded the production iteration bound.",
        )
        .with_detail("operation_id", operation_id)
        .with_detail("limit", MAX_EFFECT_DISPATCH_ITERATIONS.to_string()))
    }

    /// After CapabilityCutover is durably published, activate the retained
    /// Gateway so prior-generation package leases release before Remove/Prepare.
    #[cfg(feature = "mcp")]
    async fn activate_retained_gateway_after_capability_cutover(
        &self,
        cutover_key: &str,
    ) -> UseResult<()> {
        let activation = self
            .retained_gateway_cutover
            .lock()
            .expect("retained Gateway cutover lock")
            .clone();
        let Some(activation) = activation else {
            return Ok(());
        };
        let Some(binding) = self.composition.store().published_capability_cutover().await? else {
            return Ok(());
        };
        if binding.graph_cutover_key.as_deref() != Some(cutover_key) {
            return Ok(());
        }
        activation
            .activate_capability_cutover(cutover_key)
            .await
    }

    #[cfg(feature = "mcp")]
    async fn retained_gateway_matches_cutover(&self, cutover_key: &str) -> UseResult<bool> {
        if self
            .retained_gateway_cutover
            .lock()
            .expect("retained Gateway cutover lock")
            .is_none()
        {
            return Ok(false);
        }
        Ok(self
            .composition
            .store()
            .published_capability_cutover()
            .await?
            .and_then(|binding| binding.graph_cutover_key)
            .as_deref()
            == Some(cutover_key))
    }
}

fn production_completion_digest(
    operation_id: &str,
    plan_digest: &str,
    completed_at_ms: u64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.use.control.production-complete.v1\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(plan_digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(completed_at_ms.to_le_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// Fail closed when any legacy authority leaf is present beside Control.
pub(crate) fn reject_legacy_authority_paths(state_root: &Path) -> UseResult<()> {
    for relative in LEGACY_AUTHORITY_PATHS {
        let path = state_root.join(relative);
        if path.exists() {
            return Err(UseError::new(
                LEGACY_AUTHORITY_ERROR,
                "The installation state root contains legacy mutable authority beside Control.",
            )
            .with_detail("entry", *relative)
            .with_detail("path", path.display().to_string()));
        }
    }
    Ok(())
}

/// True when the installation already has an initialized Control database.
pub(crate) fn control_database_present(state_root: &Path) -> bool {
    state_root.join(CONTROL_STORE_DATABASE_FILE).is_file()
}

/// True when any frozen legacy authority leaf exists under the state root.
pub(crate) fn legacy_authority_present(state_root: &Path) -> bool {
    LEGACY_AUTHORITY_PATHS
        .iter()
        .any(|relative| state_root.join(relative).exists())
}

struct StandaloneRejectingFlow;

/// Mints opaque `gateway:` endpoint identities for standalone Control when no
/// managed Runtime Service publications are admitted. Live loopback URLs are
/// recorded by composition's
/// [`RecordingControlRuntimeServiceReadiness`] wrapper; this adapter never
/// stores URLs on the durable receipt. Managed Tool/MCP publications must
/// inject [`ControlRuntimeServiceReadinessPort`] instead — see
/// `require_control_runtime_readiness_for_publications`.
struct StandaloneOpaqueBindingReadiness;

#[async_trait]
impl ControlFlowEffectPort for StandaloneRejectingFlow {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        ControlEffectPortOutcome::rejected(
            ControlEffectFailure::new(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "provider.flow_unavailable",
            )
            .expect("static failure evidence"),
        )
    }
}

#[async_trait]
impl ControlRuntimeServiceReadinessPort for StandaloneOpaqueBindingReadiness {
    async fn bind_tool_service(
        &self,
        surface: &ToolSurface,
        plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeEndpointRef> {
        RuntimeEndpointRef::parse(format!(
            "gateway:tool/{}/{}",
            surface.id,
            plan.context().generation()
        ))
    }

    async fn bind_mcp_service(
        &self,
        surface: &PluginMcpSurface,
        plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<ControlRuntimeMcpReadiness> {
        let RuntimeSurfaceContract::McpService {
            protocol_version, ..
        } = plan.contract()
        else {
            return Err(UseError::new(
                PRODUCTION_ERROR,
                "Standalone MCP readiness received a non-MCP Runtime surface plan.",
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
        Ok(())
    }

    async fn remove_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        Ok(())
    }
}
