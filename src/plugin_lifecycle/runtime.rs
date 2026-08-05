use std::path::PathBuf;
use std::sync::Arc;

use a3s_runtime::contract::{RuntimeObservation, RuntimeUnitClass};
use a3s_use_core::{
    PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
};
use a3s_use_extension::{
    inspect_mcp_surface_files, inspect_tool_surface_files, PluginMcpLaunch, PluginMcpSurface,
    ToolSurface, ToolTaskSource, ToolWorkload,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::plugin_runtime::{
    RuntimeBindingObservedState, RuntimeBindingReceipt, RuntimeBindingStore, RuntimeEndpointRef,
    RuntimeMcpInitializeEvidence, RuntimeProviderSelection, RuntimeSurfaceContract,
    RuntimeSurfacePlan, SelectedRuntimeSurface,
};

use super::{
    PluginLifecycleEvidence, PluginLifecycleIntent, PluginMcpLifecycleHost, PluginToolLifecycleHost,
};

/// Readiness evidence produced after a Streamable HTTP MCP service has passed
/// standard MCP initialize negotiation through its private Gateway endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMcpServiceReadiness {
    endpoint: RuntimeEndpointRef,
    initialize: RuntimeMcpInitializeEvidence,
}

impl PluginMcpServiceReadiness {
    pub fn new(endpoint: RuntimeEndpointRef, initialize: RuntimeMcpInitializeEvidence) -> Self {
        Self {
            endpoint,
            initialize,
        }
    }
}

/// Typed Gateway/readiness boundary used only for persistent Runtime Services.
///
/// Stdio MCP never crosses this port: it remains a per-connection executable
/// launcher and is not modeled as a Runtime Service.
#[async_trait]
pub trait PluginRuntimeServiceReadinessHost: Send + Sync {
    async fn bind_tool_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        idempotency_key: &str,
    ) -> UseResult<RuntimeEndpointRef>;

    async fn bind_mcp_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        idempotency_key: &str,
    ) -> UseResult<PluginMcpServiceReadiness>;
}

/// Tool/MCP lifecycle adapter backed by explicit Runtime selections and durable
/// binding receipts.
#[derive(Clone)]
pub struct RuntimePluginSurfaceLifecycleHost {
    package_root: PathBuf,
    selection: RuntimeProviderSelection,
    store: RuntimeBindingStore,
    readiness: Arc<dyn PluginRuntimeServiceReadinessHost>,
    deadline_at_ms: Option<u64>,
}

impl RuntimePluginSurfaceLifecycleHost {
    pub fn new(
        package_root: impl Into<PathBuf>,
        selection: RuntimeProviderSelection,
        store: RuntimeBindingStore,
        readiness: Arc<dyn PluginRuntimeServiceReadinessHost>,
    ) -> Self {
        Self {
            package_root: package_root.into(),
            selection,
            store,
            readiness,
            deadline_at_ms: None,
        }
    }

