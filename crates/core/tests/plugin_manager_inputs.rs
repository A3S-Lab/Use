use a3s_use_core::{
    PlanScopeKind, PluginManagerApplyPlanInput, PluginManagerInspectInput,
    PluginManagerInstallPlanInput, PluginManagerListInstalledInput, PluginManagerPackageScopeInput,
    PluginManagerSearchInput, PluginManagerUpgradePlanInput, PluginSurfaceKind,
};

#[test]
fn manager_inputs_accept_the_frozen_tool_shapes() {
    let search: PluginManagerSearchInput = serde_json::from_value(serde_json::json!({
        "query": "cognitive",
        "kind": "okf",
        "channel": "stable",
        "cursor": "page:2",
        "limit": 20
    }))
    .unwrap();
    search.validate().unwrap();
    assert_eq!(search.kind, Some(PluginSurfaceKind::Okf));
    assert_eq!(search.page_limit(), 20);

    let inspect: PluginManagerInspectInput = serde_json::from_value(serde_json::json!({
        "packageId": "acme/cognitive",
        "version": "1.2.3",
        "channel": "stable"
    }))
    .unwrap();
    inspect.validate().unwrap();

    let list: PluginManagerListInstalledInput = serde_json::from_value(serde_json::json!({
        "scopeKind": "workspace",
        "scopeId": "workspace:alpha"
    }))
    .unwrap();
    list.validate().unwrap();
    assert_eq!(list.scope().kind, PlanScopeKind::Workspace);
    assert_eq!(list.page_limit(), 50);

    let scoped: PluginManagerPackageScopeInput = serde_json::from_value(serde_json::json!({
        "packageId": "acme/cognitive",
        "scopeKind": "user",
        "scopeId": "user:current"
    }))
    .unwrap();
    scoped.validate().unwrap();

    let install: PluginManagerInstallPlanInput = serde_json::from_value(serde_json::json!({
        "packageId": "acme/cognitive",
        "registryName": "packages",
        "versionRequirement": "^1.2",
        "channel": "stable",
        "surfaces": [
            {"kind": "tool", "id": "run"},
            {"kind": "okf", "id": "knowledge"}
        ],
        "scopeKind": "workspace",
        "scopeId": "workspace:alpha"
    }))
    .unwrap();
    install.validate().unwrap();
    assert_eq!(
        install.canonical_version_requirement().as_deref(),
        Some("^1.2")
    );
    let canonical = install.canonical_surfaces();
    assert_eq!(canonical[0].kind, PluginSurfaceKind::Okf);
    assert_eq!(canonical[1].kind, PluginSurfaceKind::Tool);

    let upgrade: PluginManagerUpgradePlanInput = serde_json::from_value(serde_json::json!({
        "packageId": "acme/cognitive",
        "versionRequirement": ">=1.2, <2",
        "scopeKind": "workspace",
        "scopeId": "workspace:alpha"
    }))
    .unwrap();
    upgrade.validate().unwrap();

    let exact: PluginManagerUpgradePlanInput = serde_json::from_value(serde_json::json!({
        "packageId": "acme/cognitive",
        "versionRequirement": "1.2.3",
        "scopeKind": "workspace",
        "scopeId": "workspace:alpha"
    }))
    .unwrap();
    exact.validate().unwrap();
    assert_eq!(
        exact.canonical_version_requirement().as_deref(),
        Some("1.2.3")
    );

    let apply: PluginManagerApplyPlanInput = serde_json::from_value(serde_json::json!({
        "operationId": "operation:install:01",
        "planDigest": format!("sha256:{}", "a".repeat(64))
    }))
    .unwrap();
    apply.validate().unwrap();
}

#[test]
fn manager_inputs_reject_ambiguous_or_unbounded_authority() {
    let duplicate_surfaces: PluginManagerInstallPlanInput =
        serde_json::from_value(serde_json::json!({
            "packageId": "acme/cognitive",
            "surfaces": [
                {"kind": "tool", "id": "run"},
                {"kind": "tool", "id": "run"}
            ],
            "scopeKind": "user",
            "scopeId": "user:current"
        }))
        .unwrap();
    assert_eq!(
        duplicate_surfaces.validate().unwrap_err().code,
        "use.plugin.manager_input_invalid"
    );

    let invalid_requirement: PluginManagerUpgradePlanInput =
        serde_json::from_value(serde_json::json!({
            "packageId": "acme/cognitive",
            "versionRequirement": "latest",
            "scopeKind": "user",
            "scopeId": "user:current"
        }))
        .unwrap();
    assert_eq!(
        invalid_requirement.validate().unwrap_err().code,
        "use.plugin.manager_input_invalid"
    );

    let empty_surfaces: PluginManagerInstallPlanInput = serde_json::from_value(serde_json::json!({
        "packageId": "acme/cognitive",
        "surfaces": [],
        "scopeKind": "user",
        "scopeId": "user:current"
    }))
    .unwrap();
    assert_eq!(
        empty_surfaces.validate().unwrap_err().code,
        "use.plugin.manager_input_invalid"
    );

    assert!(
        serde_json::from_value::<PluginManagerPackageScopeInput>(serde_json::json!({
            "packageId": "acme/cognitive",
            "scopeKind": "user",
            "scopeId": "user:current",
            "path": "/tmp/escape"
        }))
        .is_err()
    );

    let invalid_apply: PluginManagerApplyPlanInput = serde_json::from_value(serde_json::json!({
        "operationId": "../escape",
        "planDigest": format!("sha256:{}", "a".repeat(64))
    }))
    .unwrap();
    assert_eq!(
        invalid_apply.validate().unwrap_err().code,
        "use.plugin.manager_input_invalid"
    );
}
