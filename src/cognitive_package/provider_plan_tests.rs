use std::sync::Arc;

use a3s_runtime::contract::{
    HealthCheckKind, IsolationLevel, MountKind, NetworkMode, ResourceControl,
    RuntimeCapabilities, RuntimeFeature, RuntimeUnitClass,
};
use a3s_runtime::{ProviderId, RuntimeClient, RuntimeProviderFactory, RuntimeResult};
use a3s_use_core::{
    CatalogSurface, PlanActor, PlanPackageChangeKind, PlanPackageRole, PlanPolicyDecision,
    PlanScopeKind, PlannedOperationImpact, PlannedPluginRelease, PlannedStateEvidence,
    PlanningArtifactRef, PlanningSurfaceActivation, PluginCatalogRecord, PluginPackageLockHost,
    PluginPackageResolver, PluginPermissionCeiling, PluginPlanSource, PluginReleaseChannel,
    PluginSurfaceRef, ResourcePermissionCeiling, SurfacePermissionCeiling, ToolWorkloadClass,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, WorkspaceGrantProposalAuthority,
    PLUGIN_PERMISSION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};
use async_trait::async_trait;

use crate::plugin_runtime::test_support::{service_descriptor, FakeRuntime, DIGEST_A};
use crate::plugin_runtime::RuntimeSurfacePlanKey;

use super::*;

const DIGEST_B: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

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

#[tokio::test]
async fn mixed_package_produces_complete_native_and_managed_provider_plan() {
    let (transition, bundle, proposal) = mixed_inputs();
    let capabilities = runtime_capabilities();
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: Arc::new(FakeRuntime::new(capabilities, true)),
        }))
        .unwrap();
    let managed_surface = PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_owned(),
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "index".to_owned(),
        },
    };

    let planned = plan_cognitive_package_providers(
        &[transition],
        &BTreeMap::from([("acme/research".to_owned(), bundle)]),
        &BTreeMap::from([("acme/research".to_owned(), proposal)]),
        &scope(),
        &BTreeMap::from([("acme/research".to_owned(), 8)]),
        vec![RuntimeProviderAssignment::new(managed_surface, "test-runtime").unwrap()],
        &registry,
    )
    .await
    .unwrap();

    assert_eq!(planned.provider_evidence().len(), 2);
    assert_eq!(
        planned.provider_evidence()[0].provider_id,
        "a3s-use-native-launcher"
    );
    assert_eq!(planned.provider_evidence()[1].provider_id, "test-runtime");
    assert_eq!(planned.runtime_selection().surfaces().len(), 1);
    assert_eq!(
        planned.runtime_selection().surfaces()[0]
            .plan()
            .spec()
            .generation,
        8
    );
    let publications = planned.runtime_plan_publications().unwrap();
    assert_eq!(publications.len(), 1);
    assert_eq!(
        publications[0].key,
        RuntimeSurfacePlanKey::from_plan(
            planned.runtime_selection().surfaces()[0].plan(),
            planned.runtime_selection().surfaces()[0].provider(),
        )
        .unwrap()
    );
    assert_eq!(
        publications[0].plan,
        *planned.runtime_selection().surfaces()[0].plan()
    );
    planned
        .verify_reviewed_evidence(planned.provider_evidence())
        .unwrap();

    let mut changed = planned.provider_evidence().to_vec();
    changed[1].provider_build_id = "build-2".to_owned();
    let error = planned.verify_preflight_evidence(&changed).unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");

    let mut changed = planned.provider_evidence().to_vec();
    changed[1].semantics_profile_digest = DIGEST_D.to_owned();
    planned.verify_preflight_evidence(&changed).unwrap();
    let error = planned.verify_reviewed_evidence(&changed).unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");
}

