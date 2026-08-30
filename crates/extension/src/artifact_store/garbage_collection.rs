use a3s_use_core::{UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod deletion;
mod engine;
mod io;

use super::{
    artifact_store_error, validate_sha256, ArtifactInventoryEntry, ArtifactKind,
    ArtifactPhysicalState, MAX_ARTIFACT_CONTAINER_ENTRIES,
};
use crate::package::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};
pub(in crate::artifact_store) use io::{
    ensure_reference_admission_allowed, inspect_state as inspect_garbage_collection_state,
    validate_state_metadata as validate_garbage_collection_metadata,
    GARBAGE_COLLECTION_COMPLETED_RECORD, GARBAGE_COLLECTION_COMPLETED_TEMPORARY,
    GARBAGE_COLLECTION_PREPARED_RECORD, GARBAGE_COLLECTION_PREPARED_TEMPORARY,
};

pub const ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA: &str =
    "a3s.use.artifact-garbage-collection-plan.v1";
pub const ARTIFACT_GARBAGE_COLLECTION_RECORD_SCHEMA: &str =
    "a3s.use.artifact-garbage-collection-record.v1";
pub const ARTIFACT_GARBAGE_COLLECTION_RESULT_SCHEMA: &str =
    "a3s.use.artifact-garbage-collection-result.v1";
pub const MAX_ARTIFACT_GARBAGE_COLLECTION_TARGETS: usize = 1_024;

/// One exact content identity explicitly selected for garbage collection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGarbageCollectionTarget {
    pub kind: ArtifactKind,
    pub digest: String,
}

impl ArtifactGarbageCollectionTarget {
    pub fn new(kind: ArtifactKind, digest: &str) -> UseResult<Self> {
        let target = Self {
            kind,
            digest: digest.to_owned(),
        };
        target.validate()?;
        Ok(target)
    }

    pub fn validate(&self) -> UseResult<()> {
        validate_canonical_digest(&self.digest).map_err(|error| {
            garbage_collection_policy_invalid(format!(
                "An Artifact Store garbage-collection target is invalid: {}",
                error.message
            ))
        })
    }
}

/// Explicit deletion policy. Absence from this allowlist always means retain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGarbageCollectionPolicy {
    pub targets: Vec<ArtifactGarbageCollectionTarget>,
}

impl ArtifactGarbageCollectionPolicy {
    pub fn new(mut targets: Vec<ArtifactGarbageCollectionTarget>) -> UseResult<Self> {
        targets.sort();
        let policy = Self { targets };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.targets.is_empty() || self.targets.len() > MAX_ARTIFACT_GARBAGE_COLLECTION_TARGETS {
            return Err(garbage_collection_policy_invalid(format!(
                "Artifact Store garbage collection requires 1..={MAX_ARTIFACT_GARBAGE_COLLECTION_TARGETS} explicit targets."
            )));
        }
        for target in &self.targets {
            target.validate()?;
        }
        if !self.targets.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(garbage_collection_policy_invalid(
                "Artifact Store garbage-collection targets must be unique and canonically ordered.",
            ));
        }
        Ok(())
    }
}

/// Stable lifecycle evidence bound into a deletion confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ArtifactGarbageCollectionLifecycle {
    Ordinary,
    Quarantined {
        #[serde(rename = "quarantinePlanDigest")]
        quarantine_plan_digest: String,
    },
    Rehydrated {
        #[serde(rename = "quarantinePlanDigest")]
        quarantine_plan_digest: String,
        #[serde(rename = "rehydrationPlanDigest")]
        rehydration_plan_digest: String,
    },
}

/// Exact path-free physical evidence for one reviewed deletion target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGarbageCollectionEntry {
    pub kind: ArtifactKind,
    pub digest: String,
    pub physical_state: ArtifactPhysicalState,
    pub content_bytes: u64,
    pub content_files: u64,
    pub staging_entries: u64,
    pub staging_bytes: u64,
    pub lifecycle: ArtifactGarbageCollectionLifecycle,
}

