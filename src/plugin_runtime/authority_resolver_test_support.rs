use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a3s_runtime::contract::{RuntimeMountSource, SecretTarget};
use a3s_runtime::{ProviderId, RuntimeClient, RuntimeProviderFactory, RuntimeResult};
use a3s_use_core::{
    FilesystemAccess, FilesystemPermission, FilesystemScope, PlanQualifiedSurfaceRef,
    PlannedPackageState, PluginPlanningBundle, PluginWorkspaceGrantProposal, UseError, UseResult,
};
use async_trait::async_trait;

use super::tests::runtime_bundle_inputs;
use super::*;

pub(super) const PROVIDER_ID: &str = "test-runtime";

pub(super) struct StaticRuntimeFactory {
    provider_id: ProviderId,
    client: Arc<dyn RuntimeClient>,
}

impl StaticRuntimeFactory {
    pub(super) fn new(provider_id: &str, client: Arc<dyn RuntimeClient>) -> Self {
        Self {
            provider_id: ProviderId::parse(provider_id).unwrap(),
            client,
        }
    }
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

pub(super) struct ExactResolver {
    provider_id: ProviderId,
    requests: Mutex<Vec<RuntimeAuthorityResolutionRequest>>,
}

impl ExactResolver {
    pub(super) fn new(provider_id: &str) -> Self {
        Self {
            provider_id: ProviderId::parse(provider_id).unwrap(),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn requests(&self) -> Vec<RuntimeAuthorityResolutionRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl RuntimeAuthorityResolver for ExactResolver {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn resolve_surface_authority(
        &self,
        request: &RuntimeAuthorityResolutionRequest,
    ) -> UseResult<ResolvedRuntimeSurfaceAuthority> {
        self.requests.lock().unwrap().push(request.clone());
        let filesystem = request
            .filesystem()
            .iter()
            .rev()
            .map(|permission| {
                let source = match permission.scope {
                    FilesystemScope::PluginData => RuntimeMountSource::Volume {
                        volume_id: "cloud-plugin-data-cache".to_string(),
                    },
                    FilesystemScope::Temporary => RuntimeMountSource::Tmpfs {
                        size_bytes: 64 * 1024 * 1024,
                    },
                    FilesystemScope::Workspace => RuntimeMountSource::Volume {
                        volume_id: "cloud-workspace-datasets".to_string(),
                    },
                };
                RuntimeFilesystemBinding::new(permission.clone(), source)
            })
            .collect::<UseResult<Vec<_>>>()?;
        let secrets = request
            .secret_names()
            .iter()
            .rev()
            .map(|name| {
                RuntimeSecretBinding::new(
                    name,
                    "a3s-cloud-secret://018f47e8-34ce-7f2b-9460-71ad3fdbb546/018f47e8-34ce-7f2b-9460-71ad3fdbb547/7",
                    SecretTarget::Environment {
                        variable: "ACME_API_TOKEN".to_string(),
                    },
                )
            })
            .collect::<UseResult<Vec<_>>>()?;
        Ok(ResolvedRuntimeSurfaceAuthority::new(filesystem, secrets))
    }
}

pub(super) struct FailingResolver {
    provider_id: ProviderId,
}

impl FailingResolver {
    pub(super) fn new(provider_id: &str) -> Self {
        Self {
            provider_id: ProviderId::parse(provider_id).unwrap(),
        }
    }
}

#[async_trait]
impl RuntimeAuthorityResolver for FailingResolver {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn resolve_surface_authority(
        &self,
        _request: &RuntimeAuthorityResolutionRequest,
    ) -> UseResult<ResolvedRuntimeSurfaceAuthority> {
        Err(UseError::new(
            "host.secret.lookup_failed",
            "never-print-this-secret-material",
        )
        .with_detail("material", "never-print-this-secret-material"))
    }
}

pub(super) struct HangingResolver {
    provider_id: ProviderId,
}

impl HangingResolver {
    pub(super) fn new(provider_id: &str) -> Self {
        Self {
            provider_id: ProviderId::parse(provider_id).unwrap(),
        }
    }
}

#[async_trait]
impl RuntimeAuthorityResolver for HangingResolver {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn resolve_surface_authority(
        &self,
        _request: &RuntimeAuthorityResolutionRequest,
    ) -> UseResult<ResolvedRuntimeSurfaceAuthority> {
        pending().await
    }
}

pub(super) struct CountingResolver {
    provider_id: ProviderId,
    calls: Arc<AtomicUsize>,
}

impl CountingResolver {
    pub(super) fn new(provider_id: &str, calls: Arc<AtomicUsize>) -> Self {
        Self {
            provider_id: ProviderId::parse(provider_id).unwrap(),
            calls,
        }
    }
}

#[async_trait]
impl RuntimeAuthorityResolver for CountingResolver {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn resolve_surface_authority(
        &self,
        _request: &RuntimeAuthorityResolutionRequest,
    ) -> UseResult<ResolvedRuntimeSurfaceAuthority> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ResolvedRuntimeSurfaceAuthority::new(Vec::new(), Vec::new()))
    }
}

pub(super) fn authority_package() -> (PluginPlanningBundle, PlannedPackageState) {
    let (mut bundle, mut package, _) = runtime_bundle_inputs(false);
    package.permissions.surfaces[0].filesystem = vec![
        FilesystemPermission {
            scope: FilesystemScope::PluginData,
            path: "cache".to_string(),
            access: FilesystemAccess::ReadWrite,
        },
        FilesystemPermission {
            scope: FilesystemScope::Temporary,
            path: "scratch".to_string(),
            access: FilesystemAccess::ReadWrite,
        },
        FilesystemPermission {
            scope: FilesystemScope::Workspace,
            path: "datasets".to_string(),
            access: FilesystemAccess::Read,
        },
    ];
    package.permissions.surfaces[0].secrets = vec!["api-token".to_string()];
    let digest = package.permissions.descriptor_digest().unwrap();
    package.release.permission_ceiling_digest = digest.clone();
    bundle.permission_ceiling_digest = digest;
    (bundle, package)
}

pub(super) fn authority_proposal(package: &PlannedPackageState) -> PluginWorkspaceGrantProposal {
    let (_, _, mut proposal) = runtime_bundle_inputs(false);
    proposal.package_digest = package.release.package_sha256.clone();
    proposal.permission_ceiling_digest = package.release.permission_ceiling_digest.clone();
    proposal.permissions_digest = package.permissions.descriptor_digest().unwrap();
    proposal.permissions = package.permissions.clone();
    proposal.authority.decision = a3s_use_core::PlanPolicyDecision::Ask;
    proposal
}

pub(super) fn assignments(
    bundle: &PluginPlanningBundle,
    provider_id: &str,
) -> Vec<RuntimeProviderAssignment> {
    vec![RuntimeProviderAssignment::new(qualified_surface(bundle), provider_id).unwrap()]
}

pub(super) fn qualified_surface(bundle: &PluginPlanningBundle) -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: bundle.package_id.clone(),
        surface: bundle.surfaces[0].reference(),
    }
}
