//! Scope-bound capability publication for resident lifecycle and agent hosts.
//!
//! The process-wide CLI snapshot deliberately has no workspace identity. This
//! adapter joins one stable extension registry generation to exact Runtime and
//! compatibility-host observations without inventing a default scope.

use std::collections::{BTreeMap, BTreeSet};

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{PluginSurfaceRef, UseError, UseResult};
use a3s_use_extension::{ExtensionRegistry, ExtensionRegistrySnapshot, InstalledExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::plugin_runtime::{
    RuntimeBindingStore, RuntimeSurfaceObservationSnapshot, RuntimeSurfaceObserver,
};
use crate::surface_reconciler::{
    surface_owners, SurfaceObservations, SurfaceObservedState, SurfaceOwner,
};

use super::{
    built_in_capabilities, project_extension_for_session, route_matches_extension,
    CapabilityBinding, MAX_STABLE_SNAPSHOT_ATTEMPTS,
};

/// Current JSON schema for [`CapabilitySessionSnapshot`].
pub const CAPABILITY_SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const MAX_SESSION_OBSERVATIONS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Trusted adapter class allowed to report a non-Runtime surface.
pub enum CapabilityHostSurfaceOwner {
    /// Compatibility host for a package-executable Tool Task.
    ToolHost,
    /// Supervised stdio MCP compatibility host.
    McpHost,
    /// Managed Skill projection host.
    SkillHost,
    /// Sandboxed UI projection host.
    UiHost,
}

/// One trusted host adapter's current named-surface state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySurfaceObservedState {
    /// Evidence is not available yet.
    Pending,
    /// Static or lazy surface is prepared for use.
    Prepared,
    /// Eager surface is starting.
    Starting,
    /// Eager surface passed its health gate.
    Healthy,
    /// Surface observation failed.
    Failed,
    /// Surface is draining accepted work.
    Draining,
    /// Surface is stopped.
    Stopped,
}

/// One package-generation-bound report from a trusted non-Runtime adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityHostSurfaceObservation {
    package_id: String,
    package_digest: String,
    surface: PluginSurfaceRef,
    owner: CapabilityHostSurfaceOwner,
    state: CapabilitySurfaceObservedState,
}

impl CapabilityHostSurfaceObservation {
    /// Bind one host observation to an immutable package generation.
    pub fn new(
        package_id: impl Into<String>,
        package_digest: impl Into<String>,
        surface: PluginSurfaceRef,
        owner: CapabilityHostSurfaceOwner,
        state: CapabilitySurfaceObservedState,
    ) -> UseResult<Self> {
        let observation = Self {
            package_id: package_id.into(),
            package_digest: package_digest.into(),
            surface,
            owner,
            state,
        };
        observation.validate()?;
        Ok(observation)
    }

    /// Canonical `publisher/name` identity.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Canonical expanded-package SHA-256.
    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    /// Named surface reported by the adapter.
    pub fn surface(&self) -> &PluginSurfaceRef {
        &self.surface
    }

    /// Adapter class claiming the observation.
    pub fn owner(&self) -> CapabilityHostSurfaceOwner {
        self.owner
    }

    /// Current observed state.
    pub fn state(&self) -> CapabilitySurfaceObservedState {
        self.state
    }

    fn validate(&self) -> UseResult<()> {
        if !valid_package_id(&self.package_id)
            || !valid_sha256(&self.package_digest)
            || !valid_surface_id(&self.surface.id)
        {
            return Err(session_observation_error(
                "A host observation requires a canonical package, digest, and named surface.",
            ));
        }
        Ok(())
    }
}

/// Canonical host observations for exactly one explicit lifecycle scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySessionObservations {
    scope_id: String,
    surfaces: Vec<CapabilityHostSurfaceObservation>,
}

