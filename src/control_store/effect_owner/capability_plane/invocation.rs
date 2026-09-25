//! Control-backed opaque invocation resolution.
//!
//! The public Gateway only carries an opaque [`a3s_use_core::InvocationRef`].
//! This adapter
//! is the missing authority join for the Control Store: it reopens the
//! durable published cursor, validates the complete descriptor against the
//! immutable catalog, and gives a host-owned factory the exact leased
//! generation before any provider state is opened.

use std::sync::Arc;

use a3s_use_core::{
    CapabilityDescriptor, InvocationRef, PlanQualifiedSurfaceRef, PluginSurfaceKind, UseError,
    UseResult,
};
use async_trait::async_trait;
use serde_json::Value;

use crate::capability_gateway::{
    CapabilityGatewayExternalLease, CapabilityGatewayInvocation, CapabilityGatewayInvocationLease,
    CapabilityGatewayInvocationResolver, CapabilityGatewayRequestContext,
};
use crate::plugin_runtime::{
    RuntimeBindingReceipt, RuntimeBindingStore, RuntimeServiceBindingReceipt,
    RuntimeSurfacePlanStore, RuntimeSurfaceResolver, RuntimeTaskInvocation, SelectedRuntimeSurface,
};

use super::super::super::model::ControlGeneration;
use super::super::super::ControlStore;
use super::{ControlCapabilityPlaneEffectPort, ControlCapabilitySnapshotLease};

const RESOLUTION_ERROR: &str = "use.control.capability_gateway_resolution_unavailable";
const GRANT_FORBIDDEN: &str = "use.plugin.capability_gateway_forbidden";
const GENERATION_DRIFT: &str = "use.control.capability_gateway_generation_drift";
const RUNTIME_UNAVAILABLE: &str = "use.control.capability_gateway_runtime_unavailable";
const PROVIDER_MISSING: &str = "use.control.capability_gateway_provider_missing";
const BINDING_MISSING: &str = "use.control.capability_gateway_runtime_binding_missing";
pub(in crate::control_store) const ENDPOINT_ROUTE_UNAVAILABLE: &str =
    "use.control.capability_gateway_endpoint_route_unavailable";

/// Host-owned router from a receipt-owned opaque `gateway:` binding to the
/// live Tool/MCP Service endpoint used for Gateway invoke.
///
/// Control retains only the opaque endpoint identity on the durable receipt.
/// The embedding host owns the reverse map created during readiness bind and
/// must keep it generation-fenced for the complete call.
#[async_trait]
pub(in crate::control_store) trait ControlCapabilityGatewayEndpointRouter:
    Send + Sync
{
    async fn invoke_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        surface_id: &str,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value>;
}

/// Default production router: fail closed until a host injects a live map.
#[derive(Debug, Default, Clone, Copy)]
pub(in crate::control_store) struct FailClosedCapabilityGatewayEndpointRouter;

#[async_trait]
impl ControlCapabilityGatewayEndpointRouter for FailClosedCapabilityGatewayEndpointRouter {
    async fn invoke_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        surface_id: &str,
        _arguments: Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value> {
        Err(UseError::new(
            ENDPOINT_ROUTE_UNAVAILABLE,
            "Control Runtime receipt and provider lease joined, but no Gateway endpoint router is composed for opaque service bindings.",
        )
        .with_detail("endpointRef", receipt.endpoint_ref.as_str())
        .with_detail("surfaceId", surface_id)
        .with_suggestion(
            "Inject a ControlCapabilityGatewayEndpointRouter that maps receipt endpoint_ref values to the live Runtime Service endpoint created during readiness bind.",
        ))
    }
}

/// Host-owned factory for one Control-bound opaque invocation.
///
/// The factory receives the exact immutable Control lease that was validated
/// against the published catalog. It may use that authority to join the
/// descriptor to a Grant, Runtime receipt, provider process, or other private
/// binding, but must not expose those details to the Gateway client.
#[async_trait]
pub(in crate::control_store) trait ControlCapabilityGatewayInvocationFactory:
    Send + Sync
{
    async fn open(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
        lease: &ControlCapabilitySnapshotLease,
    ) -> UseResult<Box<dyn CapabilityGatewayInvocation>>;
}

