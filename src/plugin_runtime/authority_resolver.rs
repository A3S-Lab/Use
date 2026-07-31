use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use a3s_runtime::ProviderId;
use a3s_use_core::{
    FilesystemPermission, PlanQualifiedSurfaceRef, PlannedPackageState, PluginPlanningBundle,
    PluginSurfaceKind, PluginWorkspaceGrant, UseError, UseResult,
};
use async_trait::async_trait;

use super::bundle_planner::{selected_runtime_surface_refs, validate_runtime_bundle_package};
use super::provider_selector::canonicalize_provider_assignments;
use super::{
    RuntimeAuthorityBindings, RuntimeFilesystemBinding, RuntimeProviderAssignment,
    RuntimeSecretBinding, RuntimeSurfaceAuthorityBindings,
};

pub const MAX_RUNTIME_AUTHORITY_RESOLUTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Exact, non-secret planning input for one provider-bound executable surface.
///
/// The request contains reviewed logical permissions and immutable package
/// identity. It contains neither host filesystem paths nor secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAuthorityResolutionRequest {
    scope_id: String,
    package_id: String,
    package_digest: String,
    permission_ceiling_digest: String,
    permissions_digest: String,
    surface: PlanQualifiedSurfaceRef,
    generation: u64,
    provider_id: ProviderId,
    filesystem: Vec<FilesystemPermission>,
    secret_names: Vec<String>,
    ephemeral_storage_limit_bytes: u64,
}

impl RuntimeAuthorityResolutionRequest {
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn permission_ceiling_digest(&self) -> &str {
        &self.permission_ceiling_digest
    }

    pub fn permissions_digest(&self) -> &str {
        &self.permissions_digest
    }

    pub fn surface(&self) -> &PlanQualifiedSurfaceRef {
        &self.surface
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    pub fn filesystem(&self) -> &[FilesystemPermission] {
        &self.filesystem
    }

    pub fn secret_names(&self) -> &[String] {
        &self.secret_names
    }

    pub fn ephemeral_storage_limit_bytes(&self) -> u64 {
        self.ephemeral_storage_limit_bytes
    }
}

/// Untrusted process-local output from one host-owned authority resolver.
///
/// Use re-sorts and validates this output for exact permission coverage,
/// source kind, mount uniqueness, Tmpfs bounds, secret target uniqueness, and
/// provider assignment before it can enter a Runtime template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRuntimeSurfaceAuthority {
    filesystem: Vec<RuntimeFilesystemBinding>,
    secrets: Vec<RuntimeSecretBinding>,
}

impl ResolvedRuntimeSurfaceAuthority {
    pub fn new(
        filesystem: Vec<RuntimeFilesystemBinding>,
        secrets: Vec<RuntimeSecretBinding>,
    ) -> Self {
        Self {
            filesystem,
            secrets,
        }
    }

    pub fn filesystem(&self) -> &[RuntimeFilesystemBinding] {
        &self.filesystem
    }

    pub fn secrets(&self) -> &[RuntimeSecretBinding] {
        &self.secrets
    }
}

/// Host-owned provider adapter for planning-only Volume, Tmpfs, and opaque
/// secret-reference resolution.
///
/// Implementations must be deterministic, idempotent, cancellation-safe, and
/// must not fetch secret material or perform package-controlled I/O. Runtime
/// resource mutation remains part of the reviewed apply saga.
#[async_trait]
pub trait RuntimeAuthorityResolver: Send + Sync {
    fn provider_id(&self) -> &ProviderId;

    async fn resolve_surface_authority(
        &self,
        request: &RuntimeAuthorityResolutionRequest,
    ) -> UseResult<ResolvedRuntimeSurfaceAuthority>;
}

/// Explicit provider-keyed authority resolver registry with one total
/// planning deadline and no default or fallback resolver.
pub struct RuntimeAuthorityResolverRegistry {
    control_timeout: Duration,
    resolvers: BTreeMap<ProviderId, Arc<dyn RuntimeAuthorityResolver>>,
}

