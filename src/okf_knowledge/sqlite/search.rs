use std::collections::BTreeSet;

use a3s_use_core::{OkfCapabilityProjection, OkfKnowledgeObservedState, UseError, UseResult};
use rusqlite::{params, Connection};

use super::projection::{load_projection_for_search, ProjectionState, StoredProjection};
use crate::okf_knowledge::query::compare_hits;
use crate::okf_knowledge::{
    OkfKnowledgeCitation, OkfKnowledgeSearchHit, OkfKnowledgeSearchRequest,
    OkfKnowledgeSearchResponse,
};

const MAX_QUERY_TERMS: usize = 32;
const MAX_CANDIDATES_PER_PROJECTION: usize = 1_600;
const MAX_SNIPPET_CHARS: usize = 360;

#[derive(Debug)]
struct SearchDocument {
    concept_id: String,
    path: String,
    type_name: String,
    title: String,
    search_text: String,
    source_digest: String,
}

pub(super) fn search(
    connection: &Connection,
    request: &OkfKnowledgeSearchRequest,
) -> UseResult<OkfKnowledgeSearchResponse> {
    request.validate()?;
    let terms = query_terms(&request.query)?;
    let fts_query = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let candidate_limit = request
        .limit
        .saturating_mul(16)
        .clamp(request.limit, MAX_CANDIDATES_PER_PROJECTION);

    let mut hits = Vec::new();
    for projection in &request.projections {
        validate_active_projection(connection, projection)?;
        let mut documents = fts_documents(connection, projection, &fts_query, candidate_limit)?;
        if documents.is_empty() {
            documents = all_documents(connection, projection)?;
        }
        for document in documents {
            let Some(score) = score_document(&request.query, &terms, &document) else {
                continue;
            };
            hits.push(OkfKnowledgeSearchHit {
                score,
                title: document.title,
                type_name: document.type_name,
                snippet: snippet(&document.search_text),
                citation: OkfKnowledgeCitation {
                    surface: projection.surface.clone(),
                    generation: projection.generation,
                    package_digest: projection.package_digest.clone(),
                    bundle_digest: projection.bundle.content_digest.clone(),
                    projection_receipt_digest: projection.projection_receipt_digest.clone(),
                    index_digest: projection.index_digest.clone(),
                    concept_id: document.concept_id,
                    path: document.path,
                    source_digest: document.source_digest,
                },
            });
        }
    }
    hits.sort_by(compare_hits);
    let mut seen = BTreeSet::new();
    hits.retain(|hit| {
        seen.insert((
            hit.citation.surface.clone(),
            hit.citation.generation,
            hit.citation.concept_id.clone(),
        ))
    });
    hits.truncate(request.limit);
    OkfKnowledgeSearchResponse::new(request, hits)
}

fn validate_active_projection(
    connection: &Connection,
    projection: &OkfCapabilityProjection,
) -> UseResult<()> {
    projection.validate()?;
    let stored = require_projection_from_capability(connection, projection)?;
    if stored.state != ProjectionState::Promoted
        || stored.index_digest != projection.index_digest
        || stored.receipt.manifest_digest != projection.manifest_digest
        || stored.receipt.bundle != projection.bundle
        || stored.receipt.projection_id != projection.projection_id
        || stored.receipt.index_schema != projection.index_schema
        || stored.receipt.index_build_id != projection.index_build_id
    {
        return Err(stale_projection_error());
    }
    let selected = a3s_use_core::OkfSelectedGeneration {
        generation: stored.receipt.generation,
        package_digest: stored.receipt.package_digest.clone(),
        bundle_digest: stored.receipt.bundle.content_digest.clone(),
        projection_receipt_digest: stored.receipt_digest.clone(),
        index_schema: stored.receipt.index_schema.clone(),
        index_build_id: stored.receipt.index_build_id.clone(),
        index_digest: stored.index_digest.clone(),
    };
    let observation = a3s_use_core::OkfKnowledgeObservation {
        schema: a3s_use_core::OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_owned(),
        scope: stored.receipt.scope.clone(),
        surface: stored.receipt.surface.clone(),
        generation: stored.receipt.generation,
        package_digest: stored.receipt.package_digest.clone(),
        bundle_digest: stored.receipt.bundle.content_digest.clone(),
        projection_receipt_digest: stored.receipt_digest.clone(),
        index_schema: stored.receipt.index_schema.clone(),
        index_build_id: stored.receipt.index_build_id.clone(),
        state: OkfKnowledgeObservedState::Promoted,
        observed_at_ms: stored.observed_at_ms,
        index_digest: Some(stored.index_digest.clone()),
        selected: Some(selected),
    };
    let rebuilt = OkfCapabilityProjection::from_promoted(&stored.receipt, &observation)?;
    if rebuilt != *projection {
        return Err(stale_projection_error());
    }
    Ok(())
}

