use std::borrow::Cow;
use std::sync::Arc;

use a3s_use_core::{
    PluginManagerApplyPlanInput, PluginManagerInspectInput, PluginManagerInstallPlanInput,
    PluginManagerListInstalledInput, PluginManagerPackageScopeInput, PluginManagerSearchInput,
    PluginManagerToolDefinition, PluginManagerUpgradePlanInput, UseError, UseResult,
};
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolResult, Implementation, JsonObject, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::{tool_handler, ServerHandler, ServiceExt};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::cognitive_package::CognitiveRegistryAccess;

use super::{PluginManagerConfirmationProvider, PluginManagerService};

const MCP_ERROR: &str = "use.plugin.manager_mcp_invalid";

/// Standard MCP adapter over the shared typed Plugin Manager application
/// service. It owns no catalog, plan, confirmation, or lifecycle state.
#[derive(Clone)]
pub struct PluginManagerMcpServer {
    service: PluginManagerService,
    registry_access: CognitiveRegistryAccess,
    confirmation_provider: Arc<dyn PluginManagerConfirmationProvider>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for PluginManagerMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginManagerMcpServer")
            .field("service", &self.service)
            .field("registry_access", &self.registry_access)
            .finish_non_exhaustive()
    }
}

impl PluginManagerMcpServer {
    /// Compose a network-refreshing manager MCP adapter. Apply remains
    /// fail-closed unless the injected host provider returns exact durable
    /// user-confirmation evidence for the reviewed plan.
    pub fn new(
        service: PluginManagerService,
        confirmation_provider: Arc<dyn PluginManagerConfirmationProvider>,
    ) -> UseResult<Self> {
        Self::with_registry_access(
            service,
            CognitiveRegistryAccess::Refreshed,
            confirmation_provider,
        )
    }

    /// Compose an adapter with an explicit refreshed or verified-cache-only
    /// Registry policy selected by the embedding host.
    pub fn with_registry_access(
        service: PluginManagerService,
        registry_access: CognitiveRegistryAccess,
        confirmation_provider: Arc<dyn PluginManagerConfirmationProvider>,
    ) -> UseResult<Self> {
        let tool_router = frozen_tool_router()?;
        Ok(Self {
            service,
            registry_access,
            confirmation_provider,
            tool_router,
        })
    }

    pub fn service(&self) -> &PluginManagerService {
        &self.service
    }

    /// Serve standard MCP framing over stdin/stdout until the peer
    /// disconnects. No A3S-specific JSON-RPC dialect is introduced.
    pub async fn serve_stdio(self) -> UseResult<()> {
        let service = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| mcp_error(format!("Failed to start Plugin Manager MCP: {error}")))?;
        service.waiting().await.map_err(|error| {
            mcp_error(format!("Plugin Manager MCP stopped with an error: {error}"))
        })?;
        Ok(())
    }

    async fn dispatch(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match name {
            "plugin_search" => {
                let input = parse_input::<PluginManagerSearchInput>(arguments)?;
                Ok(tool_result(
                    self.service.search(input, self.registry_access).await,
                ))
            }
            "plugin_inspect" => {
                let input = parse_input::<PluginManagerInspectInput>(arguments)?;
                Ok(tool_result(
                    self.service.inspect(input, self.registry_access).await,
                ))
            }
            "plugin_list_installed" => {
                let input = parse_input::<PluginManagerListInstalledInput>(arguments)?;
                Ok(tool_result(self.service.list_installed(input).await))
            }
            "plugin_status" => {
                let input = parse_input::<PluginManagerPackageScopeInput>(arguments)?;
                Ok(tool_result(self.service.status(input).await))
            }
            "plugin_plan_install" => {
                let input = parse_input::<PluginManagerInstallPlanInput>(arguments)?;
                Ok(tool_result(
                    self.service.plan_install(input, self.registry_access).await,
                ))
            }
            "plugin_plan_upgrade" => {
                let input = parse_input::<PluginManagerUpgradePlanInput>(arguments)?;
                Ok(tool_result(
                    self.service.plan_upgrade(input, self.registry_access).await,
                ))
            }
            "plugin_plan_uninstall" => {
                let input = parse_input::<PluginManagerPackageScopeInput>(arguments)?;
                Ok(tool_result(self.service.plan_uninstall(input).await))
            }
            "plugin_apply_plan" => {
                let input = parse_input::<PluginManagerApplyPlanInput>(arguments)?;
                let result = async {
                    let reviewed = self.service.reviewed_plan(&input).await?;
                    let confirmation = self
                        .confirmation_provider
                        .confirmation_for(&reviewed)
                        .await?;
                    self.service.apply_plan(input, confirmation).await
                }
                .await;
                Ok(tool_result(result))
            }
            "plugin_plan_enable" => {
                let input = parse_input::<PluginManagerPackageScopeInput>(arguments)?;
                Ok(tool_result(self.service.plan_enable(input).await))
            }
            "plugin_plan_disable" => {
                let input = parse_input::<PluginManagerPackageScopeInput>(arguments)?;
                Ok(tool_result(self.service.plan_disable(input).await))
            }
            _ => Err(rmcp::ErrorData::invalid_params(
                "Plugin Manager tool is not part of the frozen inventory.",
                None,
            )),
        }
    }
}

