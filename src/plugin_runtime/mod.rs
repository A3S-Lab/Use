//! Typed A3S Runtime binding primitives for schema-v3 plugin surfaces.
//!
//! This module maps immutable Tool and MCP release descriptors to the public
//! `a3s-runtime` contract. It never selects a provider implicitly and it keeps
//! provider endpoint discovery outside the Runtime unit contract.

mod bundle_planner;
mod client;
mod lifecycle;
mod model;
mod plan_store;
#[cfg(test)]
mod plan_store_tests;
mod planner;
mod provider_selector;
mod provisioning;
#[cfg(test)]
pub(crate) mod provisioning_fault_matrix;
mod receipt;
mod resolver;
#[cfg(test)]
mod resolver_tests;
mod store;
mod surface_observer;
mod task;
mod task_binding;
mod task_dispatch;

pub use a3s_runtime as runtime;

pub use bundle_planner::plan_runtime_bundle;
pub use client::{runtime_capabilities_digest, PluginRuntimeClient};
pub use lifecycle::{RuntimeBindingObservation, RuntimeBindingObservedState};
pub use model::{
    RuntimeEndpointRef, RuntimeMcpInitializeEvidence, RuntimePreparedTaskBinding,
    RuntimeResourcePolicy, RuntimeServiceActivation, RuntimeServiceBindingReceipt,
    RuntimeServiceReadinessEvidence, RuntimeSurfaceContext, RuntimeSurfaceContract,
    RuntimeSurfacePlan, RuntimeTaskInvocation, RuntimeWorkloadPolicy,
    MAX_RUNTIME_SURFACE_PLAN_BYTES, RUNTIME_SERVICE_BINDING_SCHEMA, RUNTIME_SURFACE_PLAN_SCHEMA,
    RUNTIME_TASK_BINDING_SCHEMA,
};
pub use plan_store::{
    RuntimeSurfacePlanPublication, RuntimeSurfacePlanPublishResult, RuntimeSurfacePlanStore,
    MAX_RUNTIME_SURFACE_PLAN_BATCH_BYTES, MAX_RUNTIME_SURFACE_PLAN_RECORDS,
    MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES, RUNTIME_SURFACE_PLAN_STORE_SCHEMA,
};
pub use planner::{plan_mcp_service_release, plan_tool_service_release, plan_tool_task_release};
pub use provider_selector::{
    RuntimeProviderAssignment, RuntimeProviderSelection, RuntimeProviderSelector,
    SelectedRuntimeSurface,
};
pub use provisioning::{
    RuntimeServiceProvisioningPhase, RuntimeServiceProvisioningReceipt,
    RUNTIME_SERVICE_PROVISIONING_SCHEMA,
};
pub use receipt::{RuntimeBindingReadiness, RuntimeBindingReceipt};
pub use resolver::{
    CommittedRuntimeSurfaceResolver, RuntimeProviderSelectionResolver, RuntimeSurfacePlanKey,
    RuntimeSurfacePlanSource, RuntimeSurfaceResolver,
};
pub use store::{RuntimeBindingStore, MAX_RUNTIME_BINDING_GENERATIONS};
pub use surface_observer::{
    RuntimeSurfaceObservation, RuntimeSurfaceObservationSnapshot, RuntimeSurfaceObservedState,
    RuntimeSurfaceObserver, RUNTIME_SURFACE_OBSERVATION_SCHEMA_VERSION,
};
pub use task::RuntimeTaskExecution;
pub use task_dispatch::{RuntimeTaskDispatchRequest, RuntimeTaskDispatcher};

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod surface_observer_tests;
#[cfg(test)]
mod task_dispatch_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
