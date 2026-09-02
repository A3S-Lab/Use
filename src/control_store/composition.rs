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

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{PluginSurfaceRef, UseError, UseResult};
use a3s_use_extension::{ArtifactStore, ExtensionPaths, StateMaintenanceLock};

use super::dispatcher::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    ControlEffectPorts, ControlEffectRuntime,
};
use super::effect_owner::capability_plane::ControlCapabilityPlaneEffectPort;
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
use super::effect_owner::runtime::{ControlRuntimeEffectPort, ControlRuntimeServiceReadinessPort};
use super::effect_owner::static_surface::ControlStaticSurfaceEffectPort;
use super::effect_port::ControlFlowEffectPort;
use super::model::{
    ControlEffectKind, ControlEffectOwner, ControlEffectSubject, ControlGeneration,
    ControlTransition, ReviewedControlOperation,
};
use super::{ControlStore, ControlStoreMetadata};
use crate::okf_knowledge::{
    OkfKnowledgeBindingStore, OkfKnowledgeClient, SqliteOkfKnowledgeAdapter,
};
use crate::plugin_runtime::{
    CommittedRuntimeSurfaceResolver, RuntimeBindingStore, RuntimeSurfacePlanPublication,
    RuntimeSurfacePlanStore,
};

const COMPOSITION_ERROR: &str = "use.control_store.composition_invalid";
const PUBLICATION_ERROR: &str = "use.control_store.runtime_plan_publication_invalid";

/// All dependencies needed to compose one inactive Control dispatcher.
///
/// Runtime and Flow/Gateway are host-owned boundaries. The remaining owners
/// are Use-owned adapters and are constructed from the same `ExtensionPaths`
/// and `ControlStore`, so they cannot silently drift to another installation.
pub(in crate::control_store) struct ControlEffectCompositionDependencies {
    pub(in crate::control_store) runtime_registry: Arc<RuntimeClientRegistry>,
    pub(in crate::control_store) runtime_readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
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
/// boundaries.
#[derive(Clone)]
pub(in crate::control_store) struct ControlStoreRuntimeComposition {
    store: ControlStore,
    plan_store: RuntimeSurfacePlanStore,
    artifact_store: ArtifactStore,
    effects: ControlEffectRuntime,
}

impl std::fmt::Debug for ControlStoreRuntimeComposition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlStoreRuntimeComposition")
            .field("installation", &self.store.installation)
            .field("state_root", &self.store.state_root)
            .field("runtime_plan_root", &self.plan_store.root())
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
        let capability = Arc::new(ControlCapabilityPlaneEffectPort::new(store.clone()));
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
            capability,
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

    pub(in crate::control_store) async fn initialize(&self) -> UseResult<ControlStoreMetadata> {
        self.store.initialize().await
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
        let (reviewed, transition) = self
            .store
            .project_transition_under_maintenance(operation_id, committed_at_ms)
            .await?;
        validate_runtime_publications(&transition, publications)?;
        validate_runtime_publication_authority(&reviewed, publications)?;
        self.plan_store
            .publish_under_maintenance(&_maintenance, publications)
            .await?;
        self.store
            .commit_transition_under_maintenance(transition)
            .await
    }
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
