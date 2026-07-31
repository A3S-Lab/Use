use std::sync::Arc;

use a3s_runtime::contract::{MountKind, RuntimeFeature, RuntimeMountSource, SecretTarget};
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{
    FilesystemAccess, FilesystemPermission, FilesystemScope, PlanPolicyDecision,
    PlanQualifiedSurfaceRef, PluginPlanningBundle, PluginWorkspaceGrantProposal,
};
use async_trait::async_trait;

use super::test_support::{capabilities, FakeRuntime};
use super::tests::{runtime_bundle_inputs, runtime_grant_plan};
use super::*;

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

#[test]
fn explicit_authority_bindings_map_filesystems_and_secret_references() {
    let (bundle, package, proposal, bindings) = authority_inputs(64 * 1024 * 1024);
    let assignments = authority_assignments(&bundle);

    let plans = plan_runtime_bundle_with_authority(
        &bundle,
        &package,
        &proposal,
        &bindings,
        &assignments,
        8,
    )
    .unwrap();
    let spec = plans[0].spec();

    assert_eq!(spec.mounts.len(), 3);
    assert_eq!(spec.mounts[0].target, "/a3s/plugin-data/cache");
    assert!(!spec.mounts[0].read_only);
    assert!(matches!(
        &spec.mounts[0].source,
        RuntimeMountSource::Volume { volume_id }
            if volume_id == "plugin-data-acme-research-cache"
    ));
    assert_eq!(spec.mounts[1].target, "/a3s/temporary/scratch");
    assert!(matches!(
        &spec.mounts[1].source,
        RuntimeMountSource::Tmpfs { size_bytes } if *size_bytes == 64 * 1024 * 1024
    ));
    assert_eq!(spec.mounts[2].target, "/a3s/workspace/datasets");
    assert!(spec.mounts[2].read_only);
    assert!(matches!(
        &spec.mounts[2].source,
        RuntimeMountSource::Volume { volume_id }
            if volume_id == "workspace-01-datasets-read"
    ));
    assert_eq!(spec.secrets.len(), 1);
    assert_eq!(spec.secrets[0].name, "api-token");
    assert_eq!(
        spec.secrets[0].reference,
        "secret://workspace-01/acme/research/api-token"
    );
    assert_eq!(
        spec.secrets[0].target,
        SecretTarget::Environment {
            variable: "ACME_API_TOKEN".to_string()
        }
    );
}

#[test]
fn authority_bindings_require_exact_permission_coverage() {
    let (mut bundle, mut package, mut proposal, bindings) = authority_inputs(64 * 1024 * 1024);
    let assignments = authority_assignments(&bundle);

    let missing = plan_runtime_bundle(&bundle, &package, &proposal, 8).unwrap_err();
    package.permissions.surfaces[0].filesystem.clear();
    package.permissions.surfaces[0].secrets.clear();
    proposal.authority.decision = PlanPolicyDecision::Allow;
    rebind_permission_digest(&mut bundle, &mut package, &mut proposal);
    let extra = plan_runtime_bundle_with_authority(
        &bundle,
        &package,
        &proposal,
        &bindings,
        &assignments,
        8,
    )
    .unwrap_err();

    assert_eq!(missing.code, "use.plugin.runtime.authority_binding_invalid");
    assert_eq!(extra.code, "use.plugin.runtime.authority_binding_invalid");
}

#[test]
fn temporary_mounts_cannot_exceed_the_reviewed_ephemeral_limit() {
    let (bundle, package, proposal, bindings) = authority_inputs(513 * 1024 * 1024);
    let assignments = authority_assignments(&bundle);

    let error = plan_runtime_bundle_with_authority(
        &bundle,
        &package,
        &proposal,
        &bindings,
        &assignments,
        8,
    )
    .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.authority_binding_invalid");
}

#[test]
fn authority_bindings_reject_ambiguous_resources_and_inline_secrets() {
    let workspace = FilesystemPermission {
        scope: FilesystemScope::Workspace,
        path: ".".to_string(),
        access: FilesystemAccess::Read,
    };
    let wrong_source = RuntimeFilesystemBinding::new(
        workspace.clone(),
        RuntimeMountSource::Tmpfs { size_bytes: 1 },
    )
    .unwrap_err();
    let unix_host_path = RuntimeFilesystemBinding::new(
        workspace.clone(),
        RuntimeMountSource::Volume {
            volume_id: "/var/lib/a3s/data".to_string(),
        },
    )
    .unwrap_err();
    let windows_host_path = RuntimeFilesystemBinding::new(
        workspace,
        RuntimeMountSource::Volume {
            volume_id: "C:/a3s/data".to_string(),
        },
    )
    .unwrap_err();
    let inline_secret = RuntimeSecretBinding::new(
        "api-token",
        "plaintext-token",
        SecretTarget::Environment {
            variable: "ACME_API_TOKEN".to_string(),
        },
    )
    .unwrap_err();
    let file_secret = RuntimeSecretBinding::new(
        "api-token",
        "file://host/secrets/api-token",
        SecretTarget::Environment {
            variable: "ACME_API_TOKEN".to_string(),
        },
    )
    .unwrap_err();

    assert_eq!(
        wrong_source.code,
        "use.plugin.runtime.authority_binding_invalid"
    );
    assert_eq!(
        inline_secret.code,
        "use.plugin.runtime.authority_binding_invalid"
    );
    assert_eq!(
        file_secret.code,
        "use.plugin.runtime.authority_binding_invalid"
    );
    assert_eq!(
        unix_host_path.code,
        "use.plugin.runtime.authority_binding_invalid"
    );
    assert_eq!(
        windows_host_path.code,
        "use.plugin.runtime.authority_binding_invalid"
    );
}

