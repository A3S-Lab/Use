use super::*;

#[tokio::test]
async fn capabilities_always_include_browser_and_office() {
    let output = run(vec!["capabilities".to_string(), "--json".to_string()])
        .await
        .unwrap();
    let domains = output.json["data"]["domains"].as_array().unwrap();
    assert_eq!(domains[0]["id"], "browser");
    assert_eq!(domains[1]["id"], "office");
    assert!(domains[0]["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|surface| surface == "skill"));
    assert!(output.json["data"].get("customJsonRpc").is_none());
}

#[tokio::test]
async fn capability_snapshot_unifies_built_ins_without_rpc_envelopes() {
    let output = run(vec![
        "capability".to_string(),
        "snapshot".to_string(),
        "--json".to_string(),
    ])
    .await
    .unwrap();
    let registry = &output.json["data"]["registry"];
    let capabilities = registry["capabilities"].as_array().unwrap();
    let browser = capabilities
        .iter()
        .find(|capability| capability["id"] == "use/browser")
        .unwrap();
    let office = capabilities
        .iter()
        .find(|capability| capability["id"] == "use/office")
        .unwrap();

    assert_eq!(browser["origin"], "built-in");
    assert_eq!(office["origin"], "built-in");
    assert!(office["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|surface| surface == "skill"));
    assert!(office["skills"][0]["path"]
        .as_str()
        .is_some_and(|path| std::path::Path::new(path).ends_with(
            std::path::Path::new("skills")
                .join("a3s-use-office")
                .join("SKILL.md")
        )));
    assert_eq!(office["skills"][0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(registry["revision"].as_str().unwrap().len(), 64);
    assert!(output.json.get("jsonrpc").is_none());
}

#[tokio::test]
async fn component_status_uses_cli_json_contract() {
    let output = run(vec![
        "component".to_string(),
        "status".to_string(),
        "browser".to_string(),
        "--json".to_string(),
    ])
    .await
    .unwrap();
    assert_eq!(output.json["schemaVersion"], 1);
    assert_eq!(output.json["component"]["id"], "browser");
    assert!(output.json.get("jsonrpc").is_none());
}

#[cfg(feature = "browser")]
#[test]
fn browser_component_presence_preserves_runtime_ownership() {
    use a3s_use_browser::BrowserInstallSource;

    assert_eq!(
        browser_presence(BrowserInstallSource::Environment),
        "external"
    );
    assert_eq!(browser_presence(BrowserInstallSource::System), "system");
    assert_eq!(
        browser_presence(BrowserInstallSource::ManagedCache),
        "managed"
    );
    assert_eq!(browser_presence(BrowserInstallSource::Missing), "missing");
}

#[cfg(feature = "office")]
#[test]
fn office_component_presence_preserves_runtime_ownership() {
    use a3s_use_office::OfficeInstallSource;

    assert_eq!(
        office_presence(OfficeInstallSource::Environment),
        "external"
    );
    assert_eq!(office_presence(OfficeInstallSource::System), "system");
    assert_eq!(office_presence(OfficeInstallSource::Managed), "managed");
    assert_eq!(office_presence(OfficeInstallSource::Missing), "missing");
}
