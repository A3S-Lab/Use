use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod content;
mod io;

pub(super) use io::{
    inspect_container_state as inspect_rehydration_state, validate_rehydration_metadata,
    ContainerRehydrationState, REHYDRATION_PREPARED_RECORD, REHYDRATION_PREPARED_TEMPORARY,
    REHYDRATION_RECORD, REHYDRATION_TEMPORARY,
};
use io::{write_completed_record, write_prepared_record};

use super::quarantine::{canonical_json, ArtifactQuarantineRecord};
use super::{
    artifact_store_error, validate_sha256, ArtifactCollectionGuard, ArtifactKind,
    ArtifactMutationLock, ArtifactStore, MUTATION_LOCK,
};
use crate::package::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};
use content::{rehydration_storage_projection, require_plan_candidate, require_replacement};

pub const ARTIFACT_REHYDRATION_PLAN_SCHEMA: &str = "a3s.use.artifact-rehydration-plan.v1";
pub const ARTIFACT_REHYDRATION_RECORD_SCHEMA: &str = "a3s.use.artifact-rehydration-record.v1";
pub const ARTIFACT_REHYDRATION_RESULT_SCHEMA: &str = "a3s.use.artifact-rehydration-result.v1";

/// Exact, path-free recovery evidence reviewed before corrupt canonical bytes
/// are replaced. The zero-reference requirement is enforced by the A3S Use
/// global coordinator under the same collection guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRehydrationPlan {
    pub schema: String,
    pub kind: ArtifactKind,
    pub digest: String,
    pub quarantine_plan_digest: String,
    pub quarantined_observed_digest: String,
    pub quarantined_content_bytes: u64,
    pub quarantined_content_files: u64,
    pub replacement_content_bytes: u64,
    pub replacement_content_files: u64,
    pub required_reference_count: u64,
}

impl ArtifactRehydrationPlan {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ARTIFACT_REHYDRATION_PLAN_SCHEMA {
            return Err(rehydration_plan_invalid(
                "The Artifact Store rehydration plan schema is invalid.",
            ));
        }
        validate_canonical_digest(&self.digest)?;
        validate_canonical_digest(&self.quarantine_plan_digest)?;
        validate_canonical_digest(&self.quarantined_observed_digest)?;
        if self.digest == self.quarantined_observed_digest
            || self.required_reference_count != 0
            || self.quarantined_content_files == 0
            || self.replacement_content_files == 0
            || self.replacement_content_bytes == 0
        {
            return Err(rehydration_plan_invalid(
                "Artifact rehydration evidence has invalid corruption, replacement, or reference bounds.",
            ));
        }
        match self.kind {
            ArtifactKind::Blob
                if self.quarantined_content_files != 1 || self.replacement_content_files != 1 =>
            {
                return Err(rehydration_plan_invalid(
                    "Artifact Blob rehydration must describe one corrupt and one replacement file.",
                ));
            }
            ArtifactKind::ExpandedPackage
                if self.quarantined_content_files > MAX_PACKAGE_FILES as u64
                    || self.replacement_content_files > MAX_PACKAGE_FILES as u64
                    || self.quarantined_content_bytes > MAX_PACKAGE_BYTES
                    || self.replacement_content_bytes > MAX_PACKAGE_BYTES =>
            {
                return Err(rehydration_plan_invalid(
                    "Expanded-package rehydration evidence exceeds package limits.",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    /// SHA-256 over the canonical path-free plan JSON.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(canonical_json(self)?)
        ))
    }
}

/// Durable prepared/completed evidence for one exact recovery plan. The same
/// canonical bytes are published first as intent and later as completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRehydrationRecord {
    pub schema: String,
    pub plan_digest: String,
    pub plan: ArtifactRehydrationPlan,
}

impl ArtifactRehydrationRecord {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ARTIFACT_REHYDRATION_RECORD_SCHEMA {
            return Err(rehydration_state_invalid(
                "The Artifact Store rehydration record schema is invalid.",
            ));
        }
        self.plan.validate().map_err(|error| {
            rehydration_state_invalid(format!(
                "The Artifact Store rehydration record contains an invalid plan: {}",
                error.message
            ))
        })?;
        validate_canonical_digest(&self.plan_digest).map_err(|error| {
            rehydration_state_invalid(format!(
                "The Artifact Store rehydration record has an invalid plan digest: {}",
                error.message
            ))
        })?;
        if self.plan.descriptor_digest()? != self.plan_digest {
            return Err(rehydration_state_invalid(
                "The Artifact Store rehydration record does not match its plan digest.",
            ));
        }
        Ok(())
    }
}

