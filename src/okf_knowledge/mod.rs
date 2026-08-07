//! Injected A3S Knowledge port and durable exact-generation OKF evidence.

mod adapter;
mod model;
mod query;
mod sqlite;
mod store;

pub use adapter::{
    OkfKnowledgeAdapter, OkfKnowledgeClient, OkfKnowledgeStageRequest, OkfKnowledgeStageSpec,
};
pub use model::{OkfKnowledgeBinding, OKF_KNOWLEDGE_BINDING_SCHEMA};
pub use query::{
    OkfKnowledgeCitation, OkfKnowledgeSearchHit, OkfKnowledgeSearchRequest,
    OkfKnowledgeSearchResponse, OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA,
    OKF_KNOWLEDGE_SEARCH_RESPONSE_SCHEMA,
};
pub use sqlite::{
    OkfKnowledgeBackupManifest, OkfKnowledgeIntegrityReport, OkfKnowledgeSearchIndexRepair,
    OkfKnowledgeStoragePolicy, OkfKnowledgeStorageUsage, SqliteOkfKnowledgeAdapter,
    DEFAULT_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES, DEFAULT_OKF_KNOWLEDGE_SCOPE_PROJECTIONS,
    DEFAULT_OKF_KNOWLEDGE_SCOPE_TOMBSTONES, MAX_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES,
    MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS, MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
    OKF_KNOWLEDGE_BACKUP_SCHEMA, OKF_KNOWLEDGE_INTEGRITY_REPORT_SCHEMA,
    OKF_KNOWLEDGE_SEARCH_INDEX_REPAIR_SCHEMA,
};
pub use store::{
    OkfKnowledgeBindingSnapshot, OkfKnowledgeBindingStore, MAX_OKF_KNOWLEDGE_GENERATIONS,
};

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
