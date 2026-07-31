use super::*;

const NAMED_SURFACE_MANIFEST: &str =
    include_str!("../../crates/extension/fixtures/manifests/plugin-v3.acl");

fn manifest() -> ExtensionManifest {
    ExtensionManifest::parse_acl(NAMED_SURFACE_MANIFEST).unwrap()
}

fn reference(kind: PluginSurfaceKind, id: &str) -> PluginSurfaceRef {
    surface_ref(kind, id)
}

fn state<'a>(
    snapshot: &'a SurfaceReconcileSnapshot,
    kind: PluginSurfaceKind,
    id: &str,
) -> &'a ReconciledSurface {
    snapshot
        .surfaces
        .iter()
        .find(|surface| surface.surface == reference(kind, id))
        .unwrap()
}

#[test]
fn named_surface_graph_has_deterministic_dependency_levels_and_required_closure() {
    let snapshot = reconcile(
        &manifest(),
        PluginDesiredState::Enabled,
        true,
        &SurfaceObservations::new(),
    )
    .unwrap();

    for (kind, id) in [
        (PluginSurfaceKind::Tool, "convert"),
        (PluginSurfaceKind::Tool, "index"),
        (PluginSurfaceKind::Mcp, "local-library"),
        (PluginSurfaceKind::Mcp, "library"),
    ] {
        assert_eq!(state(&snapshot, kind, id).level, 0);
    }
    assert_eq!(
        state(&snapshot, PluginSurfaceKind::Skill, "review").level,
        1
    );
    assert_eq!(
        state(&snapshot, PluginSurfaceKind::Skill, "quick-look").level,
        1
    );
    assert_eq!(state(&snapshot, PluginSurfaceKind::Ui, "status").level, 1);
    assert_eq!(state(&snapshot, PluginSurfaceKind::Ui, "review").level, 2);
    for (kind, id) in [
        (PluginSurfaceKind::Tool, "convert"),
        (PluginSurfaceKind::Tool, "index"),
        (PluginSurfaceKind::Mcp, "library"),
        (PluginSurfaceKind::Skill, "review"),
        (PluginSurfaceKind::Ui, "review"),
    ] {
        assert!(state(&snapshot, kind, id).required, "{kind:?}:{id}");
    }
    for (kind, id) in [
        (PluginSurfaceKind::Mcp, "local-library"),
        (PluginSurfaceKind::Skill, "quick-look"),
        (PluginSurfaceKind::Ui, "status"),
    ] {
        assert!(!state(&snapshot, kind, id).required, "{kind:?}:{id}");
    }
    assert_eq!(snapshot.observed, PluginObservedState::Reconciling);
    assert!(!snapshot.capability_ready);
    assert!(snapshot.surfaces.iter().all(|surface| !surface.published));
}

#[test]
fn required_readiness_publishes_atomically_and_optional_gaps_are_degraded() {
    let mut observations = SurfaceObservations::from([
        (
            reference(PluginSurfaceKind::Tool, "convert"),
            SurfaceObservedState::Prepared,
        ),
        (
            reference(PluginSurfaceKind::Tool, "index"),
            SurfaceObservedState::Healthy,
        ),
        (
            reference(PluginSurfaceKind::Mcp, "library"),
            SurfaceObservedState::Healthy,
        ),
        (
            reference(PluginSurfaceKind::Ui, "review"),
            SurfaceObservedState::Prepared,
        ),
        (
            reference(PluginSurfaceKind::Mcp, "local-library"),
            SurfaceObservedState::Failed,
        ),
    ]);
    let degraded = reconcile(
        &manifest(),
        PluginDesiredState::Enabled,
        true,
        &observations,
    )
    .unwrap();

    assert_eq!(degraded.observed, PluginObservedState::Degraded);
    assert!(degraded.capability_ready);
    assert!(degraded.publishes(PluginSurfaceKind::Skill, "review"));
    assert!(degraded.publishes(PluginSurfaceKind::Skill, "quick-look"));
    assert!(!degraded.publishes(PluginSurfaceKind::Mcp, "local-library"));
    assert!(!degraded.publishes(PluginSurfaceKind::Ui, "status"));

    observations.insert(
        reference(PluginSurfaceKind::Mcp, "local-library"),
        SurfaceObservedState::Prepared,
    );
    observations.insert(
        reference(PluginSurfaceKind::Ui, "status"),
        SurfaceObservedState::Prepared,
    );
    let ready = reconcile(
        &manifest(),
        PluginDesiredState::Enabled,
        true,
        &observations,
    )
    .unwrap();

    assert_eq!(ready.observed, PluginObservedState::Ready);
    assert!(ready.capability_ready);
    assert!(ready.surfaces.iter().all(|surface| surface.published));
}

#[test]
fn required_failure_blocks_dependents_and_the_capability_generation() {
    let observations = SurfaceObservations::from([
        (
            reference(PluginSurfaceKind::Tool, "convert"),
            SurfaceObservedState::Failed,
        ),
        (
            reference(PluginSurfaceKind::Tool, "index"),
            SurfaceObservedState::Healthy,
        ),
        (
            reference(PluginSurfaceKind::Mcp, "library"),
            SurfaceObservedState::Healthy,
        ),
        (
            reference(PluginSurfaceKind::Ui, "review"),
            SurfaceObservedState::Prepared,
        ),
    ]);
    let snapshot = reconcile(
        &manifest(),
        PluginDesiredState::Enabled,
        true,
        &observations,
    )
    .unwrap();

    assert_eq!(snapshot.observed, PluginObservedState::Broken);
    assert!(!snapshot.capability_ready);
    assert_eq!(
        state(&snapshot, PluginSurfaceKind::Skill, "review").reason,
        Some(SurfaceStateReason::DependencyFailed)
    );
    assert_eq!(
        state(&snapshot, PluginSurfaceKind::Ui, "review").reason,
        Some(SurfaceStateReason::DependencyFailed)
    );
}

#[test]
fn disabled_and_absent_packages_converge_without_publishing_surfaces() {
    let disabled = reconcile(
        &manifest(),
        PluginDesiredState::InstalledDisabled,
        true,
        &SurfaceObservations::new(),
    )
    .unwrap();
    assert_eq!(disabled.observed, PluginObservedState::Installed);
    assert!(disabled
        .surfaces
        .iter()
        .all(|surface| surface.desired == SurfaceDesiredState::Stopped
            && surface.observed == SurfaceObservedState::Stopped
            && !surface.published));

    let removed = reconcile(
        &manifest(),
        PluginDesiredState::Absent,
        true,
        &SurfaceObservations::new(),
    )
    .unwrap();
    assert_eq!(removed.observed, PluginObservedState::Removed);
    assert!(!removed.capability_ready);
}

#[test]
fn incompatible_host_and_unknown_observations_fail_closed() {
    let incompatible = reconcile(
        &manifest(),
        PluginDesiredState::Enabled,
        false,
        &SurfaceObservations::new(),
    )
    .unwrap();
    assert_eq!(incompatible.observed, PluginObservedState::Incompatible);
    assert!(incompatible
        .surfaces
        .iter()
        .all(|surface| surface.observed == SurfaceObservedState::Failed));

    let observations = SurfaceObservations::from([(
        reference(PluginSurfaceKind::Tool, "unknown"),
        SurfaceObservedState::Healthy,
    )]);
    let error = reconcile(
        &manifest(),
        PluginDesiredState::Enabled,
        true,
        &observations,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.reconcile_invalid");
}

#[test]
fn reconciler_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SurfaceReconcileSnapshot>();
    assert_send_sync::<SurfaceObservations>();
}
