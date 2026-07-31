use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{StoredWorkspaceGrant, WorkspaceGrantStore};
use serde::Serialize;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::model::{StdioMcpProcessControl, StdioMcpSessionPlan};
use super::validation::unix_time_ms;

/// Current durable authorization state for one supervised stdio MCP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StdioMcpAuthorizationState {
    /// The exact planned grant revision and digest remain active.
    Active,
    /// The planned grant reached its absolute expiration time.
    Expired,
    /// The exact package-generation grant record disappeared.
    Missing,
    /// The exact package-generation grant was explicitly revoked.
    Revoked,
    /// Another grant revision or digest replaced the planned grant.
    Changed,
    /// The durable grant record could not be checked within its bound.
    Unavailable,
}

/// Last bounded durable-grant observation for a supervised stdio MCP session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpAuthorizationObservation {
    state: StdioMcpAuthorizationState,
    checked_at_ms: Option<u64>,
    observed_revision: Option<u64>,
    failure_code: Option<String>,
}

impl StdioMcpAuthorizationObservation {
    /// Current authorization state.
    pub fn state(&self) -> StdioMcpAuthorizationState {
        self.state
    }

    /// Host time for the completed check, absent only when the host clock failed.
    pub fn checked_at_ms(&self) -> Option<u64> {
        self.checked_at_ms
    }

    /// Durable grant or revocation revision observed at the exact record path.
    pub fn observed_revision(&self) -> Option<u64> {
        self.observed_revision
    }

    /// Stable failure code for a non-active observation.
    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
    }
}

#[derive(Debug, Clone)]
struct AuthorizationUpdate {
    observation: StdioMcpAuthorizationObservation,
    error: Option<UseError>,
}

pub(crate) struct AuthorizationMonitor {
    receiver: watch::Receiver<Option<AuthorizationUpdate>>,
}

impl AuthorizationMonitor {
    pub(crate) fn start(
        grants: WorkspaceGrantStore,
        plan: StdioMcpSessionPlan,
        control: Arc<dyn StdioMcpProcessControl>,
        process_done: CancellationToken,
    ) -> Self {
        let (sender, receiver) = watch::channel(None);
        tokio::spawn(async move {
            loop {
                let update = tokio::select! {
                    _ = process_done.cancelled() => return,
                    update = inspect_authorization(&grants, &plan) => update,
                };
                let active = update.error.is_none();
                sender.send_replace(Some(update));
                if !active {
                    control.terminate();
                    return;
                }

                let expiry = wait_for_optional_expiry(plan.grant_expires_at_ms());
                tokio::pin!(expiry);
                tokio::select! {
                    _ = process_done.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_millis(
                        plan.authorization_recheck_interval_ms(),
                    )) => {}
                    result = &mut expiry => {
                        let update = match result {
                            Ok(()) => authorization_failure(
                                StdioMcpAuthorizationState::Expired,
                                unix_time_ms().ok(),
                                Some(plan.grant_revision()),
                                UseError::new(
                                    "use.plugin.stdio_mcp.grant_expired",
                                    "The stdio MCP workspace grant expired.",
                                ),
                            ),
                            Err(error) => unavailable(error),
                        };
                        sender.send_replace(Some(update));
                        control.terminate();
                        return;
                    }
                }
            }
        });
        Self { receiver }
    }

    pub(crate) async fn wait_initial(&mut self, timeout: Duration) -> UseResult<()> {
        if let Some(update) = self.receiver.borrow().clone() {
            return update.error.map_or(Ok(()), Err);
        }
        let receiver = &mut self.receiver;
        tokio::time::timeout(timeout, async {
            loop {
                receiver.changed().await.map_err(|_| {
                    UseError::new(
                        "use.plugin.stdio_mcp.grant_monitor_failed",
                        "The stdio MCP grant monitor ended before its initial durable check.",
                    )
                })?;
                if let Some(update) = receiver.borrow().clone() {
                    return update.error.map_or(Ok(()), Err);
                }
            }
        })
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.grant_observation_timeout",
                "Timed out performing the initial durable stdio MCP grant check.",
            )
        })?
    }

    pub(crate) fn observation(&self) -> UseResult<StdioMcpAuthorizationObservation> {
        self.receiver
            .borrow()
            .as_ref()
            .map(|update| update.observation.clone())
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.stdio_mcp.grant_monitor_pending",
                    "The stdio MCP grant monitor has not completed its initial durable check.",
                )
            })
    }

    pub(crate) fn failure(&self) -> Option<UseError> {
        self.receiver
            .borrow()
            .as_ref()
            .and_then(|update| update.error.clone())
    }
}

