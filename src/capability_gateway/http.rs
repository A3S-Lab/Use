use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, ORIGIN, RETRY_AFTER, WWW_AUTHENTICATE};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use super::admission::{AdmissionFailure, GatewayAdmission};
use super::{mcp_error, CapabilityGatewayMcpServer};
use a3s_use_core::UseResult;

const MAX_HTTP_TOKEN_BYTES: usize = 4 * 1024;
const MAX_HTTP_ORIGIN_BYTES: usize = 2 * 1024;

/// Explicit credentials and bounded request admission for a Gateway HTTP
/// endpoint. The token is never included in `Debug` output or responses.
#[derive(Clone)]
pub struct CapabilityGatewayHttpConfig {
    pub(super) bearer_token: Arc<str>,
    pub(super) allowed_origin: Option<Arc<str>>,
    pub(super) limits: super::admission::CapabilityGatewayLimits,
}

impl std::fmt::Debug for CapabilityGatewayHttpConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayHttpConfig")
            .field("bearer_token", &"[redacted]")
            .field("allowed_origin", &self.allowed_origin)
            .field("limits", &self.limits)
            .finish()
    }
}

impl CapabilityGatewayHttpConfig {
    /// Create an endpoint configuration from a raw bearer token.
    pub fn new(token: impl Into<String>) -> UseResult<Self> {
        let token = token.into();
        validate_http_token(&token)?;
        Ok(Self {
            bearer_token: format!("Bearer {token}").into(),
            allowed_origin: None,
            limits: Default::default(),
        })
    }

    /// Allow this exact browser Origin when one is present. Without an allowed
    /// origin, requests carrying any Origin header are rejected; native
    /// clients may omit Origin in either mode.
    pub fn with_allowed_origin(mut self, origin: impl Into<String>) -> UseResult<Self> {
        let origin = origin.into();
        validate_http_origin(&origin)?;
        self.allowed_origin = Some(origin.into());
        Ok(self)
    }

    /// Replace the bounded HTTP request limits.
    pub fn with_limits(
        mut self,
        limits: super::admission::CapabilityGatewayLimits,
    ) -> UseResult<Self> {
        limits.validate()?;
        self.limits = limits;
        Ok(self)
    }
}

#[derive(Clone)]
struct CapabilityGatewayHttpGuard {
    authorization: Arc<str>,
    allowed_origin: Option<Arc<str>>,
    admission: Arc<GatewayAdmission>,
}

impl CapabilityGatewayHttpGuard {
    fn new(config: CapabilityGatewayHttpConfig) -> UseResult<Self> {
        Ok(Self {
            authorization: config.bearer_token,
            allowed_origin: config.allowed_origin,
            admission: Arc::new(GatewayAdmission::new(config.limits)?),
        })
    }
}

impl CapabilityGatewayMcpServer {
    /// Serve this frozen Gateway over standard MCP Streamable HTTP.
    ///
    /// The caller owns the listener, bearer credential, Origin policy, and
    /// shutdown token. The route is fixed at `/mcp`; no package or filesystem
    /// details are inferred from HTTP input. This method does not provide TLS:
    /// use a loopback listener or terminate TLS in a trusted reverse proxy
    /// before exposing the endpoint to another host.
    pub async fn serve_streamable_http(
        self,
        listener: tokio::net::TcpListener,
        config: CapabilityGatewayHttpConfig,
        shutdown: CancellationToken,
    ) -> UseResult<()> {
        let service: StreamableHttpService<Self, LocalSessionManager> = StreamableHttpService::new(
            move || Ok(self.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: Some(Duration::from_secs(15)),
            },
        );
        let guard = CapabilityGatewayHttpGuard::new(config)?;
        let router =
            Router::new()
                .nest_service("/mcp", service)
                .layer(middleware::from_fn_with_state(
                    guard,
                    authorize_http_request,
                ));
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .map_err(|_| mcp_error("Capability Gateway HTTP service stopped with an error."))
    }
}

async fn authorize_http_request(
    State(guard): State<CapabilityGatewayHttpGuard>,
    request: Request,
    next: Next,
) -> Response {
    let authorization = request.headers().get_all(AUTHORIZATION);
    let authorized = authorization.iter().count() == 1
        && authorization
            .iter()
            .next()
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                constant_time_eq(value.as_bytes(), guard.authorization.as_bytes())
            });
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Bearer")],
            "Unauthorized",
        )
            .into_response();
    }

    let origins = request.headers().get_all(ORIGIN);
    let origin_values = origins.iter().collect::<Vec<_>>();
    let origin = origin_values.first().and_then(|value| value.to_str().ok());
    let origin_allowed = origin_values.len() <= 1
        && (origin_values
            .first()
            .is_none_or(|value| value.to_str().is_ok()))
        && match (&guard.allowed_origin, origin) {
            (Some(expected), Some(actual)) => {
                constant_time_eq(actual.as_bytes(), expected.as_bytes())
            }
            (Some(_), None) => true,
            (None, None) => true,
            _ => false,
        };
    if !origin_allowed {
        return (StatusCode::FORBIDDEN, "Untrusted Origin").into_response();
    }

    let _permit = match guard.admission.try_acquire() {
        Ok(permit) => permit,
        Err(AdmissionFailure::InFlight | AdmissionFailure::RateLimited) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(RETRY_AFTER, "1")],
                "Rate limit exceeded",
            )
                .into_response();
        }
        Err(AdmissionFailure::StatePoisoned) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "Admission unavailable").into_response();
        }
    };
    next.run(request).await
}

fn validate_http_token(token: &str) -> UseResult<()> {
    if token.is_empty()
        || token.len() > MAX_HTTP_TOKEN_BYTES
        || !token.is_ascii()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(mcp_error(
            "The Capability Gateway bearer token is empty or invalid.",
        ));
    }
    Ok(())
}

fn validate_http_origin(origin: &str) -> UseResult<()> {
    if origin.is_empty()
        || origin.len() > MAX_HTTP_ORIGIN_BYTES
        || !origin.is_ascii()
        || origin
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(mcp_error(
            "The Capability Gateway Origin value is empty or invalid.",
        ));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let left = left.get(index).copied().unwrap_or_default();
        let right = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}
