use a3s_use_core::{InstallationId, PluginOperationAction, PluginSurfaceRef, UseResult};
use async_trait::async_trait;

use crate::plugin_lifecycle::PluginLifecycleAction;

use super::model::{
    input_error, valid_error_code, valid_sha256, ControlCapabilityEffectAuthority,
    ControlPackageEffectAuthority, ControlRuntimeBindingObservation, ControlRuntimeEffectAuthority,
};

/// Classification returned by an external effect owner.
///
/// `Deferred` and `Rejected` both mean the owner can prove that it accepted no
/// effect. A deferral is transient and becomes eligible for bounded same-key
/// retry; rejection is terminal under the committed policy. `Unknown` means
/// acceptance is ambiguous and therefore requires explicit same-key
/// reconciliation. Provider ports return this enum directly so an ordinary
/// transport error can never be mistaken for a safe no-effect result.
pub(in crate::control_store) enum ControlEffectPortOutcome<T> {
    Applied(T),
    /// The owner proves that it accepted no effect, but a bounded same-key
    /// retry may succeed after transient contention or unavailability.
    Deferred(ControlEffectFailure),
    Rejected(ControlEffectFailure),
    Unknown(ControlEffectFailure),
}

impl<T> ControlEffectPortOutcome<T> {
    pub(in crate::control_store) fn applied(application: T) -> Self {
        Self::Applied(application)
    }

    pub(in crate::control_store) fn rejected(failure: ControlEffectFailure) -> Self {
        Self::Rejected(failure)
    }

    pub(in crate::control_store) fn deferred(failure: ControlEffectFailure) -> Self {
        Self::Deferred(failure)
    }

    pub(in crate::control_store) fn unknown(failure: ControlEffectFailure) -> Self {
        Self::Unknown(failure)
    }

