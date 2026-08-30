use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod io;

use io::write_record;
pub(super) use io::{
    inspect_container_state, validate_quarantine_metadata, ContainerQuarantineState,
    QUARANTINE_RECORD, QUARANTINE_TEMPORARY,
};

use super::{
    artifact_store_error, validate_sha256, ArtifactCollectionGuard, ArtifactDigestAuditStatus,
    ArtifactKind, ArtifactStore,
};
use crate::package::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};

pub const ARTIFACT_QUARANTINE_PLAN_SCHEMA: &str = "a3s.use.artifact-quarantine-plan.v1";
pub const ARTIFACT_QUARANTINE_RECORD_SCHEMA: &str = "a3s.use.artifact-quarantine-record.v1";
pub const ARTIFACT_QUARANTINE_RESULT_SCHEMA: &str = "a3s.use.artifact-quarantine-result.v1";

/// Exact read-only corruption evidence that an operator reviews before
/// publishing a logical quarantine marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactQuarantinePlan {
    pub schema: String,
    pub kind: ArtifactKind,
    pub digest: String,
    pub observed_digest: String,
    pub content_bytes: u64,
    pub content_files: u64,
}

impl ArtifactQuarantinePlan {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ARTIFACT_QUARANTINE_PLAN_SCHEMA {
            return Err(quarantine_plan_invalid(
                "The Artifact Store quarantine plan schema is invalid.",
            ));
        }
        validate_canonical_digest(&self.digest)?;
        validate_canonical_digest(&self.observed_digest)?;
        if self.digest == self.observed_digest {
            return Err(quarantine_plan_invalid(
                "Artifact quarantine requires distinct expected and observed digests.",
            ));
        }
        match self.kind {
            ArtifactKind::Blob if self.content_files != 1 => {
                return Err(quarantine_plan_invalid(
                    "A quarantined Blob must describe exactly one content file.",
                ));
            }
            ArtifactKind::ExpandedPackage
                if self.content_files > MAX_PACKAGE_FILES as u64
                    || self.content_bytes > MAX_PACKAGE_BYTES =>
            {
                return Err(quarantine_plan_invalid(
                    "A quarantined expanded package exceeds package limits.",
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

/// Durable logical quarantine marker. The corrupt canonical content remains
/// untouched in its digest container as forensic evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactQuarantineRecord {
    pub schema: String,
    pub plan_digest: String,
    pub plan: ArtifactQuarantinePlan,
}

impl ArtifactQuarantineRecord {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ARTIFACT_QUARANTINE_RECORD_SCHEMA {
            return Err(quarantine_state_invalid(
                "The Artifact Store quarantine record schema is invalid.",
            ));
        }
        self.plan.validate().map_err(|error| {
            quarantine_state_invalid(format!(
                "The Artifact Store quarantine record contains an invalid plan: {}",
                error.message
            ))
        })?;
        validate_canonical_digest(&self.plan_digest).map_err(|error| {
            quarantine_state_invalid(format!(
                "The Artifact Store quarantine record has an invalid plan digest: {}",
                error.message
            ))
        })?;
        if self.plan.descriptor_digest()? != self.plan_digest {
            return Err(quarantine_state_invalid(
                "The Artifact Store quarantine record does not match its plan digest.",
            ));
        }
        Ok(())
    }
}

/// Replay-stable outcome of publishing one logical quarantine marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactQuarantineResult {
    pub schema: String,
    pub plan_digest: String,
    pub changed: bool,
    pub record: ArtifactQuarantineRecord,
}

impl ArtifactQuarantineResult {
    pub fn validate(&self) -> UseResult<()> {
        self.record.validate()?;
        if self.schema != ARTIFACT_QUARANTINE_RESULT_SCHEMA
            || self.plan_digest != self.record.plan_digest
        {
            return Err(quarantine_state_invalid(
                "The Artifact Store quarantine result is inconsistent.",
            ));
        }
        Ok(())
    }
}

impl ArtifactStore {
    /// Derive exact path-free quarantine evidence from a fresh full-store audit.
    /// Verified or incomplete content cannot be quarantined through this API.
    pub async fn plan_quarantine(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<ArtifactQuarantinePlan> {
        collection.ensure_store(self)?;
        let (_, container) = self.quarantine_container(kind, digest)?;
        let audit = self.audit_digests(collection).await?;
        let entry = audit
            .entries
            .iter()
            .find(|entry| entry.kind == kind && entry.digest == digest)
            .ok_or_else(|| {
                artifact_store_error(
                    "use.artifact_store.quarantine_not_auditable",
                    "Artifact quarantine requires a physical digest container.",
                )
            })?;
        let observed_digest = match entry.status {
            ArtifactDigestAuditStatus::Mismatch => {
                entry.observed_digest.clone().ok_or_else(|| {
                    quarantine_state_invalid(
                        "A mismatched digest audit entry omitted its observed digest.",
                    )
                })?
            }
            ArtifactDigestAuditStatus::Verified => {
                return Err(artifact_store_error(
                    "use.artifact_store.quarantine_not_required",
                    "Verified artifact content cannot be quarantined as corrupt.",
                ));
            }
            ArtifactDigestAuditStatus::Incomplete => {
                return Err(artifact_store_error(
                    "use.artifact_store.quarantine_not_auditable",
                    "Incomplete artifact content has no digest evidence to quarantine.",
                ));
            }
        };
        let plan = ArtifactQuarantinePlan {
            schema: ARTIFACT_QUARANTINE_PLAN_SCHEMA.to_owned(),
            kind,
            digest: digest.to_owned(),
            observed_digest,
            content_bytes: entry.content_bytes,
            content_files: entry.content_files,
        };
        plan.validate()?;
        if let ContainerQuarantineState::Quarantined(record) =
            inspect_container_state(&container, kind, digest).await?
        {
            if record.plan != plan {
                return Err(quarantine_state_invalid(
                    "Quarantined content changed after its forensic record was published.",
                ));
            }
        }
        Ok(plan)
    }

    /// Publish a durable fail-closed marker for the exact re-audited plan.
    ///
    /// Embedding hosts must collect confirmation outside package-controlled
    /// code. This method requires the exact reviewed plan digest, recomputes
    /// the plan under the same collection guard, and never moves or overwrites
    /// canonical content.
    pub async fn apply_quarantine(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
        expected_plan_digest: &str,
    ) -> UseResult<ArtifactQuarantineResult> {
        validate_canonical_digest(expected_plan_digest).map_err(|_| {
            quarantine_plan_mismatch(
                "Artifact quarantine requires an exact canonical SHA-256 plan digest.",
            )
        })?;
        let current = self.plan_quarantine(collection, kind, digest).await?;
        let actual_plan_digest = current.descriptor_digest()?;
        if actual_plan_digest != expected_plan_digest {
            return Err(quarantine_plan_mismatch(
                "Artifact content changed after quarantine review; create and confirm a new plan.",
            )
            .with_detail("actualPlanDigest", actual_plan_digest));
        }

        let (_, container) = self.quarantine_container(kind, digest)?;
        let record = ArtifactQuarantineRecord {
            schema: ARTIFACT_QUARANTINE_RECORD_SCHEMA.to_owned(),
            plan_digest: expected_plan_digest.to_owned(),
            plan: current,
        };
        record.validate()?;
        let recover_interrupted = match inspect_container_state(&container, kind, digest).await? {
            ContainerQuarantineState::Quarantined(existing) => {
                if existing != record {
                    return Err(quarantine_state_invalid(
                        "An existing Artifact Store quarantine record conflicts with the reviewed plan.",
                    ));
                }
                return quarantine_result(record, false);
            }
            ContainerQuarantineState::Interrupted => true,
            ContainerQuarantineState::None => false,
        };

        write_record(&container, &record, recover_interrupted).await?;
        match inspect_container_state(&container, kind, digest).await? {
            ContainerQuarantineState::Quarantined(observed) if observed == record => {
                quarantine_result(record, true)
            }
            _ => Err(quarantine_state_invalid(
                "The Artifact Store quarantine record was not durably published as reviewed.",
            )),
        }
    }

    /// Read one validated logical quarantine marker under the exact store guard.
    pub async fn inspect_quarantine(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<Option<ArtifactQuarantineRecord>> {
        collection.ensure_store(self)?;
        self.inspect_inventory(collection).await?;
        let (_, container) = self.quarantine_container(kind, digest)?;
        match inspect_container_state(&container, kind, digest).await? {
            ContainerQuarantineState::None => Ok(None),
            ContainerQuarantineState::Interrupted => Err(quarantine_state_invalid(
                "Artifact Store quarantine publication was interrupted before its record became durable.",
            )),
            ContainerQuarantineState::Quarantined(record) => Ok(Some(record)),
        }
    }

    pub(super) async fn ensure_container_not_quarantined(
        &self,
        container: &Path,
        kind: ArtifactKind,
        sha256: &str,
    ) -> UseResult<()> {
        let digest = format!("sha256:{sha256}");
        match inspect_container_state(container, kind, &digest).await? {
            ContainerQuarantineState::None => Ok(()),
            ContainerQuarantineState::Interrupted => Err(quarantine_state_invalid(
                "Artifact Store quarantine publication is incomplete; ordinary artifact access is denied.",
            )),
            ContainerQuarantineState::Quarantined(record) => Err(artifact_store_error(
                "use.artifact_store.quarantined",
                "Artifact content is logically quarantined and unavailable to ordinary consumers.",
            )
            .with_detail("kind", kind_name(kind))
            .with_detail("digest", digest)
            .with_detail("observedDigest", record.plan.observed_digest)
            .with_detail("planDigest", record.plan_digest)),
        }
    }

    fn quarantine_container(
        &self,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<(String, PathBuf)> {
        let sha256 = digest.strip_prefix("sha256:").ok_or_else(|| {
            artifact_store_error(
                "use.artifact_store.digest_invalid",
                "An Artifact Store quarantine digest must use the 'sha256:' prefix.",
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

fn quarantine_result(
    record: ArtifactQuarantineRecord,
    changed: bool,
) -> UseResult<ArtifactQuarantineResult> {
    let result = ArtifactQuarantineResult {
        schema: ARTIFACT_QUARANTINE_RESULT_SCHEMA.to_owned(),
        plan_digest: record.plan_digest.clone(),
        changed,
        record,
    };
    result.validate()?;
    Ok(result)
}

pub(super) fn canonical_json(value: &(impl Serialize + ?Sized)) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        quarantine_state_invalid(format!(
            "Failed to encode canonical Artifact Store quarantine evidence: {error}"
        ))
    })?;
    Ok(bytes)
}

fn validate_canonical_digest(value: &str) -> UseResult<()> {
    let sha256 = value.strip_prefix("sha256:").ok_or_else(|| {
        quarantine_plan_invalid("An Artifact Store quarantine digest must use 'sha256:'.")
    })?;
    validate_sha256(sha256).map_err(|error| quarantine_plan_invalid(error.message))
}

fn kind_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Blob => "blob",
        ArtifactKind::ExpandedPackage => "expanded-package",
    }
}

fn quarantine_plan_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.quarantine_plan_invalid", message)
}

fn quarantine_plan_mismatch(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.quarantine_plan_mismatch", message)
}

pub(super) fn quarantine_state_invalid(message: impl Into<String>) -> UseError {
    artifact_store_error("use.artifact_store.quarantine_state_invalid", message)
}