#[tokio::test]
async fn two_pass_binding_replans_grant_semantics_without_provider_drift() {
    let (transition, bundle, _) = mixed_inputs();
    let package = transition.after.unwrap();
    let transition = PlannedPackageTransition::resolved(
        "acme/research",
        PlanPackageRole::Root,
        PlanPackageChangeKind::Add,
        None,
        Some(package),
        Some(PluginPlanSource::ReleaseBundle {
            bundle_digest: DIGEST_C.to_owned(),
            package_digest: DIGEST_A.to_owned(),
        }),
    )
    .unwrap();
    let draft = PluginOperationPlanDraft::new_unbound(
        PluginOperationAction::Install,
        "acme/research",
        "use/acme/research",
        vec![transition],
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: 4096,
            installed_bytes_after: 8192,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 5,
            capability_generation: 4,
            receipt_digest: None,
        },
    )
    .unwrap();
    let provisional_binding = PluginOperationPlanBinding {
        operation_id: "install:provider-two-pass".to_owned(),
        created_at_ms: 10,
        expires_at_ms: 20,
        scope: scope(),
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: DIGEST_C.to_owned(),
            confirmation_required: true,
        },
    };
    let snapshot = PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_owned(),
        scope_id: "workspace-01".to_owned(),
        state_revision: 5,
        grants: Vec::new(),
    };
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: Arc::new(FakeRuntime::new(runtime_capabilities(), true)),
        }))
        .unwrap();
    let assignment = RuntimeProviderAssignment::new(
        PlanQualifiedSurfaceRef {
            package_id: "acme/research".to_owned(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "index".to_owned(),
            },
        },
        "test-runtime",
    )
    .unwrap();

    let bound = bind_cognitive_package_provider_plan(
        draft,
        provisional_binding,
        &snapshot,
        &BTreeMap::from([("acme/research".to_owned(), bundle)]),
        &BTreeMap::from([("acme/research".to_owned(), 8)]),
        vec![assignment],
        &registry,
        |_| {
            Ok(PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Allow,
                policy_digest: DIGEST_D.to_owned(),
                confirmation_required: false,
            })
        },
    )
    .await
    .unwrap();

    assert_eq!(bound.plan().providers.len(), 2);
    assert_eq!(bound.plan().authority.decision, PlanPolicyDecision::Allow);
    assert_eq!(bound.providers().runtime_selection().surfaces().len(), 1);
    assert_eq!(
        bound
            .grants()
            .proposal("acme/research")
            .unwrap()
            .authority
            .decision,
        PlanPolicyDecision::Allow
    );
}

#[test]
fn managed_provider_generations_follow_add_replace_and_retain_lifecycles() {
    let lock = provider_package_lock();
    let (_, bundle, _) = mixed_inputs();
    let bundles = BTreeMap::from([("acme/research".to_owned(), bundle)]);

    let add = provider_transition(PlanPackageChangeKind::Add);
    let generations = plan_cognitive_package_provider_generations(
        PluginOperationAction::Install,
        &[add],
        7,
        Some(&lock),
        &bundles,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        generations,
        BTreeMap::from([("acme/research".to_owned(), 7)])
    );

    let replace = provider_transition(PlanPackageChangeKind::Replace);
    let generations = plan_cognitive_package_provider_generations(
        PluginOperationAction::Upgrade,
        &[replace],
        7,
        Some(&lock),
        &bundles,
        &BTreeMap::from([("acme/research".to_owned(), 11)]),
    )
    .unwrap();
    assert_eq!(
        generations,
        BTreeMap::from([("acme/research".to_owned(), 12)])
    );

    let retain = provider_transition(PlanPackageChangeKind::Retain);
    let generations = plan_cognitive_package_provider_generations(
        PluginOperationAction::Upgrade,
        &[retain],
        7,
        Some(&lock),
        &bundles,
        &BTreeMap::from([("acme/research".to_owned(), 11)]),
    )
    .unwrap();
    assert_eq!(
        generations,
        BTreeMap::from([("acme/research".to_owned(), 11)])
    );
}

