use super::*;

#[tokio::test]
async fn version_json_exposes_a_typed_data_payload_for_consumers() {
    let output = run(vec!["--version".to_string(), "--json".to_string()])
        .await
        .unwrap();

    assert_eq!(output.json["schemaVersion"], 1);
    assert_eq!(output.json["ok"], true);
    assert_eq!(output.json["data"]["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn capabilities_always_include_browser_office_and_ocr() {
    let output = run(vec!["capabilities".to_string(), "--json".to_string()])
        .await
        .unwrap();
    let domains = output.json["data"]["domains"].as_array().unwrap();
    assert_eq!(domains[0]["id"], "browser");
    assert_eq!(domains[1]["id"], "office");
    assert_eq!(domains[2]["id"], "ocr");
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
    let ocr = capabilities
        .iter()
        .find(|capability| capability["id"] == "use/ocr")
        .unwrap();

    assert_eq!(browser["origin"], "built-in");
    assert_eq!(office["origin"], "built-in");
    assert_eq!(ocr["origin"], "built-in");
    #[cfg(feature = "office")]
    {
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
    }
    #[cfg(not(feature = "office"))]
    {
        assert_eq!(office["enabled"], false);
        assert_eq!(office["surfaces"], serde_json::json!([]));
        assert!(office.get("skills").is_none());
    }
    #[cfg(feature = "ocr")]
    {
        assert_eq!(ocr["enabled"], true);
        assert_eq!(ocr["mcp"]["target"], "ocr-native");
        assert!(ocr["skills"][0]["path"].as_str().is_some_and(
            |path| std::path::Path::new(path).ends_with("skills/a3s-use-ocr/SKILL.md")
        ));
        assert_eq!(ocr["skills"][0]["sha256"].as_str().unwrap().len(), 64);
    }
    #[cfg(not(feature = "ocr"))]
    {
        assert_eq!(ocr["enabled"], false);
        assert_eq!(ocr["surfaces"], serde_json::json!([]));
        assert!(ocr.get("skills").is_none());
    }
    assert_eq!(registry["revision"].as_str().unwrap().len(), 64);
    assert!(output.json.get("jsonrpc").is_none());
}

#[cfg(feature = "ocr")]
#[tokio::test]
async fn built_in_ocr_doctor_uses_the_root_cli_contract() {
    let output = run(vec![
        "ocr".to_string(),
        "doctor".to_string(),
        "--json".to_string(),
    ])
    .await
    .unwrap();

    assert_eq!(output.json["schemaVersion"], 1);
    assert_eq!(output.json["ok"], true);
    assert!(output.json["data"]["readiness"].is_string());
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
