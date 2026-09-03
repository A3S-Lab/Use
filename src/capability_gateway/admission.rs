use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a3s_use_core::UseResult;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::mcp_error;

const MAX_GATEWAY_IN_FLIGHT: usize = 1_024;
const MAX_GATEWAY_CALLS_PER_WINDOW: usize = 100_000;
const MAX_GATEWAY_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Bounded invocation admission shared by every clone of a Gateway server.
///
/// The limits are deliberately host configuration rather than package data.
/// A permit is held until the provider returns, so an upgrade or drain cannot
/// be hidden behind an unbounded queue of accepted calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityGatewayLimits {
    pub max_in_flight: usize,
    pub max_calls_per_window: usize,
    pub window: Duration,
}

impl Default for CapabilityGatewayLimits {
    fn default() -> Self {
        Self {
            max_in_flight: 32,
            max_calls_per_window: 256,
            window: Duration::from_secs(60),
        }
    }
}

impl CapabilityGatewayLimits {
    pub fn new(
        max_in_flight: usize,
        max_calls_per_window: usize,
        window: Duration,
    ) -> UseResult<Self> {
        let limits = Self {
            max_in_flight,
            max_calls_per_window,
            window,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub(super) fn validate(self) -> UseResult<()> {
        if !(1..=MAX_GATEWAY_IN_FLIGHT).contains(&self.max_in_flight)
            || !(1..=MAX_GATEWAY_CALLS_PER_WINDOW).contains(&self.max_calls_per_window)
            || self.window.is_zero()
            || self.window > MAX_GATEWAY_WINDOW
        {
            return Err(mcp_error(
                "Capability Gateway admission limits are outside the supported bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionFailure {
    InFlight,
    RateLimited,
    StatePoisoned,
}

pub(super) struct GatewayAdmission {
    limits: CapabilityGatewayLimits,
    in_flight: Arc<Semaphore>,
    calls: Mutex<VecDeque<Instant>>,
}

impl std::fmt::Debug for GatewayAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewayAdmission")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl GatewayAdmission {
    pub(super) fn new(limits: CapabilityGatewayLimits) -> UseResult<Self> {
        limits.validate()?;
        Ok(Self {
            in_flight: Arc::new(Semaphore::new(limits.max_in_flight)),
            calls: Mutex::new(VecDeque::with_capacity(limits.max_calls_per_window)),
            limits,
        })
    }

    pub(super) fn try_acquire(&self) -> Result<OwnedSemaphorePermit, AdmissionFailure> {
        let permit = self
            .in_flight
            .clone()
            .try_acquire_owned()
            .map_err(|_| AdmissionFailure::InFlight)?;
        let now = Instant::now();
        let mut calls = self
            .calls
            .lock()
            .map_err(|_| AdmissionFailure::StatePoisoned)?;
        while calls
            .front()
            .is_some_and(|started| now.duration_since(*started) >= self.limits.window)
        {
            calls.pop_front();
        }
        if calls.len() >= self.limits.max_calls_per_window {
            return Err(AdmissionFailure::RateLimited);
        }
        calls.push_back(now);
        Ok(permit)
    }
}
