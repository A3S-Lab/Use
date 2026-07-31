use a3s_use_core::{UseError, UseResult};
use serde::Serialize;

use super::model::StdioMcpSessionPlan;
use super::validation::{host_error, valid_machine_id, valid_sha256};

/// Exact provider process identity returned after spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpProcessIdentity {
    session_id: String,
    plan_digest: String,
    provider_id: String,
    provider_build_id: String,
    capability_digest: String,
    process_id: String,
    started_at_ms: u64,
}

impl StdioMcpProcessIdentity {
    /// Bind a provider's opaque process identity to the exact session plan.
    pub fn new(
        plan: &StdioMcpSessionPlan,
        process_id: impl Into<String>,
        started_at_ms: u64,
    ) -> UseResult<Self> {
        let identity = Self {
            session_id: plan.session_id().to_string(),
            plan_digest: plan.plan_digest().to_string(),
            provider_id: plan.provider().provider_id().to_string(),
            provider_build_id: plan.provider().provider_build_id().to_string(),
            capability_digest: plan.provider().capability_digest().to_string(),
            process_id: process_id.into(),
            started_at_ms,
        };
        identity.validate()?;
        Ok(identity)
    }

    /// Host session identity.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Exact session-plan digest.
    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    /// Provider's opaque OS or sandbox process identity.
    pub fn process_id(&self) -> &str {
        &self.process_id
    }

    /// Provider-observed process start time.
    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    pub(crate) fn validate_against(&self, plan: &StdioMcpSessionPlan) -> UseResult<()> {
        self.validate()?;
        if self.session_id != plan.session_id()
            || self.plan_digest != plan.plan_digest()
            || self.provider_id != plan.provider().provider_id()
            || self.provider_build_id != plan.provider().provider_build_id()
            || self.capability_digest != plan.provider().capability_digest()
        {
            return Err(UseError::new(
                "use.plugin.stdio_mcp.process_identity_mismatch",
                "The spawned stdio MCP process does not bind the exact reviewed session plan and provider.",
            ));
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.session_id)
            || !valid_sha256(&self.plan_digest)
            || !valid_machine_id(&self.provider_id)
            || !valid_machine_id(&self.provider_build_id)
            || !valid_sha256(&self.capability_digest)
            || !valid_machine_id(&self.process_id)
            || self.started_at_ms == 0
        {
            return Err(host_error(
                "A stdio MCP process identity is outside the bounded ownership contract.",
            ));
        }
        Ok(())
    }
}

/// Provider-reported process-unit state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum StdioMcpProcessState {
    /// The exact provider-owned process unit remains active.
    Running,
    /// The complete provider-owned process unit is terminal.
    Exited {
        /// Conventional process exit code, or `None` for signal/provider
        /// termination.
        exit_code: Option<i32>,
    },
}

/// One time-bounded process observation from the injected host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpProcessObservation {
    identity: StdioMcpProcessIdentity,
    state: StdioMcpProcessState,
    observed_at_ms: u64,
}

impl StdioMcpProcessObservation {
    /// Report a live exact provider-owned process unit.
    pub fn running(identity: StdioMcpProcessIdentity, observed_at_ms: u64) -> UseResult<Self> {
        Self::new(identity, StdioMcpProcessState::Running, observed_at_ms)
    }

    /// Report a terminal exact provider-owned process unit.
    pub fn exited(
        identity: StdioMcpProcessIdentity,
        exit_code: Option<i32>,
        observed_at_ms: u64,
    ) -> UseResult<Self> {
        Self::new(
            identity,
            StdioMcpProcessState::Exited { exit_code },
            observed_at_ms,
        )
    }

    /// Exact process identity.
    pub fn identity(&self) -> &StdioMcpProcessIdentity {
        &self.identity
    }

    /// Current process state.
    pub fn state(&self) -> StdioMcpProcessState {
        self.state
    }

    /// Provider observation time.
    pub fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }

    pub(crate) fn validate_against(&self, plan: &StdioMcpSessionPlan) -> UseResult<()> {
        self.identity.validate_against(plan)?;
        if self.observed_at_ms < self.identity.started_at_ms {
            return Err(host_error(
                "A stdio MCP process observation predates the exact process identity.",
            ));
        }
        Ok(())
    }

    fn new(
        identity: StdioMcpProcessIdentity,
        state: StdioMcpProcessState,
        observed_at_ms: u64,
    ) -> UseResult<Self> {
        identity.validate()?;
        if observed_at_ms < identity.started_at_ms {
            return Err(host_error(
                "A stdio MCP process observation predates process startup.",
            ));
        }
        Ok(Self {
            identity,
            state,
            observed_at_ms,
        })
    }
}
