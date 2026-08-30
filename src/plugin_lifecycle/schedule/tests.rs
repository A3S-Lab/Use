use super::*;
use crate::plugin_lifecycle::test_support::{intent, ALL_SURFACES};
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
fn checkpoint_identity_is_unique_across_packages_in_one_graph_operation() {
    let first_manifest = ExtensionManifest::parse_acl(ALL_SURFACES).unwrap();
    let second_manifest =
        ExtensionManifest::parse_acl(&ALL_SURFACES.replace("acme/research", "acme/analysis"))
            .unwrap();
    let spec = |package_id: &str| PluginLifecycleIntentSpec {
        operation_id: "operation:shared-graph:1".to_string(),
        plan_digest: format!("sha256:{}", "1".repeat(64)),
        scope: a3s_use_core::PlanScope {
            kind: a3s_use_core::PlanScopeKind::Workspace,
            id: "research".to_string(),
        },
        package_id: package_id.to_string(),
        package_digest: format!("sha256:{}", "2".repeat(64)),
        manifest_digest: format!("sha256:{}", "3".repeat(64)),
        generation: 7,
        action: PluginLifecycleAction::Install,
        retained_ui_state_surfaces: Vec::new(),
    };
    let first =
        PluginLifecycleIntent::from_manifest(spec("acme/research"), &first_manifest).unwrap();
    let second =
        PluginLifecycleIntent::from_manifest(spec("acme/analysis"), &second_manifest).unwrap();

    assert_eq!(first.checkpoints.len(), second.checkpoints.len());
    for (first, second) in first.checkpoints.iter().zip(&second.checkpoints) {
        assert_ne!(first.idempotency_key, second.idempotency_key);
    }
}

#[test]
fn checkpoint_identity_is_unique_across_installation_kinds_with_the_same_id() {
    let manifest = ExtensionManifest::parse_acl(ALL_SURFACES).unwrap();
    let spec = |kind| PluginLifecycleIntentSpec {
        operation_id: "operation:shared-scope:1".to_string(),
        plan_digest: format!("sha256:{}", "1".repeat(64)),
        scope: a3s_use_core::PlanScope {
            kind,
            id: "same/id".to_string(),
        },
        package_id: "acme/research".to_string(),
        package_digest: format!("sha256:{}", "2".repeat(64)),
        manifest_digest: format!("sha256:{}", "3".repeat(64)),
        generation: 7,
        action: PluginLifecycleAction::Install,
        retained_ui_state_surfaces: Vec::new(),
    };
    let user =
        PluginLifecycleIntent::from_manifest(spec(a3s_use_core::PlanScopeKind::User), &manifest)
            .unwrap();
    let workspace = PluginLifecycleIntent::from_manifest(
        spec(a3s_use_core::PlanScopeKind::Workspace),
        &manifest,
    )
    .unwrap();

    for (user, workspace) in user.checkpoints.iter().zip(&workspace.checkpoints) {
        assert_ne!(user.idempotency_key, workspace.idempotency_key);
    }
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

#[test]
fn replacement_retirement_binds_only_known_sorted_ui_state_surfaces() {
    let mut retirement = intent(PluginLifecycleAction::Uninstall);
    retirement.retained_ui_state_surfaces = vec!["review".to_string()];
    retirement.validate().unwrap();

    retirement.retained_ui_state_surfaces = vec!["missing".to_string()];
    assert_eq!(
        retirement.validate().unwrap_err().code,
        "use.plugin.lifecycle_invalid"
    );

    let mut install = intent(PluginLifecycleAction::Install);
    install.retained_ui_state_surfaces = vec!["review".to_string()];
    assert_eq!(
        install.validate().unwrap_err().code,
        "use.plugin.lifecycle_invalid"
    );
}

#[test]
fn lifecycle_selection_excludes_one_independent_optional_surface() {
    let manifest = ExtensionManifest::parse_acl(&ALL_SURFACES.replace(
        "\n  mcp \"catalog\" {",
        r#"
  tool "optional-export" {
    workload   = "task"
    interface  = "cli"
    executable = "bin/export"
    command    = "export"
    optional   = true
  }

  mcp "catalog" {"#,
    ))
    .unwrap();
    let selected = manifest
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface)
        .filter(|surface| surface.id != "optional-export")
        .collect::<Vec<_>>();
    let base = intent(PluginLifecycleAction::Install);
    let selected = PluginLifecycleIntent::from_manifest_selection(
        PluginLifecycleIntentSpec {
            operation_id: base.operation_id,
            plan_digest: base.plan_digest,
            scope: base.scope,
            package_id: base.package_id,
            package_digest: base.package_digest,
            manifest_digest: base.manifest_digest,
            generation: base.generation,
            action: base.action,
            retained_ui_state_surfaces: Vec::new(),
        },
        &manifest,
        &selected,
    )
    .unwrap();

    assert_eq!(selected.surfaces.len(), 6);
    assert!(selected
        .surfaces
        .iter()
        .all(|surface| surface.surface.id != "optional-export"));
}
