use a3s_use_core::{PluginManagerToolset, PLUGIN_MANAGER_TOOLSET_SCHEMA_V4};

const TOOLSET: &[u8] = include_bytes!("../fixtures/plugins/manager-toolset-v4.json");
const TOOLSET_DIGEST: &str =
    include_str!("../fixtures/plugins/manager-toolset-v4.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[test]
fn manager_toolset_exposes_only_reviewed_bounded_lifecycle_operations() {
    let toolset = PluginManagerToolset::v4();
    toolset.validate().unwrap();

    let names = toolset
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "plugin_search",
            "plugin_inspect",
            "plugin_list_installed",
            "plugin_status",
            "plugin_plan_install",
            "plugin_plan_upgrade",
            "plugin_plan_uninstall",
            "plugin_apply_plan",
            "plugin_plan_enable",
            "plugin_plan_disable",
        ]
    );
    assert!(!names.contains(&"plugin_enable"));
    assert!(!names.contains(&"plugin_disable"));
    assert!(!names.contains(&"plugin_execute"));

    let apply = toolset.tool("plugin_apply_plan").unwrap();
    assert!(apply.annotations.destructive_hint);
    assert!(apply.annotations.idempotent_hint);
    assert!(!apply.annotations.read_only_hint);
    assert_eq!(
        apply.input_schema["required"],
        serde_json::json!(["operationId", "planDigest"])
    );
    assert_eq!(
        apply.input_schema["properties"].as_object().unwrap().len(),
        2
    );

    let install = toolset.tool("plugin_plan_install").unwrap();
    assert_eq!(
        install.input_schema["properties"]["registryName"],
        serde_json::json!({
            "type": "string",
            "pattern": "^[a-z][a-z0-9-]{0,62}$"
        })
    );
    assert!(
        toolset.tool("plugin_plan_upgrade").unwrap().input_schema["properties"]
            .get("registryName")
            .is_none()
    );

    for name in ["plugin_plan_enable", "plugin_plan_disable"] {
        let tool = toolset.tool(name).unwrap();
        assert!(tool.annotations.read_only_hint);
        assert!(!tool.annotations.destructive_hint);
        assert!(!tool.annotations.idempotent_hint);
    }

    for tool in &toolset.tools {
        let schema = tool.input_schema.to_string().to_ascii_lowercase();
        for forbidden in [
            "command",
            "endpoint",
            "executable",
            "provider",
            "secret",
            "\"path\"",
            "\"url\"",
        ] {
            assert!(
                !schema.contains(forbidden),
                "{} exposes forbidden input authority: {forbidden}",
                tool.name
            );
        }
    }
}

#[test]
fn current_manager_toolset_fixture_is_canonical_and_frozen() {
    let toolset = PluginManagerToolset::from_json(TOOLSET).unwrap();
    assert_eq!(toolset, PluginManagerToolset::v4());
    assert_eq!(toolset.schema, PLUGIN_MANAGER_TOOLSET_SCHEMA_V4);
    assert_eq!(
        toolset.canonical_bytes().unwrap(),
        canonical_fixture(TOOLSET)
    );
    assert_eq!(toolset.descriptor_digest().unwrap(), TOOLSET_DIGEST);

    let mut drift: serde_json::Value = serde_json::from_slice(TOOLSET).unwrap();
    drift["tools"][7]["inputSchema"]["properties"]["url"] = serde_json::json!({"type":"string"});
    let error = PluginManagerToolset::from_json(&serde_json::to_vec(&drift).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.manager_toolset_invalid");

    let mut retired = serde_json::to_value(PluginManagerToolset::v4()).unwrap();
    retired["schema"] = serde_json::json!("a3s.use.plugin-manager-tools.v3");
    let error =
        PluginManagerToolset::from_json(&serde_json::to_vec(&retired).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.manager_toolset_invalid");
}