/// Production factory: join Grant, Control provider selection, durable Runtime
/// receipt, and reconnected provider lease before any provider I/O.
///
/// Tool Task invoke uses the joined receipt. Tool/MCP Service invoke proves the
/// receipt-owned provider is still healthy, then dispatches through the injected
/// [`ControlCapabilityGatewayEndpointRouter`]. Production composition injects
/// [`super::endpoint_router::LiveControlCapabilityGatewayEndpointRouter`]
/// backed by the bind-time route table; [`Self::new`] keeps the fail-closed
/// router for tests and callers that have not wired live routes yet.
#[derive(Clone)]
pub(in crate::control_store) struct ProductionControlInvocationFactory {
    control: ControlStore,
    resolver: Arc<dyn RuntimeSurfaceResolver>,
    bindings: RuntimeBindingStore,
    plan_store: RuntimeSurfacePlanStore,
    endpoint_router: Arc<dyn ControlCapabilityGatewayEndpointRouter>,
}

impl ProductionControlInvocationFactory {
    pub(in crate::control_store) fn new(
        control: ControlStore,
        resolver: Arc<dyn RuntimeSurfaceResolver>,
        bindings: RuntimeBindingStore,
        plan_store: RuntimeSurfacePlanStore,
    ) -> Self {
        Self::with_endpoint_router(
            control,
            resolver,
            bindings,
            plan_store,
            Arc::new(FailClosedCapabilityGatewayEndpointRouter),
        )
    }

    pub(in crate::control_store) fn with_endpoint_router(
        control: ControlStore,
        resolver: Arc<dyn RuntimeSurfaceResolver>,
        bindings: RuntimeBindingStore,
        plan_store: RuntimeSurfacePlanStore,
        endpoint_router: Arc<dyn ControlCapabilityGatewayEndpointRouter>,
    ) -> Self {
        Self {
            control,
            resolver,
            bindings,
            plan_store,
            endpoint_router,
        }
    }
}

#[async_trait]
impl ControlCapabilityGatewayInvocationFactory for ProductionControlInvocationFactory {
    async fn open(
        &self,
        descriptor: &CapabilityDescriptor,
        _context: &CapabilityGatewayRequestContext,
        lease: &ControlCapabilitySnapshotLease,
    ) -> UseResult<Box<dyn CapabilityGatewayInvocation>> {
        let generation = self.control.current_generation().await?.ok_or_else(|| {
            UseError::new(
                RESOLUTION_ERROR,
                "Control has no committed generation for Grant-authorized invocation.",
            )
        })?;
        if generation.capability.generation != lease.cursor().capability_generation {
            return Err(UseError::new(
                GENERATION_DRIFT,
                "The Control Grant generation drifted from the leased capability publication.",
            )
            .with_detail(
                "leasedCapabilityGeneration",
                lease.cursor().capability_generation,
            )
            .with_detail(
                "committedCapabilityGeneration",
                generation.capability.generation,
            ));
        }
        let package_id = descriptor.package_id.as_str();
        if !generation
            .snapshot
            .packages
            .iter()
            .any(|package| package.package_id() == package_id)
        {
            return Err(UseError::new(
                GRANT_FORBIDDEN,
                "The capability package is not selected in the committed Control installation.",
            )
            .with_detail("packageId", package_id));
        }
        let grant = generation
            .grants
            .iter()
            .find(|grant| grant.package_id() == package_id)
            .ok_or_else(|| {
                UseError::new(
                    GRANT_FORBIDDEN,
                    "No committed Control Grant authorizes this capability package.",
                )
                .with_detail("packageId", package_id)
            })?;
        if !grant
            .grant
            .permissions
            .surfaces
            .iter()
            .any(|permission| permission.surface == descriptor.surface)
        {
            return Err(UseError::new(
                GRANT_FORBIDDEN,
                "The committed Control Grant does not authorize this capability surface.",
            )
            .with_detail("packageId", package_id)
            .with_detail("surfaceKind", surface_kind_name(descriptor.surface.kind))
            .with_detail("surfaceId", descriptor.surface.id.as_str()));
        }
        if !matches!(
            descriptor.surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        ) {
            return Ok(Box::new(GrantAuthorizedControlInvocation::GrantOnly {
                invocation_ref: descriptor.invocation_ref.clone(),
            }));
        }
        let selection = generation
            .provider_selections
            .iter()
            .find(|selection| {
                selection.package_id() == package_id && selection.surface() == &descriptor.surface
            })
            .ok_or_else(|| {
                UseError::new(
                    PROVIDER_MISSING,
                    "No committed Control provider selection binds this Tool/MCP surface for invocation.",
                )
                .with_detail("packageId", package_id)
                .with_detail("surfaceKind", surface_kind_name(descriptor.surface.kind))
                .with_detail("surfaceId", descriptor.surface.id.as_str())
            })?;
        let (selected, receipt) = join_runtime_receipt(
            &self.control,
            self.resolver.as_ref(),
            &self.bindings,
            &self.plan_store,
            &generation,
            selection,
            descriptor,
        )
        .await?;
        selected.client().verify_binding_provider(&receipt).await?;
        Ok(Box::new(GrantAuthorizedControlInvocation::RuntimeJoined {
            invocation_ref: descriptor.invocation_ref.clone(),
            surface_id: descriptor.surface.id.clone(),
            selected,
            receipt,
            endpoint_router: Arc::clone(&self.endpoint_router),
        }))
    }
}

