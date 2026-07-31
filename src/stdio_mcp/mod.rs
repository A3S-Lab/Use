//! Supervised standard-MCP sessions for package-declared stdio surfaces.
//!
//! A trusted host provider owns any claimed OS confinement, process creation,
//! stderr draining, and provider-owned process-unit cleanup. A3S Use owns the immutable
//! package/grant/provider plan, bounded MCP initialization, package-generation
//! lease, live durable-authorization checks, liveness projection, and shutdown
//! state machine.

mod authorization;
mod model;
mod native_host;
mod process_model;
mod settlement;
mod supervisor;
mod transport;
mod validation;

pub use authorization::{StdioMcpAuthorizationObservation, StdioMcpAuthorizationState};
pub use model::{
    SpawnedStdioMcpSession, StdioMcpHostCapabilities, StdioMcpHostFeature, StdioMcpHostProvider,
    StdioMcpHostRoots, StdioMcpProcessControl, StdioMcpSessionPlan, StdioMcpSessionRequest,
    STDIO_MCP_SESSION_PLAN_SCHEMA,
};
pub use native_host::NativeUnconfinedStdioMcpHost;
pub use process_model::{
    StdioMcpProcessIdentity, StdioMcpProcessObservation, StdioMcpProcessState,
};
pub use settlement::StdioMcpPackageLease;
pub use supervisor::{
    PreparedStdioMcpSession, StdioMcpInitializeEvidence, StdioMcpSession, StdioMcpShutdownEvidence,
    StdioMcpSupervisor,
};

#[cfg(test)]
mod tests;
