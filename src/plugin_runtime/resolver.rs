//! Restart-safe reconstruction of committed Runtime surface plans.
//!
//! A Runtime provider selection is a process-local cache.  The durable
//! authority is the canonical, path-free plan payload keyed by the committed
//! package, scope, lifecycle generation, grant, surface, and semantics
//! digest.  This module keeps those two concerns separate: a host supplies a
//! durable payload source, while this crate validates the payload and binds it
//! to the exact provider evidence before any Runtime effect is attempted.

use std::sync::Arc;

use a3s_runtime::{ProviderId, RuntimeClientRegistry};
use a3s_use_core::{
    PlanQualifiedSurfaceRef, PlanScope, PlannedProviderEvidence, PluginSurfaceKind, UseError,
    UseResult,
};
use async_trait::async_trait;

use super::client::{
    runtime_capabilities_digest, validate_capabilities_for_plan, PluginRuntimeClient,
};
use super::model::{valid_sha256, RuntimeSurfacePlan};
use super::provider_selector::{RuntimeProviderSelection, SelectedRuntimeSurface};

const PLAN_RESOLVER_ERROR: &str = "use.plugin.runtime.plan_resolver_invalid";
const PLAN_SOURCE_UNAVAILABLE: &str = "use.plugin.runtime.plan_source_unavailable";
const PLAN_NOT_FOUND: &str = "use.plugin.runtime.plan_not_found";
const PROVIDER_EVIDENCE_CHANGED: &str = "use.plugin.runtime.provider_evidence_changed";
const MAX_PROVIDER_ID_BYTES: usize = 64;

/// Exact identity used to look up a committed Runtime plan after restart.
///
/// The key contains only portable committed facts.  It deliberately excludes
/// package roots, descriptor paths, Runtime endpoints, and live client state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSurfacePlanKey {
    pub package_id: String,
    pub package_digest: String,
    pub scope: PlanScope,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_digest: Option<String>,
    pub semantics_profile_digest: String,
    pub provider_id: String,
    pub selection_digest: String,
}

impl RuntimeSurfacePlanKey {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package_id: impl Into<String>,
        package_digest: impl Into<String>,
        scope: PlanScope,
        surface: PlanQualifiedSurfaceRef,
        generation: u64,
        grant_digest: Option<String>,
        semantics_profile_digest: impl Into<String>,
        provider_id: impl Into<String>,
        selection_digest: impl Into<String>,
    ) -> UseResult<Self> {
        let key = Self {
            package_id: package_id.into(),
            package_digest: package_digest.into(),
            scope,
            surface,
            generation,
            grant_digest,
            semantics_profile_digest: semantics_profile_digest.into(),
            provider_id: provider_id.into(),
            selection_digest: selection_digest.into(),
        };
        key.validate()?;
        Ok(key)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.package_id.is_empty()
            || self.package_id.len() > 128
            || self.package_id.contains('\0')
            || self.package_digest.is_empty()
            || !valid_sha256(&self.package_digest)
            || self.scope.validate().is_err()
            || self.surface.package_id != self.package_id
            || !matches!(
                self.surface.surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            )
            || self.surface.surface.id.is_empty()
            || self.surface.surface.id.len() > 63
            || self.generation == 0
            || self
                .grant_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || !valid_sha256(&self.semantics_profile_digest)
            || self.provider_id.is_empty()
            || self.provider_id.len() > MAX_PROVIDER_ID_BYTES
            || self
                .provider_id
                .bytes()
                .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
            || !valid_sha256(&self.selection_digest)
        {
            return Err(UseError::new(
                PLAN_RESOLVER_ERROR,
                "The committed Runtime surface plan key is invalid.",
            ));
        }
        // ProviderId has a stricter edge-hyphen rule than the portable key
        // checks above.  Parse it here so a source cannot receive an
        // unaddressable provider identity.
        ProviderId::parse(self.provider_id.clone()).map_err(|_| {
            UseError::new(
                PLAN_RESOLVER_ERROR,
                "The committed Runtime provider identity is invalid.",
            )
        })?;
        Ok(())
    }

    /// Check all key-bound fields against a decoded plan.  The descriptor
    /// digest is intentionally checked after decoding because it is not
    /// available in the outbox authority without re-reading the plan.
    pub fn matches_plan(&self, plan: &RuntimeSurfacePlan) -> bool {
        self.package_id == plan.context().package_id()
            && self.package_digest == plan.context().package_digest()
            && self.scope == *plan.context().scope()
            && self.surface == plan.surface()
            && self.generation == plan.context().generation()
            && self
                .grant_digest
                .as_deref()
                .is_none_or(|digest| digest == plan.context().grant_digest())
            && plan.spec().semantics_profile_digest.as_deref()
                == Some(self.semantics_profile_digest.as_str())
    }
}

/// Host-owned source of canonical, path-free committed Runtime plan payloads.
///
/// Implementations may use SQLite, a journal, or another durable store.  The
/// source must not derive authority from a package root or a legacy lifecycle
/// receipt.  A transient read failure should use
/// `use.plugin.runtime.plan_source_unavailable`; a missing committed record
/// should use `use.plugin.runtime.plan_not_found`.
#[async_trait]
pub trait RuntimeSurfacePlanSource: Send + Sync {
    async fn read_plan(&self, key: &RuntimeSurfacePlanKey) -> UseResult<Vec<u8>>;
}

/// Typed boundary used by the Control Runtime owner.  A resolver reconstructs
/// a plan and reconnects the exact provider, but it never chooses a default or
/// falls back to native execution.
#[async_trait]
pub trait RuntimeSurfaceResolver: Send + Sync {
    async fn resolve(
        &self,
        key: &RuntimeSurfacePlanKey,
        evidence: &PlannedProviderEvidence,
    ) -> UseResult<SelectedRuntimeSurface>;
}