async fn join_runtime_receipt(
    control: &ControlStore,
    resolver: &dyn RuntimeSurfaceResolver,
    bindings: &RuntimeBindingStore,
    plan_store: &RuntimeSurfacePlanStore,
    generation: &ControlGeneration,
    selection: &super::super::super::model::ControlProviderSelection,
    descriptor: &CapabilityDescriptor,
) -> UseResult<(SelectedRuntimeSurface, RuntimeBindingReceipt)> {
    let package_id = descriptor.package_id.as_str();
    let lifecycle_generation = generation
        .package_lifecycles
        .iter()
        .find(|lifecycle| lifecycle.package_id == package_id)
        .map(|lifecycle| lifecycle.lifecycle_generation)
        .ok_or_else(|| {
            UseError::new(
                RESOLUTION_ERROR,
                "The committed Control generation omits the package lifecycle incarnation.",
            )
            .with_detail("packageId", package_id)
        })?;
    if lifecycle_generation != descriptor.generation {
        return Err(UseError::new(
            GENERATION_DRIFT,
            "The descriptor lifecycle generation drifted from the committed Control package incarnation.",
        )
        .with_detail("descriptorGeneration", descriptor.generation)
        .with_detail("controlLifecycleGeneration", lifecycle_generation));
    }
    let qualified = PlanQualifiedSurfaceRef {
        package_id: package_id.to_owned(),
        surface: descriptor.surface.clone(),
    };
    // Runtime plans are keyed by the stable pre-confirmation Grant proposal
    // digest, not the finalized Grant digest. Discover the exact published key
    // from the durable plan store rather than reconstructing proposal history.
    let key = plan_store
        .inspect_keys()
        .await?
        .into_iter()
        .find(|key| {
            key.package_id == package_id
                && key.scope == control.installation
                && key.surface == qualified
                && key.generation == lifecycle_generation
                && key.provider_id == selection.evidence.provider_id
                && key.selection_digest == selection.selection_digest
                && key.semantics_profile_digest == selection.evidence.semantics_profile_digest
        })
        .ok_or_else(|| {
            UseError::new(
                BINDING_MISSING,
                "No durable Runtime plan publication joins this Control provider selection.",
            )
            .with_detail("packageId", package_id)
            .with_detail("surfaceKind", surface_kind_name(descriptor.surface.kind))
            .with_detail("surfaceId", descriptor.surface.id.as_str())
            .with_detail("lifecycleGeneration", lifecycle_generation)
        })?;
    let selected = resolver.resolve(&key, &selection.evidence).await?;
    let receipt = bindings
        .get_generation(&control.installation, &qualified, lifecycle_generation)
        .await?
        .ok_or_else(|| {
            UseError::new(
                BINDING_MISSING,
                "No durable Runtime binding receipt joins this Control provider selection.",
            )
            .with_detail("packageId", package_id)
            .with_detail("surfaceKind", surface_kind_name(descriptor.surface.kind))
            .with_detail("surfaceId", descriptor.surface.id.as_str())
            .with_detail("lifecycleGeneration", lifecycle_generation)
        })?;
    if receipt.surface() != &qualified
        || receipt.generation() != lifecycle_generation
        || receipt.package_digest() != selected.plan().context().package_digest()
        || receipt.provider_id() != selection.evidence.provider_id
        || receipt.provider_build_id() != selection.evidence.provider_build_id
        || receipt.capability_digest() != selection.evidence.capability_digest
        || receipt.semantics_profile_digest() != selection.evidence.semantics_profile_digest
    {
        return Err(UseError::new(
            BINDING_MISSING,
            "The durable Runtime binding receipt does not match the committed Control provider selection.",
        )
        .with_detail("packageId", package_id));
    }
    Ok((selected, receipt))
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

/// Grant-authorized handle. Tool/MCP carries a joined Runtime receipt and
/// provider lease; other surfaces remain Grant-only until a surface-specific
/// owner is composed.
enum GrantAuthorizedControlInvocation {
    GrantOnly {
        invocation_ref: InvocationRef,
    },
    RuntimeJoined {
        invocation_ref: InvocationRef,
        surface_id: String,
        selected: SelectedRuntimeSurface,
        receipt: RuntimeBindingReceipt,
        endpoint_router: Arc<dyn ControlCapabilityGatewayEndpointRouter>,
    },
}

#[async_trait]
impl CapabilityGatewayInvocation for GrantAuthorizedControlInvocation {
    async fn authorize(
        &self,
        _arguments: &Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<()> {
        // Grant/package join already succeeded at open. Absent principals are
        // denied here: discovery policy is not a substitute for invocation
        // authorization, and stdio without an injected principal must not
        // silently widen the Capability Gateway trust boundary.
        if context.principal().is_none() {
            return Err(UseError::new(
                GRANT_FORBIDDEN,
                "Capability Gateway invocation requires an authenticated principal.",
            ));
        }
        Ok(())
    }

    async fn invoke(
        &self,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value> {
        match self {
            Self::GrantOnly { invocation_ref } => Err(runtime_unavailable(invocation_ref)),
            Self::RuntimeJoined {
                invocation_ref,
                surface_id,
                selected,
                receipt,
                endpoint_router,
            } => match receipt {
                RuntimeBindingReceipt::Task(binding) => {
                    let invocation = task_invocation_from_arguments(invocation_ref, &arguments)?;
                    let plan = binding.invocation_plan(invocation)?;
                    let execution = selected
                        .client()
                        .invoke_task(&plan, binding, invocation_ref.as_str(), None)
                        .await?;
                    Ok(serde_json::json!({
                        "exitCode": execution.exit_code,
                        "stdout": execution.stdout,
                        "stderr": execution.stderr,
                        "truncated": execution.truncated,
                    }))
                }
                RuntimeBindingReceipt::Service(service) => {
                    selected.client().verify_binding_provider(receipt).await?;
                    let _observed = selected.client().observe_binding(receipt).await?;
                    endpoint_router
                        .invoke_service(service, surface_id, arguments, context)
                        .await
                }
            },
        }
    }

    async fn read_resource(
        &self,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Vec<rmcp::model::ResourceContents>> {
        match self {
            Self::GrantOnly { invocation_ref } | Self::RuntimeJoined { invocation_ref, .. } => {
                Err(runtime_unavailable(invocation_ref))
            }
        }
    }
}

fn task_invocation_from_arguments(
    invocation_ref: &InvocationRef,
    arguments: &Value,
) -> UseResult<RuntimeTaskInvocation> {
    let args = match arguments.get("args") {
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    UseError::new(
                        "use.plugin.capability_gateway_arguments_invalid",
                        "Runtime Task args must be a JSON array of strings.",
                    )
                })
            })
            .collect::<UseResult<Vec<_>>>()?,
        Some(_) => {
            return Err(UseError::new(
                "use.plugin.capability_gateway_arguments_invalid",
                "Runtime Task args must be a JSON array of strings.",
            ))
        }
        None => Vec::new(),
    };
    let invocation_id = arguments
        .get("invocationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| invocation_ref.as_str());
    RuntimeTaskInvocation::new(invocation_id, args)
}

fn runtime_unavailable(invocation_ref: &InvocationRef) -> UseError {
    UseError::new(
        RUNTIME_UNAVAILABLE,
        "Control Grant authorization succeeded, but this surface has no joined Runtime receipt for provider I/O.",
    )
    .with_detail("invocationRef", invocation_ref.as_str())
}

/// Resolver that joins an opaque descriptor to the current durable Control
/// publication and retains its generation lease through the complete call.
#[derive(Clone)]
pub(in crate::control_store) struct ControlCapabilityGatewayInvocationResolver {
    plane: Arc<ControlCapabilityPlaneEffectPort>,
    factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
}

impl ControlCapabilityGatewayInvocationResolver {
    pub(in crate::control_store) fn new(
        plane: Arc<ControlCapabilityPlaneEffectPort>,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
    ) -> Self {
        Self { plane, factory }
    }
}

#[async_trait]
impl CapabilityGatewayInvocationResolver for ControlCapabilityGatewayInvocationResolver {
    async fn resolve(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<CapabilityGatewayInvocationLease> {
        let lease = self.plane.reopen_published().await?.ok_or_else(|| {
            UseError::new(
                RESOLUTION_ERROR,
                "The published Control capability generation is unavailable for invocation.",
            )
        })?;
        lease.validate_gateway_descriptor(descriptor)?;
        let handle = self.factory.open(descriptor, context, &lease).await?;
        let external_lease: Arc<dyn CapabilityGatewayExternalLease> = Arc::new(lease);
        Ok(CapabilityGatewayInvocationLease::with_external_lease(
            descriptor.invocation_ref.clone(),
            external_lease,
            handle,
        ))
    }
}
