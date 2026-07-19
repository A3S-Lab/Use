use std::path::{Path, PathBuf};
use std::sync::Arc;

use a3s_use_browser::{
    BrowserPool, BrowserPoolConfig, PageRenderer, RenderRequest, RenderedPage, WaitCondition,
};
use a3s_use_core::{UseError, UseResult};
use url::Url;

use crate::cli::CommandOutput;

pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_NETWORK_IDLE_MS: u64 = 500;

pub(crate) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Ok(browser_help()),
        Some("render") => {
            if wants_help(&args[1..]) {
                return Ok(render_help());
            }
            let options = RenderOptions::parse(&args[1..])?;
            crate::first_use::ensure_browser_ready().await?;
            let pool = Arc::new(BrowserPool::new(BrowserPoolConfig::default()));
            let result = render_with(Arc::clone(&pool), options).await;
            pool.shutdown().await;
            result
        }
        Some(
            command @ ("open" | "list" | "navigate" | "snapshot" | "click" | "type" | "press"
            | "select" | "scroll" | "screenshot" | "close"),
        ) => {
            #[cfg(feature = "mcp")]
            {
                if wants_help(&args[1..]) {
                    Ok(crate::browser_session_cli::help(Some(command)))
                } else {
                    crate::browser_session_cli::run(command, &args[1..]).await
                }
            }
            #[cfg(not(feature = "mcp"))]
            {
                let _ = command;
                Err(UseError::new(
                    "use.mcp.disabled",
                    "Persistent Browser sessions require standard MCP support in this build.",
                ))
            }
        }
        Some(command) => Err(UseError::new(
            "use.browser.command_unknown",
            format!("Unknown Browser command '{command}'."),
        )
        .with_suggestion("Run 'a3s use browser --help'.")),
    }
}

fn browser_help() -> CommandOutput {
    let usage = concat!(
        "usage:\n",
        "  a3s-use browser render <url> [options]\n",
        "  a3s-use browser open <url> --session <id> [options]\n",
        "  a3s-use browser list\n",
        "  a3s-use browser navigate <url> --session <id> [options]\n",
        "  a3s-use browser snapshot --session <id>\n",
        "  a3s-use browser click|type|press|select <reference> [value] --session <id>\n",
        "  a3s-use browser scroll --session <id> --x <pixels> --y <pixels>\n",
        "  a3s-use browser screenshot <path> --session <id>\n",
        "  a3s-use browser close --session <id>"
    );
    CommandOutput::success(usage, serde_json::json!({ "usage": usage }))
}

fn render_help() -> CommandOutput {
    let usage = "usage: a3s-use browser render <url> [--output <path>] [--screenshot <path>] [--timeout-ms <ms>] [--wait <condition>] [--user-agent <value>] [--json]";
    CommandOutput::success(usage, serde_json::json!({ "usage": usage }))
}

fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|argument| argument == "--help" || argument == "-h")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderOptions {
    url: Url,
    timeout_ms: u64,
    wait: WaitCondition,
    user_agent: Option<String>,
    output: Option<PathBuf>,
    screenshot: Option<PathBuf>,
}

