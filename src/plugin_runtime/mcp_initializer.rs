use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rmcp::model::{ClientCapabilities, ClientInfo, Implementation, ProtocolVersion, ServerInfo};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{service::QuitReason, ServiceExt};
use url::{Host, Url};

use a3s_use_core::{UseError, UseResult};

use super::{
    RuntimeEndpointRef, RuntimeMcpInitializeEvidence, RuntimeServiceActivation,
    RuntimeSurfaceContract,
};

const MAX_BEARER_TOKEN_BYTES: usize = 4 * 1024;
const MAX_GATEWAY_URL_BYTES: usize = 4 * 1024;
const MAX_INITIALIZE_TIMEOUT_MS: u64 = 120_000;

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeMcpBearerToken(String);

impl RuntimeMcpBearerToken {
    pub fn parse(value: impl Into<String>) -> UseResult<Self> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_BEARER_TOKEN_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(connection_error(
                "A Runtime MCP bearer token is outside the bounded opaque credential contract.",
            ));
        }
        Ok(Self(value))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RuntimeMcpBearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RuntimeMcpBearerToken([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeMcpHttpConnection {
    endpoint_ref: RuntimeEndpointRef,
    unit_id: String,
    generation: u64,
    runtime_started_at_ms: u64,
    endpoint: Url,
    bearer_token: RuntimeMcpBearerToken,
}

impl RuntimeMcpHttpConnection {
    pub fn new(
        endpoint_ref: RuntimeEndpointRef,
        unit_id: impl Into<String>,
        generation: u64,
        runtime_started_at_ms: u64,
        endpoint: Url,
        bearer_token: RuntimeMcpBearerToken,
    ) -> UseResult<Self> {
        let connection = Self {
            endpoint_ref,
            unit_id: unit_id.into(),
            generation,
            runtime_started_at_ms,
            endpoint,
            bearer_token,
        };
        connection.validate()?;
        Ok(connection)
    }

    fn validate(&self) -> UseResult<()> {
        RuntimeEndpointRef::parse(self.endpoint_ref.as_str()).map_err(|_| {
            connection_error("A Runtime MCP connection contains an invalid Gateway binding.")
        })?;
        if !valid_runtime_unit_id(&self.unit_id)
            || self.generation == 0
            || self.runtime_started_at_ms == 0
            || !valid_gateway_endpoint(&self.endpoint)
        {
            return Err(connection_error(
                "A Runtime MCP connection has invalid binding, Runtime identity, or host endpoint.",
            ));
        }
        RuntimeMcpBearerToken::parse(self.bearer_token.expose())?;
        Ok(())
    }
}

impl fmt::Debug for RuntimeMcpHttpConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMcpHttpConnection")
            .field("endpoint_ref", &self.endpoint_ref)
            .field("unit_id", &self.unit_id)
            .field("generation", &self.generation)
            .field("runtime_started_at_ms", &self.runtime_started_at_ms)
            .field("endpoint", &"[REDACTED]")
            .field("bearer_token", &self.bearer_token)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeMcpInitializer {
    timeout_ms: u64,
}

impl RuntimeMcpInitializer {
    pub fn new(timeout_ms: u64) -> UseResult<Self> {
        if timeout_ms == 0 || timeout_ms > MAX_INITIALIZE_TIMEOUT_MS {
            return Err(UseError::new(
                "use.plugin.runtime.mcp_initializer_invalid",
                format!(
                    "Runtime MCP initialize timeout must be between 1 and {MAX_INITIALIZE_TIMEOUT_MS} milliseconds."
                ),
            ));
        }
        Ok(Self { timeout_ms })
    }

    pub async fn initialize(
        &self,
        activation: &RuntimeServiceActivation,
        endpoint_ref: &RuntimeEndpointRef,
        connection: &RuntimeMcpHttpConnection,
    ) -> UseResult<RuntimeMcpInitializeEvidence> {
        connection.validate()?;
        let RuntimeSurfaceContract::McpService {
            protocol_version, ..
        } = &activation.plan.contract
        else {
            return Err(UseError::new(
                "use.plugin.runtime.class_mismatch",
                "A standard MCP initialize probe requires an MCP Service activation.",
            ));
        };
        let observation = &activation.observation;
        if connection.endpoint_ref != *endpoint_ref
            || connection.unit_id != observation.unit_id
            || connection.generation != observation.generation
            || observation.started_at_ms != Some(connection.runtime_started_at_ms)
        {
            return Err(UseError::new(
                "use.plugin.runtime.mcp_connection_mismatch",
                "The host MCP connection does not bind the activated Runtime process identity.",
            ));
        }

        let requested_protocol = parse_protocol_version(protocol_version)?;
        let client_info = ClientInfo {
            protocol_version: requested_protocol,
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "a3s-use-plugin-runtime".to_string(),
                title: Some("A3S Use Plugin Runtime".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
        };
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(connection.endpoint.as_str().to_string())
                .auth_header(connection.bearer_token.expose().to_string()),
        );
        let service = tokio::time::timeout(
            Duration::from_millis(self.timeout_ms),
            client_info.serve(transport),
        )
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.runtime.mcp_initialize_timeout",
                "Timed out performing the standard MCP initialize handshake.",
            )
        })?
        .map_err(|_| {
            UseError::new(
                "use.plugin.runtime.mcp_initialize_failed",
                "The standard MCP initialize handshake failed.",
            )
        })?;
        let negotiated = service
            .peer_info()
            .cloned()
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.runtime.mcp_initialize_failed",
                    "The MCP client completed without negotiated server information.",
                )
            })
            .and_then(|server_info| validate_negotiated_protocol(protocol_version, &server_info));
        let cleanup = cancel_probe(service, self.timeout_ms).await;
        if let Err(error) = negotiated {
            return Err(attach_cleanup_error(error, cleanup));
        }
        cleanup?;

        let initialized_at_ms = unix_time_ms()?;
        let evidence =
            RuntimeMcpInitializeEvidence::new(protocol_version.clone(), initialized_at_ms)?;
        evidence.validate(protocol_version, observation.observed_at_ms)?;
        Ok(evidence)
    }
}

