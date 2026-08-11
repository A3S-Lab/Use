use std::collections::BTreeSet;

use a3s_use_core::{
    OkfCapabilityProjection, PlanQualifiedSurfaceRef, PlanScope, PluginPackageId,
    PluginSurfaceKind, UseError, UseResult,
};
use serde::{Deserialize, Serialize};

pub const OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA: &str = "a3s.use.okf-knowledge-search-request.v1";
pub const OKF_KNOWLEDGE_SEARCH_RESPONSE_SCHEMA: &str = "a3s.use.okf-knowledge-search-response.v1";
pub const OKF_KNOWLEDGE_CITATION_SCHEMA: &str = "a3s.use.okf-knowledge-citation.v1";

const MAX_QUERY_BYTES: usize = 4 * 1024;
const MAX_QUERY_PROJECTIONS: usize = 256;
const MAX_QUERY_RESULTS: usize = 100;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SNIPPET_BYTES: usize = 2 * 1024;

/// Scope-bound cited retrieval over an explicit capability/session projection.
///
/// Callers cannot ask the Knowledge backend to scan every retained index. They
/// must pass the exact promoted projections visible in their current
/// capability snapshot or session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeSearchRequest {
    pub schema: String,
    pub scope: PlanScope,
    pub query: String,
    pub limit: usize,
    pub projections: Vec<OkfCapabilityProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeCitation {
    pub schema: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub bundle_digest: String,
    pub projection_receipt_digest: String,
    pub index_digest: String,
    pub concept_id: String,
    pub path: String,
    pub source_digest: String,
}

impl OkfKnowledgeCitation {
    pub(crate) fn new(
        projection: &OkfCapabilityProjection,
        concept_id: String,
        path: String,
        source_digest: String,
    ) -> Self {
        Self {
            schema: OKF_KNOWLEDGE_CITATION_SCHEMA.to_owned(),
            surface: projection.surface.clone(),
            generation: projection.generation,
            package_digest: projection.package_digest.clone(),
            bundle_digest: projection.bundle.content_digest.clone(),
            projection_receipt_digest: projection.projection_receipt_digest.clone(),
            index_digest: projection.index_digest.clone(),
            concept_id,
            path,
            source_digest,
        }
    }