impl RenderOptions {
    fn parse(args: &[String]) -> UseResult<Self> {
        let raw_url = args
            .first()
            .filter(|value| !value.starts_with('-'))
            .ok_or_else(|| invalid_usage("browser render requires an HTTP(S) URL"))?;
        let url = Url::parse(raw_url)
            .map_err(|error| invalid_usage(format!("browser render URL is invalid: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(UseError::new(
                "use.browser.url_scheme_unsupported",
                format!(
                    "Browser render accepts HTTP(S) URLs, not '{}'.",
                    url.scheme()
                ),
            ));
        }

        let mut timeout_ms = DEFAULT_TIMEOUT_MS;
        let mut timeout_seen = false;
        let mut wait = None;
        let mut user_agent = None;
        let mut output = None;
        let mut screenshot = None;
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--json" => index += 1,
                "--timeout-ms" => {
                    if timeout_seen {
                        return Err(invalid_usage("--timeout-ms may be provided only once"));
                    }
                    timeout_seen = true;
                    timeout_ms =
                        parse_positive_u64(value(args, index, "--timeout-ms")?, "--timeout-ms")?;
                    index += 2;
                }
                "--wait" => {
                    wait = Some(unique_string_option(
                        wait,
                        value(args, index, "--wait")?,
                        "--wait",
                    )?);
                    index += 2;
                }
                "--user-agent" => {
                    user_agent = Some(unique_string_option(
                        user_agent,
                        value(args, index, "--user-agent")?,
                        "--user-agent",
                    )?);
                    index += 2;
                }
                "--output" => {
                    output = Some(unique_path_option(
                        output,
                        value(args, index, "--output")?,
                        "--output",
                    )?);
                    index += 2;
                }
                "--screenshot" => {
                    screenshot = Some(unique_path_option(
                        screenshot,
                        value(args, index, "--screenshot")?,
                        "--screenshot",
                    )?);
                    index += 2;
                }
                option => {
                    return Err(invalid_usage(format!(
                        "unknown browser render option '{option}'"
                    )))
                }
            }
        }

        Ok(Self {
            url,
            timeout_ms,
            wait: parse_wait(wait.as_deref(), timeout_ms)?,
            user_agent,
            output,
            screenshot,
        })
    }
}

async fn render_with<R>(renderer: Arc<R>, options: RenderOptions) -> UseResult<CommandOutput>
where
    R: PageRenderer + 'static,
{
    let request = RenderRequest {
        url: options.url,
        timeout_ms: options.timeout_ms,
        wait: options.wait,
        user_agent: options.user_agent,
        screenshot_path: options.screenshot.clone(),
    };
    let page = renderer.render(request).await?;
    if let Some(path) = options.output.as_deref() {
        write_html(path, page.html.as_bytes()).await?;
    }
    Ok(render_output(page, options.output, options.screenshot))
}

fn render_output(
    page: RenderedPage,
    output: Option<PathBuf>,
    screenshot: Option<PathBuf>,
) -> CommandOutput {
    let human = output.as_ref().map_or_else(
        || page.html.clone(),
        |path| format!("Rendered {} to {}.", page.final_url, path.display()),
    );
    let html = output.is_none().then(|| page.html.clone());
    CommandOutput::success(
        human,
        serde_json::json!({
            "requestedUrl": page.requested_url,
            "finalUrl": page.final_url,
            "status": page.status,
            "contentType": page.content_type,
            "elapsedMs": page.elapsed_ms,
            "artifacts": page.artifacts,
            "outputPath": output,
            "screenshotPath": screenshot,
            "html": html
        }),
    )
}

async fn write_html(path: &Path, bytes: &[u8]) -> UseResult<()> {
    tokio::fs::write(path, bytes).await.map_err(|error| {
        UseError::new(
            "use.browser.output_write_failed",
            format!(
                "Failed to write rendered HTML '{}': {error}",
                path.display()
            ),
        )
    })
}

pub(crate) fn parse_wait(value: Option<&str>, timeout_ms: u64) -> UseResult<WaitCondition> {
    let Some(value) = value else {
        return Ok(WaitCondition::DomContentLoaded);
    };
    match value {
        "load" => Ok(WaitCondition::Load),
        "dom-content-loaded" | "dom" => Ok(WaitCondition::DomContentLoaded),
        "network-idle" => Ok(WaitCondition::NetworkIdle {
            idle_ms: DEFAULT_NETWORK_IDLE_MS,
        }),
        _ if value.starts_with("selector:") => {
            let css = value.trim_start_matches("selector:");
            if css.trim().is_empty() {
                return Err(invalid_usage("selector wait requires a non-empty CSS selector"));
            }
            Ok(WaitCondition::Selector {
                css: css.to_string(),
                timeout_ms,
            })
        }
        _ if value.starts_with("delay:") => {
            let ms = parse_positive_u64(value.trim_start_matches("delay:"), "delay wait")?;
            Ok(WaitCondition::Delay { ms })
        }
        _ => Err(invalid_usage(format!(
            "unknown wait condition '{value}'; use load, dom-content-loaded, network-idle, selector:<css>, or delay:<ms>"
        ))),
    }
}

