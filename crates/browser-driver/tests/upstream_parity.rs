use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const UPSTREAM_MCP_TOOL_COUNT: usize = 151;
const UPSTREAM_MCP_STRUCTURAL_SHA256: &str =
    "cf4e1d7cdf91f4f5c4c18fe0765b0317ac9ef0a6a49965f6d48bc7332eb8e8cf";
const UPSTREAM_SKILLS: &[&str] = &[
    "agentcore",
    "core",
    "dogfood",
    "electron",
    "slack",
    "vercel-sandbox",
];

fn driver() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use-browser-driver")
}

fn isolated_command(home: &std::path::Path) -> Command {
    let mut command = Command::new(driver());
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("A3S_USE_BROWSER_HOME", home.join("a3s-browser"))
        .env("A3S_USE_BROWSER_RUNTIME_DIR", home.join("run"))
        .env("AGENT_BROWSER_SOCKET_DIR", home.join("run"))
        .env(
            "A3S_USE_BROWSER_SKILLS_DIR",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skill-data"),
        )
        .env(
            "AGENT_BROWSER_SKILLS_DIR",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skill-data"),
        );
    command
}

#[test]
fn mcp_all_profile_matches_locked_upstream_tool_contract() {
    let home = tempfile::tempdir().expect("create isolated home");
    let mut child = isolated_command(home.path())
        .args(["mcp", "--tools", "all"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start MCP server");

    let requests = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "a3s-use-parity", "version": "1" }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/list",
            "params": { "cursor": "64" }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/list",
            "params": { "cursor": "128" }
        }),
    ];
    let mut stdin = child.stdin.take().expect("MCP stdin");
    for request in requests {
        writeln!(stdin, "{request}").expect("write MCP request");
    }
    drop(stdin);

    let output = child.wait_with_output().expect("wait for MCP server");
    assert!(
        output.status.success(),
        "MCP server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let responses = String::from_utf8(output.stdout).expect("MCP output is UTF-8");
    let values = responses
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("valid MCP response"))
        .collect::<Vec<_>>();
    let mut tools = values
        .iter()
        .filter_map(|value| value.pointer("/result/tools").and_then(Value::as_array))
        .flatten()
        .map(|tool| {
            json!({
                "name": tool.get("name").cloned().unwrap_or(Value::Null),
                "inputSchema": tool.get("inputSchema").cloned().unwrap_or(Value::Null),
                "annotations": tool.get("annotations").cloned().unwrap_or(Value::Null),
            })
        })
        .collect::<Vec<_>>();

    assert_eq!(tools.len(), UPSTREAM_MCP_TOOL_COUNT);
    for tool in &mut tools {
        remove_descriptions(tool);
    }
    let bytes = serde_json::to_vec(&tools).expect("serialize normalized MCP tools");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        UPSTREAM_MCP_STRUCTURAL_SHA256,
        "MCP names, schemas, or annotations diverged from agent-browser 0.31.2"
    );
}

#[test]
fn packaged_skills_match_locked_upstream_inventory() {
    let home = tempfile::tempdir().expect("create isolated home");
    let output = isolated_command(home.path())
        .args(["skills", "list", "--json"])
        .output()
        .expect("list packaged skills");
    assert!(
        output.status.success(),
        "skills list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid skills JSON");
    let mut names = value["data"]
        .as_array()
        .expect("skills data array")
        .iter()
        .filter_map(|skill| skill["name"].as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, UPSTREAM_SKILLS);

    for skill in UPSTREAM_SKILLS {
        let output = isolated_command(home.path())
            .args(["skills", "get", skill])
            .output()
            .expect("load packaged skill");
        assert!(
            output.status.success() && !output.stdout.is_empty(),
            "packaged skill '{skill}' is unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn remove_descriptions(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("description");
            for child in map.values_mut() {
                remove_descriptions(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                remove_descriptions(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