/// Replay-stable result of completing one reviewed recovery plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRehydrationResult {
    pub schema: String,
    pub plan_digest: String,
    pub changed: bool,
    pub record: ArtifactRehydrationRecord,
}

impl ArtifactRehydrationResult {
    pub fn validate(&self) -> UseResult<()> {
        self.record.validate()?;
        if self.schema != ARTIFACT_REHYDRATION_RESULT_SCHEMA
            || self.plan_digest != self.record.plan_digest
        {
            return Err(rehydration_state_invalid(
                "The Artifact Store rehydration result is inconsistent.",
            ));
        }
        Ok(())
    }
}

impl ArtifactStore {
    /// Verify an independently supplied candidate and derive exact path-free
    /// recovery evidence. This does not mutate the Artifact Store.
    pub async fn plan_rehydration(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
        candidate: &Path,
    ) -> UseResult<ArtifactRehydrationPlan> {
        collection.ensure_store(self)?;
        self.inspect_inventory(collection).await?;
        let (_, container) = self.rehydration_container(kind, digest)?;
        let candidate = self.candidate_evidence(kind, candidate).await?;
        require_replacement(kind, digest, &candidate)?;

        match inspect_rehydration_state(&container).await? {
            ContainerRehydrationState::Prepared(record)
            | ContainerRehydrationState::InterruptedCompletion(record)
            | ContainerRehydrationState::Rehydrated(record) => {
                self.require_rehydration_binding(&record, kind, digest)
                    .await?;
                require_plan_candidate(&record.plan, &candidate)?;
                return Ok(record.plan);
            }
            ContainerRehydrationState::None | ContainerRehydrationState::InterruptedPreparation => {
            }
        }

        let quarantine = self
            .inspect_quarantine(collection, kind, digest)
            .await?
            .ok_or_else(|| {
                artifact_store_error(
                    "use.artifact_store.rehydration_not_quarantined",
                    "Verified rehydration requires an exact logical quarantine record.",
                )
            })?;
        let current_quarantine = self.plan_quarantine(collection, kind, digest).await?;
        if current_quarantine != quarantine.plan {
            return Err(rehydration_state_invalid(
                "The quarantined artifact changed after its durable record was published.",
            ));
        }
        let plan = ArtifactRehydrationPlan {
            schema: ARTIFACT_REHYDRATION_PLAN_SCHEMA.to_owned(),
            kind,
            digest: digest.to_owned(),
            quarantine_plan_digest: quarantine.plan_digest,
            quarantined_observed_digest: current_quarantine.observed_digest,
            quarantined_content_bytes: current_quarantine.content_bytes,
            quarantined_content_files: current_quarantine.content_files,
            replacement_content_bytes: candidate.content_bytes,
            replacement_content_files: candidate.content_files,
            required_reference_count: 0,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Return the durable result for an already completed exact recovery.
    ///
    /// Terminal replay is read-only: it validates the complete physical
    /// inventory, exact plan digest, quarantine binding, and canonical
    /// replacement bytes. It deliberately does not reopen the external
    /// candidate because the completed record and canonical content are the
    /// durable recovery authority after publication.
    #[doc(hidden)]
    pub async fn replay_completed_rehydration(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
        expected_plan_digest: &str,
    ) -> UseResult<Option<ArtifactRehydrationResult>> {
        collection.ensure_store(self)?;
        self.inspect_inventory(collection).await?;
        validate_canonical_digest(expected_plan_digest).map_err(|_| {
            rehydration_plan_mismatch(
                "Artifact rehydration requires an exact canonical SHA-256 plan digest.",
            )
        })?;
        let (_, container) = self.rehydration_container(kind, digest)?;
        let ContainerRehydrationState::Rehydrated(record) =
            inspect_rehydration_state(&container).await?
        else {
            return Ok(None);
        };
        if record.plan_digest != expected_plan_digest {
            return Err(rehydration_plan_mismatch(
                "The completed Artifact Store rehydration differs from the reviewed plan.",
            ));
        }
        self.require_rehydration_binding(&record, kind, digest)
            .await?;
        self.require_canonical_replacement(&record.plan, &container)
            .await?;
        rehydration_result(record, false).map(Some)
    }

    /// Apply a reviewed recovery after the higher-level A3S Use coordinator
    /// has proven zero durable references under `collection`.
    ///
    /// This low-level cross-crate seam deliberately does not infer replacement
    /// authority from the quarantine marker. Callers must not invoke it for a
    /// referenced artifact. Candidate bytes are reverified, a prepared record
    /// is made durable before canonical content moves, and the quarantine gate
    /// opens only after the matching completion record is durable.
    #[doc(hidden)]
    pub async fn apply_unreferenced_rehydration(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
        candidate: &Path,
        expected_plan_digest: &str,
    ) -> UseResult<ArtifactRehydrationResult> {
        if let Some(result) = self
            .replay_completed_rehydration(collection, kind, digest, expected_plan_digest)
            .await?
        {
            return Ok(result);
        }
        let (sha256, container) = self.rehydration_container(kind, digest)?;
        let state = inspect_rehydration_state(&container).await?;

        let (record, recover_preparation) = match state {
            ContainerRehydrationState::Prepared(record)
            | ContainerRehydrationState::InterruptedCompletion(record) => {
                if record.plan_digest != expected_plan_digest {
                    return Err(rehydration_plan_mismatch(
                        "The prepared Artifact Store rehydration differs from the reviewed plan.",
                    ));
                }
                self.require_rehydration_binding(&record, kind, digest)
                    .await?;
                (record, false)
            }
            ContainerRehydrationState::None => {
                let plan = self
                    .plan_rehydration(collection, kind, digest, candidate)
                    .await?;
                let actual = plan.descriptor_digest()?;
                if actual != expected_plan_digest {
                    return Err(rehydration_plan_mismatch(
                        "Artifact recovery evidence changed after review; create and confirm a new plan.",
                    )
                    .with_detail("actualPlanDigest", actual));
                }
                (rehydration_record(plan, expected_plan_digest)?, false)
            }
            ContainerRehydrationState::InterruptedPreparation => {
                let plan = self
                    .plan_rehydration(collection, kind, digest, candidate)
                    .await?;
                let actual = plan.descriptor_digest()?;
                if actual != expected_plan_digest {
                    return Err(rehydration_plan_mismatch(
                        "Artifact recovery evidence changed after interrupted preparation.",
                    )
                    .with_detail("actualPlanDigest", actual));
                }
                (rehydration_record(plan, expected_plan_digest)?, true)
            }
            ContainerRehydrationState::Rehydrated(_) => {
                return self
                    .replay_completed_rehydration(collection, kind, digest, expected_plan_digest)
                    .await?
                    .ok_or_else(|| {
                        rehydration_state_invalid(
                            "Artifact Store rehydration state changed during guarded application.",
                        )
                    })
            }
        };
        let candidate_evidence = self.candidate_evidence(kind, candidate).await?;
        require_plan_candidate(&record.plan, &candidate_evidence)?;

        if !matches!(
            inspect_rehydration_state(&container).await?,
            ContainerRehydrationState::Prepared(_)
                | ContainerRehydrationState::InterruptedCompletion(_)
        ) {
            write_prepared_record(&container, &record, recover_preparation).await?;
        }
        let _mutation =
            ArtifactMutationLock::acquire(&container.join(MUTATION_LOCK), "artifact rehydration")
                .await?;
        let projection = rehydration_storage_projection(&record.plan, &container).await?;
        let _storage = self
            .acquire_rehydration_storage_admission(
                collection,
                kind,
                digest,
                projection.removed_before_write_bytes,
                projection.added_bytes,
            )
            .await?;
        self.rehydrate_physical_content(&record.plan, &container, &sha256, candidate)
            .await?;

        let recover_completion = matches!(
            inspect_rehydration_state(&container).await?,
            ContainerRehydrationState::InterruptedCompletion(_)
        );
        write_completed_record(&container, &record, recover_completion).await?;
        match inspect_rehydration_state(&container).await? {
            ContainerRehydrationState::Rehydrated(observed) if observed == record => {
                rehydration_result(record, true)
            }
            _ => Err(rehydration_state_invalid(
                "Artifact Store rehydration did not durably complete as reviewed.",
            )),
        }
    }

    async fn require_rehydration_binding(
        &self,
        record: &ArtifactRehydrationRecord,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<()> {
        record.validate()?;
        if record.plan.kind != kind || record.plan.digest != digest {
            return Err(rehydration_state_invalid(
                "An Artifact Store rehydration record does not match its digest container.",
            ));
        }
        let (_, container) = self.rehydration_container(kind, digest)?;
        let quarantine =
            super::quarantine::inspect_container_state(&container, kind, digest).await?;
        let super::quarantine::ContainerQuarantineState::Quarantined(quarantine) = quarantine
        else {
            return Err(rehydration_state_invalid(
                "Artifact Store rehydration evidence has no matching quarantine record.",
            ));
        };
        validate_record_binding(record, &quarantine)
    }

    fn rehydration_container(
        &self,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<(String, PathBuf)> {
        let sha256 = digest.strip_prefix("sha256:").ok_or_else(|| {
            artifact_store_error(
                "use.artifact_store.digest_invalid",
                "An Artifact Store rehydration digest must use the 'sha256:' prefix.",
            )
        })?;
        validate_sha256(sha256)?;
        let container = match kind {
            ArtifactKind::Blob => self.blob_container(sha256),
            ArtifactKind::ExpandedPackage => self.expanded_package_container(sha256),
        };
        Ok((sha256.to_owned(), container))
    }
}

fn rehydration_record(
    plan: ArtifactRehydrationPlan,
    plan_digest: &str,
) -> UseResult<ArtifactRehydrationRecord> {
    let record = ArtifactRehydrationRecord {
        schema: ARTIFACT_REHYDRATION_RECORD_SCHEMA.to_owned(),
        plan_digest: plan_digest.to_owned(),
        plan,
    };
    record.validate()?;
    Ok(record)
}

fn rehydration_result(
    record: ArtifactRehydrationRecord,
    changed: bool,
) -> UseResult<ArtifactRehydrationResult> {
    let result = ArtifactRehydrationResult {
        schema: ARTIFACT_REHYDRATION_RESULT_SCHEMA.to_owned(),
        plan_digest: record.plan_digest.clone(),
        changed,
        record,
    };
    result.validate()?;
    Ok(result)
}

fn validate_record_binding(
    record: &ArtifactRehydrationRecord,
    quarantine: &ArtifactQuarantineRecord,
) -> UseResult<()> {
    if record.plan.kind != quarantine.plan.kind
        || record.plan.digest != quarantine.plan.digest
        || record.plan.quarantine_plan_digest != quarantine.plan_digest
        || record.plan.quarantined_observed_digest != quarantine.plan.observed_digest
        || record.plan.quarantined_content_bytes != quarantine.plan.content_bytes
        || record.plan.quarantined_content_files != quarantine.plan.content_files
    {
        return Err(rehydration_state_invalid(
            "Artifact Store rehydration evidence does not match its quarantine record.",
        ));
    }
    Ok(())
}

pub(super) fn validate_container_rehydration_state(
    kind: ArtifactKind,
    digest: &str,
    quarantine: &super::quarantine::ContainerQuarantineState,
    rehydration: &ContainerRehydrationState,
) -> UseResult<()> {
    match rehydration {
        ContainerRehydrationState::None => Ok(()),
        ContainerRehydrationState::InterruptedPreparation => {
            if matches!(
                quarantine,
                super::quarantine::ContainerQuarantineState::Quarantined(_)
            ) {
                Ok(())
            } else {
                Err(rehydration_state_invalid(
                    "Interrupted Artifact Store rehydration has no active quarantine record.",
                ))
            }
        }
        ContainerRehydrationState::Prepared(record)
        | ContainerRehydrationState::InterruptedCompletion(record)
        | ContainerRehydrationState::Rehydrated(record) => {
            let super::quarantine::ContainerQuarantineState::Quarantined(quarantine) = quarantine
            else {
                return Err(rehydration_state_invalid(
                    "Artifact Store rehydration evidence has no active quarantine record.",
                ));
            };
            if record.plan.kind != kind || record.plan.digest != digest {
                return Err(rehydration_state_invalid(
                    "Artifact Store rehydration evidence does not match its digest container.",
                ));
            }
            validate_record_binding(record, quarantine)
        }
    }
}

fn validate_canonical_digest(value: &str) -> UseResult<()> {
    let sha256 = value.strip_prefix("sha256:").ok_or_else(|| {
        rehydration_plan_invalid("An Artifact Store rehydration digest must use 'sha256:'.")
    })?;
    validate_sha256(sha256).map_err(|error| rehydration_plan_invalid(error.message))
}

pub(super) fn rehydration_plan_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.rehydration_plan_invalid", message)
}

pub(super) fn rehydration_plan_mismatch(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.rehydration_plan_mismatch", message)
}

pub(super) fn rehydration_state_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.rehydration_state_invalid", message)
}
