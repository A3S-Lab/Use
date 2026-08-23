use a3s_use_core::{PluginHostPlanResult, PluginOperationConfirmation, UseResult};
use async_trait::async_trait;

/// Trusted host boundary for reopening an explicit user confirmation.
///
/// Presentation adapters may request confirmation evidence, but they must not
/// manufacture it from an agent request. The returned value is checked again
/// against the exact durable operation and plan digest before admission.
#[async_trait]
pub trait PluginManagerConfirmationProvider: Send + Sync {
    async fn confirmation_for(
        &self,
        plan: &PluginHostPlanResult,
    ) -> UseResult<Option<PluginOperationConfirmation>>;
}

/// Read-only and unattended-host policy that never claims user confirmation.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedPluginManagerConfirmationProvider;

#[async_trait]
impl PluginManagerConfirmationProvider for FailClosedPluginManagerConfirmationProvider {
    async fn confirmation_for(
        &self,
        plan: &PluginHostPlanResult,
    ) -> UseResult<Option<PluginOperationConfirmation>> {
        plan.validate()?;
        Ok(None)
    }
}
