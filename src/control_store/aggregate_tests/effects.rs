use super::effect_fixtures::*;
use super::grant_fixtures::reviewed_grant_operation;
use super::*;

#[test]
fn effect_projection_encodes_every_lifecycle_boundary_in_dependency_order() {
    let empty_history = ControlProjectionHistory::default();
    let install = operation_at(
        "operation:effects:inventory-install",
        PluginOperationAction::Install,
        0,
        0,
    );
    let installed = install
        .project_generation(None, &empty_history, install.reviewed_at_ms + 10)
        .unwrap();
    let installed_again = install
        .project_generation(None, &empty_history, install.reviewed_at_ms + 10)
        .unwrap();
    assert_eq!(installed.effects, installed_again.effects);
    assert_effects(
        &installed.effects,
        &[
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "knowledge-host",
                "domain-knowledge",
                1,
                PluginLifecycleAction::Install,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "skill-host",
                "research",
                1,
                PluginLifecycleAction::Install,
                true,
            ),
            expected_installation(ControlEffectKind::CapabilityCutover, 1),
        ],
    );

    let installed_generation = generation(&install, &installed);
    let upgrade = operation_at(
        "operation:effects:inventory-upgrade",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    let upgraded = upgrade
        .project_generation(
            Some(&installed_generation),
            &installed.history_after,
            upgrade.reviewed_at_ms + 10,
        )
        .unwrap();
    assert_effects(
        &upgraded.effects,
        &[
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "knowledge-host",
                "domain-knowledge",
                2,
                PluginLifecycleAction::Upgrade,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "skill-host",
                "research",
                2,
                PluginLifecycleAction::Upgrade,
                true,
            ),
            expected_installation(ControlEffectKind::CapabilityCutover, 2),
            expected_package(
                ControlEffectKind::CallsDrain,
                "invocation-leases",
                1,
                PluginLifecycleAction::Uninstall,
            ),
            expected_surface(
                ControlEffectKind::SurfaceRemove,
                "skill-host",
                "research",
                1,
                PluginLifecycleAction::Uninstall,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfaceRemove,
                "knowledge-host",
                "domain-knowledge",
                1,
                PluginLifecycleAction::Uninstall,
                true,
            ),
        ],
    );

    let install = operation_at(
        "operation:effects:enablement-install",
        PluginOperationAction::Install,
        0,
        0,
    );
    let installed = install
        .project_generation(None, &empty_history, install.reviewed_at_ms + 10)
        .unwrap();
    let installed_generation = generation(&install, &installed);
    let disable = operation_at(
        "operation:effects:inventory-disable",
        PluginOperationAction::Disable,
        1,
        1,
    );
    let disabled = disable
        .project_generation(
            Some(&installed_generation),
            &installed.history_after,
            disable.reviewed_at_ms + 10,
        )
        .unwrap();
    assert_effects(
        &disabled.effects,
        &[
            expected_installation(ControlEffectKind::CapabilityCutover, 2),
            expected_package(
                ControlEffectKind::CallsDrain,
                "invocation-leases",
                1,
                PluginLifecycleAction::Disable,
            ),
            expected_surface(
                ControlEffectKind::SurfaceStop,
                "skill-host",
                "research",
                1,
                PluginLifecycleAction::Disable,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfaceStop,
                "knowledge-host",
                "domain-knowledge",
                1,
                PluginLifecycleAction::Disable,
                true,
            ),
        ],
    );

    let disabled_generation = generation(&disable, &disabled);
    let enable = operation_at(
        "operation:effects:inventory-enable",
        PluginOperationAction::Enable,
        2,
        2,
    );
    let enabled = enable
        .project_generation(
            Some(&disabled_generation),
            &disabled.history_after,
            enable.reviewed_at_ms + 10,
        )
        .unwrap();
    assert_effects(
        &enabled.effects,
        &[
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "knowledge-host",
                "domain-knowledge",
                3,
                PluginLifecycleAction::Enable,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "skill-host",
                "research",
                3,
                PluginLifecycleAction::Enable,
                true,
            ),
            expected_installation(ControlEffectKind::CapabilityCutover, 3),
        ],
    );

    let enabled_generation = generation(&enable, &enabled);
    let uninstall = operation_at(
        "operation:effects:inventory-uninstall",
        PluginOperationAction::Uninstall,
        3,
        3,
    );
    let removed = uninstall
        .project_generation(
            Some(&enabled_generation),
            &enabled.history_after,
            uninstall.reviewed_at_ms + 10,
        )
        .unwrap();
    assert_effects(
        &removed.effects,
        &[
            expected_installation(ControlEffectKind::CapabilityCutover, 4),
            expected_package(
                ControlEffectKind::CallsDrain,
                "invocation-leases",
                3,
                PluginLifecycleAction::Uninstall,
            ),
            expected_surface(
                ControlEffectKind::SurfaceRemove,
                "skill-host",
                "research",
                3,
                PluginLifecycleAction::Uninstall,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfaceRemove,
                "knowledge-host",
                "domain-knowledge",
                3,
                PluginLifecycleAction::Uninstall,
                true,
            ),
        ],
    );
}

