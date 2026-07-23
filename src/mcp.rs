//! Standard MCP adapters for built-in A3S Use domains.
//!
//! The SDK owns MCP framing and lifecycle. This module does not define an A3S
//! JSON-RPC dialect; the only JSON-RPC messages on stdio are standard MCP.

#[cfg(feature = "browser")]
mod runtime;

#[cfg(feature = "browser")]
pub(crate) use runtime::{
    browser_service_status, call_browser_tool, ensure_browser_service, serve_browser_http,
    stop_browser_service,
};

#[cfg(feature = "browser")]
mod browser {
    use std::sync::Arc;
    use std::time::Duration;

    #[cfg(test)]
    use a3s_use_browser::BrowserPoolConfig;
    use a3s_use_browser::{
        BrowserPool, BrowserSessions, OpenSessionRequest, PageRenderer, RenderRequest,
        WaitCondition,
    };
    use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
    use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
    use rmcp::{tool, tool_handler, tool_router, ServerHandler};
    use serde::{Deserialize, Serialize};
    use tokio_util::sync::CancellationToken;
    use url::Url;

    use a3s_use_core::{UseError, UseResult, UseSessionId};

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserRenderInput {
        #[schemars(description = "Absolute HTTP or HTTPS URL to render")]
        url: String,
        #[schemars(description = "Per-render deadline in milliseconds; defaults to 30000")]
        timeout_ms: Option<u64>,
        wait: Option<BrowserWaitInput>,
        #[schemars(description = "Optional user-agent override applied before navigation")]
        user_agent: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserOpenInput {
        #[schemars(description = "Stable Browser session ID")]
        session: String,
        #[schemars(description = "Absolute HTTP or HTTPS URL to open")]
        url: String,
        #[schemars(description = "Open deadline in milliseconds; defaults to 30000")]
        timeout_ms: Option<u64>,
        wait: Option<BrowserWaitInput>,
        #[schemars(description = "Optional user-agent override applied before navigation")]
        user_agent: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserSessionInput {
        #[schemars(description = "Browser session ID")]
        session: String,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserNavigateInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(description = "Absolute HTTP or HTTPS URL to navigate to")]
        url: String,
        #[schemars(description = "Navigation deadline in milliseconds; defaults to 30000")]
        timeout_ms: Option<u64>,
        wait: Option<BrowserWaitInput>,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserReferenceInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(
            description = "Element reference from the latest semantic snapshot, such as @e1"
        )]
        reference: String,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserTypeInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(description = "Element reference from the latest semantic snapshot")]
        reference: String,
        #[schemars(description = "Text to type into the referenced element")]
        text: String,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserPressInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(description = "Element reference from the latest semantic snapshot")]
        reference: String,
        #[schemars(description = "Keyboard key accepted by Chrome, such as Enter or Tab")]
        key: String,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserSelectInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(description = "Select element reference from the latest semantic snapshot")]
        reference: String,
        #[schemars(description = "Option value to select")]
        value: String,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserScrollInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(description = "Horizontal scroll delta in CSS pixels")]
        x: i64,
        #[schemars(description = "Vertical scroll delta in CSS pixels")]
        y: i64,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct BrowserScreenshotInput {
        #[schemars(description = "Browser session ID")]
        session: String,
        #[schemars(description = "Explicit local output path for the PNG artifact")]
        path: String,
    }

    #[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
    #[serde(tag = "kind", rename_all = "kebab-case")]
    enum BrowserWaitInput {
        Load,
        DomContentLoaded,
        NetworkIdle { idle_ms: u64 },
        Selector { css: String, timeout_ms: u64 },
        Delay { ms: u64 },
    }

    impl From<BrowserWaitInput> for WaitCondition {
        fn from(value: BrowserWaitInput) -> Self {
            match value {
                BrowserWaitInput::Load => Self::Load,
                BrowserWaitInput::DomContentLoaded => Self::DomContentLoaded,
                BrowserWaitInput::NetworkIdle { idle_ms } => Self::NetworkIdle { idle_ms },
                BrowserWaitInput::Selector { css, timeout_ms } => {
                    Self::Selector { css, timeout_ms }
                }
                BrowserWaitInput::Delay { ms } => Self::Delay { ms },
            }
        }
    }

    #[derive(Clone)]
    pub(super) struct BrowserMcpServer {
        pool: Arc<BrowserPool>,
        sessions: Arc<BrowserSessions>,
        shutdown: Option<CancellationToken>,
        tool_router: ToolRouter<Self>,
    }

    impl BrowserMcpServer {
        pub(super) fn persistent(
            pool: Arc<BrowserPool>,
            sessions: Arc<BrowserSessions>,
            shutdown: CancellationToken,
        ) -> Self {
            Self::with_sessions(pool, sessions, Some(shutdown))
        }

        fn with_sessions(
            pool: Arc<BrowserPool>,
            sessions: Arc<BrowserSessions>,
            shutdown: Option<CancellationToken>,
        ) -> Self {
            let mut tool_router = Self::tool_router();
            if shutdown.is_none() {
                tool_router.remove_route("browser_service_stop");
            }
            Self {
                sessions,
                pool,
                shutdown,
                tool_router,
            }
        }
    }

    #[tool_router]
    impl BrowserMcpServer {
        #[tool(
            name = "browser_doctor",
            description = "Inspect the locally available A3S Use Browser provider without installing software",
            annotations(
                read_only_hint = true,
                destructive_hint = false,
                idempotent_hint = true,
                open_world_hint = false
            )
        )]
        async fn browser_doctor(&self) -> Result<CallToolResult, rmcp::ErrorData> {
            Ok(match serde_json::to_value(a3s_use_browser::doctor()) {
                Ok(value) => CallToolResult::structured(value),
                Err(error) => tool_error(UseError::new(
                    "use.browser.diagnostic_invalid",
                    format!("Failed to encode Browser diagnostic: {error}"),
                )),
            })
        }

        #[tool(
            name = "browser_render",
            description = "Render one web page with the configured local Browser provider",
            annotations(
                read_only_hint = true,
                destructive_hint = false,
                idempotent_hint = true,
                open_world_hint = true
            )
        )]
        async fn browser_render(
            &self,
            Parameters(input): Parameters<BrowserRenderInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let url = match parse_http_url(&input.url) {
                Ok(url) => url,
                Err(error) => return Ok(tool_error(error)),
            };
            let request = RenderRequest {
                url,
                timeout_ms: input.timeout_ms.unwrap_or(30_000),
                wait: input
                    .wait
                    .map(WaitCondition::from)
                    .unwrap_or(WaitCondition::DomContentLoaded),
                user_agent: input.user_agent,
                screenshot_path: None,
            };
            Ok(tool_result(self.pool.render(request).await))
        }

        #[tool(
            name = "browser_open",
            description = "Open an isolated stateful Browser session and return its first semantic snapshot",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = true
            )
        )]
        async fn browser_open(
            &self,
            Parameters(input): Parameters<BrowserOpenInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            let url = match parse_http_url(&input.url) {
                Ok(url) => url,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions
                    .open(OpenSessionRequest {
                        session,
                        url,
                        timeout_ms: input.timeout_ms.unwrap_or(30_000),
                        wait: input
                            .wait
                            .map(WaitCondition::from)
                            .unwrap_or(WaitCondition::DomContentLoaded),
                        user_agent: input.user_agent,
                    })
                    .await,
            ))
        }

        #[tool(
            name = "browser_list",
            description = "List open Browser sessions and their current URLs",
            annotations(
                read_only_hint = true,
                destructive_hint = false,
                idempotent_hint = true,
                open_world_hint = false
            )
        )]
        async fn browser_list(&self) -> Result<CallToolResult, rmcp::ErrorData> {
            Ok(tool_result(self.sessions.list().await))
        }

        #[tool(
            name = "browser_navigate",
            description = "Navigate an open Browser session and return a fresh semantic snapshot",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = true
            )
        )]
        async fn browser_navigate(
            &self,
            Parameters(input): Parameters<BrowserNavigateInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            let url = match parse_http_url(&input.url) {
                Ok(url) => url,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions
                    .navigate(
                        &session,
                        url,
                        input
                            .wait
                            .map(WaitCondition::from)
                            .unwrap_or(WaitCondition::DomContentLoaded),
                        input.timeout_ms.unwrap_or(30_000),
                    )
                    .await,
            ))
        }

        #[tool(
            name = "browser_snapshot",
            description = "Return a compact semantic snapshot and fresh @e element references",
            annotations(
                read_only_hint = true,
                destructive_hint = false,
                idempotent_hint = true,
                open_world_hint = false
            )
        )]
        async fn browser_snapshot(
            &self,
            Parameters(input): Parameters<BrowserSessionInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(self.sessions.snapshot(&session).await))
        }

        #[tool(
            name = "browser_click",
            description = "Click an element reference from the latest semantic snapshot",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = true
            )
        )]
        async fn browser_click(
            &self,
            Parameters(input): Parameters<BrowserReferenceInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions.click(&session, &input.reference).await,
            ))
        }

        #[tool(
            name = "browser_type",
            description = "Focus an element reference and type text into it",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = true
            )
        )]
        async fn browser_type(
            &self,
            Parameters(input): Parameters<BrowserTypeInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions
                    .type_text(&session, &input.reference, &input.text)
                    .await,
            ))
        }

        #[tool(
            name = "browser_press",
            description = "Focus an element reference and press one keyboard key",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = true
            )
        )]
        async fn browser_press(
            &self,
            Parameters(input): Parameters<BrowserPressInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions
                    .press_key(&session, &input.reference, &input.key)
                    .await,
            ))
        }

        #[tool(
            name = "browser_select",
            description = "Select an option value on a referenced select element",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = true
            )
        )]
        async fn browser_select(
            &self,
            Parameters(input): Parameters<BrowserSelectInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions
                    .select(&session, &input.reference, &input.value)
                    .await,
            ))
        }

        #[tool(
            name = "browser_scroll",
            description = "Scroll the current page by explicit horizontal and vertical deltas",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = false
            )
        )]
        async fn browser_scroll(
            &self,
            Parameters(input): Parameters<BrowserScrollInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(tool_result(
                self.sessions.scroll(&session, input.x, input.y).await,
            ))
        }

        #[tool(
            name = "browser_screenshot",
            description = "Capture a full-page PNG from an open Browser session to an explicit local path",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = false,
                open_world_hint = false
            )
        )]
        async fn browser_screenshot(
            &self,
            Parameters(input): Parameters<BrowserScreenshotInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            if input.path.is_empty() {
                return Ok(tool_error(UseError::new(
                    "use.cli.invalid_usage",
                    "Browser screenshot requires a non-empty output path.",
                )));
            }
            Ok(tool_result(
                self.sessions.screenshot(&session, input.path).await,
            ))
        }

        #[tool(
            name = "browser_close",
            description = "Close one Browser session and release its tab resources",
            annotations(
                read_only_hint = false,
                destructive_hint = false,
                idempotent_hint = true,
                open_world_hint = false
            )
        )]
        async fn browser_close(
            &self,
            Parameters(input): Parameters<BrowserSessionInput>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            let session = match parse_session(&input.session) {
                Ok(session) => session,
                Err(error) => return Ok(tool_error(error)),
            };
            Ok(match self.sessions.close(&session).await {
                Ok(closed) => CallToolResult::structured(serde_json::json!({
                    "session": session,
                    "closed": closed
                })),
                Err(error) => tool_error(error),
            })
        }

        #[tool(
            name = "browser_service_stop",
            description = "Stop the authenticated persistent A3S Use Browser MCP deployment",
            annotations(
                read_only_hint = false,
                destructive_hint = true,
                idempotent_hint = true,
                open_world_hint = false
            )
        )]
        async fn browser_service_stop(&self) -> Result<CallToolResult, rmcp::ErrorData> {
            let Some(shutdown) = self.shutdown.clone() else {
                return Ok(tool_error(UseError::new(
                    "use.mcp.not_persistent",
                    "This Browser MCP server is attached to stdio and has no persistent deployment to stop.",
                )));
            };
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                shutdown.cancel();
            });
            Ok(CallToolResult::structured(serde_json::json!({
                "stopping": true,
                "protocol": "mcp-streamable-http"
            })))
        }
    }

    #[tool_handler]
    impl ServerHandler for BrowserMcpServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo {
                capabilities: ServerCapabilities::builder().enable_tools().build(),
                server_info: Implementation {
                    name: "a3s-use-browser".to_string(),
                    title: Some("A3S Use Browser".to_string()),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    icons: None,
                    website_url: Some("https://github.com/A3S-Lab/Use".to_string()),
                },
                instructions: Some(
                    "Use browser_doctor before Browser operations. browser_render is stateless; browser_open and the session tools retain state for the lifetime of this standard MCP server. Provider installation is explicit and never triggered by an MCP tool."
                        .to_string(),
                ),
                ..Default::default()
            }
        }
    }

    fn parse_session(value: &str) -> UseResult<UseSessionId> {
        UseSessionId::parse(value.to_string())
    }

    fn parse_http_url(value: &str) -> UseResult<Url> {
        match Url::parse(value) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => Ok(url),
            Ok(url) => Err(UseError::new(
                "use.browser.url_scheme_unsupported",
                format!("Browser MCP accepts HTTP(S) URLs, not '{}'.", url.scheme()),
            )),
            Err(error) => Err(UseError::new(
                "use.browser.url_invalid",
                format!("Invalid Browser URL: {error}"),
            )),
        }
    }

    fn tool_result<T: Serialize>(result: UseResult<T>) -> CallToolResult {
        match result {
            Ok(output) => match serde_json::to_value(output) {
                Ok(value) => CallToolResult::structured(value),
                Err(error) => tool_error(UseError::new(
                    "use.browser.output_invalid",
                    format!("Failed to encode Browser output: {error}"),
                )),
            },
            Err(error) => tool_error(error),
        }
    }

    fn tool_error(error: UseError) -> CallToolResult {
        CallToolResult::structured_error(serde_json::to_value(error).unwrap_or_else(|_| {
            serde_json::json!({
                "code": "use.error_encoding_failed",
                "message": "Failed to encode A3S Use error."
            })
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn browser_server_exposes_only_typed_mcp_tools() {
            let pool = Arc::new(BrowserPool::new(BrowserPoolConfig::default()));
            let server = BrowserMcpServer::with_sessions(
                Arc::clone(&pool),
                Arc::new(BrowserSessions::new(pool)),
                None,
            );
            let tools = server.tool_router.list_all();
            let mut names: Vec<&str> = tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>();
            names.sort_unstable();
            assert_eq!(
                names,
                [
                    "browser_click",
                    "browser_close",
                    "browser_doctor",
                    "browser_list",
                    "browser_navigate",
                    "browser_open",
                    "browser_press",
                    "browser_render",
                    "browser_screenshot",
                    "browser_scroll",
                    "browser_select",
                    "browser_snapshot",
                    "browser_type"
                ]
            );

            let annotations = |name: &str| {
                tools
                    .iter()
                    .find(|tool| tool.name == name)
                    .and_then(|tool| tool.annotations.as_ref())
                    .unwrap_or_else(|| panic!("{name} must declare MCP annotations"))
            };
            let list = annotations("browser_list");
            assert_eq!(list.read_only_hint, Some(true));
            assert_eq!(list.open_world_hint, Some(false));
            let render = annotations("browser_render");
            assert_eq!(render.read_only_hint, Some(true));
            assert_eq!(render.open_world_hint, Some(true));
            let click = annotations("browser_click");
            assert_eq!(click.read_only_hint, Some(false));
            assert_eq!(click.open_world_hint, Some(true));
        }

        #[test]
        fn browser_mcp_rejects_invalid_sessions_and_non_http_urls() {
            assert_eq!(
                parse_session("../escape").unwrap_err().code,
                "use.session.invalid_id"
            );
            assert_eq!(
                parse_http_url("file:///tmp/page.html").unwrap_err().code,
                "use.browser.url_scheme_unsupported"
            );
        }
    }
}
