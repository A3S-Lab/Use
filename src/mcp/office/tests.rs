use super::*;
use a3s_use_office::{
    NativeOfficeHyperlinkTarget, NativeOfficeMutation, NativeOfficeRgbColor, NativeOfficeTextFormat,
};

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

#[test]
fn office_batch_schema_exposes_typed_text_formatting() {
    let schema = schemars::schema_for!(OfficeBatchInput);
    let encoded = serde_json::to_string(&schema).unwrap();
    for expected in [
        "set-text-format",
        "fontFamily",
        "fontSizeCentipoints",
        "textColor",
        "alignment",
        "justify",
    ] {
        assert!(encoded.contains(expected), "missing {expected}");
    }

    let input: OfficeBatchInput = serde_json::from_value(serde_json::json!({
        "session": "report",
        "mutations": [{
            "operation": "set-text-format",
            "path": "/body/p[1]/r[1]",
            "format": {
                "bold": true,
                "fontSizeCentipoints": 1200,
                "textColor": { "red": 18, "green": 52, "blue": 86 }
            }
        }]
    }))
    .unwrap();
    let mutation = input.mutations.into_iter().next().unwrap();
    assert!(matches!(
        mutation.into_native().unwrap(),
        NativeOfficeMutation::SetTextFormat {
            format: NativeOfficeTextFormat {
                bold: Some(true),
                font_size_centipoints: Some(1200),
                text_color: Some(NativeOfficeRgbColor {
                    red: 18,
                    green: 52,
                    blue: 86
                }),
                ..
            },
            ..
        }
    ));
}

#[test]
fn office_batch_schema_exposes_typed_hyperlinks() {
    let schema = schemars::schema_for!(OfficeBatchInput);
    let encoded = serde_json::to_string(&schema).unwrap();
    for expected in [
        "set-hyperlink",
        "external",
        "internal",
        "uri",
        "location",
        "display",
        "tooltip",
    ] {
        assert!(encoded.contains(expected), "missing {expected}");
    }

    let input: OfficeBatchInput = serde_json::from_value(serde_json::json!({
        "session": "report",
        "mutations": [{
            "operation": "set-hyperlink",
            "path": "/body/p[1]",
            "hyperlink": {
                "target": {
                    "kind": "external",
                    "uri": "https://example.com/report"
                },
                "display": "Report",
                "tooltip": "Open report"
            }
        }]
    }))
    .unwrap();
    let mutation = input.mutations.into_iter().next().unwrap();
    assert!(matches!(
        mutation.into_native().unwrap(),
        NativeOfficeMutation::SetHyperlink {
            hyperlink: a3s_use_office::NativeOfficeHyperlink {
                target: NativeOfficeHyperlinkTarget::External { ref uri },
                display: Some(ref display),
                tooltip: Some(ref tooltip),
            },
            ..
        } if uri == "https://example.com/report"
            && display == "Report"
            && tooltip == "Open report"
    ));
}
