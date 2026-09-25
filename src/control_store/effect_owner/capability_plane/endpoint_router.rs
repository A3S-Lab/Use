//! Generation-fenced Gateway endpoint routes for Control Service invoke.
//!
//! Bind-time readiness creates opaque `gateway:` identities. Invoke-time
//! routing must use the same host-owned map: the durable receipt never stores
//! loopback URLs. This module owns that map plus the production router that
//! forwards Gateway-validated tool arguments without inventing a Use RPC
//! dialect.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use a3s_runtime::contract::{RuntimeObservation, RuntimeServiceEndpoint, TransportProtocol};
use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{PluginMcpSurface, ToolSurface};
use async_trait::async_trait;
use rmcp::model::CallToolRequestParam;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::capability_gateway::CapabilityGatewayRequestContext;
use crate::plugin_runtime::{
    RuntimeEndpointRef, RuntimeServiceBindingReceipt, RuntimeSurfaceContract, RuntimeSurfacePlan,
};

use super::super::runtime::{ControlRuntimeMcpReadiness, ControlRuntimeServiceReadinessPort};
use super::invocation::{ControlCapabilityGatewayEndpointRouter, ENDPOINT_ROUTE_UNAVAILABLE};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ROUTE_ERROR: &str = "use.control.capability_gateway_endpoint_route_invalid";

/// Live loopback target recorded when readiness bind succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlGatewayEndpointRoute {
    pub(in crate::control_store) runtime_endpoint: RuntimeServiceEndpoint,
    pub(in crate::control_store) contract: RuntimeSurfaceContract,
    pub(in crate::control_store) generation: u64,
}

/// Shared bind→invoke map keyed by opaque `gateway:` receipt identities.
#[derive(Debug, Default)]
pub(in crate::control_store) struct ControlGatewayEndpointRouteTable {
    routes: RwLock<BTreeMap<String, ControlGatewayEndpointRoute>>,
}

impl ControlGatewayEndpointRouteTable {
    pub(in crate::control_store) fn new() -> Self {
        Self::default()
    }

    pub(in crate::control_store) async fn put(
        &self,
        endpoint_ref: &RuntimeEndpointRef,
        route: ControlGatewayEndpointRoute,
    ) {
        self.routes
            .write()
            .await
            .insert(endpoint_ref.as_str().to_owned(), route);
    }

    pub(in crate::control_store) async fn get(
        &self,
        endpoint_ref: &RuntimeEndpointRef,
    ) -> Option<ControlGatewayEndpointRoute> {
        self.routes.read().await.get(endpoint_ref.as_str()).cloned()
    }

    pub(in crate::control_store) async fn remove(&self, endpoint_ref: &RuntimeEndpointRef) {
        self.routes.write().await.remove(endpoint_ref.as_str());
    }
}

/// Readiness adapter that records live Runtime endpoints beside opaque Gateway
/// bindings so invoke can resolve without storing URLs on the receipt.
#[derive(Clone)]
pub(in crate::control_store) struct RecordingControlRuntimeServiceReadiness {
    inner: Arc<dyn ControlRuntimeServiceReadinessPort>,
    table: Arc<ControlGatewayEndpointRouteTable>,
}

impl RecordingControlRuntimeServiceReadiness {
    pub(in crate::control_store) fn new(
        inner: Arc<dyn ControlRuntimeServiceReadinessPort>,
        table: Arc<ControlGatewayEndpointRouteTable>,
    ) -> Self {
        Self { inner, table }
    }
}

#[async_trait]
impl ControlRuntimeServiceReadinessPort for RecordingControlRuntimeServiceReadiness {
    async fn bind_tool_service(
        &self,
        surface: &ToolSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeEndpointRef> {
        let endpoint_ref = self
            .inner
            .bind_tool_service(
                surface,
                plan,
                observation,
                runtime_endpoint,
                idempotency_key,
                deadline_at_ms,
            )
            .await?;
        self.table
            .put(
                &endpoint_ref,
                ControlGatewayEndpointRoute {
                    runtime_endpoint: runtime_endpoint.clone(),
                    contract: plan.contract().clone(),
                    generation: observation.generation,
                },
            )
            .await;
        Ok(endpoint_ref)
    }

    async fn bind_mcp_service(
        &self,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<ControlRuntimeMcpReadiness> {
        let readiness = self
            .inner
            .bind_mcp_service(
                surface,
                plan,
                observation,
                runtime_endpoint,
                idempotency_key,
                deadline_at_ms,
            )
            .await?;
        self.table
            .put(
                &readiness.endpoint,
                ControlGatewayEndpointRoute {
                    runtime_endpoint: runtime_endpoint.clone(),
                    contract: plan.contract().clone(),
                    generation: observation.generation,
                },
            )
            .await;
        Ok(readiness)
    }

    async fn drain_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        self.inner
            .drain_service(receipt, idempotency_key, deadline_at_ms)
            .await?;
        self.table.remove(&receipt.endpoint_ref).await;
        Ok(())
    }

    async fn remove_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        self.inner
            .remove_service(receipt, idempotency_key, deadline_at_ms)
            .await?;
        self.table.remove(&receipt.endpoint_ref).await;
        Ok(())
    }
}