fn require_projection_from_capability(
    connection: &Connection,
    projection: &OkfCapabilityProjection,
) -> UseResult<StoredProjection> {
    // Operation identity and staging time are intentionally absent from a
    // capability projection. Load by exact generation, then rebuild and
    // compare the complete projection-carried evidence above.
    load_projection_for_search(
        connection,
        &projection.surface.package_id,
        &projection.surface.surface.id,
        projection.generation,
    )?
    .ok_or_else(stale_projection_error)
}

fn fts_documents(
    connection: &Connection,
    projection: &OkfCapabilityProjection,
    query: &str,
    limit: usize,
) -> UseResult<Vec<SearchDocument>> {
    let mut statement = connection
        .prepare(
            "SELECT d.concept_id, d.path, d.type_name, d.title,
                    d.search_text, d.source_digest
             FROM knowledge_documents_fts f
             JOIN knowledge_documents d ON d.row_id = f.rowid
             WHERE knowledge_documents_fts MATCH ?1
               AND d.package_id = ?2
               AND d.surface_id = ?3
               AND d.generation = ?4
             ORDER BY bm25(knowledge_documents_fts), d.path
             LIMIT ?5",
        )
        .map_err(|error| search_io("prepare FTS query", error))?;
    let rows = statement
        .query_map(
            params![
                query,
                projection.surface.package_id,
                projection.surface.surface.id,
                generation_i64(projection.generation)?,
                i64::try_from(limit)
                    .map_err(|_| search_error("The candidate limit overflowed."))?,
            ],
            row_to_document,
        )
        .map_err(|error| search_io("execute FTS query", error))?;
    collect_documents(rows)
}

fn all_documents(
    connection: &Connection,
    projection: &OkfCapabilityProjection,
) -> UseResult<Vec<SearchDocument>> {
    let mut statement = connection
        .prepare(
            "SELECT concept_id, path, type_name, title, search_text, source_digest
             FROM knowledge_documents
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3
             ORDER BY path",
        )
        .map_err(|error| search_io("prepare bounded Knowledge scan", error))?;
    let rows = statement
        .query_map(
            params![
                projection.surface.package_id,
                projection.surface.surface.id,
                generation_i64(projection.generation)?,
            ],
            row_to_document,
        )
        .map_err(|error| search_io("execute bounded Knowledge scan", error))?;
    collect_documents(rows)
}

fn row_to_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchDocument> {
    Ok(SearchDocument {
        concept_id: row.get(0)?,
        path: row.get(1)?,
        type_name: row.get(2)?,
        title: row.get(3)?,
        search_text: row.get(4)?,
        source_digest: row.get(5)?,
    })
}

fn collect_documents(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<SearchDocument>,
    >,
) -> UseResult<Vec<SearchDocument>> {
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_io("read cited Knowledge result", error))
}

fn query_terms(query: &str) -> UseResult<Vec<String>> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character.is_alphanumeric() || character == '_' {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            if !terms.contains(&current) {
                terms.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if !current.is_empty() && !terms.contains(&current) {
        terms.push(current);
    }
    if terms.is_empty() || terms.len() > MAX_QUERY_TERMS {
        return Err(search_error(format!(
            "An OKF Knowledge query must contain between 1 and {MAX_QUERY_TERMS} distinct searchable terms."
        )));
    }
    Ok(terms)
}

fn score_document(query: &str, terms: &[String], document: &SearchDocument) -> Option<u64> {
    let query = query.to_lowercase();
    let title = document.title.to_lowercase();
    let text = document.search_text.to_lowercase();
    let mut score = 0_u64;
    if title.contains(&query) {
        score += 20_000;
    }
    if text.contains(&query) {
        score += 10_000;
    }
    for term in terms {
        let title_matches = count_matches(&title, term).min(10);
        let text_matches = count_matches(&text, term).min(50);
        score += title_matches * 2_000 + text_matches * 100;
    }
    (score > 0).then_some(score)
}

fn count_matches(value: &str, term: &str) -> u64 {
    u64::try_from(value.match_indices(term).count()).unwrap_or(u64::MAX)
}

fn snippet(value: &str) -> String {
    let mut output = value.chars().take(MAX_SNIPPET_CHARS).collect::<String>();
    if value.chars().count() > MAX_SNIPPET_CHARS {
        output.push('…');
    }
    output
}

fn generation_i64(value: u64) -> UseResult<i64> {
    i64::try_from(value).map_err(|_| search_error("The OKF generation exceeds SQLite bounds."))
}

fn stale_projection_error() -> UseError {
    UseError::new(
        "use.okf.knowledge_projection_stale",
        "The OKF capability/session projection is not an exact retained promoted Knowledge generation.",
    )
}

fn search_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_query_invalid", message)
}

fn search_io(action: &str, error: rusqlite::Error) -> UseError {
    UseError::new(
        "use.okf.knowledge_database_io",
        format!("Failed to {action}: {error}"),
    )
}
