use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin::validation::{valid_machine_id, valid_package_id, valid_segment, valid_sha256};
use crate::{PlanQualifiedSurfaceRef, PluginSurfaceKind, UseError, UseResult};

use super::OkfBundleContract;

pub const OKF_PROJECTION_RECEIPT_SCHEMA: &str = "a3s.use.okf-projection-receipt.v1";
pub const OKF_KNOWLEDGE_OBSERVATION_SCHEMA: &str = "a3s.use.okf-knowledge-observation.v1";
pub const OKF_CAPABILITY_PROJECTION_SCHEMA: &str = "a3s.use.okf-capability-projection.v1";

const MAX_CONTROL_PLANE_CONTRACT_BYTES: usize = 128 * 1024;
const RECEIPT_ERROR: &str = "use.okf.projection_receipt_invalid";
const OBSERVATION_ERROR: &str = "use.okf.knowledge_observation_invalid";
const PROJECTION_ERROR: &str = "use.okf.capability_projection_invalid";

/// Host-owned evidence that one exact immutable OKF generation was staged.
///
/// This receipt contains no executable target, endpoint, secret, or personal
/// knowledge path. The parent operation remains the lifecycle authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfProjectionReceipt {
    pub schema: String,
    pub operation_id: String,
    pub scope_id: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
    pub bundle: OkfBundleContract,
    pub projection_id: String,
    pub index_schema: String,
    pub index_build_id: String,
    pub staged_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OkfKnowledgeObservedState {
    Failed,
    Promoted,
    Removed,
    Staged,
}

/// The exact Knowledge generation selected for cited retrieval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfSelectedGeneration {
    pub generation: u64,
    pub package_digest: String,
    pub bundle_digest: String,
    pub projection_receipt_digest: String,
    pub index_schema: String,
    pub index_build_id: String,
    pub index_digest: String,
}

/// Non-secret A3S Knowledge observation for one candidate generation.
///
/// `selected` records the promoted generation that remains searchable. A
/// failed or merely staged candidate may name an older last-good generation,
/// but may never select itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeObservation {
    pub schema: String,
    pub scope_id: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub bundle_digest: String,
    pub projection_receipt_digest: String,
    pub index_schema: String,
    pub index_build_id: String,
    pub state: OkfKnowledgeObservedState,
    pub observed_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<OkfSelectedGeneration>,
}

/// Exact promoted OKF evidence safe to include in a capability generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfCapabilityProjection {
    pub schema: String,
    pub scope_id: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
    pub bundle: OkfBundleContract,
    pub projection_id: String,
    pub projection_receipt_digest: String,
    pub index_schema: String,
    pub index_build_id: String,
    pub index_digest: String,
    pub observation_digest: String,
}

