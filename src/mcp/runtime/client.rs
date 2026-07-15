//! Standard MCP client used by short-lived Browser CLI invocations.

use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use rmcp::model::CallToolRequestParam;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde::de::DeserializeOwned;

use super::receipt::{validate_receipt, BrowserServiceReceipt};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) async fn call_with_receipt<T>(
    receipt: &BrowserServiceReceipt,
    name: &'static str,
    arguments: serde_json::Value,
) -> UseResult<T>
where
    T: DeserializeOwned,
{
    validate_receipt(receipt)?;
    let arguments = match arguments {
        serde_json::Value::Object(arguments) => Some(arguments),
        _ => {
            return Err(UseError::new(
                "use.mcp.arguments_invalid",
                "Browser MCP tool arguments must be a JSON object.",
            ))
        }
    };
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(receipt.endpoint.clone())
            .auth_header(receipt.token.clone()),
    );
    let client = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| {
            UseError::new(
                "use.mcp.connect_timeout",
                "Timed out connecting to the Browser MCP service.",
            )
        })?
        .map_err(mcp_transport_error)?;
    let result = client
        .call_tool(CallToolRequestParam {
            name: name.into(),
            arguments,
        })
        .await;
    let _ = client.cancel().await;
    let result = result.map_err(mcp_transport_error)?;
    if result.is_error.unwrap_or(false) {
        if let Some(value) = result.structured_content {
            return Err(
                serde_json::from_value::<UseError>(value).unwrap_or_else(|error| {
                    UseError::new(
                        "use.mcp.tool_failed",
                        format!("Browser MCP tool '{name}' returned an invalid error: {error}"),
                    )
                }),
            );
        }
        return Err(UseError::new(
            "use.mcp.tool_failed",
            format!("Browser MCP tool '{name}' failed without structured error data."),
        ));
    }
    let value = result.structured_content.ok_or_else(|| {
        UseError::new(
            "use.mcp.output_invalid",
            format!("Browser MCP tool '{name}' returned no structured output."),
        )
    })?;
    serde_json::from_value(value).map_err(|error| {
        UseError::new(
            "use.mcp.output_invalid",
            format!("Browser MCP tool '{name}' returned invalid output: {error}"),
        )
    })
}

pub(super) async fn probe(receipt: &BrowserServiceReceipt) -> bool {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        call_with_receipt::<Vec<a3s_use_browser::BrowserSessionInfo>>(
            receipt,
            "browser_list",
            serde_json::json!({}),
        ),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

fn mcp_transport_error(error: impl std::fmt::Display) -> UseError {
    UseError::new(
        "use.mcp.transport_failed",
        format!("Standard MCP transport failed: {error}"),
    )
}
