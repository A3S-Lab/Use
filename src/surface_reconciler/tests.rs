use super::runtime_observations::reconcile_with_runtime_and_knowledge;
use super::*;
use a3s_use_core::{
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt,
    OkfSelectedGeneration, PlanQualifiedSurfaceRef, OKF_KNOWLEDGE_OBSERVATION_SCHEMA,
    OKF_PROJECTION_RECEIPT_SCHEMA,
};

const NAMED_SURFACE_MANIFEST: &str =
    include_str!("../../crates/extension/fixtures/manifests/plugin-v3.acl");
const OKF_MANIFEST: &str =
    include_str!("../../crates/extension/fixtures/manifests/plugin-v3-okf.acl");
const COGNITIVE_MANIFEST: &str = include_str!(
    "../../crates/extension/fixtures/packages/plugin-v3-cognitive/package/a3s-use-extension.acl"
);

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
fn a3s_flow_sits_between_base_capabilities_and_skill_ui_consumers() {
    let manifest = ExtensionManifest::parse_acl(COGNITIVE_MANIFEST).unwrap();
    let mut observations = SurfaceObservations::from([
        (
            reference(PluginSurfaceKind::Tool, "echo"),
            SurfaceObservedState::Prepared,
        ),
        (
            reference(PluginSurfaceKind::Mcp, "context"),
            SurfaceObservedState::Prepared,
        ),
        (
            reference(PluginSurfaceKind::Okf, "domain"),
            SurfaceObservedState::Healthy,
        ),
        (
            reference(PluginSurfaceKind::Flow, "reason"),
            SurfaceObservedState::Prepared,
        ),
        (
            reference(PluginSurfaceKind::Ui, "reason"),
            SurfaceObservedState::Prepared,
        ),
    ]);
    let ready = reconcile(&manifest, PluginDesiredState::Enabled, true, &observations).unwrap();

    assert_eq!(state(&ready, PluginSurfaceKind::Flow, "reason").level, 1);
    assert_eq!(state(&ready, PluginSurfaceKind::Skill, "reason").level, 2);
    assert_eq!(state(&ready, PluginSurfaceKind::Ui, "reason").level, 3);
    assert_eq!(ready.observed, PluginObservedState::Ready);
    assert!(ready.publishes(PluginSurfaceKind::Flow, "reason"));
    assert!(ready.publishes(PluginSurfaceKind::Skill, "reason"));
    assert!(ready.publishes(PluginSurfaceKind::Ui, "reason"));

    observations.remove(&reference(PluginSurfaceKind::Flow, "reason"));
    let pending = reconcile(&manifest, PluginDesiredState::Enabled, true, &observations).unwrap();
    assert_eq!(
        state(&pending, PluginSurfaceKind::Flow, "reason").reason,
        Some(SurfaceStateReason::FlowObservationMissing)
    );
    assert_eq!(
        state(&pending, PluginSurfaceKind::Skill, "reason").reason,
        Some(SurfaceStateReason::DependencyPending)
    );
    assert!(!pending.capability_ready);
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

#[test]
fn okf_requires_a_promoted_knowledge_observation_before_atomic_publication() {
    let manifest = ExtensionManifest::parse_acl(OKF_MANIFEST).unwrap();
    let pending = reconcile(
        &manifest,
        PluginDesiredState::Enabled,
        true,
        &SurfaceObservations::new(),
    )
    .unwrap();
    let okf = state(&pending, PluginSurfaceKind::Okf, "domain-knowledge");
    assert_eq!(okf.owner, SurfaceOwner::KnowledgeHost);
    assert_eq!(okf.desired, SurfaceDesiredState::Healthy);
    assert_eq!(okf.observed, SurfaceObservedState::Pending);
    assert_eq!(
        okf.reason,
        Some(SurfaceStateReason::KnowledgeObservationMissing)
    );
    assert_eq!(
        state(&pending, PluginSurfaceKind::Skill, "research").reason,
        Some(SurfaceStateReason::DependencyPending)
    );
    assert!(!pending.capability_ready);

    let receipt = knowledge_receipt(&manifest);
    let observation = promoted_observation(&receipt);
    let ready = reconcile_with_runtime_and_knowledge(
        &manifest,
        PluginDesiredState::Enabled,
        true,
        &SurfaceObservations::new(),
        None,
        &[(receipt, observation)],
    )
    .unwrap();
    assert_eq!(ready.observed, PluginObservedState::Ready);
    assert!(ready.publishes(PluginSurfaceKind::Okf, "domain-knowledge"));
    assert!(ready.publishes(PluginSurfaceKind::Skill, "research"));
}

#[test]
fn staged_or_substituted_okf_observations_cannot_publish() {
    let manifest = ExtensionManifest::parse_acl(OKF_MANIFEST).unwrap();
    let receipt = knowledge_receipt(&manifest);
    let mut staged = promoted_observation(&receipt);
    staged.state = OkfKnowledgeObservedState::Staged;
    staged.selected = None;
    let snapshot = reconcile_with_runtime_and_knowledge(
        &manifest,
        PluginDesiredState::Enabled,
        true,
        &SurfaceObservations::new(),
        None,
        &[(receipt.clone(), staged)],
    )
    .unwrap();
    assert_eq!(
        state(&snapshot, PluginSurfaceKind::Okf, "domain-knowledge").observed,
        SurfaceObservedState::Prepared
    );
    assert!(!snapshot.capability_ready);

    let mut substituted = promoted_observation(&receipt);
    substituted.package_digest = format!("sha256:{}", "d".repeat(64));
    assert!(reconcile_with_runtime_and_knowledge(
        &manifest,
        PluginDesiredState::Enabled,
        true,
        &SurfaceObservations::new(),
        None,
        &[(receipt, substituted)],
    )
    .is_err());
}

fn knowledge_receipt(manifest: &ExtensionManifest) -> OkfProjectionReceipt {
    OkfProjectionReceipt {
        schema: OKF_PROJECTION_RECEIPT_SCHEMA.to_owned(),
        operation_id: "install:acme-knowledge:0001".to_owned(),
        scope_id: "workspace:research".to_owned(),
        surface: PlanQualifiedSurfaceRef {
            package_id: manifest.package_id.clone(),
            surface: reference(PluginSurfaceKind::Okf, "domain-knowledge"),
        },
        generation: 13,
        package_digest: format!("sha256:{}", "a".repeat(64)),
        manifest_digest: format!("sha256:{}", "b".repeat(64)),
        bundle: manifest.okf[0].bundle.clone(),
        projection_id: "knowledge:workspace:research:domain-knowledge:13".to_owned(),
        index_schema: "a3s.knowledge.okf-index.v1".to_owned(),
        index_build_id: "knowledge:0.4.0:linux-x86_64".to_owned(),
        staged_at_ms: 1_785_360_100_000,
    }
}

fn promoted_observation(receipt: &OkfProjectionReceipt) -> OkfKnowledgeObservation {
    let receipt_digest = receipt.descriptor_digest().unwrap();
    let index_digest = format!("sha256:{}", "c".repeat(64));
    OkfKnowledgeObservation {
        schema: OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_owned(),
        scope_id: receipt.scope_id.clone(),
        surface: receipt.surface.clone(),
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt_digest.clone(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        state: OkfKnowledgeObservedState::Promoted,
        observed_at_ms: receipt.staged_at_ms + 1,
        index_digest: Some(index_digest.clone()),
        selected: Some(OkfSelectedGeneration {
            generation: receipt.generation,
            package_digest: receipt.package_digest.clone(),
            bundle_digest: receipt.bundle.content_digest.clone(),
            projection_receipt_digest: receipt_digest,
            index_schema: receipt.index_schema.clone(),
            index_build_id: receipt.index_build_id.clone(),
            index_digest,
        }),
    }
}