/// Production router: resolve `gateway:` → loopback Runtime endpoint recorded
/// at bind, then forward with the plugin's native protocol.
///
/// - MCP Service: standard Streamable HTTP `tools/call`
/// - Tool Service: POST Gateway-validated JSON arguments to the receipt
///   `base_path` (no Use RPC dialect; the plugin retains its HTTP vocabulary
///   at that path)
#[derive(Clone)]
pub(in crate::control_store) struct LiveControlCapabilityGatewayEndpointRouter {
    table: Arc<ControlGatewayEndpointRouteTable>,
}

impl LiveControlCapabilityGatewayEndpointRouter {
    pub(in crate::control_store) fn new(table: Arc<ControlGatewayEndpointRouteTable>) -> Self {
        Self { table }
    }
}

#[async_trait]
impl ControlCapabilityGatewayEndpointRouter for LiveControlCapabilityGatewayEndpointRouter {
    async fn invoke_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        surface_id: &str,
        arguments: Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value> {
        let route = self.table.get(&receipt.endpoint_ref).await.ok_or_else(|| {
            UseError::new(
                ENDPOINT_ROUTE_UNAVAILABLE,
                "No live Gateway endpoint route is registered for this opaque service binding.",
            )
            .with_detail("endpointRef", receipt.endpoint_ref.as_str())
            .with_detail("surfaceId", surface_id)
        })?;
        if route.generation != receipt.generation || route.contract != receipt.contract {
            return Err(UseError::new(
                ROUTE_ERROR,
                "The live Gateway endpoint route drifted from the durable Runtime binding receipt.",
            )
            .with_detail("endpointRef", receipt.endpoint_ref.as_str()));
        }
        if route.runtime_endpoint.protocol != TransportProtocol::Tcp {
            return Err(UseError::new(
                ROUTE_ERROR,
                "Gateway service invoke requires a TCP Runtime service endpoint.",
            ));
        }
        match &receipt.contract {
            RuntimeSurfaceContract::McpService {
                endpoint_path,
                protocol_version: _,
                ..
            } => {
                invoke_mcp_service(
                    &route.runtime_endpoint,
                    endpoint_path,
                    surface_id,
                    arguments,
                )
                .await
            }
            RuntimeSurfaceContract::ToolService { base_path, .. } => {
                invoke_tool_service_http(&route.runtime_endpoint, base_path, arguments).await
            }
            RuntimeSurfaceContract::ToolTask { .. } => Err(UseError::new(
                ROUTE_ERROR,
                "A Runtime Task binding cannot be invoked through the Service endpoint router.",
            )),
        }
    }
}

async fn invoke_mcp_service(
    endpoint: &RuntimeServiceEndpoint,
    endpoint_path: &str,
    tool_name: &str,
    arguments: Value,
) -> UseResult<Value> {
    let arguments = match arguments {
        Value::Object(map) => Some(map),
        Value::Null => None,
        _ => {
            return Err(UseError::new(
                "use.plugin.capability_gateway_arguments_invalid",
                "MCP Service tool arguments must be a JSON object.",
            ))
        }
    };
    let uri = http_uri(endpoint, endpoint_path)?;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(uri),
    );
    let client = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| {
            UseError::new(
                "use.control.capability_gateway_mcp_connect_timeout",
                "Timed out connecting to the Runtime MCP Service endpoint.",
            )
        })?
        .map_err(|error| {
            UseError::new(
                "use.control.capability_gateway_mcp_transport_failed",
                format!("Failed to connect to the Runtime MCP Service: {error}"),
            )
        })?;
    let result = client
        .call_tool(CallToolRequestParam {
            name: tool_name.to_owned().into(),
            arguments,
        })
        .await;
    let _ = client.cancel().await;
    let result = result.map_err(|error| {
        UseError::new(
            "use.control.capability_gateway_mcp_transport_failed",
            format!("MCP Service tools/call failed: {error}"),
        )
    })?;
    if result.is_error.unwrap_or(false) {
        if let Some(value) = result.structured_content {
            return Err(
                serde_json::from_value::<UseError>(value).unwrap_or_else(|error| {
                    UseError::new(
                        "use.control.capability_gateway_mcp_tool_failed",
                        format!("MCP Service tool returned an invalid error: {error}"),
                    )
                }),
            );
        }
        return Err(UseError::new(
            "use.control.capability_gateway_mcp_tool_failed",
            "MCP Service tool failed without structured error data.",
        ));
    }
    Ok(result.structured_content.unwrap_or(Value::Null))
}