impl OkfProjectionReceipt {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "OKF projection receipt",
            RECEIPT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != OKF_PROJECTION_RECEIPT_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_machine_id(&self.scope_id)
            || !valid_okf_surface(&self.surface)
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || !valid_machine_id(&self.projection_id)
            || !valid_machine_id(&self.index_schema)
            || !valid_machine_id(&self.index_build_id)
            || self.staged_at_ms == 0
            || self.bundle.validate().is_err()
        {
            return Err(control_error(
                RECEIPT_ERROR,
                "The OKF projection receipt identity or exact-generation evidence is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "OKF projection receipt", RECEIPT_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl OkfSelectedGeneration {
    fn validate(&self) -> UseResult<()> {
        if self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.bundle_digest)
            || !valid_sha256(&self.projection_receipt_digest)
            || !valid_machine_id(&self.index_schema)
            || !valid_machine_id(&self.index_build_id)
            || !valid_sha256(&self.index_digest)
        {
            return Err(control_error(
                OBSERVATION_ERROR,
                "The selected OKF Knowledge generation is invalid.",
            ));
        }
        Ok(())
    }

    fn matches_candidate_identity(&self, observation: &OkfKnowledgeObservation) -> bool {
        self.generation == observation.generation
            && self.package_digest == observation.package_digest
            && self.bundle_digest == observation.bundle_digest
            && self.projection_receipt_digest == observation.projection_receipt_digest
            && self.index_schema == observation.index_schema
            && self.index_build_id == observation.index_build_id
    }
}

impl OkfKnowledgeObservation {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "OKF Knowledge observation",
            OBSERVATION_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != OKF_KNOWLEDGE_OBSERVATION_SCHEMA
            || !valid_machine_id(&self.scope_id)
            || !valid_okf_surface(&self.surface)
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.bundle_digest)
            || !valid_sha256(&self.projection_receipt_digest)
            || !valid_machine_id(&self.index_schema)
            || !valid_machine_id(&self.index_build_id)
            || self.observed_at_ms == 0
            || self
                .index_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
        {
            return Err(control_error(
                OBSERVATION_ERROR,
                "The OKF Knowledge observation identity or candidate evidence is invalid.",
            ));
        }
        if let Some(selected) = &self.selected {
            selected.validate()?;
            if selected.generation > self.generation {
                return Err(control_error(
                    OBSERVATION_ERROR,
                    "An OKF Knowledge observation cannot select a future generation.",
                ));
            }
        }

        let selected_is_candidate = self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.matches_candidate_identity(self));
        let valid_state = match self.state {
            OkfKnowledgeObservedState::Promoted => {
                self.index_digest.is_some()
                    && selected_is_candidate
                    && self.selected.as_ref().is_some_and(|selected| {
                        self.index_digest.as_deref() == Some(selected.index_digest.as_str())
                    })
            }
            OkfKnowledgeObservedState::Staged => {
                self.index_digest.is_some() && !selected_is_candidate
            }
            OkfKnowledgeObservedState::Failed => !selected_is_candidate,
            OkfKnowledgeObservedState::Removed => {
                self.index_digest.is_none() && self.selected.is_none()
            }
        };
        if !valid_state {
            return Err(control_error(
                OBSERVATION_ERROR,
                "The OKF Knowledge state, candidate index, and selected generation disagree.",
            ));
        }
        Ok(())
    }

    pub fn validate_for_receipt(&self, receipt: &OkfProjectionReceipt) -> UseResult<()> {
        self.validate()?;
        receipt.validate()?;
        if self.scope_id != receipt.scope_id
            || self.surface != receipt.surface
            || self.generation != receipt.generation
            || self.package_digest != receipt.package_digest
            || self.bundle_digest != receipt.bundle.content_digest
            || self.projection_receipt_digest != receipt.descriptor_digest()?
            || self.index_schema != receipt.index_schema
            || self.index_build_id != receipt.index_build_id
            || self.observed_at_ms < receipt.staged_at_ms
        {
            return Err(UseError::new(
                "use.okf.knowledge_observation_mismatch",
                "The OKF Knowledge observation does not belong to the exact staged package generation.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "OKF Knowledge observation", OBSERVATION_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl OkfCapabilityProjection {
    pub fn from_promoted(
        receipt: &OkfProjectionReceipt,
        observation: &OkfKnowledgeObservation,
    ) -> UseResult<Self> {
        observation.validate_for_receipt(receipt)?;
        if observation.state != OkfKnowledgeObservedState::Promoted {
            return Err(control_error(
                PROJECTION_ERROR,
                "Only a promoted OKF Knowledge observation can enter a capability generation.",
            ));
        }
        let projection = Self {
            schema: OKF_CAPABILITY_PROJECTION_SCHEMA.to_owned(),
            scope_id: receipt.scope_id.clone(),
            surface: receipt.surface.clone(),
            generation: receipt.generation,
            package_digest: receipt.package_digest.clone(),
            manifest_digest: receipt.manifest_digest.clone(),
            bundle: receipt.bundle.clone(),
            projection_id: receipt.projection_id.clone(),
            projection_receipt_digest: receipt.descriptor_digest()?,
            index_schema: receipt.index_schema.clone(),
            index_build_id: receipt.index_build_id.clone(),
            index_digest: observation.index_digest.clone().ok_or_else(|| {
                control_error(PROJECTION_ERROR, "A promoted OKF index digest is missing.")
            })?,
            observation_digest: observation.descriptor_digest()?,
        };
        projection.validate()?;
        Ok(projection)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "OKF capability projection",
            PROJECTION_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != OKF_CAPABILITY_PROJECTION_SCHEMA
            || !valid_machine_id(&self.scope_id)
            || !valid_okf_surface(&self.surface)
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || self.bundle.validate().is_err()
            || !valid_machine_id(&self.projection_id)
            || !valid_sha256(&self.projection_receipt_digest)
            || !valid_machine_id(&self.index_schema)
            || !valid_machine_id(&self.index_build_id)
            || !valid_sha256(&self.index_digest)
            || !valid_sha256(&self.observation_digest)
        {
            return Err(control_error(
                PROJECTION_ERROR,
                "The OKF capability projection does not bind valid promoted generation evidence.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "OKF capability projection", PROJECTION_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn valid_okf_surface(surface: &PlanQualifiedSurfaceRef) -> bool {
    valid_package_id(&surface.package_id)
        && surface.surface.kind == PluginSurfaceKind::Okf
        && valid_segment(&surface.surface.id)
}

fn parse_contract<T>(
    input: &[u8],
    label: &str,
    error_code: &'static str,
    validate: fn(&T) -> UseResult<()>,
) -> UseResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    if input.is_empty() || input.len() > MAX_CONTROL_PLANE_CONTRACT_BYTES {
        return Err(control_error(
            error_code,
            format!("The {label} exceeds its input bounds."),
        ));
    }
    let contract = serde_json::from_slice(input).map_err(|error| {
        control_error(
            error_code,
            format!(
                "Failed to decode the {label} at line {}, column {}.",
                error.line(),
                error.column()
            ),
        )
    })?;
    validate(&contract)?;
    Ok(contract)
}

fn canonical_json<T: Serialize>(
    value: &T,
    label: &str,
    error_code: &'static str,
) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        control_error(
            error_code,
            format!("Failed to encode canonical {label} JSON: {error}"),
        )
    })?;
    if bytes.len() > MAX_CONTROL_PLANE_CONTRACT_BYTES {
        return Err(control_error(
            error_code,
            format!("The canonical {label} exceeds its size bound."),
        ));
    }
    Ok(bytes)
}

fn canonical_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn control_error(error_code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(error_code, message)
}
