//! Browser CLI commands backed by the persistent standard MCP deployment.

use std::collections::BTreeMap;

use a3s_use_browser::{BrowserActionResult, BrowserSessionInfo, BrowserSnapshot};
use a3s_use_core::{Artifact, UseError, UseResult, UseSessionId};
use serde::Deserialize;
use url::Url;

use crate::browser_cli::{parse_positive_u64, parse_wait, DEFAULT_TIMEOUT_MS};
use crate::cli::CommandOutput;

pub(crate) async fn run(command: &str, args: &[String]) -> UseResult<CommandOutput> {
    match command {
        "open" => open(args).await,
        "list" => list(args).await,
        "navigate" => navigate(args).await,
        "snapshot" => snapshot(args).await,
        "click" => click(args).await,
        "type" => type_text(args).await,
        "press" => press(args).await,
        "select" => select(args).await,
        "scroll" => scroll(args).await,
        "screenshot" => screenshot(args).await,
        "close" => close(args).await,
        _ => Err(invalid_usage(format!(
            "unknown Browser session command '{command}'"
        ))),
    }
}

pub(crate) fn help(command: Option<&str>) -> CommandOutput {
    let usage = match command {
        Some("open") => {
            "usage: a3s-use browser open <url> --session <id> [--timeout-ms <ms>] [--wait <condition>] [--user-agent <value>] [--json]"
        }
        Some("list") => "usage: a3s-use browser list [--json]",
        Some("navigate") => {
            "usage: a3s-use browser navigate <url> --session <id> [--timeout-ms <ms>] [--wait <condition>] [--json]"
        }
        Some("snapshot") => "usage: a3s-use browser snapshot --session <id> [--json]",
        Some("click") => {
            "usage: a3s-use browser click <reference> --session <id> [--json]"
        }
        Some("type") => {
            "usage: a3s-use browser type <reference> <text> --session <id> [--json]"
        }
        Some("press") => {
            "usage: a3s-use browser press <reference> <key> --session <id> [--json]"
        }
        Some("select") => {
            "usage: a3s-use browser select <reference> <value> --session <id> [--json]"
        }
        Some("scroll") => {
            "usage: a3s-use browser scroll --session <id> --x <pixels> --y <pixels> [--json]"
        }
        Some("screenshot") => {
            "usage: a3s-use browser screenshot <path> --session <id> [--json]"
        }
        Some("close") => "usage: a3s-use browser close --session <id> [--json]",
        _ => {
            "usage: a3s-use browser open|list|navigate|snapshot|click|type|press|select|scroll|screenshot|close [args] [--json]"
        }
    };
    CommandOutput::success(usage, serde_json::json!({ "usage": usage }))
}

async fn open(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(
        args,
        &["--session", "--timeout-ms", "--wait", "--user-agent"],
    )?;
    parsed.require_positionals(1, "browser open requires exactly one URL")?;
    let session = parsed.session()?;
    let url = parse_http_url(parsed.positionals[0])?;
    let timeout_ms = parsed.positive_u64("--timeout-ms", DEFAULT_TIMEOUT_MS)?;
    let wait = parse_wait(parsed.option("--wait"), timeout_ms)?;
    let user_agent = parsed.option("--user-agent").map(ToString::to_string);
    let snapshot: BrowserSnapshot = crate::mcp::call_browser_tool(
        "browser_open",
        serde_json::json!({
            "session": session,
            "url": url,
            "timeoutMs": timeout_ms,
            "wait": wait_value(&wait),
            "userAgent": user_agent
        }),
    )
    .await?;
    Ok(snapshot_output(snapshot))
}

async fn list(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &[])?;
    parsed.require_positionals(0, "browser list accepts no positional arguments")?;
    let sessions: Vec<BrowserSessionInfo> =
        crate::mcp::call_browser_tool("browser_list", serde_json::json!({})).await?;
    let human = if sessions.is_empty() {
        "No Browser sessions are open.".to_string()
    } else {
        sessions
            .iter()
            .map(|session| format!("{}\t{}", session.session.as_str(), session.url))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(CommandOutput::success(
        human,
        serde_json::json!({ "sessions": sessions }),
    ))
}

async fn navigate(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session", "--timeout-ms", "--wait"])?;
    parsed.require_positionals(1, "browser navigate requires exactly one URL")?;
    let session = parsed.session()?;
    let url = parse_http_url(parsed.positionals[0])?;
    let timeout_ms = parsed.positive_u64("--timeout-ms", DEFAULT_TIMEOUT_MS)?;
    let wait = parse_wait(parsed.option("--wait"), timeout_ms)?;
    let snapshot: BrowserSnapshot = crate::mcp::call_browser_tool(
        "browser_navigate",
        serde_json::json!({
            "session": session,
            "url": url,
            "timeoutMs": timeout_ms,
            "wait": wait_value(&wait)
        }),
    )
    .await?;
    Ok(snapshot_output(snapshot))
}