impl CapabilitySessionObservations {
    /// Validate, sort, and deduplicate one scope's host evidence.
    pub fn new(
        scope_id: impl Into<String>,
        mut surfaces: Vec<CapabilityHostSurfaceObservation>,
    ) -> UseResult<Self> {
        let scope_id = scope_id.into();
        if !valid_scope_id(&scope_id) {
            return Err(session_observation_error(
                "Capability session observations require an explicit canonical scope.",
            ));
        }
        if surfaces.len() > MAX_SESSION_OBSERVATIONS {
            return Err(session_observation_error(format!(
                "A capability session accepts at most {MAX_SESSION_OBSERVATIONS} host observations.",
            )));
        }
        for observation in &surfaces {
            observation.validate()?;
        }
        surfaces.sort_by(|left, right| {
            left.package_id
                .cmp(&right.package_id)
                .then_with(|| left.surface.cmp(&right.surface))
                .then_with(|| left.package_digest.cmp(&right.package_digest))
                .then_with(|| left.owner.cmp(&right.owner))
        });
        if surfaces.windows(2).any(|pair| {
            pair[0].package_id == pair[1].package_id && pair[0].surface == pair[1].surface
        }) {
            return Err(session_observation_error(
                "Two host adapters reported the same package surface.",
            ));
        }
        Ok(Self { scope_id, surfaces })
    }

    /// Explicit workspace or user scope selected by the trusted host.
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Canonically ordered host observations.
    pub fn surfaces(&self) -> &[CapabilityHostSurfaceObservation] {
        &self.surfaces
    }

    fn package_ids(&self) -> BTreeSet<&str> {
        self.surfaces
            .iter()
            .map(|observation| observation.package_id.as_str())
            .collect()
    }

    fn for_package<'a>(
        &'a self,
        package_id: &'a str,
    ) -> impl Iterator<Item = &'a CapabilityHostSurfaceObservation> + 'a {
        self.surfaces
            .iter()
            .filter(move |observation| observation.package_id == package_id)
    }
}

/// Immutable scope-aware projection consumed by one authorized host session.
///
/// `revision` binds the scope, projected capabilities, complete host evidence,
/// and exact Runtime provider/generation observations. `generation` remains
/// the independently monotonic extension registry generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySessionSnapshot {
    /// Snapshot JSON schema.
    pub schema_version: u32,
    /// Explicit lifecycle/session scope.
    pub scope_id: String,
    /// Monotonic extension registry generation.
    pub generation: u64,
    /// Lowercase SHA-256 over scope, capabilities, and all observation evidence.
    pub revision: String,
    /// Sorted built-in and extension capability projections.
    pub capabilities: Vec<CapabilityBinding>,
    /// Canonically sorted non-Runtime host observations.
    pub host_observations: Vec<CapabilityHostSurfaceObservation>,
    /// Sorted exact-scope Runtime observation snapshots.
    pub runtime_observations: Vec<RuntimeSurfaceObservationSnapshot>,
}

/// Builds one stable, scope-aware capability projection.
///
/// Runtime evidence is read from [`RuntimeBindingStore`] through only the
/// explicit providers registered by the trusted host. Package-executable
/// Tools, stdio MCP, Skills, and UI remain caller-supplied host observations.
pub struct CapabilitySessionSnapshotBuilder<'a> {
    registry: &'a ExtensionRegistry,
    runtime: RuntimeSurfaceObserver<'a>,
    host_version: &'static str,
}