    pub fn with_deadline_at_ms(mut self, deadline_at_ms: Option<u64>) -> UseResult<Self> {
        if deadline_at_ms == Some(0) {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_invalid",
                "A Runtime lifecycle deadline must be positive when present.",
            ));
        }
        self.deadline_at_ms = deadline_at_ms;
        Ok(self)
    }

    pub fn package_root(&self) -> &std::path::Path {
        &self.package_root
    }

    pub fn store(&self) -> &RuntimeBindingStore {
        &self.store
    }

    async fn prepare_tool_surface(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Tool, &surface.id)?;
        let files = inspect_tool_surface_files(surface, &self.package_root).await?;
        match &surface.workload {
            ToolWorkload::Task(task)
                if matches!(&task.source, ToolTaskSource::Executable { .. }) =>
            {
                static_launcher_evidence(
                    "tool-launcher-prepared",
                    intent,
                    &surface.id,
                    idempotency_key,
                    files.digest(),
                )
            }
            ToolWorkload::Task(_) | ToolWorkload::Service(_) => {
                self.prepare_runtime_tool(intent, surface, idempotency_key, files.digest())
                    .await
            }
        }
    }

    async fn prepare_mcp_surface(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Mcp, &surface.id)?;
        let files = inspect_mcp_surface_files(surface, &self.package_root).await?;
        match &surface.launch {
            PluginMcpLaunch::Stdio { .. } => static_launcher_evidence(
                "mcp-stdio-launcher-prepared",
                intent,
                &surface.id,
                idempotency_key,
                files.digest(),
            ),
            PluginMcpLaunch::StreamableHttp { .. } => {
                self.prepare_runtime_mcp(intent, surface, idempotency_key, files.digest())
                    .await
            }
        }
    }

    async fn prepare_runtime_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
        file_digest: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let selected = self.selected(intent, PluginSurfaceKind::Tool, &surface.id)?;
        validate_tool_plan(surface, selected.plan())?;
        if let Some(evidence) = self
            .reuse_ready_binding(
                "tool-runtime-prepared",
                intent,
                selected,
                idempotency_key,
                file_digest,
            )
            .await?
        {
            return Ok(evidence);
        }

        let receipt = match &surface.workload {
            ToolWorkload::Task(_) => RuntimeBindingReceipt::Task(
                selected
                    .client()
                    .prepare_task(selected.plan(), selected.provider())
                    .await?,
            ),
            ToolWorkload::Service(_) => {
                let activation = selected
                    .client()
                    .apply_service(
                        selected.plan(),
                        selected.provider(),
                        request_id("apply-tool", idempotency_key),
                        self.deadline_at_ms,
                    )
                    .await?;
                let endpoint = self
                    .readiness
                    .bind_tool_service(
                        intent,
                        surface,
                        selected.plan(),
                        activation.observation(),
                        idempotency_key,
                    )
                    .await?;
                RuntimeBindingReceipt::Service(activation.into_tool_service_receipt(endpoint)?)
            }
        };
        validate_selected_receipt(intent, selected, &receipt)?;
        self.store.put(&receipt).await?;
        binding_evidence(
            "tool-runtime-prepared",
            intent,
            &surface.id,
            idempotency_key,
            file_digest,
            selected,
        )
    }

    async fn prepare_runtime_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
        file_digest: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let selected = self.selected(intent, PluginSurfaceKind::Mcp, &surface.id)?;
        if !matches!(
            selected.plan().contract(),
            RuntimeSurfaceContract::McpService { .. }
        ) {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_plan_mismatch",
                "A Streamable HTTP MCP surface requires an MCP Runtime Service plan.",
            ));
        }
        if let Some(evidence) = self
            .reuse_ready_binding(
                "mcp-runtime-prepared",
                intent,
                selected,
                idempotency_key,
                file_digest,
            )
            .await?
        {
            return Ok(evidence);
        }

        let activation = selected
            .client()
            .apply_service(
                selected.plan(),
                selected.provider(),
                request_id("apply-mcp", idempotency_key),
                self.deadline_at_ms,
            )
            .await?;
        let readiness = self
            .readiness
            .bind_mcp_service(
                intent,
                surface,
                selected.plan(),
                activation.observation(),
                idempotency_key,
            )
            .await?;
        let receipt = RuntimeBindingReceipt::Service(
            activation.into_mcp_service_receipt(readiness.endpoint, readiness.initialize)?,
        );
        validate_selected_receipt(intent, selected, &receipt)?;
        self.store.put(&receipt).await?;
        binding_evidence(
            "mcp-runtime-prepared",
            intent,
            &surface.id,
            idempotency_key,
            file_digest,
            selected,
        )
    }

    async fn reuse_ready_binding(
        &self,
        label: &str,
        intent: &PluginLifecycleIntent,
        selected: &SelectedRuntimeSurface,
        idempotency_key: &str,
        file_digest: &str,
    ) -> UseResult<Option<PluginLifecycleEvidence>> {
        let qualified = selected.plan().surface();
        let Some(receipt) = self
            .store
            .get_generation(&intent.scope_id, &qualified, intent.generation)
            .await?
        else {
            return Ok(None);
        };
        validate_selected_receipt(intent, selected, &receipt)?;
        let observation = selected.client().observe_binding(&receipt).await?;
        let ready = matches!(
            (&receipt, observation.state),
            (
                RuntimeBindingReceipt::Task(_),
                RuntimeBindingObservedState::Prepared
            ) | (
                RuntimeBindingReceipt::Service(_),
                RuntimeBindingObservedState::Healthy
            )
        );
        if ready {
            return binding_evidence(
                label,
                intent,
                &qualified.surface.id,
                idempotency_key,
                file_digest,
                selected,
            )
            .map(Some);
        }
        self.remove_binding(selected, &receipt, idempotency_key)
            .await?;
        Ok(None)
    }

    async fn stop_or_remove_runtime(
        &self,
        intent: &PluginLifecycleIntent,
        kind: PluginSurfaceKind,
        surface_id: &str,
        idempotency_key: &str,
        label: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, kind, surface_id)?;
        let qualified = qualified_surface(intent, kind, surface_id);
        let Some(receipt) = self
            .store
            .get_generation(&intent.scope_id, &qualified, intent.generation)
            .await?
        else {
            return missing_runtime_evidence(label, intent, surface_id, idempotency_key);
        };
        if receipt.generation() != intent.generation
            || receipt.package_digest() != intent.package_digest
        {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_binding_changed",
                "A different Runtime binding generation was preserved during lifecycle cleanup.",
            ));
        }
        let selected = self.selected(intent, kind, surface_id)?;
        validate_selected_receipt(intent, selected, &receipt)?;
        self.remove_binding(selected, &receipt, idempotency_key)
            .await?;
        projection_evidence(label, intent, surface_id, idempotency_key)
    }

    async fn remove_binding(
        &self,
        selected: &SelectedRuntimeSurface,
        receipt: &RuntimeBindingReceipt,
        idempotency_key: &str,
    ) -> UseResult<()> {
        match receipt {
            RuntimeBindingReceipt::Task(_) => {}
            RuntimeBindingReceipt::Service(service) => {
                selected
                    .client()
                    .stop_service(
                        service,
                        request_id("stop", idempotency_key),
                        self.deadline_at_ms,
                    )
                    .await?;
                selected
                    .client()
                    .remove_service(
                        service,
                        request_id("remove", idempotency_key),
                        self.deadline_at_ms,
                    )
                    .await?;
            }
        }
        self.store.remove(receipt).await?;
        Ok(())
    }

    fn selected(
        &self,
        intent: &PluginLifecycleIntent,
        kind: PluginSurfaceKind,
        surface_id: &str,
    ) -> UseResult<&SelectedRuntimeSurface> {
        let qualified = qualified_surface(intent, kind, surface_id);
        let selected = self
            .selection
            .surfaces()
            .iter()
            .find(|candidate| candidate.plan().surface() == qualified)
            .ok_or_else(|| {
                runtime_lifecycle_error(
                    "use.plugin.runtime_lifecycle_selection_missing",
                    "The executable surface has no explicit Runtime provider selection.",
                )
            })?;
        let context = selected.plan().context();
        if context.package_id() != intent.package_id
            || context.package_digest() != intent.package_digest
            || context.scope_id() != intent.scope_id
            || context.generation() != intent.generation
            || selected.provider().surface != qualified
        {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_plan_mismatch",
                "The Runtime selection does not belong to the exact lifecycle package generation.",
            ));
        }
        Ok(selected)
    }
}

