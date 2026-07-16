#![cfg(feature = "office")]

use std::path::Path;
use std::process::{Command, Output};

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

#[test]
fn native_cli_renders_all_html_formats_and_presentation_svg_without_officecli() {
    let temp = tempfile::tempdir().unwrap();
    let provider = temp.path().join("must-not-be-invoked");
    let word = temp.path().join("report.docx");
    let spreadsheet = temp.path().join("workbook.xlsx");
    let presentation = temp.path().join("deck.pptx");

    for document in [&word, &spreadsheet, &presentation] {
        run(
            &provider,
            &[
                "office",
                "native",
                "create",
                document.to_str().unwrap(),
                "--json",
            ],
        );
    }
    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            word.to_str().unwrap(),
            "/body/p[1]",
            "--text",
            "Word <semantic>",
            "--json",
        ],
    );
    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            spreadsheet.to_str().unwrap(),
            "/Sheet1/XFD1048576",
            "--text",
            "Spreadsheet & sparse",
            "--json",
        ],
    );
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
            "Presentation > preview",
            "--json",
        ],
    );

    for (document, expected) in [
        (&word, "Word &lt;semantic&gt;"),
        (&spreadsheet, "Spreadsheet &amp; sparse"),
        (&presentation, "Presentation &gt; preview"),
    ] {
        let rendered = run(
            &provider,
            &[
                "office",
                "native",
                "view",
                document.to_str().unwrap(),
                "html",
                "--json",
            ],
        );
        assert_eq!(rendered["data"]["view"], "html");
        let content = rendered["data"]["result"]["content"].as_str().unwrap();
        assert!(content.starts_with("<!doctype html>"));
        assert!(content.contains(expected));
        assert!(content.contains("Content-Security-Policy"));
        assert!(!content.contains("<script"));
        assert_ne!(rendered["data"]["result"]["sha256"], "");
    }

    let svg = run(
        &provider,
        &[
            "office",
            "native",
            "view",
            presentation.to_str().unwrap(),
            "svg",
            "--json",
        ],
    );
    assert_eq!(svg["data"]["view"], "svg");
    assert!(svg["data"]["result"]["content"]
        .as_str()
        .unwrap()
        .starts_with("<?xml version=\"1.0\""));

    let unsupported = run_failure(
        &provider,
        &[
            "office",
            "native",
            "view",
            word.to_str().unwrap(),
            "svg",
            "--json",
        ],
    );
    assert_eq!(
        unsupported["error"]["code"],
        "use.office.render_format_unsupported"
    );
}

#[test]
fn native_cli_render_files_are_atomic_and_no_clobber() {
    let temp = tempfile::tempdir().unwrap();
    let provider = temp.path().join("must-not-be-invoked");
    let document = temp.path().join("report.docx");
    let html = temp.path().join("preview.html");
    run(
        &provider,
        &[
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ],
    );
    run(
        &provider,
        &[
            "office",
            "native",
            "set",
            document.to_str().unwrap(),
            "/body/p[1]",
            "--text",
            "Artifact",
            "--json",
        ],
    );

    let written = run(
        &provider,
        &[
            "office",
            "native",
            "view",
            document.to_str().unwrap(),
            "html",
            "--output",
            html.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(
        written["data"]["result"]["outputPath"],
        html.to_str().unwrap()
    );
    assert!(written["data"]["result"].get("content").is_none());
    let original = std::fs::read(&html).unwrap();
    assert!(original.starts_with(b"<!doctype html>"));

    let refused = run_failure(
        &provider,
        &[
            "office",
            "native",
            "view",
            document.to_str().unwrap(),
            "html",
            "--output",
            html.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(refused["error"]["code"], "use.office.render_output_exists");
    assert_eq!(std::fs::read(&html).unwrap(), original);
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn native_mcp_renders_html_and_svg_without_officecli() {
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

    const TIMEOUT: Duration = Duration::from_secs(15);

    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("mcp-deck.pptx");
    let mut child = tokio::process::Command::new(binary())
        .args(["mcp", "serve", "office-native"])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
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
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "render-test", "version": "1" }
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
    let view_schema = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "office_view")
        .unwrap();
    let view_schema = view_schema["inputSchema"].to_string();
    assert!(view_schema.contains("html"));
    assert!(view_schema.contains("svg"));

    let created = call(
        &mut stdin,
        &mut stdout,
        3,
        "office_create",
        serde_json::json!({"session":"deck","file":document}),
        TIMEOUT,
    )
    .await;
    assert_ne!(created["result"]["isError"], true);

    let applied = call(
        &mut stdin,
        &mut stdout,
        4,
        "office_apply_batch",
        serde_json::json!({
            "session":"deck",
            "mutations":[{
                "operation":"add-slide",
                "parent":"/",
                "title":"MCP <render>"
            }]
        }),
        TIMEOUT,
    )
    .await;
    assert_ne!(applied["result"]["isError"], true);

    for (id, mode, marker) in [
        (5, "html", "<!doctype html>"),
        (6, "svg", "<?xml version=\"1.0\""),
    ] {
        let rendered = call(
            &mut stdin,
            &mut stdout,
            id,
            "office_view",
            serde_json::json!({"session":"deck","view":mode}),
            TIMEOUT,
        )
        .await;
        assert_ne!(rendered["result"]["isError"], true);
        let content = rendered["result"]["structuredContent"]["result"]["content"]
            .as_str()
            .unwrap();
        assert!(content.starts_with(marker));
        assert!(content.contains("MCP &lt;render&gt;"));
    }

    let closed = call(
        &mut stdin,
        &mut stdout,
        7,
        "office_close",
        serde_json::json!({"session":"deck","discard":true}),
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
}

#[cfg(feature = "mcp")]
async fn call(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    id: u32,
    name: &str,
    arguments: serde_json::Value,
    timeout: std::time::Duration,
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
    timeout: std::time::Duration,
) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

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
