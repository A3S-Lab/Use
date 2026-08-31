use super::*;

#[test]
fn applied_effect_evidence_is_typed_and_exactly_bound_to_its_intent() {
    let reviewed = operation("operation:typed-observation:shape");
    let projected = reviewed
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            reviewed.reviewed_at_ms + 10,
        )
        .unwrap();

    for (index, intent) in projected.effects.iter().enumerate() {
        let applied = application(
            intent,
            char::from_digit(u32::try_from(index).unwrap(), 16).unwrap(),
        );
        applied.validate_for(intent).unwrap();
        assert_eq!(applied.idempotency_key, intent.idempotency_key);
        assert_eq!(applied.descriptor_digest().unwrap().len(), 71);
    }

    let capability = projected
        .effects
        .iter()
        .find(|effect| effect.kind == ControlEffectKind::CapabilityCutover)
        .unwrap();
    let mut wrong_generation = application(capability, '8');
    let ControlAppliedEffectEvidence::CapabilityIndex {
        capability_generation,
        ..
    } = &mut wrong_generation.evidence
    else {
        panic!("the cutover must produce Capability Index evidence");
    };
    *capability_generation += 1;
    assert_eq!(
        wrong_generation.validate_for(capability).unwrap_err().code,
        "use.control_store.input_invalid"
    );

    let knowledge = projected
        .effects
        .iter()
        .find(|effect| effect.owner == ControlEffectOwner::KnowledgeHost)
        .unwrap();
    let mut wrong_owner = application(knowledge, '9');
    wrong_owner.evidence = ControlAppliedEffectEvidence::SkillHost {
        state: ControlSurfaceObservationState::Prepared,
        receipt_digest: digest('9'),
        content_digest: Some(digest('a')),
    };
    assert_eq!(
        wrong_owner.validate_for(knowledge).unwrap_err().code,
        "use.control_store.input_invalid"
    );
}

#[tokio::test]
async fn applied_observation_persists_canonical_typed_evidence() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:typed-observation:persist");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    store.commit_transition(candidate.clone()).await.unwrap();

    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:typed-observation",
            30,
            40,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let observed = observation(
        reviewed.operation_id(),
        &claimed.intent,
        &claimed.claim_token,
        ControlEffectOutcome::Applied,
        '7',
        35,
    );
    let expected = observed.application.clone().unwrap();
    store
        .record_effect_observation(observed.clone())
        .await
        .unwrap();

    let record = &store.effects(reviewed.operation_id()).await.unwrap()[0];
    assert_eq!(record.application.as_ref(), Some(&expected));
    assert_eq!(
        record.evidence_digest.as_deref(),
        Some(expected.descriptor_digest().unwrap().as_str())
    );
    assert!(!store.record_effect_observation(observed).await.unwrap());

    let bytes = store.export().await.unwrap();
    let verified = store.verify_export(bytes.clone()).await.unwrap();
    assert_eq!(
        verified.export.authority.effects[0].application,
        Some(expected)
    );

    let mut time_tampered: super::super::export::ControlStoreExport =
        serde_json::from_slice(&bytes).unwrap();
    time_tampered.authority.effects[0].observed_at_ms = Some(candidate.committed_at_ms - 1);
    assert_eq!(
        store
            .verify_export(canonical_json(&time_tampered))
            .await
            .unwrap_err()
            .code,
        "use.control_store.export_invalid"
    );

    let mut tampered: super::super::export::ControlStoreExport =
        serde_json::from_slice(&bytes).unwrap();
    let record = &mut tampered.authority.effects[0];
    let application = record.application.as_mut().unwrap();
    application.evidence = ControlAppliedEffectEvidence::SkillHost {
        state: ControlSurfaceObservationState::Prepared,
        receipt_digest: digest('7'),
        content_digest: Some(digest('a')),
    };
    record.evidence_digest = Some(application.descriptor_digest().unwrap());
    assert_eq!(
        store
            .verify_export(canonical_json(&tampered))
            .await
            .unwrap_err()
            .code,
        "use.control_store.export_invalid"
    );
}

