//! Exact-generation A3S Flow preflight bindings for cognitive packages.

mod lifecycle;
mod model;
mod store;

pub use lifecycle::A3sFlowLifecycleHost;
pub use model::{FlowRuntimeBinding, FLOW_RUNTIME_BINDING_SCHEMA};
pub use store::{FlowRuntimeBindingStore, MAX_FLOW_RUNTIME_GENERATIONS};

#[cfg(test)]
mod tests;