#[tool_handler]
impl ServerHandler for PluginManagerMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "a3s-use-plugin-manager".to_owned(),
                title: Some("A3S Use Plugin Manager".to_owned()),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_owned()),
            },
            instructions: Some(
                "Search and planning tools return verified, digest-bound evidence only. plugin_apply_plan reopens the exact durable plan and asks the embedding host for existing explicit user-confirmation evidence; an MCP request never implies confirmation."
                    .to_owned(),
            ),
            ..Default::default()
        }
    }
}

fn frozen_tool_router() -> UseResult<ToolRouter<PluginManagerMcpServer>> {
    let toolset = a3s_use_core::PluginManagerToolset::v4();
    toolset.validate()?;
    let mut router = ToolRouter::<PluginManagerMcpServer>::new();
    for definition in toolset.tools {
        let route_name = definition.name.clone();
        let tool = mcp_tool(definition)?;
        router.add_route(ToolRoute::new_dyn(
            tool,
            move |context: ToolCallContext<'_, PluginManagerMcpServer>| {
                let route_name = route_name.clone();
                Box::pin(async move {
                    context
                        .service
                        .dispatch(&route_name, context.arguments)
                        .await
                })
            },
        ));
    }
    Ok(router)
}

fn mcp_tool(definition: PluginManagerToolDefinition) -> UseResult<Tool> {
    let input_schema = definition
        .input_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A frozen Plugin Manager input schema is not an object."))?;
    Ok(Tool {
        name: Cow::Owned(definition.name),
        title: None,
        description: Some(Cow::Owned(definition.description)),
        input_schema: Arc::new(input_schema),
        output_schema: None,
        annotations: Some(ToolAnnotations {
            title: None,
            read_only_hint: Some(definition.annotations.read_only_hint),
            destructive_hint: Some(definition.annotations.destructive_hint),
            idempotent_hint: Some(definition.annotations.idempotent_hint),
            open_world_hint: Some(definition.annotations.open_world_hint),
        }),
        icons: None,
    })
}

fn parse_input<T: DeserializeOwned>(arguments: Option<JsonObject>) -> Result<T, rmcp::ErrorData> {
    serde_json::from_value(serde_json::Value::Object(arguments.unwrap_or_default())).map_err(
        |error| {
            rmcp::ErrorData::invalid_params(
                format!("Invalid Plugin Manager tool input: {error}"),
                None,
            )
        },
    )
}