#[test]
fn executable_effects_bind_exact_reviewed_runtime_selections_only() {
    let reviewed = reviewed_grant_operation(
        "operation:effects:runtime-owner",
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

    let mut runtime_count = 0;
    let mut static_count = 0;
    for effect in projected
        .effects
        .iter()
        .filter(|effect| effect.kind == ControlEffectKind::SurfacePrepare)
    {
        let ControlEffectSubject::Surface {
            package_id,
            surface,
            ..
        } = &effect.subject
        else {
            panic!("surface preparation must have a surface subject");
        };
        match surface.kind {
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp => {
                runtime_count += 1;
                let selection = projected
                    .provider_selections
                    .iter()
                    .find(|selection| {
                        selection.package_id() == package_id && selection.surface() == surface
                    })
                    .unwrap();
                assert_eq!(
                    effect.owner,
                    ControlEffectOwner::RuntimeProvider {
                        provider_id: selection.evidence.provider_id.clone(),
                        selection_digest: selection.selection_digest.clone(),
                    }
                );
            }
            PluginSurfaceKind::Skill => {
                static_count += 1;
                assert_eq!(effect.owner, ControlEffectOwner::SkillHost);
            }
            PluginSurfaceKind::Ui => {
                static_count += 1;
                assert_eq!(effect.owner, ControlEffectOwner::UiHost);
            }
            PluginSurfaceKind::Flow | PluginSurfaceKind::Okf => {
                panic!("the permissioned fixture has no Flow or OKF surface")
            }
        }
    }
    assert_eq!(runtime_count, 3);
    assert_eq!(static_count, 2);
    assert_eq!(projected.effects.len(), 6);
}

#[tokio::test]
async fn caller_cannot_choose_any_effect_identity_or_policy_field() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:effects:caller-authority");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);

    let mut mutations = Vec::new();

    let mut sequence = candidate.clone();
    sequence.effects[0].sequence = 7;
    rekey(&mut sequence.effects[0]);
    mutations.push(sequence);

    let mut required = candidate.clone();
    required.effects[0].required = false;
    rekey(&mut required.effects[0]);
    mutations.push(required);

    let mut owner = candidate.clone();
    owner.effects[0].owner = ControlEffectOwner::SkillHost;
    rekey(&mut owner.effects[0]);
    mutations.push(owner);

    let mut kind = candidate.clone();
    kind.effects[0].kind = ControlEffectKind::SurfaceRemove;
    rekey(&mut kind.effects[0]);
    mutations.push(kind);

    let mut key = candidate;
    key.effects[0].idempotency_key = digest('4');
    mutations.push(key);

    for mutation in mutations {
        assert_eq!(
            store.commit_transition(mutation).await.unwrap_err().code,
            "use.control_store.input_invalid"
        );
    }
    assert!(store.current_generation().await.unwrap().is_none());
    assert_eq!(
        store
            .operation(reviewed.operation_id())
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::Reviewed
    );
}

#[tokio::test]
async fn explicitly_selected_optional_surface_can_degrade_without_blocking_cutover() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = optional_surface_operation("operation:effects:optional");
    store.register_operation(reviewed.clone()).await.unwrap();
    let candidate = transition(control_installation(), &reviewed);
    assert_effects(
        &candidate.effects,
        &[
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "knowledge-host",
                "domain-knowledge",
                1,
                PluginLifecycleAction::Install,
                true,
            ),
            expected_surface(
                ControlEffectKind::SurfacePrepare,
                "skill-host",
                "research",
                1,
                PluginLifecycleAction::Install,
                false,
            ),
            expected_installation(ControlEffectKind::CapabilityCutover, 1),
        ],
    );
    store.commit_transition(candidate).await.unwrap();

    for (sequence, outcome) in [
        ControlEffectOutcome::Applied,
        ControlEffectOutcome::Rejected,
        ControlEffectOutcome::Applied,
    ]
    .into_iter()
    .enumerate()
    {
        let now = 30 + u64::try_from(sequence).unwrap() * 20;
        let token = format!("claim:optional:{sequence}");
        let claimed = store
            .claim_next_effect(claim(reviewed.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(usize::try_from(claimed.intent.sequence).unwrap(), sequence);
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent.idempotency_key,
                &claimed.claim_token,
                outcome,
                char::from_digit(u32::try_from(sequence).unwrap(), 16).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
        assert_eq!(
            store
                .operation(reviewed.operation_id())
                .await
                .unwrap()
                .unwrap()
                .status,
            ControlOperationStatus::EffectsPending
        );
    }

    let completed = store
        .complete_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('8'),
            100,
        )
        .await
        .unwrap();
    assert_eq!(completed.status, ControlOperationStatus::Completed);
    assert_eq!(
        store
            .inspect()
            .await
            .unwrap()
            .metadata
            .published_capability_generation,
        1
    );
}