    pub(in crate::control_store) fn map<U>(
        self,
        map: impl FnOnce(T) -> U,
    ) -> ControlEffectPortOutcome<U> {
        match self {
            Self::Applied(application) => ControlEffectPortOutcome::Applied(map(application)),
            Self::Deferred(failure) => ControlEffectPortOutcome::Deferred(failure),
            Self::Rejected(failure) => ControlEffectPortOutcome::Rejected(failure),
            Self::Unknown(failure) => ControlEffectPortOutcome::Unknown(failure),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectFailure {
    pub(in crate::control_store) evidence_digest: String,
    pub(in crate::control_store) error_code: String,
}

impl ControlEffectFailure {
    pub(in crate::control_store) fn new(
        evidence_digest: impl Into<String>,
        error_code: impl Into<String>,
    ) -> UseResult<Self> {
        let failure = Self {
            evidence_digest: evidence_digest.into(),
            error_code: error_code.into(),
        };
        if !valid_sha256(&failure.evidence_digest) || !valid_error_code(&failure.error_code) {
            return Err(input_error(
                "Control effect failure evidence is invalid or unbounded.",
            ));
        }
        Ok(failure)
    }
}

/// Identity shared by every typed owner request.
///
/// The deadline is claim metadata, not desired state. Owners must finish
/// before it, while every durable identity remains the exact committed outbox
/// identity across retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectRequestIdentity {
    pub(in crate::control_store) operation_id: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) operation_action: PluginOperationAction,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) sequence: u32,
    pub(in crate::control_store) idempotency_key: String,
    pub(in crate::control_store) required: bool,
    pub(in crate::control_store) attempt: u32,
    pub(in crate::control_store) deadline_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityCutoverRequest {
    pub(in crate::control_store) identity: ControlEffectRequestIdentity,
    pub(in crate::control_store) authority: ControlCapabilityEffectAuthority,
    pub(in crate::control_store) expected_capability_generation: u64,
    pub(in crate::control_store) capability_generation: u64,
    pub(in crate::control_store) descriptor_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlInvocationDrainRequest {
    pub(in crate::control_store) identity: ControlEffectRequestIdentity,
    pub(in crate::control_store) authority: ControlPackageEffectAuthority,
    pub(in crate::control_store) package_id: String,
    pub(in crate::control_store) lifecycle_generation: u64,
    pub(in crate::control_store) package_digest: String,
    pub(in crate::control_store) manifest_digest: String,
    pub(in crate::control_store) lifecycle_action: PluginLifecycleAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::control_store) enum ControlSurfaceEffectAction {
    Prepare,
    Stop,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlSurfaceEffectRequest {
    pub(in crate::control_store) identity: ControlEffectRequestIdentity,
    pub(in crate::control_store) authority: ControlPackageEffectAuthority,
    pub(in crate::control_store) package_id: String,
    pub(in crate::control_store) lifecycle_generation: u64,
    pub(in crate::control_store) package_digest: String,
    pub(in crate::control_store) manifest_digest: String,
    pub(in crate::control_store) lifecycle_action: PluginLifecycleAction,
    pub(in crate::control_store) surface: PluginSurfaceRef,
    pub(in crate::control_store) action: ControlSurfaceEffectAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlRuntimeEffectRequest {
    pub(in crate::control_store) surface: ControlSurfaceEffectRequest,
    pub(in crate::control_store) authority: ControlRuntimeEffectAuthority,
    pub(in crate::control_store) provider_id: String,
    pub(in crate::control_store) selection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlReceiptApplication {
    pub(in crate::control_store) receipt_digest: String,
}

impl ControlReceiptApplication {
    pub(in crate::control_store) fn new(receipt_digest: impl Into<String>) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
        };
        if !valid_sha256(&application.receipt_digest) {
            return Err(input_error(
                "Control effect receipt evidence must be a canonical SHA-256 digest.",
            ));
        }
        Ok(application)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlRuntimeApplication {
    pub(in crate::control_store) receipt_digest: String,
    pub(in crate::control_store) binding: Option<ControlRuntimeBindingObservation>,
}

impl ControlRuntimeApplication {
    pub(in crate::control_store) fn new(
        request: &ControlRuntimeEffectRequest,
        receipt_digest: impl Into<String>,
        binding: Option<ControlRuntimeBindingObservation>,
    ) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
            binding,
        };
        let binding_matches = match request.surface.action {
            ControlSurfaceEffectAction::Prepare => application
                .binding
                .as_ref()
                .is_some_and(ControlRuntimeBindingObservation::validate),
            ControlSurfaceEffectAction::Stop | ControlSurfaceEffectAction::Remove => {
                application.binding.is_none()
            }
        };
        if !valid_sha256(&application.receipt_digest) || !binding_matches {
            return Err(input_error(
                "Runtime effect application evidence does not match its typed request.",
            ));
        }
        Ok(application)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlSurfaceApplication {
    pub(in crate::control_store) receipt_digest: String,
    pub(in crate::control_store) materialization_digest: Option<String>,
}

impl ControlSurfaceApplication {
    pub(in crate::control_store) fn new(
        request: &ControlSurfaceEffectRequest,
        receipt_digest: impl Into<String>,
        materialization_digest: Option<String>,
    ) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
            materialization_digest,
        };
        let materialization_matches = match request.action {
            ControlSurfaceEffectAction::Prepare => application
                .materialization_digest
                .as_deref()
                .is_some_and(valid_sha256),
            ControlSurfaceEffectAction::Stop | ControlSurfaceEffectAction::Remove => {
                application.materialization_digest.is_none()
            }
        };
        if !valid_sha256(&application.receipt_digest) || !materialization_matches {
            return Err(input_error(
                "Surface effect application evidence does not match its typed request.",
            ));
        }
        Ok(application)
    }
}

#[async_trait]
pub(in crate::control_store) trait ControlCapabilityIndexEffectPort:
    Send + Sync
{
    async fn cutover(
        &self,
        request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlInvocationLeaseEffectPort:
    Send + Sync
{
    async fn drain(
        &self,
        request: &ControlInvocationDrainRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlRuntimeEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlFlowEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlKnowledgeEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlSkillEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlUiEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}
