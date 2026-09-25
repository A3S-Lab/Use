//! Live MCP adapter that snapshots the current session server per operation.

use a3s_use_core::{UseError, UseResult};
use rmcp::model::{
    CallToolRequestParam, CallToolResult, GetPromptRequestParam, GetPromptResult,
    ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParam,
    ReadResourceRequestParam, ReadResourceResult, ServerInfo,
};
use rmcp::{ServerHandler, ServiceExt};

use super::super::{CapabilityGatewayMcpServer, CapabilityGatewayTransport};
use super::{CapabilityGatewaySessionFactory, SessionOperationGuard};

#[derive(Clone)]
pub struct CapabilityGatewayLiveMcpServer {
    factory: CapabilityGatewaySessionFactory,
    transport: CapabilityGatewayTransport,
}

impl std::fmt::Debug for CapabilityGatewayLiveMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayLiveMcpServer")
            .field("catalog", &self.factory.current().catalog())
            .field("transport", &self.transport)
            .finish()
    }
}

impl CapabilityGatewayLiveMcpServer {
    pub(super) fn new(
        factory: CapabilityGatewaySessionFactory,
        transport: CapabilityGatewayTransport,
    ) -> Self {
        Self { factory, transport }
    }

    pub fn factory(&self) -> CapabilityGatewaySessionFactory {
        self.factory.clone()
    }

    pub(crate) fn with_transport(mut self, transport: CapabilityGatewayTransport) -> Self {
        self.transport = transport;
        self
    }

    fn snapshot(&self) -> UseResult<(CapabilityGatewayMcpServer, SessionOperationGuard)> {
        let operation = self.factory.enter_operation()?;
        Ok((
            self.factory.current().with_transport(self.transport),
            operation,
        ))
    }
}

impl ServerHandler for CapabilityGatewayLiveMcpServer {
    fn get_info(&self) -> ServerInfo {
        // `get_info` is a local protocol description and has no provider or
        // payload side effect.  Keep it available while draining so clients
        // can observe the endpoint's final server metadata.
        self.factory
            .current()
            .with_transport(self.transport)
            .get_info()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::call_tool(&server, request, request_context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::list_tools(&server, request, request_context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::list_resources(&server, request, request_context).await
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::list_prompts(&server, request, request_context).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::read_resource(&server, request, request_context).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::get_prompt(&server, request, request_context).await
    }

    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        let Ok((server, _operation)) = self.snapshot() else {
            return;
        };
        ServerHandler::on_initialized(&server, context).await;
    }
}

fn session_state_error_data(_error: UseError) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_request(
        "The Capability Gateway session is draining or already drained.",
        Some(serde_json::json!({ "code": super::SESSION_STATE_ERROR })),
    )
}
