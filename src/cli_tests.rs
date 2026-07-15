use super::*;

#[tokio::test]
async fn capabilities_always_include_browser_and_office() {
    let output = run(vec!["capabilities".to_string(), "--json".to_string()])
        .await
        .unwrap();
    let domains = output.json["data"]["domains"].as_array().unwrap();
    assert_eq!(domains[0]["id"], "browser");
    assert_eq!(domains[1]["id"], "office");
    assert!(output.json["data"].get("customJsonRpc").is_none());
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