#[async_trait]
impl PluginToolLifecycleHost for RuntimePluginSurfaceLifecycleHost {
    async fn prepare_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.prepare_tool_surface(intent, surface, idempotency_key)
            .await
    }

    async fn stop_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        match &surface.workload {
            ToolWorkload::Task(task)
                if matches!(&task.source, ToolTaskSource::Executable { .. }) =>
            {
                validate_surface(intent, PluginSurfaceKind::Tool, &surface.id)?;
                projection_evidence("tool-launcher-hidden", intent, &surface.id, idempotency_key)
            }
            _ => {
                self.stop_or_remove_runtime(
                    intent,
                    PluginSurfaceKind::Tool,
                    &surface.id,
                    idempotency_key,
                    "tool-runtime-stopped",
                )
                .await
            }
        }
    }

    async fn remove_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        match &surface.workload {
            ToolWorkload::Task(task)
                if matches!(&task.source, ToolTaskSource::Executable { .. }) =>
            {
                validate_surface(intent, PluginSurfaceKind::Tool, &surface.id)?;
                projection_evidence(
                    "tool-launcher-removed",
                    intent,
                    &surface.id,
                    idempotency_key,
                )
            }
            _ => {
                self.stop_or_remove_runtime(
                    intent,
                    PluginSurfaceKind::Tool,
                    &surface.id,
                    idempotency_key,
                    "tool-runtime-removed",
                )
                .await
            }
        }
    }
}

