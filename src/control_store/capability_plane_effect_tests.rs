use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use a3s_use_core::{
    CapabilityDescriptionProof, CapabilityDescriptor, CapabilityDescriptorKind,
    CapabilityGatewayCatalog, CapabilityPublicationEvidence, CapabilityToolAnnotations,
    InvocationRef, PluginOperationAction, PluginPackageId, PluginSurfaceKind, PluginSurfaceRef,
};

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
    ControlCapabilityDescriptorSnapshotKey, ControlCapabilityDescriptorSnapshotStore,
    ControlCapabilityPlaneEffectPort, ControlCapabilitySignerPolicy,
};
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
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
use super::ControlStore;
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;

struct EmptyCatalogProjection;

struct UnauthorizedCatalogProjection;

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
    let fixture = prepared_capability_plane(operation_id).await;
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

async fn prepared_capability_plane(operation_id: &str) -> InstalledCapabilityPlaneFixture {
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
            Arc::new(EmptyCatalogProjection),
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
    assert!(matches!(
        result,
        ControlEffectDispatchResult::Observed {
            sequence: observed_sequence,
            attempt: observed_attempt,
            outcome,
            observation_changed: true,
            ..
        } if observed_sequence == sequence
            && observed_attempt == attempt
            && outcome == expected_outcome
    ));
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

fn catalog_path(state_root: &std::path::Path, digest: &str) -> std::path::PathBuf {
    let hex = digest.strip_prefix("sha256:").unwrap();
    state_root
        .join("capability-gateway")
        .join("catalogs")
        .join("sha256")
        .join(&hex[..2])
        .join(format!("{hex}.json"))
}
