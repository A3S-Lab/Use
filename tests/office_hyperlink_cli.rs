#![cfg(feature = "office")]

use std::path::Path;
use std::process::{Command, Output};

#[cfg(feature = "mcp")]
use std::process::Stdio;
#[cfg(feature = "mcp")]
use std::time::Duration;
#[cfg(feature = "mcp")]
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
}

fn execute(provider: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("A3S_OFFICECLI_EXECUTABLE", provider)
        .output()
        .unwrap()
}

fn run(provider: &Path, args: &[&str]) -> serde_json::Value {
    let output = execute(provider, args);
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn run_failure(provider: &Path, args: &[&str]) -> serde_json::Value {
    let output = execute(provider, args);
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn create(provider: &Path, document: &Path) {
    run(
        provider,
        &[
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ],
    );
}

#[test]
fn native_cli_manages_typed_hyperlinks_in_all_formats_without_officecli() {
    let temp = tempfile::tempdir().unwrap();
    let provider = temp.path().join("must-not-be-invoked");

    let word = temp.path().join("links.docx");
    create(&provider, &word);
    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            word.to_str().unwrap(),
            "/body/p[1]",
            "--text",
            "Before ",
            "--json",
        ],
    );
    let word_link = run(
        &provider,
        &[
            "office",
            "native",
            "add",
            word.to_str().unwrap(),
            "/body/p[1]",
            "--type",
            "hyperlink",
            "--url",
            "https://example.com/report",
            "--display",
            "Open report",
            "--tooltip",
            "A3S report",
            "--json",
        ],
    );
    assert_eq!(word_link["data"]["operation"], "set-hyperlink");
    assert_eq!(word_link["data"]["path"], "/body/p[1]/hyperlink[1]");
    assert_eq!(word_link["data"]["node"]["text"], "Open report");
    assert_eq!(
        word_link["data"]["node"]["format"]["target"],
        "https://example.com/report"
    );
    let updated = run(
        &provider,
        &[
            "office",
            "native",
            "set",
            word.to_str().unwrap(),
            "/body/p[1]/hyperlink[1]",
            "--location",
            "section_1",
            "--display",
            "Jump",
            "--json",
        ],
    );
    assert_eq!(updated["data"]["operation"], "set-hyperlink");
    assert_eq!(updated["data"]["node"]["format"]["target"], "section_1");

    let spreadsheet = temp.path().join("links.xlsx");
    create(&provider, &spreadsheet);
    let cell_link = run(
        &provider,
        &[
            "office",
            "native",
            "add",
            spreadsheet.to_str().unwrap(),
            "/Sheet1/A1",
            "--type",
            "hyperlink",
            "--url",
            "https://example.com/data",
            "--text",
            "Data",
            "--json",
        ],
    );
    assert_eq!(cell_link["data"]["path"], "/Sheet1/A1/hyperlink");
    assert_eq!(cell_link["data"]["node"]["format"]["display"], "Data");

    let presentation = temp.path().join("links.pptx");
    create(&provider, &presentation);
    run(
        &provider,
        &[
            "office",
            "native",
            "add",
            presentation.to_str().unwrap(),
            "/",
            "--type",
            "slide",
            "--text",
            "Linked shape",
            "--json",
        ],
    );
    let shape_link = run(
        &provider,
        &[
            "office",
            "native",
            "set",
            presentation.to_str().unwrap(),
            "/slide[1]/shape[1]",
            "--url",
            "https://example.com/slides",
            "--tooltip",
            "Open slides",
            "--json",
        ],
    );
    assert_eq!(shape_link["data"]["operation"], "set-hyperlink");
    let queried = run(
        &provider,
        &[
            "office",
            "native",
            "query",
            presentation.to_str().unwrap(),
            "hyperlink",
            "--json",
        ],
    );
    assert_eq!(queried["data"]["matches"], 1);
    assert_eq!(
        queried["data"]["results"][0]["path"],
        "/slide[1]/shape[1]/hyperlink"
    );

    for (document, link) in [
        (&word, "/body/p[1]/hyperlink[1]"),
        (&spreadsheet, "/Sheet1/A1/hyperlink"),
        (&presentation, "/slide[1]/shape[1]/hyperlink"),
    ] {
        run(
            &provider,
            &[
                "office",
                "native",
                "remove",
                document.to_str().unwrap(),
                link,
                "--json",
            ],
        );
        assert_eq!(
            run(
                &provider,
                &[
                    "office",
                    "native",
                    "query",
                    document.to_str().unwrap(),
                    "hyperlink",
                    "--json",
                ],
            )["data"]["matches"],
            0
        );
    }
    assert!(!provider.exists());
}

