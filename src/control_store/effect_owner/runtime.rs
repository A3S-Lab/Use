#![allow(dead_code)]

use std::sync::Arc;

use a3s_runtime::contract::{RuntimeObservation, RuntimeServiceEndpoint};
use a3s_use_core::{PluginSurfaceKind, UseError, UseResult};
use a3s_use_extension::{ArtifactStore, PluginMcpSurface, ToolSurface};
use async_trait::async_trait;

use super::super::effect_port::{
    ControlEffectPortOutcome, ControlRuntimeApplication,
    ControlRuntimeEffectPort as ControlRuntimeEffectPortTrait, ControlRuntimeEffectRequest,
    ControlSurfaceEffectAction,
};
use super::super::model::ControlEffectOwner;
use crate::plugin_runtime::{
    RuntimeBindingReceipt, RuntimeBindingStore, RuntimeEndpointRef, RuntimeMcpInitializeEvidence,
    RuntimeProviderSelection, RuntimeServiceBindingReceipt, RuntimeServiceProvisioningPhase,
    RuntimeServiceProvisioningReceipt, RuntimeServiceReadinessEvidence, RuntimeSurfaceContract,
    RuntimeSurfacePlan, SelectedRuntimeSurface,
};

mod evidence;
mod validation;

use evidence::{
    authority_error, before_effect_failure, checkpoint_application, prepare_application, rejected,
    runtime_error, runtime_request_id, service_endpoint, unknown,
};
use validation::{
    validate_mcp_payload, validate_plan_identity, validate_receipt, validate_tool_payload,
};

const RUNTIME_RECEIPT_DOMAIN: &[u8] = b"a3s.use.control-runtime-receipt.v1\0";
const RUNTIME_FAILURE_DOMAIN: &[u8] = b"a3s.use.control-runtime-failure.v1\0";
const RUNTIME_AUTHORITY_ERROR: &str = "use.control_store.runtime_authority_invalid";
const RUNTIME_OWNER_ERROR: &str = "use.control_store.runtime_owner_failed";
const RUNTIME_PLAN_ERROR: &str = "use.control_store.runtime_plan_invalid";
const RUNTIME_PENDING_ERROR: &str = "use.control_store.runtime_pending_recovery";

/// Readiness result returned by the typed Gateway adapter for a Streamable
/// HTTP MCP service. The endpoint is opaque and the initialize evidence is
/// retained only in the Runtime service receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlRuntimeMcpReadiness {
    pub(in crate::control_store) endpoint: RuntimeEndpointRef,
    pub(in crate::control_store) initialize: RuntimeMcpInitializeEvidence,
}

