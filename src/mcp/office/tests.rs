use super::*;

#[test]
fn native_office_server_exposes_only_bounded_typed_tools() {
    let server = NativeOfficeMcpServer::new();
    let tools = server.tool_router.list_all();
    let mut names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "office_apply_batch",
            "office_close",
            "office_create",
            "office_get",
            "office_list",
            "office_merge_template",
            "office_open",
            "office_query",
            "office_raw_xml",
            "office_save",
            "office_validate",
            "office_view",
        ]
    );
}

#[test]
fn office_view_schema_exposes_typed_view_options() {
    let schema = schemars::schema_for!(OfficeViewInput);
    let encoded = serde_json::to_string(&schema).unwrap();
    assert!(encoded.contains("annotated"));
    assert!(encoded.contains("screenshot"));
    assert!(encoded.contains("issues"));
    assert!(encoded.contains("issueType"));
    assert!(encoded.contains("missing_alt_text"));
    assert!(encoded.contains("limit"));
    assert!(encoded.contains("output"));
    assert!(encoded.contains("timeoutMs"));

    let input: OfficeViewInput = serde_json::from_value(serde_json::json!({
        "session": "report",
        "view": "screenshot",
        "output": "report.png",
        "timeoutMs": 30_000
    }))
    .unwrap();
    assert_eq!(input.view, OfficeView::Screenshot);
    assert_eq!(input.output.as_deref(), Some("report.png"));
    assert_eq!(input.timeout_ms, Some(30_000));

    let issues: OfficeViewInput = serde_json::from_value(serde_json::json!({
        "session": "report",
        "view": "issues",
        "issueType": "missing_alt_text",
        "limit": 10
    }))
    .unwrap();
    assert_eq!(issues.view, OfficeView::Issues);
    assert_eq!(
        issues.issue_type,
        Some(input::OfficeIssueFilter::MissingAltText)
    );
    assert_eq!(issues.limit, Some(10));

    let annotated: OfficeViewInput = serde_json::from_value(serde_json::json!({
        "session": "report",
        "view": "annotated",
        "limit": 25
    }))
    .unwrap();
    assert_eq!(annotated.view, OfficeView::Annotated);
    assert_eq!(annotated.limit, Some(25));
}
