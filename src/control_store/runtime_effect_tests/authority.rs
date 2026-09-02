use super::*;

#[tokio::test]
async fn runtime_owner_rejects_split_package_authority_before_artifact_or_provider_access() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    let owner = owner(&fixture);
    let mut request = request(&fixture, ControlSurfaceEffectAction::Prepare);
    request.authority.package.snapshot_digest = digest('9');

    let outcome = ControlRuntimeEffectPort::apply_surface(&owner, &request).await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("split committed package authority must be rejected");
    };
    assert_eq!(
        failure.error_code,
        "use.control_store.runtime_authority_invalid"
    );
    assert_eq!(fixture.runtime.apply_count(), 0);
}

#[tokio::test]
async fn runtime_owner_rejects_tampered_artifact_before_provider_apply() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    std::fs::write(
        fixture.package_root.join("releases/index-tool-v1.json"),
        b"tampered before Runtime preparation",
    )
    .unwrap();
    let owner = owner(&fixture);

    let outcome = ControlRuntimeEffectPort::apply_surface(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("tampered Artifact Store bytes must be rejected before provider apply");
    };
    assert_eq!(
        failure.error_code,
        "use.extension.release_descriptor_invalid"
    );
    assert_eq!(fixture.runtime.apply_count(), 0);
    assert_eq!(fixture.readiness.tool_bind_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_owner_defers_artifact_contention_without_provider_effects() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    let collection = fixture.store.acquire_collection().await.unwrap();
    let owner = owner(&fixture);

    let outcome = ControlRuntimeEffectPort::apply_surface(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    drop(collection);
    let ControlEffectPortOutcome::Deferred(failure) = outcome else {
        panic!("Artifact Store contention must be a proved no-effect deferral");
    };
    assert_eq!(failure.error_code, "use.artifact_store.busy");
    assert_eq!(fixture.runtime.apply_count(), 0);
}

#[tokio::test]
async fn runtime_owner_refuses_to_checkpoint_stop_while_provisioning_is_pending() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    let selected = &fixture.selection.surfaces()[0];
    let prepare = request(&fixture, ControlSurfaceEffectAction::Prepare);
    let pending = RuntimeServiceProvisioningReceipt::from_plan(
        selected.plan(),
        selected.provider(),
        prepare.surface.identity.idempotency_key,
        "control-runtime-apply-test",
    )
    .unwrap();
    fixture.bindings.put_provisioning(&pending).await.unwrap();
    let owner = owner(&fixture);

    let outcome = ControlRuntimeEffectPort::apply_surface(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Stop),
    )
    .await;
    let ControlEffectPortOutcome::Unknown(failure) = outcome else {
        panic!("a retained provisioning record must prevent a false stopped checkpoint");
    };
    assert_eq!(
        failure.error_code,
        "use.control_store.runtime_pending_recovery"
    );
    assert_eq!(fixture.runtime.stop_count(), 0);
    assert_eq!(fixture.readiness.drain_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_owner_rejects_conflicting_pending_authority_before_removal() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    let selected = &fixture.selection.surfaces()[0];
    let prepare = request(&fixture, ControlSurfaceEffectAction::Prepare);
    let mut pending = RuntimeServiceProvisioningReceipt::from_plan(
        selected.plan(),
        selected.provider(),
        prepare.surface.identity.idempotency_key,
        "control-runtime-apply-test",
    )
    .unwrap();
    pending.grant_digest = digest('9');
    fixture.bindings.put_provisioning(&pending).await.unwrap();
    let owner = owner(&fixture);

    let outcome = ControlRuntimeEffectPort::apply_surface(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Remove),
    )
    .await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("conflicting pending Runtime authority must not be removed");
    };
    assert_eq!(
        failure.error_code,
        "use.control_store.runtime_authority_invalid"
    );
    assert_eq!(fixture.runtime.remove_count(), 0);
    assert_eq!(fixture.readiness.remove_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_owner_never_expands_the_committed_claim_deadline() {
    let fixture = runtime_owner_fixture(FixtureSurface::ToolService).await;
    let owner = owner(&fixture).with_deadline_at_ms(Some(30_000)).unwrap();

    applied(
        &owner,
        &request(&fixture, ControlSurfaceEffectAction::Prepare),
    )
    .await;

    assert_eq!(
        fixture.readiness.last_deadline_at_ms.load(Ordering::SeqCst),
        20_000
    );
}
