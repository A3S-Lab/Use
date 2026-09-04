use std::sync::Arc;

use a3s_runtime::{ProviderId, RuntimeClientRegistry};
use a3s_use_core::{PlanQualifiedSurfaceRef, PlanScope, PluginSurfaceKind, PluginSurfaceRef};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionRegistry, ToolTaskSource, ToolWorkload,
};

use super::client::{runtime_error, PluginRuntimeClient};
use super::model::{
    runtime_input_error, valid_machine_id, valid_surface_segment, RuntimeTaskInvocation,
};
use super::{
    RuntimeBindingReceipt, RuntimeBindingStore, RuntimePreparedTaskBinding, RuntimeTaskExecution,
};
use a3s_use_core::{UseError, UseResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTaskDispatchRequest {
    identity: ExtensionLifecycleIdentity,
    scope: PlanScope,
    surface_id: String,
    invocation: RuntimeTaskInvocation,
    request_id: String,
    deadline_at_ms: Option<u64>,
}

impl RuntimeTaskDispatchRequest {
    pub fn new(
        identity: ExtensionLifecycleIdentity,
        scope: PlanScope,
        surface_id: impl Into<String>,
        invocation: RuntimeTaskInvocation,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<Self> {
        let request = Self {
            identity,
            scope,
            surface_id: surface_id.into(),
            invocation,
            request_id: request_id.into(),
            deadline_at_ms,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn identity(&self) -> &ExtensionLifecycleIdentity {
        &self.identity
    }

    pub fn scope(&self) -> &PlanScope {
        &self.scope
    }

    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    pub fn invocation(&self) -> &RuntimeTaskInvocation {
        &self.invocation
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn deadline_at_ms(&self) -> Option<u64> {
        self.deadline_at_ms
    }

    fn surface(&self) -> PlanQualifiedSurfaceRef {
        PlanQualifiedSurfaceRef {
            package_id: self.identity.package_id().to_string(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: self.surface_id.clone(),
            },
        }
    }

    fn validate(&self) -> UseResult<()> {
        if !valid_surface_segment(&self.surface_id)
            || !valid_machine_id(&self.request_id)
            || self.deadline_at_ms == Some(0)
        {
            return Err(runtime_input_error(
                "Runtime Task dispatch requires bounded surface, request, and deadline identities.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct RuntimeTaskDispatcher {
    registry: ExtensionRegistry,
    bindings: RuntimeBindingStore,
    providers: Arc<RuntimeClientRegistry>,
}

impl std::fmt::Debug for RuntimeTaskDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeTaskDispatcher")
            .field("registry", &self.registry)
            .field("bindings", &self.bindings)
            .finish_non_exhaustive()
    }
}

impl RuntimeTaskDispatcher {
    pub fn new(
        registry: ExtensionRegistry,
        bindings: RuntimeBindingStore,
        providers: Arc<RuntimeClientRegistry>,
    ) -> Self {
        Self {
            registry,
            bindings,
            providers,
        }
    }

    /// Invoke one exact currently published Runtime Task generation.
    ///
    /// The Registry lease is held until provider cleanup finishes, so disable,
    /// upgrade, and uninstall drain accepted calls before removing the binding.
    /// Provider identity is recovered from the durable receipt and reverified;
    /// current host assignments can never silently redirect an installed Task.
    pub async fn invoke(
        &self,
        request: RuntimeTaskDispatchRequest,
    ) -> UseResult<RuntimeTaskExecution> {
        request.validate()?;
        let lease = self
            .registry
            .acquire_published_lifecycle_generation(request.identity())
            .await?
            .ok_or_else(|| generation_unavailable(&request))?;
        validate_manifest_surface(lease.extension(), request.surface_id())?;

        let surface = request.surface();
        let receipt = self
            .bindings
            .get_generation(request.scope(), &surface, request.identity().generation())
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.runtime.binding_missing",
                    "The published Runtime Task generation has no durable binding receipt.",
                )
            })?;
        let RuntimeBindingReceipt::Task(binding) = receipt else {
            return Err(binding_mismatch());
        };
        super::validate_task_descriptor_binding(lease.extension(), request.surface_id(), &binding)?;
        validate_dispatch_binding(&binding, &request, &surface)?;

        let provider_id = ProviderId::parse(binding.provider_id.clone())
            .map_err(|error| runtime_input_error(error.to_string()))?;
        let client = self
            .providers
            .connect(&provider_id)
            .await
            .map_err(|error| runtime_error("connect the installed Runtime Task provider", error))?;
        let plan = binding.invocation_plan(request.invocation.clone())?;
        PluginRuntimeClient::new(client)
            .invoke_task(&plan, &binding, request.request_id, request.deadline_at_ms)
            .await
    }
}

fn validate_manifest_surface(
    extension: &a3s_use_extension::InstalledExtension,
    surface_id: &str,
) -> UseResult<()> {
    let matches = extension.manifest.tools.iter().any(|surface| {
        surface.id == surface_id
            && matches!(
                &surface.workload,
                ToolWorkload::Task(task)
                    if matches!(&task.source, ToolTaskSource::Release { .. }) && !task.interactive
            )
    });
    if !matches {
        return Err(binding_mismatch());
    }
    Ok(())
}

fn validate_dispatch_binding(
    binding: &RuntimePreparedTaskBinding,
    request: &RuntimeTaskDispatchRequest,
    surface: &PlanQualifiedSurfaceRef,
) -> UseResult<()> {
    binding.validate()?;
    if &binding.surface != surface
        || binding.scope != *request.scope()
        || binding.package_digest != request.identity().package_digest()
        || binding.generation() != request.identity().generation()
    {
        return Err(binding_mismatch());
    }
    Ok(())
}

fn generation_unavailable(request: &RuntimeTaskDispatchRequest) -> UseError {
    UseError::new(
        "use.plugin.runtime.generation_unavailable",
        "The exact Runtime Task package generation is no longer published or accepting calls.",
    )
    .with_detail("packageId", request.identity().package_id())
    .with_detail("generation", request.identity().generation())
}

fn binding_mismatch() -> UseError {
    UseError::new(
        "use.plugin.runtime.binding_mismatch",
        "The Runtime Task binding does not match the exact published package surface.",
    )
}