async fn snapshot(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(0, "browser snapshot accepts no positional arguments")?;
    let snapshot: BrowserSnapshot = crate::mcp::call_browser_tool(
        "browser_snapshot",
        serde_json::json!({ "session": parsed.session()? }),
    )
    .await?;
    Ok(snapshot_output(snapshot))
}

async fn click(args: &[String]) -> UseResult<CommandOutput> {
    let (parsed, reference) = reference_command(args, "click")?;
    let result: BrowserActionResult = crate::mcp::call_browser_tool(
        "browser_click",
        serde_json::json!({ "session": parsed.session()?, "reference": reference }),
    )
    .await?;
    Ok(action_output(result))
}

async fn type_text(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(
        2,
        "browser type requires exactly one reference and one text argument",
    )?;
    let result: BrowserActionResult = crate::mcp::call_browser_tool(
        "browser_type",
        serde_json::json!({
            "session": parsed.session()?,
            "reference": parsed.positionals[0],
            "text": parsed.positionals[1]
        }),
    )
    .await?;
    Ok(action_output(result))
}

async fn press(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(
        2,
        "browser press requires exactly one reference and one key",
    )?;
    let result: BrowserActionResult = crate::mcp::call_browser_tool(
        "browser_press",
        serde_json::json!({
            "session": parsed.session()?,
            "reference": parsed.positionals[0],
            "key": parsed.positionals[1]
        }),
    )
    .await?;
    Ok(action_output(result))
}

async fn select(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(
        2,
        "browser select requires exactly one reference and one value",
    )?;
    let result: BrowserActionResult = crate::mcp::call_browser_tool(
        "browser_select",
        serde_json::json!({
            "session": parsed.session()?,
            "reference": parsed.positionals[0],
            "value": parsed.positionals[1]
        }),
    )
    .await?;
    Ok(action_output(result))
}

async fn scroll(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session", "--x", "--y"])?;
    parsed.require_positionals(0, "browser scroll accepts no positional arguments")?;
    let x = parsed.required_i64("--x")?;
    let y = parsed.required_i64("--y")?;
    let result: BrowserActionResult = crate::mcp::call_browser_tool(
        "browser_scroll",
        serde_json::json!({ "session": parsed.session()?, "x": x, "y": y }),
    )
    .await?;
    Ok(action_output(result))
}

async fn screenshot(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(1, "browser screenshot requires exactly one output path")?;
    let artifact: Artifact = crate::mcp::call_browser_tool(
        "browser_screenshot",
        serde_json::json!({
            "session": parsed.session()?,
            "path": parsed.positionals[0]
        }),
    )
    .await?;
    Ok(CommandOutput::success(
        format!("Saved Browser screenshot to {}.", artifact.path.display()),
        serde_json::json!({ "artifact": artifact }),
    ))
}

async fn close(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(0, "browser close accepts no positional arguments")?;
    let session = parsed.session()?;
    let result: CloseResult =
        crate::mcp::call_browser_tool("browser_close", serde_json::json!({ "session": session }))
            .await?;
    let human = if result.closed {
        format!("Closed Browser session '{}'.", result.session.as_str())
    } else {
        format!(
            "Browser session '{}' was not open.",
            result.session.as_str()
        )
    };
    Ok(CommandOutput::success(human, serde_json::json!(result)))
}

fn reference_command<'a>(
    args: &'a [String],
    command: &str,
) -> UseResult<(ParsedArgs<'a>, &'a str)> {
    let parsed = ParsedArgs::parse(args, &["--session"])?;
    parsed.require_positionals(
        1,
        format!("browser {command} requires exactly one element reference"),
    )?;
    let reference = parsed.positionals[0];
    Ok((parsed, reference))
}