#[tokio::test]
async fn capability_cutover_publishes_atomically_before_drain_and_completion() {
    let (_temporary, store) = initialized_store().await;
    let installed = operation_at(
        "operation:cutover-observation:install",
        PluginOperationAction::Install,
        0,
        0,
    );
    store.register_operation(installed.clone()).await.unwrap();
    let installed_transition = transition(control_installation(), &installed);
    store
        .commit_transition(installed_transition.clone())
        .await
        .unwrap();
    apply_all_effects(&store, &installed, 30).await;

    let prior = store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&prior).unwrap();
    let upgrade = operation_at(
        "operation:cutover-observation:upgrade",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    store.register_operation(upgrade.clone()).await.unwrap();
    let candidate = projected_transition(&upgrade, &prior, &history);
    store.commit_transition(candidate.clone()).await.unwrap();

    let mut now = 120;
    loop {
        let token = format!("claim:cutover:{now}");
        let claimed = store
            .claim_next_effect(claim(upgrade.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
            .unwrap();
        let kind = claimed.intent.kind;
        store
            .record_effect_observation(observation(
                upgrade.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(claimed.intent.sequence % 16, 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
        now += 20;
        if kind == ControlEffectKind::CapabilityCutover {
            break;
        }
    }

    let inspection = store.inspect().await.unwrap();
    assert_eq!(inspection.metadata.current_generation, 2);
    assert_eq!(inspection.metadata.published_capability_generation, 2);
    let published = store.current_generation().await.unwrap().unwrap();
    assert_eq!(
        published.capability_status,
        ControlCapabilityStatus::Published
    );
    let published_at = published.capability_published_at_ms.unwrap();
    assert_eq!(published_at, now - 15);
    assert_eq!(
        store
            .operation(upgrade.operation_id())
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::EffectsPending
    );
    let next = store
        .claim_next_effect(claim(
            upgrade.operation_id(),
            "claim:cutover:drain",
            now,
            now + 10,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.intent.kind, ControlEffectKind::CallsDrain);

    store
        .record_effect_observation(observation(
            upgrade.operation_id(),
            &next.intent,
            &next.claim_token,
            ControlEffectOutcome::Rejected,
            'd',
            now + 5,
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .operation(upgrade.operation_id())
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::EffectsPending
    );
    assert_eq!(
        store
            .inspect()
            .await
            .unwrap()
            .metadata
            .published_capability_generation,
        2
    );
    assert_eq!(
        store
            .claim_next_effect(claim(
                upgrade.operation_id(),
                "claim:cutover:implicit-retry",
                now + 20,
                now + 30,
                false,
            ))
            .await
            .unwrap_err()
            .code,
        "use.control_store.reconciliation_required"
    );
    let retried = store
        .claim_next_effect(claim(
            upgrade.operation_id(),
            "claim:cutover:explicit-retry",
            now + 20,
            now + 30,
            true,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.intent.idempotency_key, next.intent.idempotency_key);
    assert_eq!(retried.attempt, 2);
    store
        .record_effect_observation(observation(
            upgrade.operation_id(),
            &retried.intent,
            &retried.claim_token,
            ControlEffectOutcome::Applied,
            'e',
            now + 25,
        ))
        .await
        .unwrap();

    apply_all_effects(&store, &upgrade, now + 40).await;
    let completed = store.current_generation().await.unwrap().unwrap();
    assert_eq!(
        completed.capability_status,
        ControlCapabilityStatus::Published
    );
    assert_eq!(completed.capability_published_at_ms, Some(published_at));
    store
        .verify_export(store.export().await.unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn terminal_completion_cannot_predate_provider_observations() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:typed-observation:terminal-time");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    store.commit_transition(candidate).await.unwrap();

    let mut now = 30;
    let mut last_observed_at = 0;
    loop {
        let token = format!("claim:terminal-time:{now}");
        let Some(claimed) = store
            .claim_next_effect(claim(reviewed.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
        else {
            break;
        };
        last_observed_at = now + 5;
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(claimed.intent.sequence % 16, 16).unwrap(),
                last_observed_at,
            ))
            .await
            .unwrap();
        now += 20;
    }

    assert_eq!(
        store
            .complete_operation(
                reviewed.operation_id(),
                reviewed.plan_digest(),
                &digest('e'),
                last_observed_at - 1,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.conflict"
    );
    assert_eq!(
        store
            .complete_operation(
                reviewed.operation_id(),
                reviewed.plan_digest(),
                &digest('e'),
                last_observed_at,
            )
            .await
            .unwrap()
            .status,
        ControlOperationStatus::Completed
    );
}

#[tokio::test]
async fn provider_observation_cannot_predate_the_committed_transition() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:typed-observation:commit-time");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    let committed_at_ms = candidate.committed_at_ms;
    store.commit_transition(candidate).await.unwrap();

    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:typed-observation:commit-time",
            committed_at_ms + 10,
            committed_at_ms + 20,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let result = store
        .record_effect_observation(observation(
            reviewed.operation_id(),
            &claimed.intent,
            &claimed.claim_token,
            ControlEffectOutcome::Applied,
            'c',
            committed_at_ms - 1,
        ))
        .await;

    assert_eq!(result.unwrap_err().code, "use.control_store.conflict");
    assert_eq!(
        store.effects(reviewed.operation_id()).await.unwrap()[0].status,
        ControlEffectStatus::Claimed
    );
}

#[tokio::test]
async fn provider_observations_must_follow_effect_sequence_order() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:typed-observation:sequence-time");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    store.commit_transition(candidate).await.unwrap();

    let first = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:typed-observation:sequence-time:0",
            30,
            50,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    store
        .record_effect_observation(observation(
            reviewed.operation_id(),
            &first.intent,
            &first.claim_token,
            ControlEffectOutcome::Applied,
            '1',
            40,
        ))
        .await
        .unwrap();

    let second = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:typed-observation:sequence-time:1",
            41,
            60,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let result = store
        .record_effect_observation(observation(
            reviewed.operation_id(),
            &second.intent,
            &second.claim_token,
            ControlEffectOutcome::Applied,
            '2',
            39,
        ))
        .await;

    assert_eq!(result.unwrap_err().code, "use.control_store.conflict");
    assert_eq!(
        store.effects(reviewed.operation_id()).await.unwrap()[1].status,
        ControlEffectStatus::Claimed
    );
}

#[test]
fn runtime_service_observation_rejects_ambient_or_unbound_endpoints() {
    let reviewed = super::grant_fixtures::reviewed_grant_operation(
        "operation:typed-observation:runtime",
        PluginOperationAction::Install,
        None,
        None,
    );
    let projected = reviewed
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            reviewed.reviewed_at_ms + 10,
        )
        .unwrap();
    let runtime = projected
        .effects
        .iter()
        .find(|effect| matches!(effect.owner, ControlEffectOwner::RuntimeProvider { .. }))
        .unwrap();
    let mut applied = application(runtime, 'f');
    let ControlAppliedEffectEvidence::RuntimeProvider { binding, .. } = &mut applied.evidence
    else {
        panic!("the executable surface must produce Runtime evidence");
    };
    *binding = Some(ControlRuntimeBindingObservation::Service {
        endpoint_ref: "https://127.0.0.1:4100/mcp".to_string(),
        readiness_digest: digest('f'),
    });
    assert_eq!(
        applied.validate_for(runtime).unwrap_err().code,
        "use.control_store.input_invalid"
    );
}