impl ArtifactGarbageCollectionEntry {
    fn validate(&self) -> UseResult<()> {
        ArtifactGarbageCollectionTarget::new(self.kind, &self.digest).map_err(|error| {
            garbage_collection_plan_invalid(format!(
                "A garbage-collection entry has an invalid identity: {}",
                error.message
            ))
        })?;
        let incomplete_with_content = self.physical_state == ArtifactPhysicalState::Incomplete
            && (self.content_bytes != 0 || self.content_files != 0);
        let invalid_blob = self.kind == ArtifactKind::Blob
            && self.physical_state == ArtifactPhysicalState::Complete
            && self.content_files != 1;
        let invalid_staging = self.staging_entries == 0 && self.staging_bytes != 0;
        let exceeds_bounds = self.content_files > MAX_PACKAGE_FILES as u64
            || (self.kind == ArtifactKind::ExpandedPackage
                && self.content_bytes > MAX_PACKAGE_BYTES)
            || self.staging_entries > MAX_ARTIFACT_CONTAINER_ENTRIES as u64;
        if incomplete_with_content || invalid_blob || invalid_staging || exceeds_bounds {
            return Err(garbage_collection_plan_invalid(
                "A garbage-collection entry has inconsistent or excessive physical evidence.",
            ));
        }
        match &self.lifecycle {
            ArtifactGarbageCollectionLifecycle::Ordinary => {}
            ArtifactGarbageCollectionLifecycle::Quarantined {
                quarantine_plan_digest,
            } => {
                validate_plan_digest(quarantine_plan_digest)?;
                if self.physical_state != ArtifactPhysicalState::Complete {
                    return Err(garbage_collection_plan_invalid(
                        "A quarantined garbage-collection entry must have canonical content.",
                    ));
                }
            }
            ArtifactGarbageCollectionLifecycle::Rehydrated {
                quarantine_plan_digest,
                rehydration_plan_digest,
            } => {
                validate_plan_digest(quarantine_plan_digest)?;
                validate_plan_digest(rehydration_plan_digest)?;
                if self.physical_state != ArtifactPhysicalState::Complete {
                    return Err(garbage_collection_plan_invalid(
                        "A rehydrated garbage-collection entry must have canonical content.",
                    ));
                }
            }
        }
        self.content_bytes
            .checked_add(self.staging_bytes)
            .ok_or_else(|| {
                garbage_collection_plan_invalid(
                    "Garbage-collection entry byte accounting overflowed.",
                )
            })?;
        Ok(())
    }
}

/// Reviewed deletion evidence. The predecessor prevents confirmation replay
/// from being confused with a later object that reused the same digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGarbageCollectionPlan {
    pub schema: String,
    pub policy: ArtifactGarbageCollectionPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predecessor_plan_digest: Option<String>,
    pub artifacts: Vec<ArtifactGarbageCollectionEntry>,
    pub artifact_count: u64,
    pub reclaimable_bytes: u64,
    pub required_reference_count: u64,
}

impl ArtifactGarbageCollectionPlan {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA {
            return Err(garbage_collection_plan_invalid(
                "The Artifact Store garbage-collection plan schema is invalid.",
            ));
        }
        self.policy.validate().map_err(|error| {
            garbage_collection_plan_invalid(format!(
                "The garbage-collection plan contains an invalid policy: {}",
                error.message
            ))
        })?;
        if let Some(predecessor) = &self.predecessor_plan_digest {
            validate_plan_digest(predecessor)?;
        }
        if self.artifacts.len() != self.policy.targets.len()
            || self.required_reference_count != 0
            || self.artifact_count != self.artifacts.len() as u64
        {
            return Err(garbage_collection_plan_invalid(
                "The garbage-collection plan target or reference accounting is inconsistent.",
            ));
        }
        let mut reclaimable_bytes = 0_u64;
        for (target, artifact) in self.policy.targets.iter().zip(&self.artifacts) {
            artifact.validate()?;
            if artifact.kind != target.kind || artifact.digest != target.digest {
                return Err(garbage_collection_plan_invalid(
                    "Garbage-collection evidence does not match its explicit target.",
                ));
            }
            reclaimable_bytes = reclaimable_bytes
                .checked_add(artifact.content_bytes)
                .and_then(|total| total.checked_add(artifact.staging_bytes))
                .ok_or_else(|| {
                    garbage_collection_plan_invalid(
                        "Garbage-collection plan byte accounting overflowed.",
                    )
                })?;
        }
        if reclaimable_bytes != self.reclaimable_bytes {
            return Err(garbage_collection_plan_invalid(
                "The garbage-collection plan reclaimable byte total is inconsistent.",
            ));
        }
        Ok(())
    }

    /// SHA-256 over canonical, path-free plan JSON.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(canonical_json(self)?)
        ))
    }
}

/// Durable intent/completion record for one exact reviewed plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGarbageCollectionRecord {
    pub schema: String,
    pub plan_digest: String,
    pub plan: ArtifactGarbageCollectionPlan,
}

impl ArtifactGarbageCollectionRecord {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ARTIFACT_GARBAGE_COLLECTION_RECORD_SCHEMA {
            return Err(garbage_collection_state_invalid(
                "The Artifact Store garbage-collection record schema is invalid.",
            ));
        }
        self.plan.validate().map_err(|error| {
            garbage_collection_state_invalid(format!(
                "The garbage-collection record contains an invalid plan: {}",
                error.message
            ))
        })?;
        validate_plan_digest(&self.plan_digest).map_err(|error| {
            garbage_collection_state_invalid(format!(
                "The garbage-collection record has an invalid plan digest: {}",
                error.message
            ))
        })?;
        if self.plan.descriptor_digest()? != self.plan_digest {
            return Err(garbage_collection_state_invalid(
                "The garbage-collection record does not match its plan digest.",
            ));
        }
        Ok(())
    }
}

/// Replay-stable outcome of one exact garbage-collection operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGarbageCollectionResult {
    pub schema: String,
    pub plan_digest: String,
    pub changed: bool,
    pub removed: Vec<ArtifactGarbageCollectionEntry>,
    pub reclaimed_bytes: u64,
    pub record: ArtifactGarbageCollectionRecord,
}

