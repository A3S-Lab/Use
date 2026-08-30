use super::*;

#[tokio::test]
async fn root_lifecycle_actions_form_one_consecutive_capability_history() {
    let (_temporary, store) = initialized_store().await;

    let install = operation_at(
        "operation:lifecycle:install",
        PluginOperationAction::Install,
        0,
        0,
    );
    store.register_operation(install.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &install))
        .await
        .unwrap();
    apply_all_effects(&store, &install, 30).await;

    let disable = operation_at(
        "operation:lifecycle:disable",
        PluginOperationAction::Disable,
        1,
        1,
    );
    store.register_operation(disable.clone()).await.unwrap();
    let mut disabled = transition(control_installation(), &disable);
    disabled.snapshot.packages[0].enabled = false;
    disabled.grants.clear();
    disabled.bindings.clear();
    let package_subject = disabled.effects[0].subject.clone();
    disabled.effects[0].kind = ControlEffectKind::CapabilityHide;
    disabled.effects[0].subject = capability_subject(&disable);
    disabled.effects[1].kind = ControlEffectKind::GrantRevoke;
    disabled.effects[1].subject = package_subject;
    store.commit_transition(disabled).await.unwrap();
    apply_all_effects(&store, &disable, 230).await;

    let enable = operation_at(
        "operation:lifecycle:enable",
        PluginOperationAction::Enable,
        2,
        2,
    );
    store.register_operation(enable.clone()).await.unwrap();
    let mut enabled = transition(control_installation(), &enable);
    enabled.effects[0].kind = ControlEffectKind::GrantApply;
    enabled.effects[1].kind = ControlEffectKind::CapabilityPublish;
    store.commit_transition(enabled).await.unwrap();
    apply_all_effects(&store, &enable, 430).await;

    let uninstall = operation_at(
        "operation:lifecycle:uninstall",
        PluginOperationAction::Uninstall,
        3,
        3,
    );
    store.register_operation(uninstall.clone()).await.unwrap();
    let mut removed = transition(control_installation(), &uninstall);
    removed.snapshot = InstallationSnapshot::from_root_locks(
        control_installation(),
        4,
        removed.snapshot.host.clone(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    removed.package_lifecycles.clear();
    removed.grants.clear();
    removed.bindings.clear();
    let package_subject = removed.effects[0].subject.clone();
    removed.effects[0].kind = ControlEffectKind::CapabilityHide;
    removed.effects[0].subject = capability_subject(&uninstall);
    removed.effects[1].kind = ControlEffectKind::PackageRemove;
    removed.effects[1].installation_generation = 3;
    removed.effects[1].subject = package_subject;
    store.commit_transition(removed).await.unwrap();
    apply_all_effects(&store, &uninstall, 630).await;

    let inspection = store.inspect().await.unwrap();
    assert_eq!(inspection.metadata.current_generation, 4);
    assert_eq!(inspection.metadata.published_capability_generation, 4);
    let export = store
        .verify_export(store.export().await.unwrap())
        .await
        .unwrap();
    assert_eq!(export.export.authority.generations.len(), 4);
    assert_eq!(
        export.export.authority.generations[..3]
            .iter()
            .map(|generation| generation.package_lifecycles[0].lifecycle_generation)
            .collect::<Vec<_>>(),
        vec![41, 41, 41]
    );
    assert_eq!(
        export.export.authority.generations[..3]
            .iter()
            .map(|generation| generation.snapshot.packages[0].state_generation)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(export.export.authority.generations[3]
        .package_lifecycles
        .is_empty());
    assert!(export.export.authority.generations[..3]
        .iter()
        .all(|generation| generation.capability_status == ControlCapabilityStatus::Retired));
    assert_eq!(
        export.export.authority.generations[3].capability_status,
        ControlCapabilityStatus::Published
    );
    assert!(export.export.authority.generations[3]
        .snapshot
        .roots
        .is_empty());
    let uninstall_effects = export
        .export
        .authority
        .effects
        .iter()
        .filter(|effect| effect.operation_id == uninstall.operation_id)
        .collect::<Vec<_>>();
    assert_eq!(uninstall_effects.len(), 2);
    assert_eq!(uninstall_effects[0].intent.installation_generation, 4);
    assert!(matches!(
        &uninstall_effects[0].intent.subject,
        ControlEffectSubject::Installation { .. }
    ));
    assert_eq!(uninstall_effects[1].intent.installation_generation, 3);
    assert!(matches!(
        &uninstall_effects[1].intent.subject,
        ControlEffectSubject::Package { .. }
    ));
}

#[tokio::test]
async fn invalid_effect_generation_references_roll_back_the_whole_transition() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:invalid-reference:1");
    store.register_operation(reviewed.clone()).await.unwrap();
    let mut candidate = transition(control_installation(), &reviewed);
    candidate.effects[0].installation_generation = 99;

    let error = store.commit_transition(candidate).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.input_invalid");

    let mut wrong_action = transition(control_installation(), &reviewed);
    if let ControlEffectSubject::Package { action, .. } = &mut wrong_action.effects[0].subject {
        *action = PluginLifecycleAction::Upgrade;
    } else {
        panic!("package commit must retain a package subject");
    }
    assert_eq!(
        store
            .commit_transition(wrong_action)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let mut wrong_lifecycle = transition(control_installation(), &reviewed);
    if let ControlEffectSubject::Package {
        lifecycle_generation,
        ..
    } = &mut wrong_lifecycle.effects[0].subject
    {
        *lifecycle_generation = 42;
    } else {
        panic!("package commit must retain a package subject");
    }
    assert_eq!(
        store
            .commit_transition(wrong_lifecycle)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let mut wrong_capability = transition(control_installation(), &reviewed);
    if let ControlEffectSubject::Installation {
        descriptor_digest, ..
    } = &mut wrong_capability.effects[1].subject
    {
        *descriptor_digest = digest('e');
    } else {
        panic!("capability publication must retain an installation subject");
    }
    assert_eq!(
        store
            .commit_transition(wrong_capability)
            .await
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );
    assert_eq!(
        store.inspect().await.unwrap().metadata.current_generation,
        0
    );
    assert!(store.current_generation().await.unwrap().is_none());
    assert_eq!(
        store
            .operation(&reviewed.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::Reviewed
    );
    assert!(store
        .effects(&reviewed.operation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn package_lifecycle_generation_is_independent_of_installation_and_state_generations() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:independent-package-generation:1");
    store.register_operation(reviewed.clone()).await.unwrap();

    let mut candidate = transition(control_installation(), &reviewed);
    candidate.snapshot.packages[0].state_generation = 7;
    let committed = store.commit_transition(candidate).await.unwrap();
    assert_eq!(committed.snapshot.generation, 1);
    assert_eq!(committed.snapshot.packages[0].state_generation, 7);
    assert!(store
        .effects(&reviewed.operation_id)
        .await
        .unwrap()
        .iter()
        .all(|effect| {
            effect.intent.installation_generation == 1
                && effect
                    .intent
                    .subject
                    .package_identity()
                    .is_none_or(|(_, generation)| generation == 41)
        }));
}
