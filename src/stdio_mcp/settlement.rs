use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ExtensionRouteLease, InstalledExtension};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::model::{StdioMcpProcessControl, StdioMcpSessionPlan};
use super::process_model::{StdioMcpProcessObservation, StdioMcpProcessState};

const SETTLEMENT_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Exact active package-generation lease accepted by the stdio MCP lifecycle.
///
/// Constructing this type from [`ExtensionRouteLease`] keeps disable,
/// uninstall, and generation cleanup blocked until the supervised process unit
/// reaches terminal state.
pub struct StdioMcpPackageLease {
    extension: InstalledExtension,
    _guard: Box<dyn Send + Sync>,
}

impl StdioMcpPackageLease {
    /// Wrap one registry-validated active package lease.
    pub fn from_route_lease(lease: ExtensionRouteLease) -> Self {
        Self {
            extension: lease.extension().clone(),
            _guard: Box::new(lease),
        }
    }

    /// Installed immutable package generation pinned by this lease.
    pub fn extension(&self) -> &InstalledExtension {
        &self.extension
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        extension: InstalledExtension,
        guard: impl Send + Sync + 'static,
    ) -> Self {
        Self {
            extension,
            _guard: Box::new(guard),
        }
    }
}

impl fmt::Debug for StdioMcpPackageLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioMcpPackageLease")
            .field("package_id", &self.extension.receipt.package_id)
            .field("package_root", &self.extension.receipt.package_root)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
enum SettlementState {
    Pending,
    Fault(UseError),
    Terminal(StdioMcpProcessObservation),
}

pub(crate) struct LeaseSettlement {
    receiver: watch::Receiver<SettlementState>,
    process_done: CancellationToken,
}

impl LeaseSettlement {
    pub(crate) fn start(
        lease: StdioMcpPackageLease,
        plan: StdioMcpSessionPlan,
        control: Arc<dyn StdioMcpProcessControl>,
    ) -> Self {
        let (sender, receiver) = watch::channel(SettlementState::Pending);
        let process_done = CancellationToken::new();
        let completion = process_done.clone();
        tokio::spawn(async move {
            loop {
                match terminal_observation(control.wait_for_exit().await, &plan) {
                    Ok(observation) => {
                        sender.send_replace(SettlementState::Terminal(observation));
                        completion.cancel();
                        drop(lease);
                        return;
                    }
                    Err(error) => {
                        sender.send_replace(SettlementState::Fault(error));
                        control.terminate();
                        tokio::time::sleep(SETTLEMENT_RETRY_INTERVAL).await;
                    }
                }
            }
        });
        Self {
            receiver,
            process_done,
        }
    }

    pub(crate) fn process_done(&self) -> CancellationToken {
        self.process_done.clone()
    }

    pub(crate) async fn wait(
        &mut self,
        timeout: Duration,
    ) -> UseResult<Option<StdioMcpProcessObservation>> {
        let receiver = &mut self.receiver;
        let completed = tokio::time::timeout(timeout, async {
            loop {
                match receiver.borrow().clone() {
                    SettlementState::Pending => {}
                    SettlementState::Fault(error) => return Err(error),
                    SettlementState::Terminal(observation) => return Ok(observation),
                }
                receiver.changed().await.map_err(|_| {
                    UseError::new(
                        "use.plugin.stdio_mcp.lease_monitor_failed",
                        "The stdio MCP lease monitor ended without terminal process evidence.",
                    )
                })?;
            }
        })
        .await;
        match completed {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }
}

fn terminal_observation(
    result: UseResult<StdioMcpProcessObservation>,
    plan: &StdioMcpSessionPlan,
) -> UseResult<StdioMcpProcessObservation> {
    let observation = result.map_err(|error| {
        UseError::new(
            "use.plugin.stdio_mcp.lease_monitor_failed",
            "The stdio MCP host failed while waiting for terminal process-unit evidence.",
        )
        .with_detail("hostCode", error.code)
        .with_detail("hostMessage", error.message)
    })?;
    observation.validate_against(plan)?;
    if !matches!(observation.state(), StdioMcpProcessState::Exited { .. }) {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.host_invalid",
            "The stdio MCP host completed its exit wait with a live process.",
        ));
    }
    Ok(observation)
}