#[test]
fn native_cli_rejects_active_or_ambiguous_hyperlink_targets_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let provider = temp.path().join("must-not-be-invoked");
    let word = temp.path().join("invalid.docx");
    create(&provider, &word);
    let before = std::fs::read(&word).unwrap();

    let invalid = run_failure(
        &provider,
        &[
            "office",
            "native",
            "set",
            word.to_str().unwrap(),
            "/body/p[1]",
            "--url",
            "javascript:alert(1)",
            "--json",
        ],
    );
    assert_eq!(invalid["error"]["code"], "use.office.hyperlink_uri_invalid");
    assert_eq!(std::fs::read(&word).unwrap(), before);

    let ambiguous = run_failure(
        &provider,
        &[
            "office",
            "native",
            "set",
            word.to_str().unwrap(),
            "/body/p[1]",
            "--url",
            "https://example.com",
            "--location",
            "section_1",
            "--json",
        ],
    );
    assert_eq!(ambiguous["error"]["code"], "use.cli.invalid_usage");
    assert_eq!(std::fs::read(&word).unwrap(), before);

    let duplicate_display = run_failure(
        &provider,
        &[
            "office",
            "native",
            "add",
            word.to_str().unwrap(),
            "/body/p[1]",
            "--type",
            "hyperlink",
            "--url",
            "https://example.com",
            "--text",
            "First",
            "--display",
            "Second",
            "--json",
        ],
    );
    assert_eq!(duplicate_display["error"]["code"], "use.cli.invalid_usage");
    assert_eq!(std::fs::read(&word).unwrap(), before);
    assert!(!provider.exists());
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn native_standard_mcp_applies_and_reads_unsaved_typed_hyperlinks() {
    const TIMEOUT: Duration = Duration::from_secs(15);

    let temp = tempfile::tempdir().unwrap();
    let provider = temp.path().join("must-not-be-invoked");
    let document = temp.path().join("mcp-links.docx");
    let mut child = tokio::process::Command::new(binary())
        .args(["mcp", "serve", "office-native"])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();

    request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-06-18",
                "capabilities":{},
                "clientInfo":{"name":"office-hyperlink-test","version":"1"}
            }
        }),
        TIMEOUT,
    )
    .await;
    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
        )
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let tools = request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        TIMEOUT,
    )
    .await;
    let batch_schema = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "office_apply_batch")
        .unwrap()["inputSchema"]
        .to_string();
    assert!(batch_schema.contains("set-hyperlink"));
    assert!(batch_schema.contains("external"));
    assert!(batch_schema.contains("internal"));

    let created = call(
        &mut stdin,
        &mut stdout,
        3,
        "office_create",
        serde_json::json!({"session":"report","file":document}),
        TIMEOUT,
    )
    .await;
    assert_ne!(created["result"]["isError"], true, "{created}");
    let applied = call(
        &mut stdin,
        &mut stdout,
        4,
        "office_apply_batch",
        serde_json::json!({
            "session":"report",
            "mutations":[
                {
                    "operation":"set-text",
                    "path":"/body/p[1]",
                    "text":"Before "
                },
                {
                    "operation":"set-hyperlink",
                    "path":"/body/p[1]",
                    "hyperlink":{
                        "target":{
                            "kind":"external",
                            "uri":"https://example.com/mcp"
                        },
                        "display":"MCP link",
                        "tooltip":"Typed MCP hyperlink"
                    }
                }
            ]
        }),
        TIMEOUT,
    )
    .await;
    assert_ne!(applied["result"]["isError"], true, "{applied}");
    assert_eq!(
        applied["result"]["structuredContent"]["result"]["applied"],
        2
    );

    let read = call(
        &mut stdin,
        &mut stdout,
        5,
        "office_get",
        serde_json::json!({
            "session":"report",
            "path":"/body/p[1]/hyperlink[1]",
            "depth":0
        }),
        TIMEOUT,
    )
    .await;
    assert_ne!(read["result"]["isError"], true, "{read}");
    let node = &read["result"]["structuredContent"]["node"];
    assert_eq!(node["type"], "hyperlink");
    assert_eq!(node["text"], "MCP link");
    assert_eq!(node["format"]["target"], "https://example.com/mcp");

    let saved = call(
        &mut stdin,
        &mut stdout,
        6,
        "office_save",
        serde_json::json!({"session":"report"}),
        TIMEOUT,
    )
    .await;
    assert_ne!(saved["result"]["isError"], true, "{saved}");
    let closed = call(
        &mut stdin,
        &mut stdout,
        7,
        "office_close",
        serde_json::json!({"session":"report"}),
        TIMEOUT,
    )
    .await;
    assert_eq!(closed["result"]["structuredContent"]["closed"], true);

    drop(stdin);
    let status = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let mut diagnostics = Vec::new();
    stderr.read_to_end(&mut diagnostics).await.unwrap();
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    assert!(!provider.exists());
}

#[cfg(feature = "mcp")]
async fn call(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    id: u32,
    name: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> serde_json::Value {
    request(
        stdin,
        stdout,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }),
        timeout,
    )
    .await
}

#[cfg(feature = "mcp")]
async fn request(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    value: serde_json::Value,
    timeout: Duration,
) -> serde_json::Value {
    use tokio::io::AsyncBufReadExt;

    let mut encoded = serde_json::to_vec(&value).unwrap();
    encoded.push(b'\n');
    stdin.write_all(&encoded).await.unwrap();
    stdin.flush().await.unwrap();
    let mut line = String::new();
    let bytes = tokio::time::timeout(timeout, stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes > 0, "native Office MCP closed before responding");
    serde_json::from_str(&line).unwrap()
}