#[async_trait]
impl PluginMcpLifecycleHost for RuntimePluginSurfaceLifecycleHost {
    async fn prepare_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.prepare_mcp_surface(intent, surface, idempotency_key)
            .await
    }

    async fn stop_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        match &surface.launch {
            PluginMcpLaunch::Stdio { .. } => {
                validate_surface(intent, PluginSurfaceKind::Mcp, &surface.id)?;
                projection_evidence(
                    "mcp-stdio-launcher-hidden",
                    intent,
                    &surface.id,
                    idempotency_key,
                )
            }
            PluginMcpLaunch::StreamableHttp { .. } => {
                self.stop_or_remove_runtime(
                    intent,
                    PluginSurfaceKind::Mcp,
                    &surface.id,
                    idempotency_key,
                    "mcp-runtime-stopped",
                )
                .await
            }
        }
    }

    async fn remove_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        match &surface.launch {
            PluginMcpLaunch::Stdio { .. } => {
                validate_surface(intent, PluginSurfaceKind::Mcp, &surface.id)?;
                projection_evidence(
                    "mcp-stdio-launcher-removed",
                    intent,
                    &surface.id,
                    idempotency_key,
                )
            }
            PluginMcpLaunch::StreamableHttp { .. } => {
                self.stop_or_remove_runtime(
                    intent,
                    PluginSurfaceKind::Mcp,
                    &surface.id,
                    idempotency_key,
                    "mcp-runtime-removed",
                )
                .await
            }
        }
    }
}

fn validate_tool_plan(surface: &ToolSurface, plan: &RuntimeSurfacePlan) -> UseResult<()> {
    let matches = matches!(
        (&surface.workload, plan.contract(), plan.spec().class),
        (
            ToolWorkload::Task(_),
            RuntimeSurfaceContract::ToolTask { .. },
            RuntimeUnitClass::Task,
        ) | (
            ToolWorkload::Service(_),
            RuntimeSurfaceContract::ToolService { .. },
            RuntimeUnitClass::Service,
        )
    );
    if matches {
        Ok(())
    } else {
        Err(runtime_lifecycle_error(
            "use.plugin.runtime_lifecycle_plan_mismatch",
            "The Tool manifest workload does not match its selected Runtime plan.",
        ))
    }
}

