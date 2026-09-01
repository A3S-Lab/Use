use std::sync::Arc;

use a3s_use_core::{
    OkfCapabilityProjection, OkfKnowledgeObservedState, PlanQualifiedSurfaceRef, PluginSurfaceKind,
    PluginSurfaceRef,
};

use super::aggregate_tests::fixtures::{control_installation, operation, transition};
use super::dispatcher::{
    ControlEffectDispatchRequest, ControlEffectDispatchResult, ControlEffectDispatcher,
    SystemControlEffectClock,
};
use super::effect_owner::knowledge::ControlOkfKnowledgeEffectPort;
use super::knowledge_effect_test_support::{
    control_ports_with_knowledge, knowledge_owner_fixture_for,
};
use super::model::{ControlAppliedEffectEvidence, ControlEffectOutcome, ControlEffectStatus};
use super::ControlStore;
use crate::okf_knowledge::OkfKnowledgeSearchRequest;

#[tokio::test]
async fn committed_claim_dispatches_through_the_real_knowledge_owner_and_persists_evidence() {
    let installation = control_installation();
    let (fixture, artifact_admission) = knowledge_owner_fixture_for(installation.clone()).await;
    let store = ControlStore::from_extension_paths(&fixture.paths).unwrap();
    store.initialize().await.unwrap();
    let reviewed = operation("operation:knowledge-dispatch");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(installation.clone(), &reviewed))
        .await
        .unwrap();
    drop(artifact_admission);
    let owner = Arc::new(ControlOkfKnowledgeEffectPort::new(
        fixture.paths.artifact_store(),
        fixture.client.clone(),
        fixture.bindings.clone(),
    ));
    let dispatcher = ControlEffectDispatcher::new(
        store.clone(),
        control_ports_with_knowledge(owner),
        Arc::new(SystemControlEffectClock),
    );

    let result = dispatcher
        .dispatch_next(ControlEffectDispatchRequest {
            operation_id: reviewed.operation_id().to_string(),
            worker_id: "worker:knowledge-dispatch".to_string(),
            claim_token: "claim:knowledge-dispatch".to_string(),
            lease_duration_ms: 10_000,
            provider_timeout_ms: 5_000,
            deferred_retry_delay_ms: 1_000,
            explicit_reconciliation: false,
        })
        .await
        .unwrap();

    assert!(matches!(
        result,
        ControlEffectDispatchResult::Observed {
            sequence: 0,
            attempt: 1,
            outcome: ControlEffectOutcome::Applied,
            observation_changed: true,
            ..
        }
    ));
    let effect = &store.effects(reviewed.operation_id()).await.unwrap()[0];
    assert_eq!(effect.status, ControlEffectStatus::Applied);
    let ControlAppliedEffectEvidence::KnowledgeHost {
        projection_digest: Some(projection_digest),
        ..
    } = &effect.application.as_ref().unwrap().evidence
    else {
        panic!("the durable application must contain Knowledge projection evidence");
    };
    let surface = PlanQualifiedSurfaceRef {
        package_id: "acme/knowledge".to_string(),
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_string(),
        },
    };
    let binding = fixture
        .bindings
        .get(&installation, &surface, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        binding.observation.state,
        OkfKnowledgeObservedState::Promoted
    );
    let projection =
        OkfCapabilityProjection::from_promoted(&binding.receipt, &binding.observation).unwrap();
    assert_eq!(projection.descriptor_digest().unwrap(), *projection_digest);
    let search = fixture
        .client
        .search(
            &OkfKnowledgeSearchRequest::new(installation, "runtime authority", 5, vec![projection])
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!search.hits.is_empty());
}