async fn inspect_authorization(
    grants: &WorkspaceGrantStore,
    plan: &StdioMcpSessionPlan,
) -> AuthorizationUpdate {
    let checked_at_ms = match unix_time_ms() {
        Ok(checked_at_ms) => checked_at_ms,
        Err(error) => return unavailable(error),
    };
    if plan
        .grant_expires_at_ms()
        .is_some_and(|expires_at_ms| checked_at_ms >= expires_at_ms)
    {
        return authorization_failure(
            StdioMcpAuthorizationState::Expired,
            Some(checked_at_ms),
            Some(plan.grant_revision()),
            UseError::new(
                "use.plugin.stdio_mcp.grant_expired",
                "The stdio MCP workspace grant expired.",
            ),
        );
    }

    let observed = tokio::time::timeout(
        Duration::from_millis(plan.authorization_recheck_interval_ms()),
        grants.observe(plan.scope_id(), plan.package_id(), plan.package_digest()),
    )
    .await;
    let record = match observed {
        Err(_) => {
            return unavailable(UseError::new(
                "use.plugin.stdio_mcp.grant_observation_timeout",
                "Timed out rechecking the durable stdio MCP workspace grant.",
            ))
        }
        Ok(Err(error)) => {
            return unavailable(
                UseError::new(
                    "use.plugin.stdio_mcp.grant_observation_failed",
                    "Failed to recheck the durable stdio MCP workspace grant.",
                )
                .with_detail("storeCode", error.code)
                .with_detail("storeMessage", error.message),
            )
        }
        Ok(Ok(record)) => record,
    };

    match record {
        None => authorization_failure(
            StdioMcpAuthorizationState::Missing,
            Some(checked_at_ms),
            None,
            UseError::new(
                "use.plugin.stdio_mcp.grant_missing",
                "The stdio MCP package-generation workspace grant no longer exists.",
            ),
        ),
        Some(StoredWorkspaceGrant::Revoked(revocation)) => authorization_failure(
            StdioMcpAuthorizationState::Revoked,
            Some(checked_at_ms),
            Some(revocation.revision),
            UseError::new(
                "use.plugin.stdio_mcp.grant_revoked",
                "The stdio MCP package-generation workspace grant was revoked.",
            ),
        ),
        Some(StoredWorkspaceGrant::Granted(receipt))
            if receipt.revision == plan.grant_revision()
                && receipt.grant_digest == plan.grant_digest() =>
        {
            AuthorizationUpdate {
                observation: StdioMcpAuthorizationObservation {
                    state: StdioMcpAuthorizationState::Active,
                    checked_at_ms: Some(checked_at_ms),
                    observed_revision: Some(receipt.revision),
                    failure_code: None,
                },
                error: None,
            }
        }
        Some(StoredWorkspaceGrant::Granted(receipt)) => authorization_failure(
            StdioMcpAuthorizationState::Changed,
            Some(checked_at_ms),
            Some(receipt.revision),
            UseError::new(
                "use.plugin.stdio_mcp.grant_changed",
                "The active workspace grant changed after the stdio MCP session started.",
            ),
        ),
    }
}

fn authorization_failure(
    state: StdioMcpAuthorizationState,
    checked_at_ms: Option<u64>,
    observed_revision: Option<u64>,
    error: UseError,
) -> AuthorizationUpdate {
    AuthorizationUpdate {
        observation: StdioMcpAuthorizationObservation {
            state,
            checked_at_ms,
            observed_revision,
            failure_code: Some(error.code.clone()),
        },
        error: Some(error),
    }
}

fn unavailable(error: UseError) -> AuthorizationUpdate {
    authorization_failure(
        StdioMcpAuthorizationState::Unavailable,
        unix_time_ms().ok(),
        None,
        error,
    )
}

async fn wait_for_optional_expiry(expires_at_ms: Option<u64>) -> UseResult<()> {
    let Some(expires_at_ms) = expires_at_ms else {
        return pending::<UseResult<()>>().await;
    };
    const MAX_CLOCK_RECHECK_MS: u64 = 60 * 60 * 1000;
    loop {
        let now_ms = unix_time_ms()?;
        if now_ms >= expires_at_ms {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(
            (expires_at_ms - now_ms).min(MAX_CLOCK_RECHECK_MS),
        ))
        .await;
    }
}