impl<'a> CapabilitySessionSnapshotBuilder<'a> {
    /// Bind the extension registry and Runtime observation dependencies.
    pub fn new(
        registry: &'a ExtensionRegistry,
        runtime_store: &'a RuntimeBindingStore,
        runtime_providers: &'a RuntimeClientRegistry,
    ) -> Self {
        Self {
            registry,
            runtime: RuntimeSurfaceObserver::new(runtime_store, runtime_providers),
            host_version: env!("CARGO_PKG_VERSION"),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_host_version(
        registry: &'a ExtensionRegistry,
        runtime_store: &'a RuntimeBindingStore,
        runtime_providers: &'a RuntimeClientRegistry,
        host_version: &'static str,
    ) -> Self {
        Self {
            registry,
            runtime: RuntimeSurfaceObserver::new(runtime_store, runtime_providers),
            host_version,
        }
    }

    /// Project one immutable session snapshot after a stable registry read.
    pub async fn snapshot(
        &self,
        observations: &CapabilitySessionObservations,
    ) -> UseResult<CapabilitySessionSnapshot> {
        for _ in 0..MAX_STABLE_SNAPSHOT_ATTEMPTS {
            let before = self.registry.snapshot().await?;
            let Some(projected) = self.project_extensions(&before, observations).await? else {
                continue;
            };
            let after = self.registry.snapshot().await?;
            if before != after {
                continue;
            }
            validate_observed_packages(observations, &projected.package_ids)?;
            let mut capabilities = built_in_capabilities().await?;
            capabilities.extend(projected.capabilities);
            capabilities.sort_by(|left, right| left.id().cmp(right.id()));
            let revision = session_revision(
                observations.scope_id(),
                &capabilities,
                observations.surfaces(),
                &projected.runtime_observations,
            )?;
            return Ok(CapabilitySessionSnapshot {
                schema_version: CAPABILITY_SESSION_SNAPSHOT_SCHEMA_VERSION,
                scope_id: observations.scope_id().to_string(),
                generation: before.generation,
                revision,
                capabilities,
                host_observations: observations.surfaces().to_vec(),
                runtime_observations: projected.runtime_observations,
            });
        }
        Err(UseError::new(
            "use.capability.registry_busy",
            "The extension registry changed repeatedly while a scoped capability session was projected.",
        )
        .with_suggestion("Retry the capability session snapshot after the current component operation."))
    }

    async fn project_extensions(
        &self,
        registry: &ExtensionRegistrySnapshot,
        observations: &CapabilitySessionObservations,
    ) -> UseResult<Option<SessionProjection>> {
        let mut capabilities = Vec::with_capacity(registry.routes.len());
        let mut runtime_observations = Vec::new();
        let mut package_ids = BTreeSet::new();

        for route in &registry.routes {
            #[cfg(feature = "ocr")]
            if route.route == "ocr" {
                continue;
            }
            let Some(extension) = self.registry.get(&route.package_id).await? else {
                return Ok(None);
            };
            let surfaces = extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if !route_matches_extension(route, &extension, &surfaces) {
                return Ok(None);
            }
            package_ids.insert(route.package_id.clone());
            let host_observations = validate_host_observations(
                &extension,
                observations.for_package(&route.package_id),
            )?;
            let runtime = self
                .runtime_observation(observations.scope_id(), &extension)
                .await?;
            capabilities.push(
                project_extension_for_session(
                    &extension,
                    surfaces,
                    self.host_version,
                    &host_observations,
                    runtime.as_ref(),
                )
                .await?,
            );
            if let Some(runtime) = runtime {
                runtime_observations.push(runtime);
            }
        }
        runtime_observations.sort_by(|left, right| left.package_id().cmp(right.package_id()));
        Ok(Some(SessionProjection {
            capabilities,
            runtime_observations,
            package_ids,
        }))
    }

    async fn runtime_observation(
        &self,
        scope_id: &str,
        extension: &InstalledExtension,
    ) -> UseResult<Option<RuntimeSurfaceObservationSnapshot>> {
        if extension.manifest.schema_version != 3 {
            return Ok(None);
        }
        let Some(package_digest) = canonical_receipt_package_digest(extension)? else {
            return Ok(None);
        };
        self.runtime
            .observe_manifest(scope_id, &package_digest, &extension.manifest)
            .await
            .map(Some)
    }
}

struct SessionProjection {
    capabilities: Vec<CapabilityBinding>,
    runtime_observations: Vec<RuntimeSurfaceObservationSnapshot>,
    package_ids: BTreeSet<String>,
}

fn validate_host_observations<'a>(
    extension: &InstalledExtension,
    observations: impl Iterator<Item = &'a CapabilityHostSurfaceObservation>,
) -> UseResult<SurfaceObservations> {
    let observations = observations.collect::<Vec<_>>();
    if observations.is_empty() {
        return Ok(SurfaceObservations::new());
    }
    if extension.manifest.schema_version != 3 {
        return Err(session_observation_error(
            "Only schema-v3 named surfaces accept scoped host observations.",
        ));
    }
    let expected_digest = canonical_receipt_package_digest(extension)?.ok_or_else(|| {
        session_observation_error(
            "The installed package has no immutable digest for scoped host observations.",
        )
    })?;
    let owners = surface_owners(&extension.manifest)?;
    let mut validated = BTreeMap::new();
    for observation in observations {
        if observation.package_digest != expected_digest {
            return Err(session_observation_error(
                "A host observation belongs to a different immutable package generation.",
            ));
        }
        let expected_owner = owners.get(&observation.surface).ok_or_else(|| {
            session_observation_error("A host observation references an unknown named surface.")
        })?;
        let actual_owner = internal_owner(observation.owner);
        if *expected_owner == SurfaceOwner::Runtime || *expected_owner != actual_owner {
            return Err(session_observation_error(
                "A host observation does not belong to the manifest-selected surface owner.",
            ));
        }
        if validated
            .insert(observation.surface.clone(), observation.state.into())
            .is_some()
        {
            return Err(session_observation_error(
                "Two host adapters reported the same package surface.",
            ));
        }
    }
    Ok(validated)
}

fn validate_observed_packages(
    observations: &CapabilitySessionObservations,
    installed: &BTreeSet<String>,
) -> UseResult<()> {
    if let Some(package_id) = observations
        .package_ids()
        .into_iter()
        .find(|package_id| !installed.contains(*package_id))
    {
        return Err(session_observation_error(format!(
            "Host observations reference package '{package_id}', which is absent from the stable registry snapshot.",
        )));
    }
    Ok(())
}

fn canonical_receipt_package_digest(extension: &InstalledExtension) -> UseResult<Option<String>> {
    let Some(digest) = extension.receipt.package_sha256.as_deref() else {
        return Ok(None);
    };
    let digest = format!("sha256:{digest}");
    if !valid_sha256(&digest) {
        return Err(session_observation_error(
            "The installed receipt contains a noncanonical package digest.",
        ));
    }
    Ok(Some(digest))
}

fn session_revision(
    scope_id: &str,
    capabilities: &[CapabilityBinding],
    host_observations: &[CapabilityHostSurfaceObservation],
    runtime_observations: &[RuntimeSurfaceObservationSnapshot],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RevisionInput<'a> {
        schema_version: u32,
        scope_id: &'a str,
        capabilities: &'a [CapabilityBinding],
        host_observations: &'a [CapabilityHostSurfaceObservation],
        runtime_observations: &'a [RuntimeSurfaceObservationSnapshot],
    }

