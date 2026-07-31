//! Typed A3S Runtime binding primitives for schema-v3 plugin surfaces.
//!
//! This module maps immutable Tool and MCP release descriptors to the public
//! `a3s-runtime` contract. It never selects a provider implicitly and it keeps
//! provider endpoint discovery outside the Runtime unit contract. Host-owned
//! Volume, Tmpfs, and opaque secret references are resolved through an
//! explicit provider-keyed process-local registry.

mod authority;
mod authority_resolver;
mod binding_operation;
mod binding_operation_io;
mod binding_operation_lifecycle;
mod broker;
mod bundle_planner;
mod client;
mod lifecycle;
#[cfg(feature = "extensions")]
mod mcp_initializer;
mod model;
mod planner;
mod provider_selector;
mod receipt;
mod store;
mod surface_observer;
mod task;
mod task_output;

pub use a3s_runtime as runtime;

pub use authority::{
    RuntimeAuthorityBindings, RuntimeFilesystemBinding, RuntimeSecretBinding,
    RuntimeSurfaceAuthorityBindings, RUNTIME_PLUGIN_DATA_MOUNT_ROOT, RUNTIME_TEMPORARY_MOUNT_ROOT,
    RUNTIME_WORKSPACE_MOUNT_ROOT,
};
pub use authority_resolver::{
    ResolvedRuntimeSurfaceAuthority, RuntimeAuthorityResolutionRequest, RuntimeAuthorityResolver,
    RuntimeAuthorityResolverRegistry, MAX_RUNTIME_AUTHORITY_RESOLUTION_TIMEOUT,
};
pub use binding_operation::{
    RuntimeBindingCandidateKind, RuntimeBindingCandidatePlan, RuntimeBindingCutoverEvidence,
    RuntimeBindingOperationIntent, RuntimeBindingOperationJournal, RuntimeBindingOperationPhase,
    RuntimeBindingRetirementEvidence, RUNTIME_BINDING_CUTOVER_SCHEMA,
    RUNTIME_BINDING_OPERATION_SCHEMA,
};
pub use broker::{PluginRuntimeBroker, RuntimeBundlePreflight};
pub use bundle_planner::{plan_runtime_bundle, plan_runtime_bundle_with_authority};
pub use client::{runtime_capabilities_digest, PluginRuntimeClient};
pub use lifecycle::{RuntimeBindingObservation, RuntimeBindingObservedState};
#[cfg(feature = "extensions")]
pub use mcp_initializer::{RuntimeMcpBearerToken, RuntimeMcpHttpConnection, RuntimeMcpInitializer};
pub use model::{
    RuntimeEndpointRef, RuntimeMcpInitializeEvidence, RuntimePreparedTaskBinding,
    RuntimeResourcePolicy, RuntimeServiceActivation, RuntimeServiceBindingReceipt,
    RuntimeServiceReadinessEvidence, RuntimeSurfaceContext, RuntimeSurfaceContract,
    RuntimeSurfacePlan, RuntimeTaskInvocation, RuntimeWorkloadPolicy,
    RUNTIME_SERVICE_BINDING_SCHEMA, RUNTIME_TASK_BINDING_SCHEMA,
};
pub use planner::{plan_mcp_service_release, plan_tool_service_release, plan_tool_task_release};
pub use provider_selector::{
    RuntimeProviderAssignment, RuntimeProviderSelection, RuntimeProviderSelector,
    SelectedRuntimeSurface,
};
pub use receipt::{RuntimeBindingReadiness, RuntimeBindingReceipt};
pub use store::RuntimeBindingStore;
pub use surface_observer::{
    RuntimeSurfaceObservation, RuntimeSurfaceObservationSnapshot, RuntimeSurfaceObservedState,
    RuntimeSurfaceObserver, RUNTIME_SURFACE_OBSERVATION_SCHEMA_VERSION,
};
pub use task::RuntimeTaskExecution;
pub use task_output::{
    RuntimeTaskOutputSummary, RuntimeTaskStreamingExecution, MAX_IN_MEMORY_TASK_OUTPUT_BYTES,
};

#[cfg(test)]
mod authority_resolver_tests;
#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod binding_operation_tests;
#[cfg(all(test, feature = "mcp", feature = "extensions"))]
mod mcp_initializer_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod surface_observer_tests;
#[cfg(test)]
mod task_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