#[test]
fn managed_provider_generations_reject_missing_or_exhausted_prior_generation() {
    let lock = provider_package_lock();
    let (_, bundle, _) = mixed_inputs();
    let bundles = BTreeMap::from([("acme/research".to_owned(), bundle)]);
    let replace = provider_transition(PlanPackageChangeKind::Replace);

    let missing = plan_cognitive_package_provider_generations(
        PluginOperationAction::Upgrade,
        std::slice::from_ref(&replace),
        7,
        Some(&lock),
        &bundles,
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert_eq!(missing.code, "use.plugin.provider_plan_invalid");
    assert!(missing.message.contains("omitted its prior generation"));

    let exhausted = plan_cognitive_package_provider_generations(
        PluginOperationAction::Upgrade,
        &[replace],
        7,
        Some(&lock),
        &bundles,
        &BTreeMap::from([("acme/research".to_owned(), u64::MAX)]),
    )
    .unwrap_err();
    assert_eq!(exhausted.code, "use.plugin.provider_plan_invalid");
    assert!(exhausted.message.contains("generation is exhausted"));
}

#[tokio::test]
async fn managed_provider_plan_rejects_missing_or_extra_host_evidence() {
    let (transition, bundle, proposal) = mixed_inputs();
    let bundles = BTreeMap::from([("acme/research".to_owned(), bundle)]);
    let proposals = BTreeMap::from([("acme/research".to_owned(), proposal)]);
    let registry = RuntimeClientRegistry::new();

    let missing_proposal = plan_cognitive_package_providers(
        std::slice::from_ref(&transition),
        &bundles,
        &BTreeMap::new(),
        &scope(),
        &BTreeMap::from([("acme/research".to_owned(), 8)]),
        Vec::new(),
        &registry,
    )
    .await
    .unwrap_err();
    assert_eq!(missing_proposal.code, "use.plugin.provider_plan_invalid");

    let extra_generation = plan_cognitive_package_providers(
        &[transition],
        &bundles,
        &proposals,
        &scope(),
        &BTreeMap::from([
            ("acme/research".to_owned(), 8),
            ("acme/unrelated".to_owned(), 9),
        ]),
        Vec::new(),
        &registry,
    )
    .await
    .unwrap_err();
    assert_eq!(extra_generation.code, "use.plugin.provider_plan_invalid");
}

#[tokio::test]
async fn selected_runtime_disappearance_never_falls_back_to_native() {
    let (transition, bundle, proposal) = mixed_inputs();
    let managed_surface = PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_owned(),
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "index".to_owned(),
        },
    };
    let error = plan_cognitive_package_providers(
        &[transition],
        &BTreeMap::from([("acme/research".to_owned(), bundle)]),
        &BTreeMap::from([("acme/research".to_owned(), proposal)]),
        &scope(),
        &BTreeMap::from([("acme/research".to_owned(), 8)]),
        vec![RuntimeProviderAssignment::new(managed_surface, "missing-runtime").unwrap()],
        &RuntimeClientRegistry::new(),
    )
    .await
    .unwrap_err();

    assert_ne!(error.code, "use.plugin.provider_plan_invalid");
    assert!(error.message.contains("selected Runtime provider"));
}

fn mixed_inputs() -> (
    PlannedPackageTransition,
    PluginPlanningBundle,
    PluginWorkspaceGrantProposal,
) {
    let descriptor = service_descriptor();
    let native_surface = PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_owned(),
    };
    let managed_surface = PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "index".to_owned(),
    };
    let permissions = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_owned(),
        surfaces: vec![
            SurfacePermissionCeiling {
                surface: native_surface.clone(),
                native_execution: true,
                child_process: false,
                filesystem: Vec::new(),
                network_egress: Vec::new(),
                private_service: false,
                secrets: Vec::new(),
                resources: Some(ResourcePermissionCeiling {
                    cpu_millis: 500,
                    memory_bytes: 256 * 1024 * 1024,
                    pids: 64,
                    ephemeral_storage_bytes: 512 * 1024 * 1024,
                    task_timeout_ms: Some(120_000),
                    max_stdout_bytes: Some(4 * 1024 * 1024),
                    max_stderr_bytes: Some(1024 * 1024),
                }),
                ui_http: Vec::new(),
            },
            SurfacePermissionCeiling {
                surface: managed_surface.clone(),
                native_execution: false,
                child_process: false,
                filesystem: Vec::new(),
                network_egress: Vec::new(),
                private_service: true,
                secrets: Vec::new(),
                resources: Some(ResourcePermissionCeiling {
                    cpu_millis: 500,
                    memory_bytes: 256 * 1024 * 1024,
                    pids: 64,
                    ephemeral_storage_bytes: 512 * 1024 * 1024,
                    task_timeout_ms: None,
                    max_stdout_bytes: None,
                    max_stderr_bytes: None,
                }),
                ui_http: Vec::new(),
            },
        ],
    };
    let permission_digest = permissions.descriptor_digest().unwrap();
    let package = PlannedPackageState {
        release: PlannedPluginRelease {
            package_id: "acme/research".to_owned(),
            version: "2.0.0".to_owned(),
            channel: PluginReleaseChannel::Stable,
            target: "linux-x86_64".to_owned(),
            package_sha256: DIGEST_A.to_owned(),
            manifest_sha256: DIGEST_B.to_owned(),
            permission_ceiling_digest: permission_digest.clone(),
            surfaces: vec![
                CatalogSurface {
                    kind: PluginSurfaceKind::Tool,
                    id: native_surface.id.clone(),
                    optional: false,
                    workload: Some(ToolWorkloadClass::Task),
                    mcp_transport: None,
                    mcp_tool_count: None,
                    okf_bundle: None,
                    requires: Vec::new(),
                },
                CatalogSurface {
                    kind: PluginSurfaceKind::Tool,
                    id: managed_surface.id.clone(),
                    optional: false,
                    workload: Some(ToolWorkloadClass::Service),
                    mcp_transport: None,
                    mcp_tool_count: None,
                    okf_bundle: None,
                    requires: Vec::new(),
                },
            ],
        },
        permissions: permissions.clone(),
    };
    let bundle = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: package.release.package_id.clone(),
        version: package.release.version.clone(),
        channel: package.release.channel,
        target: package.release.target.clone(),
        archive_sha256: DIGEST_C.to_owned(),
        package_sha256: package.release.package_sha256.clone(),
        manifest_sha256: package.release.manifest_sha256.clone(),
        permission_ceiling_digest: permission_digest.clone(),
        surfaces: vec![
            ExecutablePlanningSurface::ToolTaskNative {
                id: native_surface.id,
                activation: PlanningSurfaceActivation::Lazy,
                executable: "bin/acme-research".to_owned(),
                command: "acme-convert".to_owned(),
                json_output: true,
                timeout_ms: 120_000,
            },
            ExecutablePlanningSurface::ToolService {
                id: managed_surface.id,
                activation: PlanningSurfaceActivation::Eager,
                base_path: "/api".to_owned(),
                artifact: PlanningArtifactRef {
                    uri: format!(
                        "oci://registry.example/acme/research-index@{}",
                        descriptor.artifact.digest
                    ),
                    digest: descriptor.artifact.digest.clone(),
                    media_type: descriptor.artifact.media_type.clone(),
                },
                descriptor,
            },
        ],
    };
    let proposal = PluginWorkspaceGrantProposal {
        schema: PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA.to_owned(),
        operation_id: "install:provider-plan".to_owned(),
        scope_id: "workspace-01".to_owned(),
        package_id: package.release.package_id.clone(),
        package_digest: package.release.package_sha256.clone(),
        permission_ceiling_digest: permission_digest.clone(),
        permissions_digest: permission_digest,
        permissions,
        authority: WorkspaceGrantProposalAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: DIGEST_D.to_owned(),
        },
        created_at_ms: 1,
        apply_expires_at_ms: 2,
        grant_expires_at_ms: None,
    };
    (
        PlannedPackageTransition {
            package_id: "acme/research".to_owned(),
            role: PlanPackageRole::Root,
            change: PlanPackageChangeKind::Add,
            before: None,
            after: Some(package),
            source: None,
            surfaces: Vec::new(),
        },
        bundle,
        proposal,
    )
}

