use a3s_use_core::{PlanScope, UseError, UseResult};
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::index::{self, IndexedDocument};
use super::policy::OkfKnowledgeStoragePolicy;
use super::projection::{
    database_invalid, database_io, load_projection, ProjectionState, StoredProjection,
};
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
    validate_document_content(connection)?;
    let document_count = count_rows(connection, "knowledge_documents")?;
    Ok((storage, document_count))
}

/// Validate the retained source bytes that are used by both search citations
/// and exact reads. The document table is an authoritative cache only when
/// every row remains bounded, UTF-8, and digest-identical to its recorded
/// source evidence. This check intentionally runs during every audit and
/// before search-index repair so database tampering fails closed.
fn validate_document_content(connection: &Connection) -> UseResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, surface_id, generation
             FROM knowledge_projections
             ORDER BY package_id, surface_id, generation",
        )
        .map_err(|error| database_io("prepare Knowledge projection content audit", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| database_io("query Knowledge projection content audit", error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| database_io("read Knowledge projection content audit", error))?
    {
        let package_id = row
            .get::<_, String>(0)
            .map_err(|error| database_io("read audited Knowledge package identity", error))?;
        let surface_id = row
            .get::<_, String>(1)
            .map_err(|error| database_io("read audited Knowledge surface identity", error))?;
        let generation = row
            .get::<_, i64>(2)
            .map_err(|error| database_io("read audited Knowledge generation", error))?;
        let generation = u64::try_from(generation)
            .map_err(|_| database_invalid("An audited Knowledge generation is negative."))?;
        let projection = load_projection(connection, &package_id, &surface_id, generation)?
            .ok_or_else(|| database_invalid("An audited Knowledge projection disappeared."))?;
        validate_projection_documents(connection, &projection)?;
    }
    Ok(())
}

fn validate_projection_documents(
    connection: &Connection,
    projection: &StoredProjection,
) -> UseResult<()> {
    let receipt = &projection.receipt;
    let mut statement = connection
        .prepare(
            "SELECT concept_id, path, type_name, title, search_text,
                    source_digest, content
             FROM knowledge_documents
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3
             ORDER BY path",
        )
        .map_err(|error| database_io("prepare retained Knowledge document audit", error))?;
    let mut rows = statement
        .query(rusqlite::params![
            receipt.surface.package_id,
            receipt.surface.surface.id,
            i64::try_from(receipt.generation).map_err(|_| {
                database_invalid("An audited Knowledge generation exceeds SQLite bounds.")
            })?,
        ])
        .map_err(|error| database_io("query retained Knowledge document audit", error))?;
    let mut documents = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| database_io("read retained Knowledge document audit", error))?
    {
        let document = IndexedDocument {
            concept_id: row
                .get(0)
                .map_err(|error| database_io("read Knowledge concept identity", error))?,
            path: row
                .get(1)
                .map_err(|error| database_io("read Knowledge document path", error))?,
            type_name: row
                .get(2)
                .map_err(|error| database_io("read Knowledge document type", error))?,
            title: row
                .get(3)
                .map_err(|error| database_io("read Knowledge document title", error))?,
            search_text: row
                .get(4)
                .map_err(|error| database_io("read Knowledge document search text", error))?,
            source_digest: row
                .get(5)
                .map_err(|error| database_io("read Knowledge document source digest", error))?,
            content: row
                .get(6)
                .map_err(|error| database_io("read Knowledge document content", error))?,
        };
        validate_document_bytes(&document, receipt.bundle.limits.max_document_bytes)?;
        documents.push(document);
        if u64::try_from(documents.len())
            .ok()
            .is_none_or(|count| count > receipt.bundle.limits.max_concepts)
        {
            return Err(database_invalid(
                "A retained Knowledge projection exceeds its immutable concept bound.",
            ));
        }
    }
    if projection.state == ProjectionState::Removed {
        if !documents.is_empty() {
            return Err(database_invalid(
                "A removed Knowledge projection still retains source documents.",
            ));
        }
        return Ok(());
    }
    if u64::try_from(documents.len()).ok() != Some(receipt.bundle.concept_count) {
        return Err(database_invalid(
            "A retained Knowledge projection does not contain its immutable concept count.",
        ));
    }
    let rebuilt = index::descriptor_digest(&receipt.bundle.content_digest, &documents)
        .map_err(|error| database_invalid(error.message))?;
    if rebuilt != projection.index_digest {
        return Err(database_invalid(
            "A retained Knowledge projection does not match its immutable search descriptor.",
        ));
    }
    Ok(())
}

fn validate_document_bytes(document: &IndexedDocument, max_document_bytes: u64) -> UseResult<()> {
    let path = &document.path;
    let content = &document.content;
    if content.is_empty() {
        return Err(database_invalid(format!(
            "The retained Knowledge document '{}' is empty.",
            path
        )));
    }
    let content_bytes = u64::try_from(content.len()).map_err(|_| {
        database_invalid(format!(
            "The retained Knowledge document '{}' byte count overflowed.",
            path
        ))
    })?;
    if content_bytes > max_document_bytes {
        return Err(database_invalid(format!(
            "The retained Knowledge document '{}' exceeds its immutable OKF document bound.",
            path
        )));
    }
    if std::str::from_utf8(content).is_err() {
        return Err(database_invalid(format!(
            "The retained Knowledge document '{}' is not valid UTF-8 Markdown.",
            path
        )));
    }
    let actual_digest = format!("sha256:{:x}", Sha256::digest(content));
    if actual_digest != document.source_digest {
        return Err(database_invalid(format!(
            "The retained Knowledge document '{}' does not match its source digest.",
            path
        )));
    }
    Ok(())
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