fn parse_protocol_version(value: &str) -> UseResult<ProtocolVersion> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(|_| {
        UseError::new(
            "use.plugin.runtime.mcp_protocol_invalid",
            "The release MCP protocol version cannot be encoded for negotiation.",
        )
    })
}

fn validate_negotiated_protocol(expected: &str, server_info: &ServerInfo) -> UseResult<()> {
    if server_info.protocol_version.to_string() != expected {
        return Err(UseError::new(
            "use.plugin.runtime.mcp_protocol_mismatch",
            "The MCP server negotiated a protocol version different from the signed release.",
        )
        .with_detail("expectedProtocolVersion", expected)
        .with_detail(
            "negotiatedProtocolVersion",
            server_info.protocol_version.to_string(),
        ));
    }
    Ok(())
}

async fn cancel_probe<Service>(
    service: rmcp::service::RunningService<rmcp::RoleClient, Service>,
    timeout_ms: u64,
) -> UseResult<()>
where
    Service: rmcp::service::Service<rmcp::RoleClient>,
{
    let reason = tokio::time::timeout(Duration::from_millis(timeout_ms), service.cancel())
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.runtime.mcp_probe_cleanup_failed",
                "Timed out closing the standard MCP initialize probe session.",
            )
        })?
        .map_err(|_| {
            UseError::new(
                "use.plugin.runtime.mcp_probe_cleanup_failed",
                "Failed to close the standard MCP initialize probe session.",
            )
        })?;
    match reason {
        QuitReason::Cancelled | QuitReason::Closed => Ok(()),
        QuitReason::JoinError(_) => Err(UseError::new(
            "use.plugin.runtime.mcp_probe_cleanup_failed",
            "The standard MCP initialize probe session closed with a worker failure.",
        )),
    }
}

fn attach_cleanup_error(primary: UseError, cleanup: UseResult<()>) -> UseError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => primary
            .with_detail("cleanupCode", cleanup.code)
            .with_detail("cleanupMessage", cleanup.message),
    }
}

fn unix_time_ms() -> UseResult<u64> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            UseError::new(
                "use.plugin.runtime.mcp_clock_invalid",
                "The host clock is before the Unix epoch during MCP initialization.",
            )
        })?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| {
        UseError::new(
            "use.plugin.runtime.mcp_clock_invalid",
            "The host clock cannot be represented by the MCP initialization contract.",
        )
    })
}

fn valid_gateway_endpoint(endpoint: &Url) -> bool {
    if endpoint.as_str().len() > MAX_GATEWAY_URL_BYTES
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return false;
    }
    match endpoint.scheme() {
        "https" => endpoint.host().is_some(),
        "http" => match endpoint.host() {
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            None => false,
        },
        _ => false,
    }
}

fn valid_runtime_unit_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
}

fn connection_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.mcp_connection_invalid", message)
}
