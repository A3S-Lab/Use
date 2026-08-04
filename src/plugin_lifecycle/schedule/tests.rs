use super::*;
use crate::plugin_lifecycle::test_support::intent;
use a3s_use_core::{PluginSurfaceKind, PluginSurfaceRef};

fn surface(kind: PluginSurfaceKind, id: &str) -> PluginSurfaceRef {
    PluginSurfaceRef {
        kind,
        id: id.to_string(),
    }
}

#[test]
fn one_package_orders_all_six_surface_kinds_and_required_closure() {
    let intent = intent(PluginLifecycleAction::Install);
    assert_eq!(intent.surfaces.len(), 6);
    assert_eq!(
        intent
            .surfaces
            .iter()
            .map(|surface| (surface.surface.clone(), surface.level, surface.required))
            .collect::<Vec<_>>(),
        vec![
            (surface(PluginSurfaceKind::Mcp, "catalog"), 0, true),
            (surface(PluginSurfaceKind::Okf, "papers"), 0, true),
            (surface(PluginSurfaceKind::Tool, "query"), 0, true),
            (surface(PluginSurfaceKind::Flow, "review"), 1, true),
            (surface(PluginSurfaceKind::Skill, "review"), 2, true),
            (surface(PluginSurfaceKind::Ui, "review"), 3, true),
        ]
    );
    assert_eq!(
        intent.checkpoints.first().unwrap().kind,
        PluginLifecycleCheckpointKind::PackageCommitted
    );
    assert_eq!(
        intent.checkpoints.last().unwrap().kind,
        PluginLifecycleCheckpointKind::CapabilityPublished
    );
    assert!(intent
        .checkpoints
        .iter()
        .all(|checkpoint| checkpoint.required));
    assert_eq!(intent.descriptor_digest().unwrap().len(), 71);
}

#[test]
fn uninstall_hides_and_drains_before_reverse_dependency_removal() {
    let intent = intent(PluginLifecycleAction::Uninstall);
    let kinds = intent
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds[0], PluginLifecycleCheckpointKind::CapabilityHidden);
    assert_eq!(kinds[1], PluginLifecycleCheckpointKind::CallsDrained);
    assert_eq!(
        intent.checkpoints[2..8]
            .iter()
            .map(|checkpoint| checkpoint.surface.clone().unwrap())
            .collect::<Vec<_>>(),
        vec![
            surface(PluginSurfaceKind::Ui, "review"),
            surface(PluginSurfaceKind::Skill, "review"),
            surface(PluginSurfaceKind::Flow, "review"),
            surface(PluginSurfaceKind::Tool, "query"),
            surface(PluginSurfaceKind::Okf, "papers"),
            surface(PluginSurfaceKind::Mcp, "catalog"),
        ]
    );
    assert_eq!(
        intent.checkpoints.last().unwrap().kind,
        PluginLifecycleCheckpointKind::PackageRemoved
    );
}

#[test]
fn lifecycle_intent_rejects_checkpoint_drift() {
    let mut intent = intent(PluginLifecycleAction::Enable);
    intent.checkpoints.swap(0, 1);
    let error = intent.validate().unwrap_err();
    assert_eq!(error.code, "use.plugin.lifecycle_invalid");
}
