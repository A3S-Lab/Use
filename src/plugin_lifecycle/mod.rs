//! Cross-sub-saga binding for one reviewed plugin lifecycle operation.
//!
//! The umbrella host remains the durable parent-saga owner. This module binds
//! its reviewed plan to scope-specific workspace-grant and Runtime-binding
//! intents, verifies readiness/completion gates, and derives both child
//! cutovers from one capability publication.

mod binding;
mod children;
mod cutover;
mod progress;
mod validation;

pub use binding::{
    PluginLifecycleGrantIntentBinding, PluginLifecycleOperationBinding,
    PluginLifecycleRuntimeIntentBinding, PLUGIN_LIFECYCLE_OPERATION_BINDING_SCHEMA,
};
pub use cutover::{PluginLifecycleCutoverEvidence, PLUGIN_LIFECYCLE_CUTOVER_SCHEMA};

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