fn validate_selected_receipt(
    intent: &PluginLifecycleIntent,
    selected: &SelectedRuntimeSurface,
    receipt: &RuntimeBindingReceipt,
) -> UseResult<()> {
    receipt.validate()?;
    let plan = selected.plan();
    let provider = selected.provider();
    let common = receipt.surface() == &plan.surface()
        && receipt.scope_id() == intent.scope_id
        && receipt.package_digest() == intent.package_digest
        && receipt.generation() == intent.generation
        && receipt.provider_id() == provider.provider_id
        && receipt.provider_build_id() == provider.provider_build_id
        && receipt.capability_digest() == provider.capability_digest
        && receipt.semantics_profile_digest() == provider.semantics_profile_digest;
    let exact = match receipt {
        RuntimeBindingReceipt::Task(task) => {
            task.descriptor_digest == plan.descriptor_digest()
                && task.artifact_digest == plan.spec().artifact.digest
                && task.artifact_media_type == plan.spec().artifact.media_type
        }
        RuntimeBindingReceipt::Service(service) => {
            service.descriptor_digest == plan.descriptor_digest()
                && service.spec_digest == plan.spec().digest().unwrap_or_default()
                && service.contract == *plan.contract()
        }
    };
    if common && exact {
        Ok(())
    } else {
        Err(runtime_lifecycle_error(
            "use.plugin.runtime_lifecycle_binding_mismatch",
            "The Runtime binding receipt does not match the selected lifecycle generation.",
        ))
    }
}

fn validate_surface(
    intent: &PluginLifecycleIntent,
    kind: PluginSurfaceKind,
    surface_id: &str,
) -> UseResult<()> {
    intent.validate()?;
    let reference = PluginSurfaceRef {
        kind,
        id: surface_id.to_string(),
    };
    if intent
        .surfaces
        .iter()
        .any(|candidate| candidate.surface == reference)
    {
        Ok(())
    } else {
        Err(runtime_lifecycle_error(
            "use.plugin.runtime_lifecycle_surface_mismatch",
            "The executable lifecycle call is absent from the admitted package surface inventory.",
        ))
    }
}

fn qualified_surface(
    intent: &PluginLifecycleIntent,
    kind: PluginSurfaceKind,
    surface_id: &str,
) -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: intent.package_id.clone(),
        surface: PluginSurfaceRef {
            kind,
            id: surface_id.to_string(),
        },
    }
}

fn binding_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
    file_digest: &str,
    selected: &SelectedRuntimeSurface,
) -> UseResult<PluginLifecycleEvidence> {
    let plan = selected.plan();
    let provider = selected.provider();
    let subject = format!(
        "{file_digest}\n{}\n{}\n{}\n{}\n{}\n{}",
        plan.descriptor_digest(),
        plan.spec().digest().map_err(|error| {
            runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_evidence_invalid",
                format!("Failed to digest the selected Runtime spec: {error}"),
            )
        })?,
        provider.provider_id,
        provider.provider_build_id,
        provider.capability_digest,
        provider.semantics_profile_digest,
    );
    let subject = format!("sha256:{:x}", Sha256::digest(subject.as_bytes()));
    lifecycle_evidence(label, intent, surface_id, idempotency_key, &subject)
}

fn static_launcher_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
    file_digest: &str,
) -> UseResult<PluginLifecycleEvidence> {
    lifecycle_evidence(label, intent, surface_id, idempotency_key, file_digest)
}

fn projection_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
) -> UseResult<PluginLifecycleEvidence> {
    let subject = format!(
        "{}\n{}\n{}\n{}",
        intent.scope_id, intent.package_id, intent.generation, intent.package_digest
    );
    let subject = format!("sha256:{:x}", Sha256::digest(subject.as_bytes()));
    lifecycle_evidence(label, intent, surface_id, idempotency_key, &subject)
}

fn missing_runtime_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
) -> UseResult<PluginLifecycleEvidence> {
    projection_evidence(label, intent, surface_id, idempotency_key)
}

fn lifecycle_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
    subject_digest: &str,
) -> UseResult<PluginLifecycleEvidence> {
    let identity = format!(
        "{label}\n{idempotency_key}\n{}\n{}\n{surface_id}\n{}\n{}\n{subject_digest}",
        intent.package_id, intent.scope_id, intent.generation, intent.manifest_digest
    );
    PluginLifecycleEvidence::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

fn request_id(label: &str, idempotency_key: &str) -> String {
    format!(
        "use:{label}:{:x}",
        Sha256::digest(idempotency_key.as_bytes())
    )
}

fn runtime_lifecycle_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests;
