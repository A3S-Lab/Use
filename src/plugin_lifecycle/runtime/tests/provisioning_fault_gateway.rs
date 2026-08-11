use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::provisioning_fault_io::{
    append_durable_line, read_optional_json, sync_test_parent, write_new_json,
};
use super::*;
use crate::plugin_runtime::provisioning_fault_matrix::{crash_after_checkpoint, GATEWAY_EFFECT};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GatewayEffect {
    package_id: String,
    generation: u64,
    surface_kind: String,
    surface_id: String,
    idempotency_key: String,
    endpoint: String,
    protocol_version: Option<String>,
    initialized_at_ms: Option<u64>,
}

pub(super) struct DurableReadiness {
    root: PathBuf,
}

impl DurableReadiness {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    async fn bind(
        &self,
        intent: &PluginLifecycleIntent,
        kind: PluginSurfaceKind,
        surface_id: &str,
        idempotency_key: &str,
        protocol_version: Option<String>,
        initialized_at_ms: Option<u64>,
    ) -> UseResult<GatewayEffect> {
        append_durable_line(
            &self.root.join("bind-attempts.log"),
            &format!("{idempotency_key}\n"),
        )
        .await
        .map_err(gateway_io)?;
        let path = self.root.join("route.json");
        if let Some(effect) = read_optional_json::<GatewayEffect>(&path)
            .await
            .map_err(gateway_io)?
        {
            validate_gateway_identity(&effect, intent, kind, surface_id, idempotency_key)?;
            return Ok(effect);
        }
        let effect = GatewayEffect {
            package_id: intent.package_id.clone(),
            generation: intent.generation,
            surface_kind: surface_kind_name(kind).to_string(),
            surface_id: surface_id.to_string(),
            idempotency_key: idempotency_key.to_string(),
            endpoint: endpoint_id(intent, surface_id),
            protocol_version,
            initialized_at_ms,
        };
        write_new_json(&path, &effect).await.map_err(gateway_io)?;
        crash_after_checkpoint(GATEWAY_EFFECT);
        Ok(effect)
    }
}

#[async_trait]
impl PluginRuntimeServiceReadinessHost for DurableReadiness {
    async fn bind_tool_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        _plan: &RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeEndpointRef> {
        if runtime_endpoint.port_name != "http" {
            return Err(gateway_error("Tool Runtime endpoint changed."));
        }
        let effect = self
            .bind(
                intent,
                PluginSurfaceKind::Tool,
                &surface.id,
                idempotency_key,
                None,
                None,
            )
            .await?;
        RuntimeEndpointRef::parse(effect.endpoint)
    }

    async fn bind_mcp_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<PluginMcpServiceReadiness> {
        if runtime_endpoint.port_name != "mcp" {
            return Err(gateway_error("MCP Runtime endpoint changed."));
        }
        let RuntimeSurfaceContract::McpService {
            protocol_version, ..
        } = plan.contract()
        else {
            return Err(gateway_error("MCP Runtime contract changed."));
        };
        let effect = self
            .bind(
                intent,
                PluginSurfaceKind::Mcp,
                &surface.id,
                idempotency_key,
                Some(protocol_version.clone()),
                Some(observation.observed_at_ms + 1),
            )
            .await?;
        Ok(PluginMcpServiceReadiness::new(
            RuntimeEndpointRef::parse(effect.endpoint)?,
            RuntimeMcpInitializeEvidence::new(
                effect
                    .protocol_version
                    .ok_or_else(|| gateway_error("MCP protocol evidence is missing."))?,
                effect
                    .initialized_at_ms
                    .ok_or_else(|| gateway_error("MCP initialize time is missing."))?,
            )?,
        ))
    }

    async fn drain_service(
        &self,
        intent: &PluginLifecycleIntent,
        receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        let effect = read_optional_json::<GatewayEffect>(&self.root.join("route.json"))
            .await
            .map_err(gateway_io)?
            .ok_or_else(|| gateway_error("Gateway route disappeared before drain."))?;
        validate_gateway_receipt(&effect, intent, receipt)
    }

    async fn remove_service(
        &self,
        intent: &PluginLifecycleIntent,
        receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        let path = self.root.join("route.json");
        let effect = read_optional_json::<GatewayEffect>(&path)
            .await
            .map_err(gateway_io)?
            .ok_or_else(|| gateway_error("Gateway route disappeared before removal."))?;
        validate_gateway_receipt(&effect, intent, receipt)?;
        tokio::fs::remove_file(&path).await.map_err(gateway_io)?;
        sync_test_parent(&self.root).await.map_err(gateway_io)
    }
}

fn validate_gateway_identity(
    effect: &GatewayEffect,
    intent: &PluginLifecycleIntent,
    kind: PluginSurfaceKind,
    surface_id: &str,
    idempotency_key: &str,
) -> UseResult<()> {
    if effect.package_id != intent.package_id
        || effect.generation != intent.generation
        || effect.surface_kind != surface_kind_name(kind)
        || effect.surface_id != surface_id
        || effect.idempotency_key != idempotency_key
    {
        return Err(gateway_error(
            "Gateway bind identity changed during replay.",
        ));
    }
    Ok(())
}

fn validate_gateway_receipt(
    effect: &GatewayEffect,
    intent: &PluginLifecycleIntent,
    receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
) -> UseResult<()> {
    validate_gateway_identity(
        effect,
        intent,
        receipt.surface.surface.kind,
        &receipt.surface.surface.id,
        &effect.idempotency_key,
    )?;
    if effect.endpoint != receipt.endpoint_ref.as_str() {
        return Err(gateway_error("Gateway endpoint receipt changed."));
    }
    Ok(())
}

fn gateway_io(error: io::Error) -> UseError {
    UseError::new(
        "use.plugin.test_gateway_io",
        format!("Durable test Gateway I/O failed: {error}"),
    )
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Ui => "ui",
    }
}

fn gateway_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.test_gateway_identity", message)
}