impl ArtifactGarbageCollectionResult {
    pub fn validate(&self) -> UseResult<()> {
        self.record.validate()?;
        if self.schema != ARTIFACT_GARBAGE_COLLECTION_RESULT_SCHEMA
            || self.plan_digest != self.record.plan_digest
            || self.removed != self.record.plan.artifacts
            || self.reclaimed_bytes != self.record.plan.reclaimable_bytes
        {
            return Err(garbage_collection_state_invalid(
                "The Artifact Store garbage-collection result is inconsistent.",
            ));
        }
        Ok(())
    }
}

fn garbage_collection_entry(
    physical: &ArtifactInventoryEntry,
    lifecycle: ArtifactGarbageCollectionLifecycle,
) -> UseResult<ArtifactGarbageCollectionEntry> {
    let entry = ArtifactGarbageCollectionEntry {
        kind: physical.kind,
        digest: physical.digest.clone(),
        physical_state: physical.state,
        content_bytes: physical.content_bytes,
        content_files: physical.content_files,
        staging_entries: physical.staging_entries,
        staging_bytes: physical.staging_bytes,
        lifecycle,
    };
    entry.validate()?;
    Ok(entry)
}

fn record_for_plan(
    plan: ArtifactGarbageCollectionPlan,
    expected_plan_digest: &str,
) -> UseResult<ArtifactGarbageCollectionRecord> {
    let actual = plan.descriptor_digest()?;
    if actual != expected_plan_digest {
        return Err(garbage_collection_plan_mismatch(
            "Artifact Store garbage-collection evidence changed after review; create and confirm a new plan.",
        )
        .with_detail("actualPlanDigest", actual));
    }
    let record = ArtifactGarbageCollectionRecord {
        schema: ARTIFACT_GARBAGE_COLLECTION_RECORD_SCHEMA.to_owned(),
        plan_digest: expected_plan_digest.to_owned(),
        plan,
    };
    record.validate()?;
    Ok(record)
}

fn require_record_confirmation(
    record: &ArtifactGarbageCollectionRecord,
    policy: &ArtifactGarbageCollectionPolicy,
    expected_plan_digest: &str,
) -> UseResult<()> {
    record.validate()?;
    if record.plan_digest != expected_plan_digest || &record.plan.policy != policy {
        return Err(garbage_collection_plan_mismatch(
            "The active Artifact Store garbage collection differs from the confirmed policy or plan.",
        ));
    }
    Ok(())
}

fn garbage_collection_result(
    record: ArtifactGarbageCollectionRecord,
    changed: bool,
) -> UseResult<ArtifactGarbageCollectionResult> {
    let result = ArtifactGarbageCollectionResult {
        schema: ARTIFACT_GARBAGE_COLLECTION_RESULT_SCHEMA.to_owned(),
        plan_digest: record.plan_digest.clone(),
        changed,
        removed: record.plan.artifacts.clone(),
        reclaimed_bytes: record.plan.reclaimable_bytes,
        record,
    };
    result.validate()?;
    Ok(result)
}

#[cfg(test)]
pub(super) async fn prepare_record_for_test(
    root: &std::path::Path,
    record: &ArtifactGarbageCollectionRecord,
) -> UseResult<()> {
    io::write_prepared_record(root, record, false).await
}

fn canonical_json(value: &(impl Serialize + ?Sized)) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        garbage_collection_state_invalid(format!(
            "Failed to encode canonical Artifact Store garbage-collection evidence: {error}"
        ))
    })?;
    Ok(bytes)
}

fn validate_canonical_digest(value: &str) -> UseResult<()> {
    let sha256 = value.strip_prefix("sha256:").ok_or_else(|| {
        garbage_collection_plan_invalid(
            "An Artifact Store garbage-collection digest must use 'sha256:'.",
        )
    })?;
    validate_sha256(sha256).map_err(|error| garbage_collection_plan_invalid(error.message))
}

fn validate_plan_digest(value: &str) -> UseResult<()> {
    validate_canonical_digest(value)
}

fn validate_expected_plan_digest(value: &str) -> UseResult<()> {
    validate_plan_digest(value).map_err(|_| {
        garbage_collection_plan_mismatch(
            "Artifact Store garbage collection requires an exact canonical SHA-256 plan digest.",
        )
    })
}

fn garbage_collection_policy_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error(
        "use.artifact_store.garbage_collection_policy_invalid",
        message,
    )
}

fn garbage_collection_plan_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error(
        "use.artifact_store.garbage_collection_plan_invalid",
        message,
    )
}

fn garbage_collection_plan_mismatch(message: impl Into<String>) -> UseError {
    artifact_store_error(
        "use.artifact_store.garbage_collection_plan_mismatch",
        message,
    )
}

pub(super) fn garbage_collection_state_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error(
        "use.artifact_store.garbage_collection_state_invalid",
        message,
    )
}

pub(super) fn garbage_collection_in_progress() -> UseError {
    artifact_store_error(
        "use.artifact_store.garbage_collection_in_progress",
        "Artifact Store garbage collection is incomplete and must be resumed before new references or maintenance are admitted.",
    )
}
