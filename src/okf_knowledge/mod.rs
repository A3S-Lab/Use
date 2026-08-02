//! Injected A3S Knowledge port and durable exact-generation OKF evidence.

mod adapter;
mod model;
mod store;

pub use adapter::{
    OkfKnowledgeAdapter, OkfKnowledgeClient, OkfKnowledgeStageRequest, OkfKnowledgeStageSpec,
};
pub use model::{OkfKnowledgeBinding, OKF_KNOWLEDGE_BINDING_SCHEMA};
pub use store::{
    OkfKnowledgeBindingSnapshot, OkfKnowledgeBindingStore, MAX_OKF_KNOWLEDGE_GENERATIONS,
};

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
