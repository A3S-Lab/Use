use super::effect_fixtures::optional_surface_operation;
use super::grant_fixtures::reviewed_grant_operation;
use super::projections::{generation, reviewed_install, test_host, verified_record};
use super::*;
use a3s_use_core::PluginPackageDependency;

#[tokio::test]
async fn claim_carries_the_exact_committed_package_authority() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:authority:install");
    store.register_operation(reviewed.clone()).await.unwrap();
    let committed = store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();

    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:authority:install",
            30,
            40,
            false,
        ))
        .await
        .unwrap()
        .unwrap();

    let ControlEffectAuthority::KnowledgeHost(authority) = &claimed.authority else {
        panic!("the first fixture effect must carry Knowledge authority");
    };
    let selected = committed
        .snapshot
        .package_selection("acme/knowledge")
        .unwrap();
    let lifecycle = committed
        .package_lifecycles
        .iter()
        .find(|candidate| candidate.package_id == "acme/knowledge")
        .unwrap();
    assert_eq!(authority.generation_operation_id, committed.operation_id);
    assert_eq!(
        authority.installation_generation,
        committed.snapshot.generation
    );
    assert_eq!(authority.snapshot_digest, committed.snapshot_digest);
    assert_eq!(authority.host, committed.snapshot.host);
    assert_eq!(authority.package, *selected);
    assert_eq!(
        authority.lifecycle_generation,
        lifecycle.lifecycle_generation
    );
    assert_eq!(authority.grant, None);
}