    let bytes = serde_json::to_vec(&RevisionInput {
        schema_version: CAPABILITY_SESSION_SNAPSHOT_SCHEMA_VERSION,
        scope_id,
        capabilities,
        host_observations,
        runtime_observations,
    })
    .map_err(|error| {
        UseError::new(
            "use.capability.snapshot_invalid",
            format!("Failed to encode the capability session snapshot: {error}"),
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn internal_owner(owner: CapabilityHostSurfaceOwner) -> SurfaceOwner {
    match owner {
        CapabilityHostSurfaceOwner::ToolHost => SurfaceOwner::ToolHost,
        CapabilityHostSurfaceOwner::McpHost => SurfaceOwner::McpHost,
        CapabilityHostSurfaceOwner::SkillHost => SurfaceOwner::SkillHost,
        CapabilityHostSurfaceOwner::UiHost => SurfaceOwner::UiHost,
    }
}

impl From<CapabilitySurfaceObservedState> for SurfaceObservedState {
    fn from(state: CapabilitySurfaceObservedState) -> Self {
        match state {
            CapabilitySurfaceObservedState::Pending => Self::Pending,
            CapabilitySurfaceObservedState::Prepared => Self::Prepared,
            CapabilitySurfaceObservedState::Starting => Self::Starting,
            CapabilitySurfaceObservedState::Healthy => Self::Healthy,
            CapabilitySurfaceObservedState::Failed => Self::Failed,
            CapabilitySurfaceObservedState::Draining => Self::Draining,
            CapabilitySurfaceObservedState::Stopped => Self::Stopped,
        }
    }
}

fn valid_scope_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

fn valid_package_id(value: &str) -> bool {
    value.len() <= 128 && value.split('/').count() == 2 && value.split('/').all(valid_surface_id)
}

fn valid_surface_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn session_observation_error(message: impl Into<String>) -> UseError {
    UseError::new("use.capability.session_observation_invalid", message)
}
