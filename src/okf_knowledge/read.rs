use std::convert::TryFrom;

use a3s_use_core::{OkfCapabilityProjection, PlanScope, UseError, UseResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::query::{OkfKnowledgeCitation, OKF_KNOWLEDGE_CITATION_SCHEMA};

pub const OKF_KNOWLEDGE_READ_REQUEST_SCHEMA: &str = "a3s.use.okf-knowledge-read-request.v1";
pub const OKF_KNOWLEDGE_READ_RESPONSE_SCHEMA: &str = "a3s.use.okf-knowledge-read-response.v1";

const MAX_READ_BYTES: usize = 8 * 1024 * 1024;
const MAX_READ_PATH_BYTES: usize = 4 * 1024;

/// Read one exact cited OKF document from one reviewed capability projection.
///
/// The request carries the complete projection instead of a package or file
/// path alone. This prevents a caller from widening a read after search or
/// substituting a different lifecycle generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeReadRequest {
    pub schema: String,
    pub scope: PlanScope,
    pub projection: OkfCapabilityProjection,
    pub citation: OkfKnowledgeCitation,
    pub max_bytes: usize,
}

/// The bounded UTF-8 Markdown document and its unchanged source citation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeReadResponse {
    pub schema: String,
    pub scope: PlanScope,
    pub citation: OkfKnowledgeCitation,
    pub content: String,
    pub byte_count: usize,
}

impl OkfKnowledgeReadRequest {
    pub fn new(
        scope: PlanScope,
        projection: OkfCapabilityProjection,
        citation: OkfKnowledgeCitation,
        max_bytes: usize,
    ) -> UseResult<Self> {
        let request = Self {
            schema: OKF_KNOWLEDGE_READ_REQUEST_SCHEMA.to_owned(),
            scope,
            projection,
            citation,
            max_bytes,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.projection.validate()?;
        if self.schema != OKF_KNOWLEDGE_READ_REQUEST_SCHEMA
            || self.scope != self.projection.scope
            || self.max_bytes == 0
            || self.max_bytes > MAX_READ_BYTES
            || u64::try_from(self.max_bytes)
                .ok()
                .is_none_or(|value| value > self.projection.bundle.limits.max_document_bytes)
            || self.citation.path.len() > MAX_READ_PATH_BYTES
        {
            return Err(read_request_error(
                "The OKF Knowledge read request exceeds its exact-generation or byte bounds.",
            ));
        }
        self.citation
            .validate_for_projection(&self.projection)
            .map_err(|error| read_request_error(error.message))
    }
}

impl OkfKnowledgeReadResponse {
    pub fn new(request: &OkfKnowledgeReadRequest, content: String) -> UseResult<Self> {
        let response = Self {
            schema: OKF_KNOWLEDGE_READ_RESPONSE_SCHEMA.to_owned(),
            scope: request.scope.clone(),
            citation: request.citation.clone(),
            byte_count: content.len(),
            content,
        };
        response.validate_for(request)?;
        Ok(response)
    }

    pub fn validate_for(&self, request: &OkfKnowledgeReadRequest) -> UseResult<()> {
        request.validate()?;
        if self.schema != OKF_KNOWLEDGE_READ_RESPONSE_SCHEMA
            || self.scope != request.scope
            || self.citation != request.citation
            || self.byte_count != self.content.len()
            || self.byte_count == 0
            || self.byte_count > request.max_bytes
        {
            return Err(read_response_error(
                "The OKF Knowledge read response does not match the exact request.",
            ));
        }
        if !self.content.is_char_boundary(self.content.len()) {
            return Err(read_response_error(
                "The OKF Knowledge read response is not valid UTF-8.",
            ));
        }
        let digest = format!("sha256:{:x}", Sha256::digest(self.content.as_bytes()));
        if digest != self.citation.source_digest
            || self.citation.schema != OKF_KNOWLEDGE_CITATION_SCHEMA
        {
            return Err(read_response_error(
                "The OKF Knowledge read content does not match its cited source digest.",
            ));
        }
        Ok(())
    }
}

fn read_request_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_read_request_invalid", message)
}

fn read_response_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_read_response_invalid", message)
}