#[tokio::test]
async fn broker_retains_authority_bindings_across_both_provider_passes() {
    let (bundle, package, proposal, bindings) = authority_inputs(64 * 1024 * 1024);
    let grant_plan = runtime_grant_plan(&package, &proposal);
    let assignments = authority_assignments(&bundle);
    let plans = plan_runtime_bundle_with_authority(
        &bundle,
        &package,
        &proposal,
        &bindings,
        &assignments,
        8,
    )
    .unwrap();
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plans[0]), true));
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: runtime,
        }))
        .unwrap();

    let selection = PluginRuntimeBroker::new(&registry)
        .preflight_bundle_with_authority(bundle, package, "workspace-01", 8, bindings, assignments)
        .await
        .unwrap()
        .authorize_grant_plan(&grant_plan)
        .await
        .unwrap();

    assert_eq!(selection.surfaces()[0].plan().spec().mounts.len(), 3);
    assert_eq!(selection.surfaces()[0].plan().spec().secrets.len(), 1);
}

#[tokio::test]
async fn provider_must_advertise_mount_and_secret_reference_support() {
    let (bundle, package, proposal, bindings) = authority_inputs(64 * 1024 * 1024);
    let assignments = authority_assignments(&bundle);
    let provisional = plan_runtime_bundle_with_authority(
        &bundle,
        &package,
        &proposal,
        &bindings,
        &assignments,
        8,
    )
    .unwrap();
    let mut unsupported = capabilities(&provisional[0]);
    unsupported
        .mount_kinds
        .retain(|kind| *kind != MountKind::Volume);
    unsupported
        .features
        .retain(|feature| *feature != RuntimeFeature::SecretReferences);
    let runtime = Arc::new(FakeRuntime::new(unsupported, true));
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: runtime,
        }))
        .unwrap();

    let error = PluginRuntimeBroker::new(&registry)
        .preflight_bundle_with_authority(bundle, package, "workspace-01", 8, bindings, assignments)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.capability_missing");
}

#[test]
fn authority_binding_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeAuthorityBindings>();
    assert_send_sync::<RuntimeSurfaceAuthorityBindings>();
    assert_send_sync::<RuntimeFilesystemBinding>();
    assert_send_sync::<RuntimeSecretBinding>();
}

fn authority_inputs(
    temporary_bytes: u64,
) -> (
    PluginPlanningBundle,
    a3s_use_core::PlannedPackageState,
    PluginWorkspaceGrantProposal,
    RuntimeAuthorityBindings,
) {
    let (mut bundle, mut package, mut proposal) = runtime_bundle_inputs(false);
    let plugin_data = FilesystemPermission {
        scope: FilesystemScope::PluginData,
        path: "cache".to_string(),
        access: FilesystemAccess::ReadWrite,
    };
    let temporary = FilesystemPermission {
        scope: FilesystemScope::Temporary,
        path: "scratch".to_string(),
        access: FilesystemAccess::ReadWrite,
    };
    let workspace = FilesystemPermission {
        scope: FilesystemScope::Workspace,
        path: "datasets".to_string(),
        access: FilesystemAccess::Read,
    };
    package.permissions.surfaces[0].filesystem =
        vec![plugin_data.clone(), temporary.clone(), workspace.clone()];
    package.permissions.surfaces[0].secrets = vec!["api-token".to_string()];
    rebind_permission_digest(&mut bundle, &mut package, &mut proposal);
    proposal.authority.decision = PlanPolicyDecision::Ask;

    let surface = PlanQualifiedSurfaceRef {
        package_id: package.release.package_id.clone(),
        surface: package.permissions.surfaces[0].surface.clone(),
    };
    let bindings = RuntimeAuthorityBindings::new(vec![RuntimeSurfaceAuthorityBindings::new(
        surface,
        "test-runtime",
        vec![
            RuntimeFilesystemBinding::new(
                plugin_data,
                RuntimeMountSource::Volume {
                    volume_id: "plugin-data-acme-research-cache".to_string(),
                },
            )
            .unwrap(),
            RuntimeFilesystemBinding::new(
                temporary,
                RuntimeMountSource::Tmpfs {
                    size_bytes: temporary_bytes,
                },
            )
            .unwrap(),
            RuntimeFilesystemBinding::new(
                workspace,
                RuntimeMountSource::Volume {
                    volume_id: "workspace-01-datasets-read".to_string(),
                },
            )
            .unwrap(),
        ],
        vec![RuntimeSecretBinding::new(
            "api-token",
            "secret://workspace-01/acme/research/api-token",
            SecretTarget::Environment {
                variable: "ACME_API_TOKEN".to_string(),
            },
        )
        .unwrap()],
    )
    .unwrap()])
    .unwrap();
    (bundle, package, proposal, bindings)
}

fn authority_assignments(bundle: &PluginPlanningBundle) -> Vec<RuntimeProviderAssignment> {
    vec![RuntimeProviderAssignment::new(
        PlanQualifiedSurfaceRef {
            package_id: bundle.package_id.clone(),
            surface: bundle.surfaces[0].reference(),
        },
        "test-runtime",
    )
    .unwrap()]
}

fn rebind_permission_digest(
    bundle: &mut PluginPlanningBundle,
    package: &mut a3s_use_core::PlannedPackageState,
    proposal: &mut PluginWorkspaceGrantProposal,
) {
    let digest = package.permissions.descriptor_digest().unwrap();
    package.release.permission_ceiling_digest = digest.clone();
    bundle.permission_ceiling_digest = digest.clone();
    proposal.permission_ceiling_digest = digest;
    proposal.permissions_digest = package.permissions.descriptor_digest().unwrap();
    proposal.permissions = package.permissions.clone();
}
