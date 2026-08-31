//! Injected A3S Knowledge port and durable exact-generation OKF evidence.

mod adapter;
mod lease;
mod model;
mod query;
mod read;
mod recovery;
mod sqlite;
mod store;

pub use adapter::{
    OkfKnowledgeAdapter, OkfKnowledgeClient, OkfKnowledgeStageRequest, OkfKnowledgeStageSpec,
};
pub use lease::{OkfKnowledgeLease, OkfKnowledgeLeaseProvider};
pub use model::{OkfKnowledgeBinding, OKF_KNOWLEDGE_BINDING_SCHEMA};
pub use query::{
    OkfKnowledgeCitation, OkfKnowledgeSearchHit, OkfKnowledgeSearchRequest,
    OkfKnowledgeSearchResponse, OKF_KNOWLEDGE_CITATION_SCHEMA, OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA,
    OKF_KNOWLEDGE_SEARCH_RESPONSE_SCHEMA,
};
pub use read::{
    OkfKnowledgeReadRequest, OkfKnowledgeReadResponse, OKF_KNOWLEDGE_READ_REQUEST_SCHEMA,
    OKF_KNOWLEDGE_READ_RESPONSE_SCHEMA,
};
pub use recovery::{
    OkfKnowledgeDatabaseEvidence, OkfKnowledgeFileEvidence, OkfKnowledgeRecoveryManager,
    OkfKnowledgeRestoreDiagnostic, OkfKnowledgeRestoreOperationDiagnostic,
    OkfKnowledgeRestoreOperationDiagnosticStatus, OkfKnowledgeRestorePlan,
    OkfKnowledgeRestorePlanStatus, OkfKnowledgeRestoreResult,
    OKF_KNOWLEDGE_RESTORE_DIAGNOSTIC_SCHEMA, OKF_KNOWLEDGE_RESTORE_OPERATION_SCHEMA,
    OKF_KNOWLEDGE_RESTORE_PLAN_SCHEMA, OKF_KNOWLEDGE_RESTORE_RESULT_SCHEMA,
};
pub use sqlite::{
    OkfKnowledgeBackupManifest, OkfKnowledgeBackupRetentionEntry, OkfKnowledgeBackupRetentionPlan,
    OkfKnowledgeBackupRetentionPolicy, OkfKnowledgeBackupRetentionResult,
    OkfKnowledgeIntegrityReport, OkfKnowledgeSearchIndexRepair, OkfKnowledgeStoragePolicy,
    OkfKnowledgeStorageUsage, SqliteOkfKnowledgeAdapter,
    DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BACKUPS,
    DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BYTES, DEFAULT_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES,
    DEFAULT_OKF_KNOWLEDGE_SCOPE_PROJECTIONS, DEFAULT_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
    MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BACKUPS, MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BYTES,
    MAX_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES, MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS,
    MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES, OKF_KNOWLEDGE_BACKUP_RETENTION_PLAN_SCHEMA,
    OKF_KNOWLEDGE_BACKUP_RETENTION_RESULT_SCHEMA, OKF_KNOWLEDGE_BACKUP_SCHEMA,
    OKF_KNOWLEDGE_INTEGRITY_REPORT_SCHEMA, OKF_KNOWLEDGE_SEARCH_INDEX_REPAIR_SCHEMA,
};
pub(crate) use sqlite::{ScopeDatabaseGuard, VerifiedOkfKnowledgeBackup};
pub use store::{
    OkfKnowledgeBindingSnapshot, OkfKnowledgeBindingStore, MAX_OKF_KNOWLEDGE_GENERATIONS,
};

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
