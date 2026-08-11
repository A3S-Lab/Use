use super::RuntimeServiceProvisioningPhase;

pub(crate) const CHECKPOINT_CRASH_EXIT_CODE: i32 = 87;
pub(crate) const FAULT_CHECKPOINT_ENV: &str = "A3S_USE_TEST_RUNTIME_PROVISIONING_CHECKPOINT";
pub(crate) const REQUESTED_SYNCED: &str = "requested-synced";
pub(crate) const RUNTIME_EFFECT: &str = "runtime-effect";
pub(crate) const RUNTIME_APPLIED_SYNCED: &str = "runtime-applied-synced";
pub(crate) const GATEWAY_EFFECT: &str = "gateway-effect";
pub(crate) const GATEWAY_READY_SYNCED: &str = "gateway-ready-synced";
pub(crate) const BINDING_SYNCED: &str = "binding-synced";

pub(crate) fn crash_after_provisioning_phase(phase: RuntimeServiceProvisioningPhase) {
    let checkpoint = match phase {
        RuntimeServiceProvisioningPhase::Requested => REQUESTED_SYNCED,
        RuntimeServiceProvisioningPhase::RuntimeApplied => RUNTIME_APPLIED_SYNCED,
        RuntimeServiceProvisioningPhase::GatewayReady => GATEWAY_READY_SYNCED,
    };
    crash_after_checkpoint(checkpoint);
}

pub(crate) fn crash_after_checkpoint(checkpoint: &str) {
    if std::env::var(FAULT_CHECKPOINT_ENV).as_deref() == Ok(checkpoint) {
        std::process::exit(CHECKPOINT_CRASH_EXIT_CODE);
    }
}