/// Resolver backed by a durable plan source and an explicit Runtime registry.
#[derive(Clone)]
pub struct CommittedRuntimeSurfaceResolver {
    source: Arc<dyn RuntimeSurfacePlanSource>,
    registry: Arc<RuntimeClientRegistry>,
}

impl std::fmt::Debug for CommittedRuntimeSurfaceResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommittedRuntimeSurfaceResolver")
            .field("source", &"host-owned")
            .field("registry", &"explicit")
            .finish()
    }
}

impl CommittedRuntimeSurfaceResolver {
    pub fn new(
        source: Arc<dyn RuntimeSurfacePlanSource>,
        registry: Arc<RuntimeClientRegistry>,
    ) -> Self {
        Self { source, registry }
    }
}

#[async_trait]
impl RuntimeSurfaceResolver for CommittedRuntimeSurfaceResolver {
    async fn resolve(
        &self,
        key: &RuntimeSurfacePlanKey,
        evidence: &PlannedProviderEvidence,
    ) -> UseResult<SelectedRuntimeSurface> {
        key.validate()?;
        validate_evidence(key, evidence)?;
        let bytes = self
            .source
            .read_plan(key)
            .await
            .map_err(normalize_source_error)?;
        let plan = RuntimeSurfacePlan::from_canonical_bytes(&bytes)?;
        if !key.matches_plan(&plan) {
            return Err(UseError::new(
                PLAN_RESOLVER_ERROR,
                "The durable Runtime plan does not match its committed lookup key.",
            ));
        }
        plan.validate()?;

        let provider_id = ProviderId::parse(evidence.provider_id.clone()).map_err(|_| {
            UseError::new(
                PLAN_RESOLVER_ERROR,
                "The committed Runtime provider identity is invalid.",
            )
        })?;
        let client = self.registry.connect(&provider_id).await.map_err(|error| {
            UseError::new(
                "use.plugin.runtime.provider_unavailable",
                format!("Failed to reconnect the committed Runtime provider: {error}"),
            )
        })?;
        let client = PluginRuntimeClient::new(client);
        client.verify_plan(&plan, evidence).await?;
        // `verify_plan` checks the provider evidence and plan capabilities. The
        // explicit checks below make that invariant visible at this boundary
        // and protect it if the client implementation changes later.
        let capabilities = client.client.capabilities().await.map_err(|error| {
            UseError::new(
                "use.plugin.runtime.provider_unavailable",
                format!("Failed to read the committed Runtime capabilities: {error}"),
            )
        })?;
        capabilities.validate().map_err(|error| {
            UseError::new(
                PLAN_RESOLVER_ERROR,
                format!("The Runtime provider returned invalid capabilities: {error}"),
            )
        })?;
        let capability_digest = runtime_capabilities_digest(&capabilities)?;
        if capability_digest != evidence.capability_digest
            || capabilities.provider_build != evidence.provider_build_id
        {
            return Err(UseError::new(
                PROVIDER_EVIDENCE_CHANGED,
                "The reconnected Runtime provider no longer matches committed evidence.",
            ));
        }
        validate_capabilities_for_plan(&plan, &capabilities)?;
        Ok(SelectedRuntimeSurface::from_parts(
            plan,
            evidence.clone(),
            client,
        ))
    }
}

/// Qualification-only resolver for an already connected process-local
/// selection.  Production hosts should use `CommittedRuntimeSurfaceResolver`
/// with a durable source instead.
#[derive(Debug, Clone)]
pub struct RuntimeProviderSelectionResolver {
    selection: RuntimeProviderSelection,
}

impl RuntimeProviderSelectionResolver {
    pub fn new(selection: RuntimeProviderSelection) -> Self {
        Self { selection }
    }
}

#[async_trait]
impl RuntimeSurfaceResolver for RuntimeProviderSelectionResolver {
    async fn resolve(
        &self,
        key: &RuntimeSurfacePlanKey,
        evidence: &PlannedProviderEvidence,
    ) -> UseResult<SelectedRuntimeSurface> {
        key.validate()?;
        validate_evidence(key, evidence)?;
        let selected = self
            .selection
            .surfaces()
            .iter()
            .find(|candidate| candidate.plan().surface() == key.surface)
            .cloned()
            .ok_or_else(|| {
                UseError::new(
                    PLAN_NOT_FOUND,
                    "The process-local Runtime selection has no exact committed surface.",
                )
            })?;
        if !key.matches_plan(selected.plan()) || selected.provider() != evidence {
            return Err(UseError::new(
                PLAN_RESOLVER_ERROR,
                "The process-local Runtime selection differs from committed authority.",
            ));
        }
        Ok(selected)
    }
}

fn validate_evidence(
    key: &RuntimeSurfacePlanKey,
    evidence: &PlannedProviderEvidence,
) -> UseResult<()> {
    if evidence.surface != key.surface
        || evidence.provider_id != key.provider_id
        || evidence.semantics_profile_digest != key.semantics_profile_digest
    {
        return Err(UseError::new(
            PLAN_RESOLVER_ERROR,
            "Runtime provider evidence does not match the committed plan key.",
        ));
    }
    Ok(())
}

fn normalize_source_error(error: UseError) -> UseError {
    if error.code == PLAN_SOURCE_UNAVAILABLE || error.code == PLAN_NOT_FOUND {
        error
    } else {
        UseError::new(
            PLAN_SOURCE_UNAVAILABLE,
            format!(
                "The committed Runtime plan source could not be read: {}",
                error.message
            ),
        )
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<CommittedRuntimeSurfaceResolver>();
    assert_send_sync::<RuntimeProviderSelectionResolver>();
};