async fn invoke_tool_service_http(
    endpoint: &RuntimeServiceEndpoint,
    base_path: &str,
    arguments: Value,
) -> UseResult<Value> {
    let uri = http_uri(endpoint, base_path)?;
    let client = reqwest::Client::builder()
        .timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| {
            UseError::new(
                ROUTE_ERROR,
                format!("Failed to build the Tool Service HTTP client: {error}"),
            )
        })?;
    let response = client
        .post(uri)
        .json(&arguments)
        .send()
        .await
        .map_err(|error| {
            UseError::new(
                "use.control.capability_gateway_tool_service_transport_failed",
                format!("Tool Service HTTP invoke failed: {error}"),
            )
        })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| {
        UseError::new(
            "use.control.capability_gateway_tool_service_transport_failed",
            format!("Failed to read the Tool Service response: {error}"),
        )
    })?;
    if !status.is_success() {
        return Err(UseError::new(
            "use.control.capability_gateway_tool_service_failed",
            format!("Tool Service HTTP invoke returned status {status}."),
        )
        .with_detail("status", status.as_u16()));
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body).map_err(|error| {
        UseError::new(
            "use.control.capability_gateway_tool_service_output_invalid",
            format!("Tool Service returned non-JSON output: {error}"),
        )
    })
}

fn http_uri(endpoint: &RuntimeServiceEndpoint, path: &str) -> UseResult<String> {
    if !path.starts_with('/') {
        return Err(UseError::new(
            ROUTE_ERROR,
            "Service endpoint paths must be absolute HTTP paths.",
        ));
    }
    Ok(format!(
        "http://{}:{}{path}",
        endpoint.address, endpoint.port
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::{RuntimeServiceReadinessEvidence, RUNTIME_SERVICE_BINDING_SCHEMA};
    use a3s_use_core::{
        InstallationId, InstallationKind, PlanEnforcementProfile, PlanQualifiedSurfaceRef,
        PluginSurfaceKind, PluginSurfaceRef,
    };
    use axum::{routing::post, Json, Router};

    fn digest(ch: char) -> String {
        format!(
            "sha256:{}",
            std::iter::repeat(ch).take(64).collect::<String>()
        )
    }

    fn workspace_scope() -> InstallationId {
        InstallationId {
            kind: InstallationKind::Workspace,
            id: "workspace-01".to_owned(),
        }
    }

    fn tool_service_receipt(
        endpoint_ref: RuntimeEndpointRef,
        generation: u64,
        base_path: &str,
    ) -> RuntimeServiceBindingReceipt {
        RuntimeServiceBindingReceipt {
            schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
            surface: PlanQualifiedSurfaceRef {
                package_id: "acme/research".to_string(),
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Tool,
                    id: "index".to_string(),
                },
            },
            package_digest: digest('a'),
            scope: workspace_scope(),
            descriptor_digest: digest('b'),
            provider_id: "test-runtime".to_string(),
            provider_build_id: "build-1".to_string(),
            capability_digest: digest('c'),
            enforcement: PlanEnforcementProfile::Container,
            unit_id: "use:service:0123456789abcdef".to_string(),
            generation,
            spec_digest: digest('d'),
            semantics_profile_digest: digest('e'),
            endpoint_ref,
            runtime_started_at_ms: 900,
            observation_revision: 1,
            last_healthy_at_ms: 900,
            contract: RuntimeSurfaceContract::ToolService {
                port_name: "http".to_string(),
                base_path: base_path.to_string(),
                shutdown_grace_ms: 30_000,
                api_contract_digest: None,
            },
            tool_schema_attestation: None,
            readiness: RuntimeServiceReadinessEvidence::HttpHealthy,
        }
    }

    #[tokio::test]
    async fn live_router_fails_closed_without_route() {
        let router = LiveControlCapabilityGatewayEndpointRouter::new(Arc::new(
            ControlGatewayEndpointRouteTable::new(),
        ));
        let receipt = tool_service_receipt(
            RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap(),
            7,
            "/api",
        );
        let error = router
            .invoke_service(
                &receipt,
                "index",
                Value::Object(Default::default()),
                &CapabilityGatewayRequestContext::stdio(),
            )
            .await
            .expect_err("missing live route must fail closed");
        assert_eq!(error.code, ENDPOINT_ROUTE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn live_router_forwards_tool_service_http_to_recorded_endpoint() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/api",
            post(
                |Json(body): Json<Value>| async move { Json(serde_json::json!({ "echo": body })) },
            ),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let table = Arc::new(ControlGatewayEndpointRouteTable::new());
        let endpoint_ref = RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap();
        let runtime_endpoint =
            RuntimeServiceEndpoint::node_local_tcp("http", port).expect("loopback tcp");
        let contract = RuntimeSurfaceContract::ToolService {
            port_name: "http".to_string(),
            base_path: "/api".to_string(),
            shutdown_grace_ms: 30_000,
            api_contract_digest: None,
        };
        table
            .put(
                &endpoint_ref,
                ControlGatewayEndpointRoute {
                    runtime_endpoint,
                    contract: contract.clone(),
                    generation: 7,
                },
            )
            .await;

        let router = LiveControlCapabilityGatewayEndpointRouter::new(Arc::clone(&table));
        let mut receipt = tool_service_receipt(endpoint_ref, 7, "/api");
        receipt.contract = contract;
        let result = router
            .invoke_service(
                &receipt,
                "index",
                serde_json::json!({ "q": "search" }),
                &CapabilityGatewayRequestContext::stdio(),
            )
            .await
            .expect("recorded Tool Service route must forward");
        assert_eq!(result, serde_json::json!({ "echo": { "q": "search" } }));
    }
}