    pub(crate) fn validate_for_projection(
        &self,
        projection: &OkfCapabilityProjection,
    ) -> UseResult<()> {
        projection.validate()?;
        if self.schema != OKF_KNOWLEDGE_CITATION_SCHEMA
            || self.surface != projection.surface
            || self.generation != projection.generation
            || self.package_digest != projection.package_digest
            || self.bundle_digest != projection.bundle.content_digest
            || self.projection_receipt_digest != projection.projection_receipt_digest
            || self.index_digest != projection.index_digest
            || !valid_sha256(&self.source_digest)
            || !valid_concept_id(&self.concept_id)
            || !valid_relative_path(&self.path)
        {
            return Err(response_error(
                "The OKF Knowledge citation does not bind the exact capability projection.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeSearchHit {
    pub score: u64,
    pub title: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub snippet: String,
    pub citation: OkfKnowledgeCitation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeSearchResponse {
    pub schema: String,
    pub scope: PlanScope,
    pub query: String,
    pub hits: Vec<OkfKnowledgeSearchHit>,
}

impl OkfKnowledgeSearchRequest {
    pub fn new(
        scope: PlanScope,
        query: impl Into<String>,
        limit: usize,
        projections: Vec<OkfCapabilityProjection>,
    ) -> UseResult<Self> {
        let request = Self {
            schema: OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA.to_owned(),
            scope,
            query: query.into(),
            limit,
            projections,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA
            || !valid_machine_id(&self.scope.id)
            || self.query.trim() != self.query
            || self.query.is_empty()
            || self.query.len() > MAX_QUERY_BYTES
            || self.query.chars().any(char::is_control)
            || self.limit == 0
            || self.limit > MAX_QUERY_RESULTS
            || self.projections.is_empty()
            || self.projections.len() > MAX_QUERY_PROJECTIONS
        {
            return Err(query_error(
                "The OKF Knowledge search request exceeds its identity, query, projection, or result bounds.",
            ));
        }

        let mut identities = BTreeSet::new();
        for projection in &self.projections {
            projection.validate()?;
            if projection.scope != self.scope {
                return Err(query_error(
                    "An OKF Knowledge search projection belongs to a different User or Workspace scope.",
                ));
            }
            let identity = (
                projection.surface.clone(),
                projection.generation,
                projection.projection_receipt_digest.clone(),
            );
            if !identities.insert(identity) {
                return Err(query_error(
                    "An OKF Knowledge search projection is repeated.",
                ));
            }
        }
        Ok(())
    }
}

impl OkfKnowledgeSearchResponse {
    pub fn new(
        request: &OkfKnowledgeSearchRequest,
        hits: Vec<OkfKnowledgeSearchHit>,
    ) -> UseResult<Self> {
        let response = Self {
            schema: OKF_KNOWLEDGE_SEARCH_RESPONSE_SCHEMA.to_owned(),
            scope: request.scope.clone(),
            query: request.query.clone(),
            hits,
        };
        response.validate_for(request)?;
        Ok(response)
    }

    pub fn validate_for(&self, request: &OkfKnowledgeSearchRequest) -> UseResult<()> {
        request.validate()?;
        if self.schema != OKF_KNOWLEDGE_SEARCH_RESPONSE_SCHEMA
            || self.scope != request.scope
            || self.query != request.query
            || self.hits.len() > request.limit
        {
            return Err(response_error(
                "The OKF Knowledge search response does not match the exact request.",
            ));
        }

        let mut citations = BTreeSet::new();
        for (position, hit) in self.hits.iter().enumerate() {
            validate_hit(hit, request)?;
            if position > 0 && compare_hits(&self.hits[position - 1], hit).is_gt() {
                return Err(response_error(
                    "OKF Knowledge search hits are not in deterministic relevance order.",
                ));
            }
            let citation = (
                hit.citation.surface.clone(),
                hit.citation.generation,
                hit.citation.concept_id.clone(),
                hit.citation.path.clone(),
            );
            if !citations.insert(citation) {
                return Err(response_error(
                    "The OKF Knowledge search response repeats a cited concept.",
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn compare_hits(
    left: &OkfKnowledgeSearchHit,
    right: &OkfKnowledgeSearchHit,
) -> std::cmp::Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.citation.surface.cmp(&right.citation.surface))
        .then_with(|| left.citation.generation.cmp(&right.citation.generation))
        .then_with(|| left.citation.path.cmp(&right.citation.path))
}

fn validate_hit(hit: &OkfKnowledgeSearchHit, request: &OkfKnowledgeSearchRequest) -> UseResult<()> {
    let citation = &hit.citation;
    if hit.score == 0
        || hit.title.is_empty()
        || hit.title.len() > MAX_TITLE_BYTES
        || hit.title.chars().any(char::is_control)
        || hit.type_name.is_empty()
        || hit.type_name.len() > 256
        || hit.type_name.chars().any(char::is_control)
        || hit.snippet.is_empty()
        || hit.snippet.len() > MAX_SNIPPET_BYTES
        || PluginPackageId::parse(citation.surface.package_id.clone()).is_err()
        || citation.surface.surface.kind != PluginSurfaceKind::Okf
        || citation.generation == 0
    {
        return Err(response_error(
            "An OKF Knowledge search hit contains invalid cited evidence.",
        ));
    }
    let Some(projection) = request.projections.iter().find(|projection| {
        projection.surface == citation.surface && projection.generation == citation.generation
    }) else {
        return Err(response_error(
            "An OKF Knowledge search citation is outside the reviewed capability/session projection.",
        ));
    };
    citation.validate_for_projection(projection)
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn valid_concept_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4 * 1024
        && !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."))
}

fn valid_relative_path(value: &str) -> bool {
    value.ends_with(".md") && valid_concept_id(value)
}

fn query_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_search_request_invalid", message)
}

fn response_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_search_response_invalid", message)
}
