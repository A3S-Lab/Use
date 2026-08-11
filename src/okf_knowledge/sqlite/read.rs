use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::okf_knowledge::{OkfKnowledgeReadRequest, OkfKnowledgeReadResponse};

use super::projection::database_io;
use super::search::validate_active_projection;

/// Read one bounded Markdown document after revalidating the exact promoted
/// projection and its source digest. The database is treated as an indexed
/// cache; it cannot silently turn changed bytes into accepted knowledge.
pub(super) fn read(
    connection: &Connection,
    request: &OkfKnowledgeReadRequest,
) -> a3s_use_core::UseResult<OkfKnowledgeReadResponse> {
    request.validate()?;
    validate_active_projection(connection, &request.projection)?;

    let row = connection
        .query_row(
            "SELECT length(content), source_digest
             FROM knowledge_documents
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3
               AND concept_id = ?4 AND path = ?5",
            params![
                request.projection.surface.package_id,
                request.projection.surface.surface.id,
                i64::try_from(request.projection.generation).map_err(|_| {
                    read_error("The OKF Knowledge generation exceeds SQLite bounds.")
                })?,
                request.citation.concept_id,
                request.citation.path,
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| database_io("locate cited Knowledge document", error))?
        .ok_or_else(|| read_error("The exact cited OKF Knowledge document is unavailable."))?;
    let byte_count = usize::try_from(row.0)
        .map_err(|_| read_error("The cited OKF document byte count is invalid."))?;
    if byte_count == 0 || byte_count > request.max_bytes {
        return Err(read_error(
            "The cited OKF document exceeds the requested read bound.",
        ));
    }
    if row.1 != request.citation.source_digest {
        return Err(read_error(
            "The retained OKF document source digest differs from the citation.",
        ));
    }

    let content = connection
        .query_row(
            "SELECT content
             FROM knowledge_documents
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3
               AND concept_id = ?4 AND path = ?5",
            params![
                request.projection.surface.package_id,
                request.projection.surface.surface.id,
                i64::try_from(request.projection.generation).map_err(|_| {
                    read_error("The OKF Knowledge generation exceeds SQLite bounds.")
                })?,
                request.citation.concept_id,
                request.citation.path,
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(|error| database_io("read cited OKF Knowledge document", error))?;
    if content.len() != byte_count {
        return Err(read_error(
            "The cited OKF document changed while it was being read.",
        ));
    }
    let digest = format!("sha256:{:x}", Sha256::digest(&content));
    if digest != request.citation.source_digest {
        return Err(read_error(
            "The cited OKF document content differs from its source digest.",
        ));
    }
    let content = String::from_utf8(content)
        .map_err(|_| read_error("The cited OKF document is not valid UTF-8 Markdown."))?;
    OkfKnowledgeReadResponse::new(request, content)
}

fn read_error(message: impl Into<String>) -> a3s_use_core::UseError {
    a3s_use_core::UseError::new("use.okf.knowledge_read_failed", message)
}
