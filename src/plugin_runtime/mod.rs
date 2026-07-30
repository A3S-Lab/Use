//! Typed A3S Runtime binding primitives for schema-v3 plugin surfaces.
//!
//! This module maps immutable Tool and MCP release descriptors to the public
//! `a3s-runtime` contract. It never selects a provider implicitly and it keeps
//! provider endpoint discovery outside the Runtime unit contract.

mod client;
mod lifecycle;
mod model;
mod planner;
mod receipt;
mod store;
mod task;

pub use a3s_runtime as runtime;

pub use client::{runtime_capabilities_digest, PluginRuntimeClient};
pub use lifecycle::{RuntimeBindingObservation, RuntimeBindingObservedState};
pub use model::{
    RuntimeEndpointRef, RuntimeMcpInitializeEvidence, RuntimePreparedTaskBinding,
    RuntimeResourcePolicy, RuntimeServiceActivation, RuntimeServiceBindingReceipt,
    RuntimeServiceReadinessEvidence, RuntimeSurfaceContext, RuntimeSurfaceContract,
    RuntimeSurfacePlan, RuntimeTaskInvocation, RuntimeWorkloadPolicy,
    RUNTIME_SERVICE_BINDING_SCHEMA, RUNTIME_TASK_BINDING_SCHEMA,
};
pub use planner::{plan_mcp_service_release, plan_tool_service_release, plan_tool_task_release};
pub use receipt::{RuntimeBindingReadiness, RuntimeBindingReceipt};
pub use store::RuntimeBindingStore;
pub use task::RuntimeTaskExecution;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
