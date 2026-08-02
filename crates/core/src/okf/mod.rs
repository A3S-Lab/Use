//! Bounded, non-executable Open Knowledge Format contracts.
//!
//! This module is the single conformance boundary shared by future manifest,
//! catalog, plan, receipt, and A3S Knowledge adapters. It validates immutable
//! content only; OKF executor and attester fields never dispatch work here.

use std::collections::BTreeMap;

use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{UseError, UseResult};

mod control_plane;
mod document;
mod path;

pub use control_plane::{
    OkfCapabilityProjection, OkfKnowledgeObservation, OkfKnowledgeObservedState,
    OkfProjectionReceipt, OkfSelectedGeneration, OKF_CAPABILITY_PROJECTION_SCHEMA,
    OKF_KNOWLEDGE_OBSERVATION_SCHEMA, OKF_PROJECTION_RECEIPT_SCHEMA,
};

pub const OKF_BUNDLE_CONTRACT_SCHEMA: &str = "a3s.use.okf-bundle.v1";

const OKF_CONTRACT_MAX_BYTES: usize = 128 * 1024;
const MAX_OKF_FILES: u64 = 4_096;
const MAX_OKF_CONCEPTS: u64 = 4_096;
const MAX_OKF_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OKF_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_OKF_LINKS_PER_DOCUMENT: u64 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OkfFormatVersion {
    #[serde(rename = "0.1")]
    V0_1,
    #[serde(rename = "0.2")]
    V0_2,
}

impl OkfFormatVersion {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_1 => "0.1",
            Self::V0_2 => "0.2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfBundleLimits {
    pub max_files: u64,
    pub max_concepts: u64,
    pub max_expanded_bytes: u64,
    pub max_document_bytes: u64,
    pub max_links_per_document: u64,
}

impl Default for OkfBundleLimits {
    fn default() -> Self {
        Self {
            max_files: 256,
            max_concepts: 64,
            max_expanded_bytes: 64 * 1024 * 1024,
            max_document_bytes: 1024 * 1024,
            max_links_per_document: 2_048,
        }
    }
}

