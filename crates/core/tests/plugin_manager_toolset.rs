use a3s_use_core::PluginManagerToolset;

const TOOLSET: &[u8] = include_bytes!("../fixtures/plugins/manager-toolset-v1.json");
const TOOLSET_DIGEST: &str =
    include_str!("../fixtures/plugins/manager-toolset-v1.sha256").trim_ascii_end();
const TOOLSET_V2: &[u8] = include_bytes!("../fixtures/plugins/manager-toolset-v2.json");
const TOOLSET_V2_DIGEST: &str =
    include_str!("../fixtures/plugins/manager-toolset-v2.sha256").trim_ascii_end();
const TOOLSET_V3: &[u8] = include_bytes!("../fixtures/plugins/manager-toolset-v3.json");
const TOOLSET_V3_DIGEST: &str =
    include_str!("../fixtures/plugins/manager-toolset-v3.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[test]
fn manager_toolset_exposes_only_bounded_lifecycle_operations() {
    let toolset = PluginManagerToolset::v1();
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
            "plugin_enable",
            "plugin_disable",
        ]
    );
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
fn manager_toolset_fixture_is_canonical_and_frozen() {
    let toolset = PluginManagerToolset::from_json(TOOLSET).unwrap();
    assert_eq!(toolset, PluginManagerToolset::v1());
    assert_eq!(
        toolset.canonical_bytes().unwrap(),
        canonical_fixture(TOOLSET)
    );
    assert_eq!(toolset.descriptor_digest().unwrap(), TOOLSET_DIGEST);

    let mut drift: serde_json::Value = serde_json::from_slice(TOOLSET).unwrap();
    drift["tools"][7]["inputSchema"]["properties"]["url"] = serde_json::json!({"type":"string"});
    let error = PluginManagerToolset::from_json(&serde_json::to_vec(&drift).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.manager_toolset_invalid");
}

#[test]
fn manager_toolset_v2_adds_only_the_okf_policy_surface() {
    let v1 = PluginManagerToolset::v1();
    let v2 = PluginManagerToolset::v2();
    v2.validate().unwrap();

    assert_eq!(v2.schema, "a3s.use.plugin-manager-tools.v2");
    assert_eq!(v2.tools.len(), v1.tools.len());
    assert_eq!(
        v2.tool("plugin_search").unwrap().input_schema["properties"]["kind"]["enum"],
        serde_json::json!(["mcp", "okf", "skill", "tool", "ui"])
    );
    assert_eq!(
        v2.tool("plugin_plan_install").unwrap().input_schema["properties"]["surfaces"]["items"]
            ["properties"]["kind"]["enum"],
        serde_json::json!(["mcp", "okf", "skill", "tool", "ui"])
    );
    assert_eq!(
        PluginManagerToolset::from_json(&v2.canonical_bytes().unwrap()).unwrap(),
        v2
    );
    assert_eq!(v2.canonical_bytes().unwrap(), canonical_fixture(TOOLSET_V2));
    assert_eq!(v2.descriptor_digest().unwrap(), TOOLSET_V2_DIGEST);
}

#[test]
fn manager_toolset_v3_adds_flow_without_rewriting_v1_or_v2() {
    let v1 = PluginManagerToolset::v1();
    let v2 = PluginManagerToolset::v2();
    let v3 = PluginManagerToolset::v3();
    v3.validate().unwrap();

    assert_eq!(v3.schema, "a3s.use.plugin-manager-tools.v3");
    assert_eq!(v3.tools.len(), v2.tools.len());
    assert_eq!(
        v3.tool("plugin_search").unwrap().input_schema["properties"]["kind"]["enum"],
        serde_json::json!(["flow", "mcp", "okf", "skill", "tool", "ui"])
    );
    assert_eq!(
        v3.tool("plugin_plan_install").unwrap().input_schema["properties"]["surfaces"]["items"]
            ["properties"]["kind"]["enum"],
        serde_json::json!(["flow", "mcp", "okf", "skill", "tool", "ui"])
    );
    assert_eq!(
        PluginManagerToolset::from_json(&v3.canonical_bytes().unwrap()).unwrap(),
        v3
    );
    assert_eq!(v3.canonical_bytes().unwrap(), canonical_fixture(TOOLSET_V3));
    assert_eq!(v3.descriptor_digest().unwrap(), TOOLSET_V3_DIGEST);

    assert_eq!(v1.canonical_bytes().unwrap(), canonical_fixture(TOOLSET));
    assert_eq!(v2.canonical_bytes().unwrap(), canonical_fixture(TOOLSET_V2));
}