#[tokio::test]
async fn capability_claim_carries_only_the_terminal_materialization_for_each_target_surface() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = optional_surface_operation("operation:authority:capability");
    store.register_operation(reviewed.clone()).await.unwrap();
    let committed = store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();

    for (sequence, outcome) in [
        ControlEffectOutcome::Applied,
        ControlEffectOutcome::Rejected,
    ]
    .into_iter()
    .enumerate()
    {
        let now = 30 + u64::try_from(sequence).unwrap() * 20;
        let token = format!("claim:authority:surface:{sequence}");
        let claimed = store
            .claim_next_effect(claim(reviewed.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
            .unwrap();
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                outcome,
                char::from_digit(u32::try_from(sequence).unwrap(), 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
    }

    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:authority:capability",
            70,
            80,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = &claimed.authority else {
        panic!("the third fixture effect must carry Capability Index authority");
    };

    assert_eq!(authority.generation, committed);
    assert_eq!(authority.materializations.len(), 2);
    assert!(matches!(
        authority.materializations[0].state,
        ControlCapabilitySurfaceState::Prepared { .. }
    ));
    assert!(matches!(
        authority.materializations[1].state,
        ControlCapabilitySurfaceState::Degraded { .. }
    ));
    assert_eq!(
        authority.materializations[0]
            .intent
            .subject
            .surface()
            .unwrap()
            .id,
        "domain-knowledge"
    );
    assert_eq!(
        authority.materializations[1]
            .intent
            .subject
            .surface()
            .unwrap()
            .id,
        "research"
    );
}

#[tokio::test]
async fn post_cutover_teardown_uses_the_prior_package_incarnation() {
    let (_temporary, store) = initialized_store().await;
    let install = operation("operation:authority:prior-install");
    let installed_projection = install
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            install.reviewed_at_ms + 10,
        )
        .unwrap();
    store.register_operation(install.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &install))
        .await
        .unwrap();
    apply_all_effects(&store, &install, 30).await;
    let prior = store.current_generation().await.unwrap().unwrap();

    let upgrade = operation_at(
        "operation:authority:upgrade",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    store.register_operation(upgrade.clone()).await.unwrap();
    let candidate = projected_transition(&upgrade, &prior, &installed_projection.history_after);
    let target = store.commit_transition(candidate).await.unwrap();

    for sequence in 0..3_u32 {
        let now = 200 + u64::from(sequence) * 20;
        let token = format!("claim:authority:upgrade:{sequence}");
        let claimed = store
            .claim_next_effect(claim(upgrade.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
            .unwrap();
        store
            .record_effect_observation(observation(
                upgrade.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence, 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
    }

    let claimed = store
        .claim_next_effect(claim(
            upgrade.operation_id(),
            "claim:authority:prior-drain",
            270,
            280,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::InvocationLeases(authority) = &claimed.authority else {
        panic!("the first post-cutover effect must carry invocation authority");
    };
    assert_eq!(authority.installation_generation, 1);
    assert_eq!(authority.snapshot_digest, prior.snapshot_digest);
    assert_eq!(
        authority.package,
        *prior.snapshot.package_selection("acme/knowledge").unwrap()
    );
    assert_ne!(authority.package, target.snapshot.packages[0]);
    assert_eq!(authority.lifecycle_generation, 1);
}

#[tokio::test]
async fn capability_authority_reuses_retained_surface_observations_from_history() {
    let shared = verified_record("acme/shared", Vec::new(), 'c');
    let first_lock = PluginPackageResolver::new(test_host())
        .resolve(
            verified_record(
                "acme/root-a",
                vec![PluginPackageDependency::new("acme/shared", "^1.0.0").unwrap()],
                'a',
            ),
            vec![shared.clone()],
        )
        .unwrap();
    let second_lock = PluginPackageResolver::new(test_host())
        .resolve(
            verified_record(
                "acme/root-b",
                vec![PluginPackageDependency::new("acme/shared", "^1.0.0").unwrap()],
                'b',
            ),
            vec![shared],
        )
        .unwrap();
    let first = reviewed_install("operation:authority:root-a", first_lock, None, 0, 0);
    let first_projection = first
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            first.reviewed_at_ms + 10,
        )
        .unwrap();
    let installation = first.envelope.plan.scope.clone();
    let temporary = tempfile::tempdir().unwrap();
    let store = ControlStore::new(temporary.path().join("state"), installation).unwrap();
    store.initialize().await.unwrap();
    store.register_operation(first.clone()).await.unwrap();
    store
        .commit_transition(transition_from_projected(&first, &first_projection))
        .await
        .unwrap();
    apply_all_effects(&store, &first, 130).await;

    let first_generation = generation(&first, &first_projection);
    let second = reviewed_install(
        "operation:authority:root-b",
        second_lock,
        Some(&first_projection.snapshot),
        1,
        1,
    );
    let second_projection = second
        .project_generation(
            Some(&first_generation),
            &first_projection.history_after,
            second.reviewed_at_ms + 10,
        )
        .unwrap();
    store.register_operation(second.clone()).await.unwrap();
    store
        .commit_transition(transition_from_projected(&second, &second_projection))
        .await
        .unwrap();

    for sequence in 0..2_u32 {
        let now = 300 + u64::from(sequence) * 20;
        let token = format!("claim:authority:root-b:{sequence}");
        let claimed = store
            .claim_next_effect(claim(second.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
            .unwrap();
        store
            .record_effect_observation(observation(
                second.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence + 8, 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
    }

    let claimed = store
        .claim_next_effect(claim(
            second.operation_id(),
            "claim:authority:root-b:cutover",
            350,
            360,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the second graph must reach its Capability Index effect");
    };
    assert_eq!(authority.materializations.len(), 6);
    let historical = authority
        .materializations
        .iter()
        .filter(|materialization| materialization.intent.installation_generation == 1)
        .count();
    let current = authority
        .materializations
        .iter()
        .filter(|materialization| materialization.intent.installation_generation == 2)
        .count();
    assert_eq!((historical, current), (4, 2));
    assert!(authority.materializations.iter().all(|materialization| {
        matches!(
            materialization.state,
            ControlCapabilitySurfaceState::Prepared { .. }
        )
    }));
}

#[tokio::test]
async fn runtime_claim_carries_the_exact_grant_and_reviewed_provider_selection() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = reviewed_grant_operation(
        "operation:authority:runtime",
        PluginOperationAction::Install,
        None,
        None,
    );
    store.register_operation(reviewed.clone()).await.unwrap();
    let committed = store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();

    let claimed = loop {
        let effects = store.effects(reviewed.operation_id()).await.unwrap();
        let sequence = effects
            .iter()
            .find(|effect| {
                effect.status != ControlEffectStatus::Applied
                    && !(effect.status == ControlEffectStatus::Rejected && !effect.intent.required)
            })
            .unwrap()
            .intent
            .sequence;
        let now = 30 + u64::from(sequence) * 20;
        let token = format!("claim:authority:runtime:{sequence}");
        let claimed = store
            .claim_next_effect(claim(reviewed.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
            .unwrap();
        if matches!(
            &claimed.authority,
            ControlEffectAuthority::RuntimeProvider(_)
        ) {
            break claimed;
        }
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence, 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
    };

    let ControlEffectAuthority::RuntimeProvider(authority) = claimed.authority else {
        unreachable!();
    };
    let surface = claimed.intent.subject.surface().unwrap();
    let expected_provider = committed
        .provider_selections
        .iter()
        .find(|selection| {
            selection.package_id() == authority.package.package.package_id()
                && selection.surface() == surface
        })
        .unwrap();
    let expected_grant = committed
        .grants
        .iter()
        .find(|grant| grant.package_id() == authority.package.package.package_id())
        .unwrap();
    let expected_proposal_digest = reviewed
        .authorization
        .grant_transition
        .as_ref()
        .and_then(|transition| {
            transition
                .change_set
                .changes
                .iter()
                .find(|change| change.package_id == expected_grant.package_id())
                .and_then(|change| change.after.as_ref())
        })
        .unwrap()
        .descriptor_digest()
        .unwrap();
    assert_eq!(authority.provider_selection, *expected_provider);
    assert_eq!(authority.package.grant.as_ref(), Some(expected_grant));
    assert_eq!(
        authority.grant_proposal_digest.as_deref(),
        Some(expected_proposal_digest.as_str())
    );
    assert_ne!(
        authority.grant_proposal_digest.as_deref(),
        Some(expected_grant.grant_digest.as_str())
    );
}

#[tokio::test]
async fn claim_fails_closed_when_committed_grant_coverage_is_removed() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = reviewed_grant_operation(
        "operation:authority:missing-grant",
        PluginOperationAction::Install,
        None,
        None,
    );
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let connection = rusqlite::Connection::open(store.database_path()).unwrap();
    connection.execute("DELETE FROM control_grant", []).unwrap();
    drop(connection);

    assert_eq!(
        store
            .claim_next_effect(claim(
                reviewed.operation_id(),
                "claim:authority:missing-grant",
                30,
                40,
                false,
            ))
            .await
            .unwrap_err()
            .code,
        "use.control_store.corrupt"
    );
}

fn transition_from_projected(
    operation: &ReviewedControlOperation,
    projected: &ProjectedControlGeneration,
) -> ControlTransition {
    ControlTransition {
        operation_id: operation.operation_id().to_string(),
        plan_digest: operation.plan_digest().to_string(),
        snapshot: projected.snapshot.clone(),
        package_lifecycles: projected.package_lifecycles.clone(),
        grants: projected.grants.clone(),
        provider_selections: projected.provider_selections.clone(),
        capability: projected.capability.clone(),
        effects: projected.effects.clone(),
        committed_at_ms: operation.reviewed_at_ms + 10,
    }
}
