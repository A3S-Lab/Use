use a3s_use_core::{PlanScope, UseError, UseResult};
use rusqlite::Connection;
use serde::Serialize;

use super::policy::OkfKnowledgeStoragePolicy;
use super::projection::{database_invalid, database_io};
use super::storage::{self, OkfKnowledgeStorageUsage};

pub const OKF_KNOWLEDGE_INTEGRITY_REPORT_SCHEMA: &str = "a3s.use.okf-knowledge-integrity-report.v1";
pub const OKF_KNOWLEDGE_SEARCH_INDEX_REPAIR_SCHEMA: &str =
    "a3s.use.okf-knowledge-search-index-repair.v1";

/// Non-secret evidence that one complete Knowledge scope is internally sound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfKnowledgeIntegrityReport {
    pub schema: String,
    pub scope: PlanScope,
    pub document_count: u64,
    pub indexed_document_count: u64,
    pub storage: OkfKnowledgeStorageUsage,
}

/// Result of rebuilding only the FTS5 index derived from retained documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfKnowledgeSearchIndexRepair {
    pub schema: String,
    pub scope: PlanScope,
    pub rebuilt_document_count: u64,
    pub after: OkfKnowledgeIntegrityReport,
}

pub(super) fn audit(
    connection: &Connection,
    scope: &PlanScope,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<OkfKnowledgeIntegrityReport> {
    let (storage, document_count) = validate_authority(connection, scope, policy)?;
    let indexed_document_count = indexed_document_count(connection)?;
    validate_search_index(connection, document_count, indexed_document_count)?;
    Ok(OkfKnowledgeIntegrityReport {
        schema: OKF_KNOWLEDGE_INTEGRITY_REPORT_SCHEMA.to_owned(),
        scope: scope.clone(),
        document_count,
        indexed_document_count,
        storage,
    })
}

pub(super) fn repair_search_index(
    connection: &mut Connection,
    scope: &PlanScope,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<OkfKnowledgeSearchIndexRepair> {
    // Receipt, scope, projection, foreign-key, and core SQLite evidence must
    // already be valid. Repair is allowed to derive FTS rows from the retained
    // document table, but it cannot invent or rewrite lifecycle authority.
    let (_, rebuilt_document_count) = validate_authority(connection, scope, policy)?;
    connection
        .execute_batch(
            "INSERT INTO knowledge_documents_fts(knowledge_documents_fts) VALUES('rebuild');
             INSERT INTO knowledge_documents_fts(knowledge_documents_fts) VALUES('optimize');",
        )
        .map_err(|error| {
            search_index_error(format!(
                "Failed to rebuild the derived Knowledge search index: {error}"
            ))
        })?;
    let after = audit(connection, scope, policy)?;
    Ok(OkfKnowledgeSearchIndexRepair {
        schema: OKF_KNOWLEDGE_SEARCH_INDEX_REPAIR_SCHEMA.to_owned(),
        scope: scope.clone(),
        rebuilt_document_count,
        after,
    })
}

fn validate_authority(
    connection: &Connection,
    scope: &PlanScope,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<(OkfKnowledgeStorageUsage, u64)> {
    validate_sqlite_integrity(connection)?;
    validate_foreign_keys(connection)?;
    let storage = storage::usage(connection, scope, policy)?;
    let document_count = count_rows(connection, "knowledge_documents")?;
    Ok((storage, document_count))
}

fn validate_sqlite_integrity(connection: &Connection) -> UseResult<()> {
    let mut statement = connection
        .prepare("PRAGMA integrity_check(100)")
        .map_err(|error| database_io("prepare Knowledge integrity check", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| database_io("run Knowledge integrity check", error))?;
    let mut messages = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| database_io("read Knowledge integrity check", error))?
    {
        if messages.len() >= 100 {
            return Err(database_invalid(
                "The Knowledge database integrity report exceeded its bound.",
            ));
        }
        messages.push(
            row.get::<_, String>(0)
                .map_err(|error| database_io("decode Knowledge integrity check", error))?,
        );
    }
    if messages.as_slice() != ["ok"] {
        return Err(database_invalid(
            "The Knowledge database failed SQLite integrity validation.",
        ));
    }
    Ok(())
}

fn validate_foreign_keys(connection: &Connection) -> UseResult<()> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|error| database_io("prepare Knowledge foreign-key check", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| database_io("run Knowledge foreign-key check", error))?;
    if rows
        .next()
        .map_err(|error| database_io("read Knowledge foreign-key check", error))?
        .is_some()
    {
        return Err(database_invalid(
            "The Knowledge database contains orphaned projection or selection rows.",
        ));
    }
    Ok(())
}

fn indexed_document_count(connection: &Connection) -> UseResult<u64> {
    count_rows(connection, "knowledge_documents_fts")
        .map_err(|error| search_index_error(error.message))
}

fn count_rows(connection: &Connection, table: &str) -> UseResult<u64> {
    let value = connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| database_io("count Knowledge rows", error))?;
    u64::try_from(value).map_err(|_| database_invalid("A Knowledge row count is negative."))
}

fn validate_search_index(
    connection: &Connection,
    document_count: u64,
    indexed_document_count: u64,
) -> UseResult<()> {
    if indexed_document_count != document_count {
        return Err(search_index_error(
            "The derived Knowledge search index row count does not match its authoritative document table.",
        ));
    }
    connection
        .execute(
            "INSERT INTO knowledge_documents_fts(knowledge_documents_fts, rank)
             VALUES('integrity-check', 1)",
            [],
        )
        .map_err(|_| {
            search_index_error(
                "The derived Knowledge search index does not match its authoritative document table.",
            )
        })?;
    Ok(())
}

fn search_index_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_search_index_invalid", message)
        .with_suggestion("Run 'a3s-use knowledge repair-search-index --yes' for this exact scope.")
}
