//! Standard MCP tools for native Office plus local PP-OCRv6 document parsing.

use a3s_use_ocr::{ensure_ppocr_v6_ready, OcrRuntimeStatus};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Serialize;

use crate::{
    DocumentClient, DocumentDiagnostic, DocumentInspectRequest, DocumentInspectResult,
    DocumentParseRequest, DocumentParseResult, UseError, UseResult,
};

#[derive(Clone)]
pub struct DocumentMcpServer {
    client: DocumentClient,
    tool_router: ToolRouter<Self>,
}

impl DocumentMcpServer {
    pub fn new(client: DocumentClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(DocumentClient::from_env()?))
    }

    /// Serve standard MCP framing over stdin/stdout until the peer disconnects.
    pub async fn serve_stdio(self) -> UseResult<()> {
        let service = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| mcp_error("start", error))?;
        service
            .waiting()
            .await
            .map_err(|error| mcp_error("run", error))?;
        Ok(())
    }
}

#[tool_router]
impl DocumentMcpServer {
    #[tool(
        name = "document_doctor",
        description = "Inspect native Office and local PP-OCRv6 readiness without reading a document or making a network request",
        output_schema = rmcp::handler::server::tool::cached_schema_for_type::<DocumentDiagnostic>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn document_doctor(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(Ok(self.client.diagnostic())))
    }

    #[tool(
        name = "document_inspect",
        description = "Inspect bounded native Office structure, text, semantic units, and embedded raster candidates without running OCR or installing models",
        output_schema = rmcp::handler::server::tool::cached_schema_for_type::<DocumentInspectResult>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn document_inspect(
        &self,
        Parameters(request): Parameters<DocumentInspectRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(self.client.inspect(request).await))
    }

    #[tool(
        name = "document_parse",
        description = "Parse bounded DOCX, XLSX, PPTX, or raster input with native structure and selected local PP-OCRv6 evidence; this read-only tool never downloads missing models",
        output_schema = rmcp::handler::server::tool::cached_schema_for_type::<DocumentParseResult>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn document_parse(
        &self,
        Parameters(request): Parameters<DocumentParseRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(self.client.parse(request).await))
    }

    #[tool(
        name = "document_install_ocr",
        description = "Install or repair the pinned local PP-OCRv6 model bundle from its official HTTPS source with fixed size and SHA-256 checks",
        output_schema = rmcp::handler::server::tool::cached_schema_for_type::<OcrRuntimeStatus>(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn document_install_ocr(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(ensure_ppocr_v6_ready().await))
    }
}

#[tool_handler]
impl ServerHandler for DocumentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "a3s-use-document".to_string(),
                title: Some("A3S Use Document".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_string()),
            },
            instructions: Some(
                "Call document_inspect first. Native DOCX, XLSX, and PPTX text is the structural source of truth. Use OCR auto only for required or suggested raster candidates, or pass exact semantic imagePaths. If PP-OCRv6 is missing, request document_install_ocr through host confirmation and then retry document_parse. Parsing never uses Tesseract, Python, Microsoft Office, LibreOffice, Browser, or an off-device service. Preserve source, part, embedded-image, model, confidence, polygon, and bounding-box provenance."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

fn tool_result<T>(result: UseResult<T>) -> CallToolResult
where
    T: Serialize,
{
    match result {
        Ok(output) => match serde_json::to_value(output) {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => tool_error(UseError::new(
                "use.document.output_invalid",
                format!("Failed to encode document MCP output: {error}"),
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

fn mcp_error(action: &str, error: impl std::fmt::Display) -> UseError {
    UseError::new(
        "use.document.mcp_failed",
        format!("Failed to {action} the document MCP server: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_exposes_typed_document_tools_and_a_bounded_installer() {
        let server = DocumentMcpServer::new(DocumentClient::from_env().unwrap());
        let mut tools = server.tool_router.list_all();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<&str>>(),
            [
                "document_doctor",
                "document_inspect",
                "document_install_ocr",
                "document_parse"
            ]
        );
        let parse = tools
            .iter()
            .find(|tool| tool.name == "document_parse")
            .unwrap();
        let install = tools
            .iter()
            .find(|tool| tool.name == "document_install_ocr")
            .unwrap();
        assert_eq!(
            parse
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint),
            Some(true)
        );
        assert_eq!(
            parse
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.open_world_hint),
            Some(false)
        );
        assert_eq!(
            install
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint),
            Some(false)
        );
        assert_eq!(
            install
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.open_world_hint),
            Some(true)
        );
    }
}