fn tool_result<T: Serialize>(result: UseResult<T>) -> CallToolResult {
    match result {
        Ok(output) => match serde_json::to_value(output) {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => tool_error(mcp_error(format!(
                "Failed to encode Plugin Manager output: {error}"
            ))),
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

fn mcp_error(message: impl Into<String>) -> UseError {
    UseError::new(MCP_ERROR, message)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use a3s_use_core::{PlanScopeKind, PluginManagedScope, PLUGIN_MANAGED_SCOPE_SCHEMA_V2};
    use a3s_use_extension::{ExtensionPaths, ExtensionRegistry};
    use rmcp::model::CallToolRequestParam;
    use rmcp::{ClientHandler, ServiceExt};

    use crate::cognitive_package::{
        CognitivePackageHostManager, StandaloneCognitivePackageAuthorizationProvider,
        StandaloneCognitivePackageLifecycleFactory,
    };

    use super::super::FailClosedPluginManagerConfirmationProvider;
    use super::*;

    #[derive(Debug, Clone, Copy, Default)]
    struct TestClient;

    impl ClientHandler for TestClient {}

    #[test]
    fn dynamic_router_is_the_exact_frozen_toolset() {
        let router = frozen_tool_router().unwrap();
        let tools = router.list_all();
        let frozen = a3s_use_core::PluginManagerToolset::v4();
        assert_eq!(tools.len(), frozen.tools.len());

        for expected in frozen.tools {
            let actual = tools
                .iter()
                .find(|tool| tool.name == expected.name)
                .unwrap_or_else(|| panic!("missing MCP tool {}", expected.name));
            assert_eq!(
                actual.description.as_deref(),
                Some(expected.description.as_str())
            );
            assert_eq!(
                serde_json::Value::Object(actual.input_schema.as_ref().clone()),
                expected.input_schema
            );
            let annotations = actual.annotations.as_ref().unwrap();
            assert_eq!(
                annotations.read_only_hint,
                Some(expected.annotations.read_only_hint)
            );
            assert_eq!(
                annotations.destructive_hint,
                Some(expected.annotations.destructive_hint)
            );
            assert_eq!(
                annotations.idempotent_hint,
                Some(expected.annotations.idempotent_hint)
            );
            assert_eq!(
                annotations.open_world_hint,
                Some(expected.annotations.open_world_hint)
            );
        }
    }

    #[tokio::test]
    async fn adapter_uses_standard_mcp_initialization_list_and_call() {
        let (temporary, server) = test_server();
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_handle = tokio::spawn(async move {
            server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = TestClient.serve(client_transport).await.unwrap();

        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), 10);
        assert!(tools.iter().any(|tool| tool.name == "plugin_apply_plan"));
        let result = client
            .call_tool(CallToolRequestParam {
                name: "plugin_list_installed".into(),
                arguments: Some(
                    serde_json::json!({
                        "scopeKind": "workspace",
                        "scopeId": "workspace:plugin-manager-mcp-tests",
                        "limit": 10
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            })
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value["packages"].as_array())
                .map(Vec::len),
            Some(0)
        );

        client.cancel().await.unwrap();
        server_handle.await.unwrap();
        drop(temporary);
    }

    fn test_server() -> (tempfile::TempDir, PluginManagerMcpServer) {
        let temporary = tempfile::tempdir().unwrap();
        let scope = PluginManagedScope {
            schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
            host_id: "host:plugin-manager-mcp-tests".to_owned(),
            scope_kind: PlanScopeKind::Workspace,
            scope_id: "workspace:plugin-manager-mcp-tests".to_owned(),
            authority_id: "user:plugin-manager-mcp-tests".to_owned(),
            fence_generation: 31,
            fence_digest: format!("sha256:{}", "3".repeat(64)),
        };
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
        );
        let host = CognitivePackageHostManager::new(
            scope,
            "use:plugin-manager-mcp-tests",
            ExtensionRegistry::new(paths),
            Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
            Arc::new(StandaloneCognitivePackageAuthorizationProvider),
        )
        .unwrap();
        let service = PluginManagerService::new(host, 31).unwrap();
        let server = PluginManagerMcpServer::new(
            service,
            Arc::new(FailClosedPluginManagerConfirmationProvider),
        )
        .unwrap();
        (temporary, server)
    }
}