/// Narrow Gateway boundary used by the inactive Control Runtime owner.
///
/// It deliberately has no lifecycle intent, package root, or legacy Registry
/// parameter. Implementations receive only the already verified manifest
/// surface, reviewed Runtime plan, and provider observation. A production
/// host can adapt its Gateway client to this trait during the coordinated
/// Control cutover.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub(in crate::control_store) trait ControlRuntimeServiceReadinessPort:
    Send + Sync
{
    async fn bind_tool_service(
        &self,
        surface: &ToolSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeEndpointRef>;

    async fn bind_mcp_service(
        &self,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<ControlRuntimeMcpReadiness>;

    async fn drain_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<()>;

    async fn remove_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        idempotency_key: &str,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<()>;
}

/// Committed-authority Runtime owner for managed Tool and MCP surfaces.
///
/// The owner is intentionally usable before production cutover: its plan
/// selection is injected as a typed `RuntimeProviderSelection`, while the
/// committed Control request remains the sole desired-state authority. It
/// never resolves a provider from a package path or from a legacy lifecycle
/// record. Durable Runtime receipts and Service provisioning records are used
/// for exact-generation replay and crash recovery.
#[derive(Clone)]
pub(in crate::control_store) struct ControlRuntimeEffectPort {
    artifact_store: ArtifactStore,
    selection: RuntimeProviderSelection,
    bindings: RuntimeBindingStore,
    readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
    deadline_at_ms: Option<u64>,
}

impl ControlRuntimeEffectPort {
    pub(in crate::control_store) fn new(
        artifact_store: ArtifactStore,
        selection: RuntimeProviderSelection,
        bindings: RuntimeBindingStore,
        readiness: Arc<dyn ControlRuntimeServiceReadinessPort>,
    ) -> Self {
        Self {
            artifact_store,
            selection,
            bindings,
            readiness,
            deadline_at_ms: None,
        }
    }

    pub(in crate::control_store) fn with_deadline_at_ms(
        mut self,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<Self> {
        if deadline_at_ms == Some(0) {
            return Err(runtime_error(
                RUNTIME_AUTHORITY_ERROR,
                "A Runtime owner deadline must be positive when present.",
            ));
        }
        self.deadline_at_ms = deadline_at_ms;
        Ok(self)
    }

    async fn apply(
        &self,
        request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let selected = match self.validate_request(request) {
            Ok(selected) => selected,
            Err(error) => return rejected(request, error.code),
        };
        match request.surface.action {
            ControlSurfaceEffectAction::Prepare => self.prepare(request, selected).await,
            ControlSurfaceEffectAction::Stop => self.stop(request, selected).await,
            ControlSurfaceEffectAction::Remove => self.remove(request, selected).await,
        }
    }

    fn validate_request(
        &self,
        request: &ControlRuntimeEffectRequest,
    ) -> UseResult<SelectedRuntimeSurface> {
        let kind = request.surface.surface.kind;
        if !matches!(kind, PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp) {
            return Err(runtime_error(
                RUNTIME_AUTHORITY_ERROR,
                "Runtime effects can target only Tool or MCP surfaces.",
            ));
        }
        if request.surface.authority != request.authority.package {
            return Err(runtime_error(
                RUNTIME_AUTHORITY_ERROR,
                "The Runtime request carries inconsistent committed package authority.",
            ));
        }
        self.bindings
            .installation()
            .ensure_same(&request.surface.identity.installation)
            .map_err(|_| authority_error())?;
        request
            .surface
            .validate_for_owner(
                kind,
                ControlEffectOwner::RuntimeProvider {
                    provider_id: request.provider_id.clone(),
                    selection_digest: request.selection_digest.clone(),
                },
            )
            .map_err(|_| authority_error())?;
        request.authority.provider_selection.validate()?;
        let committed = &request.authority.provider_selection;
        if committed.evidence.provider_id != request.provider_id
            || committed.selection_digest != request.selection_digest
            || committed.qualified_surface().package_id != request.surface.package_id
            || committed.qualified_surface().surface != request.surface.surface
        {
            return Err(authority_error());
        }
        let selected = self
            .selection
            .surfaces()
            .iter()
            .find(|candidate| candidate.plan().surface() == *committed.qualified_surface())
            .cloned()
            .ok_or_else(|| {
                runtime_error(
                    RUNTIME_PLAN_ERROR,
                    "The injected Runtime selection has no exact committed surface.",
                )
            })?;
        if selected.provider() != &committed.evidence {
            return Err(runtime_error(
                RUNTIME_AUTHORITY_ERROR,
                "The injected Runtime provider evidence differs from committed authority.",
            ));
        }
        validate_plan_identity(&request.surface, &request.authority, &selected)?;
        Ok(selected)
    }

    async fn prepare(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: SelectedRuntimeSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let qualified = selected.plan().surface();
        let existing = match self
            .bindings
            .get_generation(
                &request.surface.identity.installation,
                &qualified,
                request.surface.lifecycle_generation,
            )
            .await
        {
            Ok(existing) => existing,
            Err(error) => return before_effect_failure(request, "binding-read", error),
        };
        if let Some(receipt) = existing {
            if let Err(error) = validate_receipt(request, &selected, &receipt) {
                return rejected(request, error.code);
            }
            if let Err(error) = self
                .reconcile_committed_provisioning(request, &selected, &receipt)
                .await
            {
                return before_effect_failure(request, "binding-reconcile", error);
            }
            return prepare_application(request, &receipt, "binding-replay");
        }

        let surface = match self.read_artifact(request, &selected).await {
            Ok(value) => value,
            Err(error) => return before_effect_failure(request, "artifact-read", error),
        };
        if !matches!(
            selected.plan().contract(),
            RuntimeSurfaceContract::ToolTask { .. }
        ) {
            if let Err(error) = selected
                .client()
                .verify_plan(selected.plan(), selected.provider())
                .await
            {
                return before_effect_failure(request, "provider-verify", error);
            }
        }

        match (selected.plan().contract(), surface) {
            (RuntimeSurfaceContract::ToolTask { .. }, ManagedSurface::Tool(_)) => {
                self.prepare_task(request, &selected).await
            }
            (RuntimeSurfaceContract::ToolService { .. }, ManagedSurface::Tool(surface)) => {
                self.prepare_service(request, &selected, ManagedSurface::Tool(surface))
                    .await
            }
            (RuntimeSurfaceContract::McpService { .. }, ManagedSurface::Mcp(surface)) => {
                self.prepare_service(request, &selected, ManagedSurface::Mcp(surface))
                    .await
            }
            _ => rejected(request, RUNTIME_PLAN_ERROR),
        }
    }

    async fn read_artifact(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: &SelectedRuntimeSurface,
    ) -> UseResult<ManagedSurface> {
        let package = self
            .artifact_store
            .acquire_verified_package(&request.authority.package.package.package.catalog)
            .await?;
        match request.surface.surface.kind {
            PluginSurfaceKind::Tool => {
                let payload = package
                    .read_tool_surface(&request.surface.surface.id)
                    .await?;
                validate_tool_payload(selected.plan(), &payload)?;
                Ok(ManagedSurface::Tool(payload.surface().clone()))
            }
            PluginSurfaceKind::Mcp => {
                let payload = package
                    .read_mcp_surface(&request.surface.surface.id)
                    .await?;
                validate_mcp_payload(selected.plan(), &payload)?;
                Ok(ManagedSurface::Mcp(payload.surface().clone()))
            }
            PluginSurfaceKind::Flow
            | PluginSurfaceKind::Okf
            | PluginSurfaceKind::Skill
            | PluginSurfaceKind::Ui => Err(authority_error()),
        }
    }

    async fn prepare_task(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: &SelectedRuntimeSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let binding = match selected
            .client()
            .prepare_task(selected.plan(), selected.provider())
            .await
        {
            Ok(binding) => binding,
            Err(error) => return before_effect_failure(request, "task-prepare", error),
        };
        let receipt = RuntimeBindingReceipt::Task(binding);
        if let Err(error) = validate_receipt(request, selected, &receipt) {
            return rejected(request, error.code);
        }
        if let Err(error) = self.bindings.put(&receipt).await {
            return unknown(request, "task-receipt", error);
        }
        prepare_application(request, &receipt, "task-prepare")
    }

    async fn prepare_service(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: &SelectedRuntimeSurface,
        surface: ManagedSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let readiness = &self.readiness;
        let lifecycle_key = request.surface.identity.idempotency_key.clone();
        let apply_request_id = runtime_request_id("apply", &lifecycle_key);
        let expected = match RuntimeServiceProvisioningReceipt::from_plan(
            selected.plan(),
            selected.provider(),
            lifecycle_key.clone(),
            apply_request_id.clone(),
        ) {
            Ok(expected) => expected,
            Err(error) => return rejected(request, error.code),
        };
        let mut provisioning = match self
            .bindings
            .get_provisioning(&expected.scope, &expected.surface, expected.generation)
            .await
        {
            Ok(Some(current)) => {
                match current.matches_plan(
                    selected.plan(),
                    selected.provider(),
                    &lifecycle_key,
                    &apply_request_id,
                ) {
                    Ok(true) => current,
                    Ok(false) => return rejected(request, RUNTIME_AUTHORITY_ERROR),
                    Err(error) => return rejected(request, error.code),
                }
            }
            Ok(None) => {
                if let Err(error) = self.bindings.put_provisioning(&expected).await {
                    return before_effect_failure(request, "provisioning-start", error);
                }
                expected
            }
            Err(error) => return before_effect_failure(request, "provisioning-read", error),
        };

        if provisioning.phase == RuntimeServiceProvisioningPhase::Requested {
            let activation = match selected
                .client()
                .apply_service(
                    selected.plan(),
                    selected.provider(),
                    apply_request_id.clone(),
                    self.deadline(request),
                )
                .await
            {
                Ok(activation) => activation,
                Err(error) => return unknown(request, "service-apply", error),
            };
            let observation = activation.observation().clone();
            if let Err(error) = provisioning.record_runtime_observation(
                selected.plan(),
                selected.provider(),
                observation,
            ) {
                return unknown(request, "service-observation", error);
            }
            if let Err(error) = self.bindings.put_provisioning(&provisioning).await {
                return unknown(request, "runtime-observation", error);
            }
        }

        if provisioning.phase == RuntimeServiceProvisioningPhase::RuntimeApplied {
            let observation = match provisioning.observation.as_ref() {
                Some(observation) => observation,
                None => {
                    return unknown(
                        request,
                        "service-observation",
                        runtime_error(
                            RUNTIME_PENDING_ERROR,
                            "Runtime provisioning omitted its observation.",
                        ),
                    )
                }
            };
            let runtime_endpoint = match service_endpoint(selected.plan(), observation) {
                Ok(endpoint) => endpoint,
                Err(error) => return unknown(request, "service-endpoint", error),
            };
            let (endpoint, readiness_evidence) = match surface {
                ManagedSurface::Tool(surface) => match readiness
                    .bind_tool_service(
                        &surface,
                        selected.plan(),
                        observation,
                        &runtime_endpoint,
                        &lifecycle_key,
                        self.deadline(request),
                    )
                    .await
                {
                    Ok(endpoint) => (endpoint, RuntimeServiceReadinessEvidence::HttpHealthy),
                    Err(error) => return unknown(request, "gateway-bind", error),
                },
                ManagedSurface::Mcp(surface) => match readiness
                    .bind_mcp_service(
                        &surface,
                        selected.plan(),
                        observation,
                        &runtime_endpoint,
                        &lifecycle_key,
                        self.deadline(request),
                    )
                    .await
                {
                    Ok(result) => (
                        result.endpoint,
                        RuntimeServiceReadinessEvidence::McpInitialized {
                            initialize: result.initialize,
                        },
                    ),
                    Err(error) => return unknown(request, "gateway-bind", error),
                },
            };
            if let Err(error) = provisioning.record_gateway_readiness(endpoint, readiness_evidence)
            {
                return unknown(request, "gateway-evidence", error);
            }
            if let Err(error) = self.bindings.put_provisioning(&provisioning).await {
                return unknown(request, "gateway-observation", error);
            }
        }

        let receipt = match provisioning.binding_receipt() {
            Ok(receipt) => RuntimeBindingReceipt::Service(receipt),
            Err(error) => return unknown(request, "service-receipt", error),
        };
        if let Err(error) = validate_receipt(request, selected, &receipt) {
            return rejected(request, error.code);
        }
        if let Err(error) = self
            .bindings
            .commit_provisioning(&provisioning, &receipt)
            .await
        {
            return unknown(request, "service-commit", error);
        }
        prepare_application(request, &receipt, "service-prepare")
    }

    /// Retire the crash-safe overlap left when the final binding was synced
    /// before its Gateway-ready provisioning record could be removed.
    async fn reconcile_committed_provisioning(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: &SelectedRuntimeSurface,
        receipt: &RuntimeBindingReceipt,
    ) -> UseResult<()> {
        let Some(provisioning) = self
            .bindings
            .get_provisioning(receipt.scope(), receipt.surface(), receipt.generation())
            .await?
        else {
            return Ok(());
        };
        let RuntimeBindingReceipt::Service(service) = receipt else {
            return Err(runtime_error(
                RUNTIME_PENDING_ERROR,
                "A Runtime Task cannot own retained Service provisioning evidence.",
            ));
        };
        let apply_request_id =
            runtime_request_id("apply", &request.surface.identity.idempotency_key);
        if !provisioning.matches_plan(
            selected.plan(),
            selected.provider(),
            &request.surface.identity.idempotency_key,
            &apply_request_id,
        )? || provisioning.phase != RuntimeServiceProvisioningPhase::GatewayReady
            || provisioning.binding_receipt()? != *service
        {
            return Err(runtime_error(
                RUNTIME_PENDING_ERROR,
                "The final Runtime binding conflicts with retained provisioning evidence.",
            ));
        }
        self.bindings.remove_provisioning(&provisioning).await?;
        Ok(())
    }

    async fn stop(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: SelectedRuntimeSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let qualified = selected.plan().surface();
        let receipt = match self
            .bindings
            .get_generation(
                &request.surface.identity.installation,
                &qualified,
                request.surface.lifecycle_generation,
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => return before_effect_failure(request, "binding-read", error),
        };
        let Some(receipt) = receipt else {
            return self.stop_pending_or_checkpoint(request, selected).await;
        };
        if let Err(error) = validate_receipt(request, &selected, &receipt) {
            return rejected(request, error.code);
        }
        let RuntimeBindingReceipt::Service(service) = &receipt else {
            return checkpoint_application(request, "stopped", Some(&receipt));
        };
        let readiness = &self.readiness;
        if let Err(error) = selected.client().verify_binding_provider(&receipt).await {
            return before_effect_failure(request, "provider-retire-verify", error);
        }
        if let Err(error) = readiness
            .drain_service(
                service,
                &request.surface.identity.idempotency_key,
                self.deadline(request),
            )
            .await
        {
            return unknown(request, "gateway-drain", error);
        }
        if let Err(error) = selected
            .client()
            .stop_service(
                service,
                runtime_request_id("stop", &request.surface.identity.idempotency_key),
                self.deadline(request),
            )
            .await
        {
            return unknown(request, "service-stop", error);
        }
        checkpoint_application(request, "stopped", Some(&receipt))
    }

    async fn remove(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: SelectedRuntimeSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let qualified = selected.plan().surface();
        let receipt = match self
            .bindings
            .get_generation(
                &request.surface.identity.installation,
                &qualified,
                request.surface.lifecycle_generation,
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => return before_effect_failure(request, "binding-read", error),
        };
        let Some(receipt) = receipt else {
            return self.remove_pending_or_checkpoint(request, selected).await;
        };
        if let Err(error) = validate_receipt(request, &selected, &receipt) {
            return rejected(request, error.code);
        }
        match &receipt {
            RuntimeBindingReceipt::Task(_) => {
                if let Err(error) = self.bindings.remove(&receipt).await {
                    return unknown(request, "task-remove-receipt", error);
                }
                checkpoint_application(request, "removed", Some(&receipt))
            }
            RuntimeBindingReceipt::Service(service) => {
                let readiness = &self.readiness;
                if let Err(error) = selected.client().verify_binding_provider(&receipt).await {
                    return before_effect_failure(request, "provider-retire-verify", error);
                }
                if let Err(error) = readiness
                    .drain_service(
                        service,
                        &request.surface.identity.idempotency_key,
                        self.deadline(request),
                    )
                    .await
                {
                    return unknown(request, "gateway-drain", error);
                }
                if let Err(error) = selected
                    .client()
                    .stop_service(
                        service,
                        runtime_request_id("stop", &request.surface.identity.idempotency_key),
                        self.deadline(request),
                    )
                    .await
                {
                    return unknown(request, "service-stop", error);
                }
                if let Err(error) = readiness
                    .remove_service(
                        service,
                        &request.surface.identity.idempotency_key,
                        self.deadline(request),
                    )
                    .await
                {
                    return unknown(request, "gateway-remove", error);
                }
                if let Err(error) = selected
                    .client()
                    .remove_service(
                        service,
                        runtime_request_id("remove", &request.surface.identity.idempotency_key),
                        self.deadline(request),
                    )
                    .await
                {
                    return unknown(request, "service-remove", error);
                }
                if let Err(error) = self.bindings.remove(&receipt).await {
                    return unknown(request, "service-remove-receipt", error);
                }
                checkpoint_application(request, "removed", Some(&receipt))
            }
        }
    }

    async fn stop_pending_or_checkpoint(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: SelectedRuntimeSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let pending = match self
            .bindings
            .get_provisioning(
                selected.plan().context().scope(),
                &selected.plan().surface(),
                selected.plan().context().generation(),
            )
            .await
        {
            Ok(pending) => pending,
            Err(error) => return before_effect_failure(request, "provisioning-read", error),
        };
        let Some(pending) = pending else {
            return checkpoint_application(request, "stopped", None);
        };
        if let Err(error) = validate_pending_plan(&pending, &selected) {
            return rejected(request, error.code);
        }
        unknown(
            request,
            "pending-stop",
            runtime_error(
                RUNTIME_PENDING_ERROR,
                "A pending Runtime Service requires exact preparation replay before it can be stopped.",
            ),
        )
    }

    async fn remove_pending_or_checkpoint(
        &self,
        request: &ControlRuntimeEffectRequest,
        selected: SelectedRuntimeSurface,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        let Some(pending) = (match self
            .bindings
            .get_provisioning(
                selected.plan().context().scope(),
                &selected.plan().surface(),
                selected.plan().context().generation(),
            )
            .await
        {
            Ok(pending) => pending,
            Err(error) => return before_effect_failure(request, "provisioning-read", error),
        }) else {
            return checkpoint_application(request, "removed", None);
        };
        if let Err(error) = validate_pending_plan(&pending, &selected) {
            return rejected(request, error.code);
        }
        if pending.phase != RuntimeServiceProvisioningPhase::Requested {
            return unknown(
                request,
                "pending-removal",
                runtime_error(
                    RUNTIME_PENDING_ERROR,
                    "A pending Runtime Service requires exact preparation replay before removal.",
                ),
            );
        }
        match selected
            .client()
            .provisioning_service_exists(selected.plan(), selected.provider())
            .await
        {
            Ok(true) => unknown(
                request,
                "pending-removal",
                runtime_error(
                    RUNTIME_PENDING_ERROR,
                    "A Runtime Service exists for a requested provisioning record; replay preparation before removal.",
                ),
            ),
            Ok(false) => {
                if let Err(error) = self.bindings.remove_provisioning(&pending).await {
                    return unknown(request, "pending-remove", error);
                }
                checkpoint_application(request, "removed", None)
            }
            Err(error) => before_effect_failure(request, "pending-inspect", error),
        }
    }

    fn deadline(&self, request: &ControlRuntimeEffectRequest) -> Option<u64> {
        Some(
            self.deadline_at_ms
                .map_or(request.surface.identity.deadline_at_ms, |configured| {
                    configured.min(request.surface.identity.deadline_at_ms)
                }),
        )
    }
}

fn validate_pending_plan(
    pending: &RuntimeServiceProvisioningReceipt,
    selected: &SelectedRuntimeSurface,
) -> UseResult<()> {
    if !pending.matches_plan(
        selected.plan(),
        selected.provider(),
        &pending.lifecycle_idempotency_key,
        &pending.apply_request_id,
    )? {
        return Err(authority_error());
    }
    Ok(())
}

#[async_trait]
impl ControlRuntimeEffectPortTrait for ControlRuntimeEffectPort {
    async fn apply_surface(
        &self,
        request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        self.apply(request).await
    }
}

#[derive(Debug, Clone)]
enum ManagedSurface {
    Tool(ToolSurface),
    Mcp(PluginMcpSurface),
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlRuntimeMcpReadiness>();
    assert_send_sync::<ControlRuntimeEffectPort>();
};