fn snapshot_output(snapshot: BrowserSnapshot) -> CommandOutput {
    let mut human = format!("{}\n{}\n\n{}", snapshot.title, snapshot.url, snapshot.text);
    if !snapshot.elements.is_empty() {
        human.push_str("\n\nInteractive elements:\n");
        human.push_str(
            &snapshot
                .elements
                .iter()
                .map(|element| {
                    let value = element
                        .value
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(|value| format!(" value={value:?}"))
                        .unwrap_or_default();
                    format!(
                        "{} {} {:?}{}",
                        element.reference, element.role, element.name, value
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    CommandOutput::success(human, serde_json::json!({ "snapshot": snapshot }))
}

fn action_output(result: BrowserActionResult) -> CommandOutput {
    CommandOutput::success(
        format!(
            "Browser {} completed in session '{}' at {}.",
            result.action,
            result.session.as_str(),
            result.url
        ),
        serde_json::json!({ "result": result }),
    )
}

fn parse_http_url(value: &str) -> UseResult<Url> {
    let url = Url::parse(value)
        .map_err(|error| invalid_usage(format!("Browser URL is invalid: {error}")))?;
    if matches!(url.scheme(), "http" | "https") {
        Ok(url)
    } else {
        Err(UseError::new(
            "use.browser.url_scheme_unsupported",
            format!(
                "Browser sessions accept HTTP(S) URLs, not '{}'.",
                url.scheme()
            ),
        ))
    }
}

fn wait_value(wait: &a3s_use_browser::WaitCondition) -> serde_json::Value {
    match wait {
        a3s_use_browser::WaitCondition::Load => serde_json::json!({ "kind": "load" }),
        a3s_use_browser::WaitCondition::DomContentLoaded => {
            serde_json::json!({ "kind": "dom-content-loaded" })
        }
        a3s_use_browser::WaitCondition::NetworkIdle { idle_ms } => {
            serde_json::json!({ "kind": "network-idle", "idleMs": idle_ms })
        }
        a3s_use_browser::WaitCondition::Selector { css, timeout_ms } => {
            serde_json::json!({ "kind": "selector", "css": css, "timeoutMs": timeout_ms })
        }
        a3s_use_browser::WaitCondition::Delay { ms } => {
            serde_json::json!({ "kind": "delay", "ms": ms })
        }
    }
}

#[derive(Debug)]
struct ParsedArgs<'a> {
    positionals: Vec<&'a str>,
    options: BTreeMap<&'a str, &'a str>,
}

impl<'a> ParsedArgs<'a> {
    fn parse(args: &'a [String], allowed_options: &[&str]) -> UseResult<Self> {
        let mut positionals = Vec::new();
        let mut options = BTreeMap::new();
        let mut index = 0;
        while index < args.len() {
            let argument = args[index].as_str();
            if argument == "--json" {
                index += 1;
                continue;
            }
            if argument.starts_with("--") {
                if !allowed_options.contains(&argument) {
                    return Err(invalid_usage(format!(
                        "unknown Browser option '{argument}'"
                    )));
                }
                if options.contains_key(argument) {
                    return Err(invalid_usage(format!(
                        "{argument} may be provided only once"
                    )));
                }
                let value = args
                    .get(index + 1)
                    .map(String::as_str)
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or_else(|| invalid_usage(format!("{argument} requires a value")))?;
                options.insert(argument, value);
                index += 2;
                continue;
            }
            positionals.push(argument);
            index += 1;
        }
        Ok(Self {
            positionals,
            options,
        })
    }

    fn option(&self, name: &str) -> Option<&'a str> {
        self.options.get(name).copied()
    }

    fn session(&self) -> UseResult<UseSessionId> {
        let value = self
            .option("--session")
            .ok_or_else(|| invalid_usage("--session requires a session ID"))?;
        UseSessionId::parse(value.to_string())
    }

    fn positive_u64(&self, name: &str, default: u64) -> UseResult<u64> {
        self.option(name)
            .map(|value| parse_positive_u64(value, name))
            .unwrap_or(Ok(default))
    }

    fn required_i64(&self, name: &str) -> UseResult<i64> {
        self.option(name)
            .ok_or_else(|| invalid_usage(format!("{name} requires a value")))?
            .parse::<i64>()
            .map_err(|_| invalid_usage(format!("{name} must be an integer")))
    }

    fn require_positionals(&self, expected: usize, message: impl Into<String>) -> UseResult<()> {
        if self.positionals.len() == expected {
            Ok(())
        } else {
            Err(invalid_usage(message))
        }
    }
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CloseResult {
    session: UseSessionId,
    closed: bool,
}

fn invalid_usage(message: impl Into<String>) -> UseError {
    UseError::new("use.cli.invalid_usage", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_missing_option_values_and_duplicates() {
        assert_eq!(
            ParsedArgs::parse(
                &["--session".to_string(), "--json".to_string()],
                &["--session"]
            )
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );
        assert_eq!(
            ParsedArgs::parse(
                &[
                    "--session".to_string(),
                    "one".to_string(),
                    "--session".to_string(),
                    "two".to_string()
                ],
                &["--session"]
            )
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );
    }

    #[test]
    fn parser_accepts_negative_scroll_deltas() {
        let args = [
            "--session".to_string(),
            "research".to_string(),
            "--x".to_string(),
            "0".to_string(),
            "--y".to_string(),
            "-500".to_string(),
        ];
        let parsed = ParsedArgs::parse(&args, &["--session", "--x", "--y"]).unwrap();
        assert_eq!(parsed.required_i64("--y").unwrap(), -500);
    }
}