fn provider_transition(change: PlanPackageChangeKind) -> PlannedPackageTransition {
    let (transition, _, _) = mixed_inputs();
    let after = transition.after.unwrap();
    match change {
        PlanPackageChangeKind::Add => PlannedPackageTransition::resolved(
            "acme/research",
            PlanPackageRole::Root,
            change,
            None,
            Some(after),
            Some(PluginPlanSource::ReleaseBundle {
                bundle_digest: DIGEST_C.to_owned(),
                package_digest: DIGEST_A.to_owned(),
            }),
        ),
        PlanPackageChangeKind::Replace => {
            let mut before = after.clone();
            before.release.version = "1.0.0".to_owned();
            before.release.package_sha256 = DIGEST_D.to_owned();
            before.release.manifest_sha256 = DIGEST_C.to_owned();
            PlannedPackageTransition::resolved(
                "acme/research",
                PlanPackageRole::Root,
                change,
                Some(before),
                Some(after),
                Some(PluginPlanSource::ReleaseBundle {
                    bundle_digest: DIGEST_C.to_owned(),
                    package_digest: DIGEST_A.to_owned(),
                }),
            )
        }
        PlanPackageChangeKind::Retain => PlannedPackageTransition::resolved(
            "acme/research",
            PlanPackageRole::Root,
            change,
            Some(after.clone()),
            Some(after),
            None,
        ),
        PlanPackageChangeKind::Remove => unreachable!(),
    }
    .unwrap()
}

fn provider_package_lock() -> PluginPackageLock {
    let record = PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-v3.json"
    ))
    .unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://packages.example.test/catalog/".to_owned(),
        root_sha256: DIGEST_D.to_owned(),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap()
}

fn scope() -> PlanScope {
    PlanScope {
        kind: PlanScopeKind::Workspace,
        id: "workspace-01".to_owned(),
    }
}

fn runtime_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        schema: RuntimeCapabilities::SCHEMA.to_owned(),
        provider_id: ProviderId::parse("test-runtime").unwrap(),
        provider_build: "build-1".to_owned(),
        unit_classes: vec![RuntimeUnitClass::Service],
        artifact_media_types: vec!["application/vnd.oci.image.index.v1+json".to_owned()],
        isolation_levels: vec![IsolationLevel::Container],
        network_modes: vec![NetworkMode::Service],
        mount_kinds: Vec::<MountKind>::new(),
        health_check_kinds: vec![HealthCheckKind::Http],
        resource_controls: vec![
            ResourceControl::Cpu,
            ResourceControl::Memory,
            ResourceControl::Pids,
            ResourceControl::EphemeralStorage,
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