fn value<'a>(args: &'a [String], index: usize, option: &str) -> UseResult<&'a str> {
    args.get(index + 1)
        .map(String::as_str)
        .filter(|value| !value.is_empty() && !value.starts_with("--"))
        .ok_or_else(|| invalid_usage(format!("{option} requires a value")))
}

fn unique_string_option(current: Option<String>, value: &str, option: &str) -> UseResult<String> {
    if current.is_some() {
        return Err(invalid_usage(format!("{option} may be provided only once")));
    }
    Ok(value.to_string())
}

fn unique_path_option(current: Option<PathBuf>, value: &str, option: &str) -> UseResult<PathBuf> {
    if current.is_some() {
        return Err(invalid_usage(format!("{option} may be provided only once")));
    }
    Ok(PathBuf::from(value))
}

pub(crate) fn parse_positive_u64(value: &str, label: &str) -> UseResult<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_usage(format!("{label} must be a positive integer")))
}

fn invalid_usage(message: impl Into<String>) -> UseError {
    UseError::new("use.cli.invalid_usage", message)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;

    use super::*;

    struct FixtureRenderer;

    #[async_trait]
    impl PageRenderer for FixtureRenderer {
        async fn render(&self, request: RenderRequest) -> UseResult<RenderedPage> {
            Ok(RenderedPage {
                requested_url: request.url.clone(),
                final_url: request.url,
                status: Some(200),
                content_type: Some("text/html".to_string()),
                html: "<main>fixture</main>".to_string(),
                elapsed_ms: Duration::from_millis(2).as_millis() as u64,
                artifacts: Vec::new(),
            })
        }
    }

    #[test]
    fn parses_typed_render_options() {
        let options = RenderOptions::parse(&[
            "https://example.com".to_string(),
            "--timeout-ms".to_string(),
            "1500".to_string(),
            "--wait".to_string(),
            "selector:main".to_string(),
            "--output".to_string(),
            "page.html".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert_eq!(options.timeout_ms, 1_500);
        assert_eq!(
            options.wait,
            WaitCondition::Selector {
                css: "main".to_string(),
                timeout_ms: 1_500
            }
        );
        assert_eq!(options.output, Some(PathBuf::from("page.html")));
    }

    #[test]
    fn rejects_non_http_urls_and_unknown_waits() {
        assert_eq!(
            RenderOptions::parse(&["file:///tmp/page.html".to_string()])
                .unwrap_err()
                .code,
            "use.browser.url_scheme_unsupported"
        );
        assert_eq!(
            RenderOptions::parse(&[
                "https://example.com".to_string(),
                "--wait".to_string(),
                "forever".to_string(),
            ])
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );
        assert_eq!(
            RenderOptions::parse(&[
                "https://example.com".to_string(),
                "--output".to_string(),
                "--json".to_string(),
            ])
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );
    }

    #[tokio::test]
    async fn render_writes_explicit_html_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("page.html");
        let command = render_with(
            Arc::new(FixtureRenderer),
            RenderOptions {
                url: Url::parse("https://example.com").unwrap(),
                timeout_ms: DEFAULT_TIMEOUT_MS,
                wait: WaitCondition::DomContentLoaded,
                user_agent: None,
                output: Some(output.clone()),
                screenshot: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&output).await.unwrap(),
            "<main>fixture</main>"
        );
        assert_eq!(
            command.json["data"]["outputPath"],
            output.to_string_lossy().as_ref()
        );
        assert!(command.json["data"]["html"].is_null());
    }
}