impl OkfBundleLimits {
    fn validate(&self) -> UseResult<()> {
        if self.max_files == 0
            || self.max_files > MAX_OKF_FILES
            || self.max_concepts == 0
            || self.max_concepts > self.max_files
            || self.max_concepts > MAX_OKF_CONCEPTS
            || self.max_expanded_bytes == 0
            || self.max_expanded_bytes > MAX_OKF_EXPANDED_BYTES
            || self.max_document_bytes == 0
            || self.max_document_bytes > self.max_expanded_bytes
            || self.max_document_bytes > MAX_OKF_DOCUMENT_BYTES
            || self.max_links_per_document == 0
            || self.max_links_per_document > MAX_OKF_LINKS_PER_DOCUMENT
        {
            return Err(contract_error(
                "The OKF bundle limits are outside the supported bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfBundleContract {
    pub schema: String,
    pub format_version: OkfFormatVersion,
    pub root: String,
    pub content_digest: String,
    pub concept_count: u64,
    pub file_count: u64,
    pub expanded_bytes: u64,
    pub limits: OkfBundleLimits,
}

impl OkfBundleContract {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > OKF_CONTRACT_MAX_BYTES {
            return Err(contract_error(
                "The OKF bundle contract exceeds its input bounds.",
            ));
        }
        let contract = serde_json::from_slice(input).map_err(|error| {
            contract_error(format!(
                "Failed to decode the OKF bundle contract at line {}, column {}.",
                error.line(),
                error.column()
            ))
        })?;
        Self::validate_contract(&contract)?;
        Ok(contract)
    }

    pub fn validate(&self) -> UseResult<()> {
        Self::validate_contract(self)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            contract_error(format!(
                "Failed to encode canonical OKF bundle contract JSON: {error}"
            ))
        })?;
        if bytes.len() > OKF_CONTRACT_MAX_BYTES {
            return Err(contract_error(
                "The canonical OKF bundle contract exceeds its size bound.",
            ));
        }
        Ok(bytes)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }

    pub fn verify_inspection(&self, inspection: &OkfBundleInspection) -> UseResult<()> {
        self.validate()?;
        if self.format_version != inspection.format_version
            || self.content_digest != inspection.content_digest
            || self.concept_count != inspection.concept_count
            || self.file_count != inspection.file_count
            || self.expanded_bytes != inspection.expanded_bytes
            || self.limits != inspection.limits
        {
            return Err(UseError::new(
                "use.okf.contract_mismatch",
                format!(
                    "The inspected OKF bundle does not match the declared contract: expected digest {}, concepts {}, files {}, bytes {}; observed digest {}, concepts {}, files {}, bytes {}.",
                    self.content_digest,
                    self.concept_count,
                    self.file_count,
                    self.expanded_bytes,
                    inspection.content_digest,
                    inspection.concept_count,
                    inspection.file_count,
                    inspection.expanded_bytes
                ),
            ));
        }
        Ok(())
    }

    fn validate_contract(contract: &Self) -> UseResult<()> {
        contract.limits.validate()?;
        if contract.schema != OKF_BUNDLE_CONTRACT_SCHEMA {
            return Err(contract_error(
                "The OKF bundle contract schema is unsupported.",
            ));
        }
        let normalized_root = path::normalize_bundle_root(&contract.root)?;
        if normalized_root != contract.root {
            return Err(contract_error("The OKF bundle root is not canonical."));
        }
        if !valid_sha256(&contract.content_digest)
            || contract.concept_count == 0
            || contract.file_count < contract.concept_count
            || contract.file_count > contract.limits.max_files
            || contract.concept_count > contract.limits.max_concepts
            || contract.expanded_bytes == 0
            || contract.expanded_bytes > contract.limits.max_expanded_bytes
        {
            return Err(contract_error("The OKF bundle evidence is invalid."));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfBundleFile {
    pub path: String,
    pub content: Vec<u8>,
}

impl OkfBundleFile {
    pub fn new(path: impl Into<String>, content: impl AsRef<[u8]>) -> Self {
        Self {
            path: path.into(),
            content: content.as_ref().to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfConceptSummary {
    pub id: String,
    pub path: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub link_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OkfDiagnosticCode {
    DanglingLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfBundleDiagnostic {
    pub code: OkfDiagnosticCode,
    pub path: String,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfBundleInspection {
    pub format_version: OkfFormatVersion,
    pub content_digest: String,
    pub concept_count: u64,
    pub file_count: u64,
    pub expanded_bytes: u64,
    pub limits: OkfBundleLimits,
    pub concepts: Vec<OkfConceptSummary>,
    pub diagnostics: Vec<OkfBundleDiagnostic>,
}

pub fn inspect_okf_bundle(
    format_version: OkfFormatVersion,
    limits: OkfBundleLimits,
    input_files: impl IntoIterator<Item = OkfBundleFile>,
) -> UseResult<OkfBundleInspection> {
    let input_files = input_files.into_iter().collect::<Vec<_>>();
    inspect_okf_bundle_files(format_version, limits, &input_files)
}

/// Inspect an immutable borrowed OKF file snapshot without copying its bytes.
///
/// This is the preferred boundary for host adapters that must validate the
/// exact in-memory payload immediately before handing it to A3S Knowledge.
pub fn inspect_okf_bundle_files(
    format_version: OkfFormatVersion,
    limits: OkfBundleLimits,
    input_files: &[OkfBundleFile],
) -> UseResult<OkfBundleInspection> {
    limits.validate()?;
    let mut files = BTreeMap::new();
    let mut expanded_bytes = 0_u64;

    for file in input_files {
        let path = path::normalize_bundle_file_path(&file.path)?;
        if files.contains_key(&path) {
            return Err(bundle_error(format!(
                "OKF bundle file '{path}' is declared more than once."
            )));
        }
        expanded_bytes = expanded_bytes
            .checked_add(u64::try_from(file.content.len()).map_err(|_| limit_error())?)
            .ok_or_else(limit_error)?;
        if expanded_bytes > limits.max_expanded_bytes {
            return Err(limit_error());
        }
        files.insert(path, file.content.as_slice());
        if files.len() as u64 > limits.max_files {
            return Err(limit_error());
        }
    }

    let paths = files.keys().cloned().collect();
    let mut concepts = Vec::new();
    let mut diagnostics = Vec::new();
    for (path, content) in &files {
        if !path.ends_with(".md") {
            continue;
        }
        if content.len() as u64 > limits.max_document_bytes {
            return Err(limit_error());
        }
        let inspection = document::inspect_document(
            path,
            content,
            format_version,
            &paths,
            limits.max_links_per_document,
        )?;
        if let Some(concept) = inspection.concept {
            concepts.push(concept);
        }
        diagnostics.extend(inspection.diagnostics);
    }
    if concepts.is_empty() {
        return Err(bundle_error(
            "An OKF bundle must contain at least one concept document.",
        ));
    }
    if concepts.len() as u64 > limits.max_concepts {
        return Err(limit_error());
    }
    concepts.sort_by(|left, right| left.path.cmp(&right.path));
    diagnostics.sort_by(|left, right| {
        (&left.path, &left.target, left.code).cmp(&(&right.path, &right.target, right.code))
    });

    let file_count = files.len() as u64;
    let concept_count = concepts.len() as u64;
    Ok(OkfBundleInspection {
        format_version,
        content_digest: content_digest(&files),
        concept_count,
        file_count,
        expanded_bytes,
        limits,
        concepts,
        diagnostics,
    })
}

fn content_digest(files: &BTreeMap<String, &[u8]>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"a3s-use-okf-expanded-bundle-v1\0");
    for (path, content) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((content.len() as u64).to_be_bytes());
        digest.update(content);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn contract_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.contract_invalid", message)
}

fn bundle_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.bundle_invalid", message)
}

fn path_escape(path: &str, message: impl Into<String>) -> UseError {
    UseError::new("use.okf.path_escape", message).with_detail("path", path)
}

fn limit_error() -> UseError {
    UseError::new(
        "use.okf.limit_exceeded",
        "The OKF bundle exceeds its declared conformance limits.",
    )
}