impl RuntimeAuthorityResolverRegistry {
    pub fn new(control_timeout: Duration) -> UseResult<Self> {
        if control_timeout.is_zero() || control_timeout > MAX_RUNTIME_AUTHORITY_RESOLUTION_TIMEOUT {
            return Err(resolver_registry_error(
                "Runtime authority resolution requires a positive timeout no greater than 60 seconds.",
            ));
        }
        Ok(Self {
            control_timeout,
            resolvers: BTreeMap::new(),
        })
    }

    pub fn control_timeout(&self) -> Duration {
        self.control_timeout
    }

    pub fn register(&mut self, resolver: Arc<dyn RuntimeAuthorityResolver>) -> UseResult<()> {
        let provider_id = resolver.provider_id().clone();
        if self.resolvers.contains_key(&provider_id) {
            return Err(resolver_registry_error(
                "A Runtime authority resolver is already registered for this provider.",
            )
            .with_detail("providerId", provider_id.as_str()));
        }
        self.resolvers.insert(provider_id, resolver);
        Ok(())
    }

    pub fn contains(&self, provider_id: &ProviderId) -> bool {
        self.resolvers.contains_key(provider_id)
    }

    /// Resolve exact authority for the reviewed package state.
    ///
    /// The single timeout bounds all resolver calls. Dropping the future must
    /// be safe because resolver implementations are planning-only.
    pub async fn resolve_bindings(
        &self,
        bundle: &PluginPlanningBundle,
        package: &PlannedPackageState,
        scope_id: &str,
        generation: u64,
        assignments: &[RuntimeProviderAssignment],
    ) -> UseResult<RuntimeAuthorityBindings> {
        tokio::time::timeout(
            self.control_timeout,
            self.resolve_bindings_unbounded(bundle, package, scope_id, generation, assignments),
        )
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.runtime.authority_resolution_timeout",
                "Host Runtime authority resolution exceeded its total planning deadline.",
            )
            .with_detail("timeoutMs", self.control_timeout.as_millis() as u64)
        })?
    }

    async fn resolve_bindings_unbounded(
        &self,
        bundle: &PluginPlanningBundle,
        package: &PlannedPackageState,
        scope_id: &str,
        generation: u64,
        assignments: &[RuntimeProviderAssignment],
    ) -> UseResult<RuntimeAuthorityBindings> {
        validate_runtime_bundle_package(bundle, package, generation)?;
        PluginWorkspaceGrant::validate_identity(scope_id, &bundle.package_id).map_err(|_| {
            resolution_invalid(
                None,
                None,
                "Runtime authority resolution requires one valid workspace and package identity.",
            )
        })?;

        let expected = selected_runtime_surface_refs(package)
            .into_iter()
            .map(|surface| PlanQualifiedSurfaceRef {
                package_id: bundle.package_id.clone(),
                surface,
            })
            .collect::<Vec<_>>();
        let assignments = canonicalize_provider_assignments(&expected, assignments.to_vec())?;

        let mut surfaces = Vec::new();
        for permission in &package.permissions.surfaces {
            if permission.filesystem.is_empty() && permission.secrets.is_empty() {
                continue;
            }
            if !matches!(
                permission.surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            ) {
                return Err(resolution_invalid(
                    None,
                    None,
                    "Runtime authority resolution received authority for a non-executable surface.",
                ));
            }
            let surface = PlanQualifiedSurfaceRef {
                package_id: bundle.package_id.clone(),
                surface: permission.surface.clone(),
            };
            let assignment = assignments
                .binary_search_by(|assignment| assignment.surface().cmp(&surface))
                .ok()
                .and_then(|index| assignments.get(index))
                .ok_or_else(|| {
                    resolution_invalid(
                        None,
                        Some(&surface),
                        "Runtime authority resolution has no exact provider assignment.",
                    )
                })?;
            let resolver = self
                .resolvers
                .get(assignment.provider_id())
                .ok_or_else(|| {
                    UseError::new(
                        "use.plugin.runtime.authority_resolver_unavailable",
                        "The explicitly assigned Runtime provider has no host authority resolver.",
                    )
                    .with_detail("providerId", assignment.provider_id().as_str())
                    .with_detail("packageId", surface.package_id.clone())
                    .with_detail("surfaceId", surface.surface.id.clone())
                })?;
            let ephemeral_storage_limit_bytes = permission
                .resources
                .as_ref()
                .map(|resources| resources.ephemeral_storage_bytes)
                .ok_or_else(|| {
                    resolution_invalid(
                        Some(assignment.provider_id()),
                        Some(&surface),
                        "Runtime authority resolution requires the reviewed resource ceiling.",
                    )
                })?;
            let request = RuntimeAuthorityResolutionRequest {
                scope_id: scope_id.to_string(),
                package_id: bundle.package_id.clone(),
                package_digest: bundle.package_sha256.clone(),
                permission_ceiling_digest: bundle.permission_ceiling_digest.clone(),
                permissions_digest: package.permissions.descriptor_digest()?,
                surface: surface.clone(),
                generation,
                provider_id: assignment.provider_id().clone(),
                filesystem: permission.filesystem.clone(),
                secret_names: permission.secrets.clone(),
                ephemeral_storage_limit_bytes,
            };
            let resolved = resolver
                .resolve_surface_authority(&request)
                .await
                .map_err(|_| resolution_failed(assignment.provider_id(), &surface))?;
            let binding = RuntimeSurfaceAuthorityBindings::new(
                surface.clone(),
                assignment.provider_id().as_str(),
                resolved.filesystem,
                resolved.secrets,
            )
            .map_err(|_| {
                resolution_invalid(
                    Some(assignment.provider_id()),
                    Some(&surface),
                    "A host Runtime authority resolver returned invalid or ambiguous resources.",
                )
            })?;
            surfaces.push(binding);
        }

        let bindings = RuntimeAuthorityBindings::new(surfaces).map_err(|_| {
            resolution_invalid(
                None,
                None,
                "Host Runtime authority resolution returned ambiguous surface resources.",
            )
        })?;
        bindings
            .validate_against(&bundle.package_id, &package.permissions)
            .and_then(|()| bindings.validate_provider_assignments(&assignments))
            .map_err(|_| {
                resolution_invalid(
                    None,
                    None,
                    "Host Runtime authority resolution did not exactly cover the reviewed authority.",
                )
            })?;
        Ok(bindings)
    }
}

impl fmt::Debug for RuntimeAuthorityResolverRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeAuthorityResolverRegistry")
            .field("control_timeout", &self.control_timeout)
            .field("provider_ids", &self.resolvers.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn resolution_failed(provider_id: &ProviderId, surface: &PlanQualifiedSurfaceRef) -> UseError {
    UseError::new(
        "use.plugin.runtime.authority_resolution_failed",
        "The host Runtime authority resolver failed closed.",
    )
    .with_detail("providerId", provider_id.as_str())
    .with_detail("packageId", surface.package_id.clone())
    .with_detail("surfaceId", surface.surface.id.clone())
}

fn resolution_invalid(
    provider_id: Option<&ProviderId>,
    surface: Option<&PlanQualifiedSurfaceRef>,
    message: &'static str,
) -> UseError {
    let mut error = UseError::new("use.plugin.runtime.authority_resolution_invalid", message);
    if let Some(provider_id) = provider_id {
        error = error.with_detail("providerId", provider_id.as_str());
    }
    if let Some(surface) = surface {
        error = error
            .with_detail("packageId", surface.package_id.clone())
            .with_detail("surfaceId", surface.surface.id.clone());
    }
    error
}

fn resolver_registry_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.authority_resolver_invalid", message)
}
