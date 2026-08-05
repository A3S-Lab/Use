//! Durable package-level lifecycle planning for cognitive plugins.
//!
//! A schema-v3 package is the unit of identity, trust, generation,
//! installation, enablement, upgrade, and removal. Tool, MCP, OKF, Flow, Skill,
//! and UI contributions participate in one ordered operation; they are not
//! independently installable packages.

mod coordinator;
mod grant;
mod graph;
mod journal;
mod model;
mod okf;
mod registry_hosts;
mod runtime;
mod schedule;
mod static_surfaces;
mod store;

pub use coordinator::{
    PluginCapabilityLifecycleHost, PluginFlowLifecycleHost, PluginLifecycleCoordinator,
    PluginLifecycleEvidence, PluginLifecycleHosts, PluginMcpLifecycleHost, PluginOkfLifecycleHost,
    PluginPackageLifecycleHost, PluginSkillLifecycleHost, PluginToolLifecycleHost,
    PluginUiLifecycleHost,
};
pub use grant::{PluginCapabilityCutoverEvidence, PluginGrantLifecycleUnit};
pub use graph::{
    PluginGraphCapabilityLifecycleHost, PluginGraphCapabilityPublication,
    PluginPackageGraphLifecycleCoordinator, PluginPackageLifecycleUnit,
    PluginPackagePublicationEvidence, PluginPackageRollbackEvidence,
};
pub use journal::{
    PluginLifecycleCheckpointOutcome, PluginLifecycleCheckpointReceipt, PluginLifecycleFailure,
    PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
    PLUGIN_LIFECYCLE_OPERATION_SCHEMA,
};
pub use model::{
    PluginLifecycleAction, PluginLifecycleCheckpoint, PluginLifecycleCheckpointKind,
    PluginLifecycleIntent, PluginLifecycleIntentSpec, PluginLifecycleSurface, PluginSurfaceHost,
    PLUGIN_LIFECYCLE_INTENT_SCHEMA,
};
pub use okf::OkfKnowledgeLifecycleHost;
pub use registry_hosts::{
    ExtensionCapabilityLifecycleHost, ExtensionGraphCapabilityLifecycleHost,
    ExtensionPackageLifecycleHost,
};
pub use runtime::{
    PluginMcpServiceReadiness, PluginRuntimeServiceReadinessHost, RuntimePluginSurfaceLifecycleHost,
};
pub use static_surfaces::StaticPluginSurfaceLifecycleHost;
pub use store::PluginLifecycleJournalStore;

#[cfg(test)]
mod test_support;
