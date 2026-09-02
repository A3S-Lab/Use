use std::sync::Arc;

use a3s_use_core::PluginOperationAction;

use super::aggregate_tests::fixtures::{
    apply_all_effects, claim, control_installation, digest, initialized_store, observation,
    operation, operation_at, projected_transition, transition,
};
use super::dispatcher::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    ControlEffectDispatcher, ControlEffectPorts, SystemControlEffectClock,
};
use super::effect_owner::capability_plane::ControlCapabilityPlaneEffectPort;
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
use super::effect_owner::static_surface::ControlStaticSurfaceEffectPort;
use super::effect_port::{
    ControlEffectPortOutcome, ControlFlowEffectPort, ControlRuntimeApplication,
    ControlRuntimeEffectPort, ControlRuntimeEffectRequest, ControlSurfaceApplication,
    ControlSurfaceEffectRequest,
};
use super::knowledge_effect_test_support::{knowledge_owner_fixture_for, KnowledgeOwnerFixture};
use super::model::{
    ControlAppliedEffectEvidence, ControlEffectOutcome, ControlEffectOwner, ControlEffectStatus,
    ControlProjectionHistory, ReviewedControlOperation,
};
use super::ControlStore;

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

    let plane = Arc::new(ControlCapabilityPlaneEffectPort::new(store.clone()));
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
    for sequence in 0..3_u32 {
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
    store
        .complete_operation(
            installed.operation_id(),
            installed.plan_digest(),
            &digest('f'),
            SystemControlEffectClock.now_ms().unwrap(),
        )
        .await
        .unwrap();
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
