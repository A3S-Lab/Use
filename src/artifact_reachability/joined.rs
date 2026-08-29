use std::collections::BTreeMap;

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{
    ArtifactInventoryEntry, ArtifactKind, ArtifactPhysicalState, ArtifactStoreInventory,
    ARTIFACT_STORE_INVENTORY_SCHEMA, MAX_ARTIFACT_STORE_INVENTORY_ENTRIES,
};
use serde::{Deserialize, Serialize};

use super::{
    merge_expectation, valid_artifact_digest, validate_reference, ArtifactReferenceInventory,
    ArtifactReferenceSource, RawArtifactReference, ARTIFACT_REFERENCE_INVENTORY_SCHEMA,
    MAX_ARTIFACT_REFERENCE_FACTS,
};

pub const ARTIFACT_REACHABILITY_INVENTORY_SCHEMA: &str =
    "a3s.use.artifact-reachability-inventory.v1";

/// One logical owner of an artifact digest. Package and filesystem paths are
/// deliberately absent; the source-specific inventories retain finer audit
/// evidence where it exists.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReferenceOwner {
    pub source: ArtifactReferenceSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<InstallationId>,
    pub reference_count: u64,
}

/// Physical measurements for one canonical digest container. These values do
/// not assert that content matches the digest encoded by its path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactPhysicalEvidence {
    pub state: ArtifactPhysicalState,
    pub content_bytes: u64,
    pub content_files: u64,
    pub staging_entries: u64,
    pub staging_bytes: u64,
}

/// Comparison of signed or durable size expectations with physical metadata.
/// This is not a cryptographic integrity result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactMeasurementStatus {
    Unspecified,
    Unavailable,
    Matches,
    Mismatch,
}

/// One logical/physical join row for a unique `(kind, digest)` identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReachabilityEntry {
    pub kind: ArtifactKind,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_files: Option<u64>,
    pub references: Vec<ArtifactReferenceOwner>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical: Option<ArtifactPhysicalEvidence>,
    pub measurement_status: ArtifactMeasurementStatus,
}

/// Checked global storage accounting derived from the same joined view.
/// `physical_bytes` sums canonical content and abandoned staging file lengths;
/// it is not filesystem allocated-block usage and excludes directory metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactStorageUsage {
    pub artifact_keys: u64,
    pub referenced_artifacts: u64,
    pub physical_artifacts: u64,
    pub unreferenced_artifacts: u64,
    pub missing_referenced_artifacts: u64,
    pub incomplete_physical_artifacts: u64,
    pub measurement_mismatches: u64,
    pub content_bytes: u64,
    pub content_files: u64,
    pub staging_entries: u64,
    pub staging_bytes: u64,
    pub physical_bytes: u64,
    pub referenced_content_bytes: u64,
    pub unreferenced_content_bytes: u64,
}

/// Deterministic union of logical references and physical Artifact Store
/// evidence captured while new reference publication is frozen. Reference
/// retirement may leave conservative extra owners in this immutable view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReachabilityInventory {
    pub schema: String,
    pub artifacts: Vec<ArtifactReachabilityEntry>,
    pub usage: ArtifactStorageUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ArtifactKey {
    kind: ArtifactKind,
    digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OwnerKey {
    source: ArtifactReferenceSource,
    installation: Option<InstallationId>,
}

#[derive(Debug, Default)]
struct ArtifactBuilder {
    expected_bytes: Option<u64>,
    expected_files: Option<u64>,
    references: BTreeMap<OwnerKey, u64>,
    physical: Option<ArtifactPhysicalEvidence>,
}

pub(super) fn join(
    references: ArtifactReferenceInventory,
    physical: ArtifactStoreInventory,
) -> UseResult<ArtifactReachabilityInventory> {
    if references.schema != ARTIFACT_REFERENCE_INVENTORY_SCHEMA
        || physical.schema != ARTIFACT_STORE_INVENTORY_SCHEMA
    {
        return Err(join_invalid(
            "An artifact inventory input has an incompatible schema.",
        ));
    }

    let mut builders = BTreeMap::<ArtifactKey, ArtifactBuilder>::new();
    let mut reference_facts = 0_u64;
    for reference in references.entries {
        if reference.reference_count == 0 {
            return Err(join_invalid(
                "An artifact reference owner has a zero reference count.",
            ));
        }
        validate_reference(&RawArtifactReference {
            kind: reference.kind,
            digest: reference.digest.clone(),
            source: reference.source,
            installation: reference.installation.clone(),
            expected_bytes: reference.expected_bytes,
            expected_files: reference.expected_files,
        })?;
        reference_facts = reference_facts
            .checked_add(reference.reference_count)
            .ok_or_else(join_limit)?;
        if reference_facts > MAX_ARTIFACT_REFERENCE_FACTS {
            return Err(join_limit());
        }
        let builder = builders
            .entry(ArtifactKey {
                kind: reference.kind,
                digest: reference.digest,
            })
            .or_default();
        builder.expected_bytes =
            merge_expectation(builder.expected_bytes, reference.expected_bytes, "byte")?;
        builder.expected_files = merge_expectation(
            builder.expected_files,
            reference.expected_files,
            "file-count",
        )?;
        if builder
            .references
            .insert(
                OwnerKey {
                    source: reference.source,
                    installation: reference.installation,
                },
                reference.reference_count,
            )
            .is_some()
        {
            return Err(join_invalid(
                "The artifact reference inventory contains a duplicate owner.",
            ));
        }
    }

    for entry in physical.entries {
        validate_physical_entry(&entry)?;
        let builder = builders
            .entry(ArtifactKey {
                kind: entry.kind,
                digest: entry.digest,
            })
            .or_default();
        if builder.physical.is_some() {
            return Err(join_invalid(
                "The physical Artifact Store inventory contains a duplicate digest.",
            ));
        }
        builder.physical = Some(ArtifactPhysicalEvidence {
            state: entry.state,
            content_bytes: entry.content_bytes,
            content_files: entry.content_files,
            staging_entries: entry.staging_entries,
            staging_bytes: entry.staging_bytes,
        });
    }

    let artifacts = builders
        .into_iter()
        .map(|(key, builder)| {
            let measurement_status = measurement_status(
                builder.expected_bytes,
                builder.expected_files,
                builder.physical.as_ref(),
            );
            ArtifactReachabilityEntry {
                kind: key.kind,
                digest: key.digest,
                expected_bytes: builder.expected_bytes,
                expected_files: builder.expected_files,
                references: builder
                    .references
                    .into_iter()
                    .map(|(owner, reference_count)| ArtifactReferenceOwner {
                        source: owner.source,
                        installation: owner.installation,
                        reference_count,
                    })
                    .collect(),
                physical: builder.physical,
                measurement_status,
            }
        })
        .collect::<Vec<_>>();
    let usage = storage_usage(&artifacts)?;
    Ok(ArtifactReachabilityInventory {
        schema: ARTIFACT_REACHABILITY_INVENTORY_SCHEMA.to_owned(),
        artifacts,
        usage,
    })
}

fn validate_physical_entry(entry: &ArtifactInventoryEntry) -> UseResult<()> {
    let incomplete_with_content = entry.state == ArtifactPhysicalState::Incomplete
        && (entry.content_bytes != 0 || entry.content_files != 0);
    let invalid_blob_files = entry.kind == ArtifactKind::Blob
        && entry.state == ArtifactPhysicalState::Complete
        && entry.content_files != 1;
    if !valid_artifact_digest(&entry.digest)
        || incomplete_with_content
        || invalid_blob_files
        || (entry.staging_entries == 0 && entry.staging_bytes != 0)
    {
        return Err(join_invalid(
            "A physical Artifact Store entry has inconsistent identity or measurements.",
        ));
    }
    Ok(())
}

fn measurement_status(
    expected_bytes: Option<u64>,
    expected_files: Option<u64>,
    physical: Option<&ArtifactPhysicalEvidence>,
) -> ArtifactMeasurementStatus {
    if expected_bytes.is_none() && expected_files.is_none() {
        return ArtifactMeasurementStatus::Unspecified;
    }
    let Some(physical) = physical.filter(|value| value.state == ArtifactPhysicalState::Complete)
    else {
        return ArtifactMeasurementStatus::Unavailable;
    };
    if expected_bytes.is_none_or(|expected| expected == physical.content_bytes)
        && expected_files.is_none_or(|expected| expected == physical.content_files)
    {
        ArtifactMeasurementStatus::Matches
    } else {
        ArtifactMeasurementStatus::Mismatch
    }
}

pub(super) fn validated_usage(
    inventory: &ArtifactReachabilityInventory,
) -> UseResult<ArtifactStorageUsage> {
    if inventory.schema != ARTIFACT_REACHABILITY_INVENTORY_SCHEMA {
        return Err(join_invalid(
            "The artifact reachability inventory schema is incompatible.",
        ));
    }
    let usage = storage_usage(&inventory.artifacts)?;
    if usage != inventory.usage {
        return Err(join_invalid(
            "The artifact reachability usage summary does not match its entries.",
        ));
    }
    Ok(usage)
}

fn storage_usage(artifacts: &[ArtifactReachabilityEntry]) -> UseResult<ArtifactStorageUsage> {
    let mut usage = ArtifactStorageUsage {
        artifact_keys: 0,
        referenced_artifacts: 0,
        physical_artifacts: 0,
        unreferenced_artifacts: 0,
        missing_referenced_artifacts: 0,
        incomplete_physical_artifacts: 0,
        measurement_mismatches: 0,
        content_bytes: 0,
        content_files: 0,
        staging_entries: 0,
        staging_bytes: 0,
        physical_bytes: 0,
        referenced_content_bytes: 0,
        unreferenced_content_bytes: 0,
    };
    let mut previous_key = None;
    let mut reference_facts = 0_u64;
    for artifact in artifacts {
        let key = ArtifactKey {
            kind: artifact.kind,
            digest: artifact.digest.clone(),
        };
        if previous_key
            .as_ref()
            .is_some_and(|previous| previous >= &key)
        {
            return Err(join_invalid(
                "The joined artifact inventory is not uniquely and canonically ordered.",
            ));
        }
        validate_joined_entry(artifact, &mut reference_facts)?;
        previous_key = Some(key);
        usage.artifact_keys = checked_add(usage.artifact_keys, 1)?;
        if usage.artifact_keys
            > MAX_ARTIFACT_REFERENCE_FACTS + MAX_ARTIFACT_STORE_INVENTORY_ENTRIES as u64
        {
            return Err(join_limit());
        }
        let referenced = !artifact.references.is_empty();
        if referenced {
            usage.referenced_artifacts = checked_add(usage.referenced_artifacts, 1)?;
        }
        if artifact.measurement_status == ArtifactMeasurementStatus::Mismatch {
            usage.measurement_mismatches = checked_add(usage.measurement_mismatches, 1)?;
        }
        let Some(physical) = &artifact.physical else {
            if referenced {
                usage.missing_referenced_artifacts =
                    checked_add(usage.missing_referenced_artifacts, 1)?;
            }
            continue;
        };
        usage.physical_artifacts = checked_add(usage.physical_artifacts, 1)?;
        if usage.physical_artifacts > MAX_ARTIFACT_STORE_INVENTORY_ENTRIES as u64 {
            return Err(join_limit());
        }
        if physical.state == ArtifactPhysicalState::Incomplete {
            usage.incomplete_physical_artifacts =
                checked_add(usage.incomplete_physical_artifacts, 1)?;
        }
        usage.content_bytes = checked_add(usage.content_bytes, physical.content_bytes)?;
        usage.content_files = checked_add(usage.content_files, physical.content_files)?;
        usage.staging_entries = checked_add(usage.staging_entries, physical.staging_entries)?;
        usage.staging_bytes = checked_add(usage.staging_bytes, physical.staging_bytes)?;
        let physical_bytes = checked_add(physical.content_bytes, physical.staging_bytes)?;
        usage.physical_bytes = checked_add(usage.physical_bytes, physical_bytes)?;
        if referenced {
            usage.referenced_content_bytes =
                checked_add(usage.referenced_content_bytes, physical.content_bytes)?;
        } else {
            usage.unreferenced_artifacts = checked_add(usage.unreferenced_artifacts, 1)?;
            usage.unreferenced_content_bytes =
                checked_add(usage.unreferenced_content_bytes, physical.content_bytes)?;
        }
    }
    Ok(usage)
}

fn validate_joined_entry(
    artifact: &ArtifactReachabilityEntry,
    reference_facts: &mut u64,
) -> UseResult<()> {
    if !valid_artifact_digest(&artifact.digest)
        || (artifact.references.is_empty()
            && (artifact.expected_bytes.is_some() || artifact.expected_files.is_some()))
        || (artifact.references.is_empty() && artifact.physical.is_none())
    {
        return Err(join_invalid(
            "A joined artifact entry has inconsistent identity or evidence.",
        ));
    }
    let mut previous_owner = None;
    for owner in &artifact.references {
        if owner.reference_count == 0 {
            return Err(join_invalid(
                "A joined artifact reference owner has a zero count.",
            ));
        }
        let owner_key = OwnerKey {
            source: owner.source,
            installation: owner.installation.clone(),
        };
        if previous_owner
            .as_ref()
            .is_some_and(|previous| previous >= &owner_key)
        {
            return Err(join_invalid(
                "A joined artifact entry contains duplicate or unordered owners.",
            ));
        }
        validate_reference(&RawArtifactReference {
            kind: artifact.kind,
            digest: artifact.digest.clone(),
            source: owner.source,
            installation: owner.installation.clone(),
            expected_bytes: artifact.expected_bytes,
            expected_files: artifact.expected_files,
        })?;
        *reference_facts = checked_add(*reference_facts, owner.reference_count)?;
        if *reference_facts > MAX_ARTIFACT_REFERENCE_FACTS {
            return Err(join_limit());
        }
        previous_owner = Some(owner_key);
    }
    if let Some(physical) = &artifact.physical {
        validate_physical_entry(&ArtifactInventoryEntry {
            kind: artifact.kind,
            digest: artifact.digest.clone(),
            state: physical.state,
            content_bytes: physical.content_bytes,
            content_files: physical.content_files,
            staging_entries: physical.staging_entries,
            staging_bytes: physical.staging_bytes,
        })?;
    }
    if artifact.measurement_status
        != measurement_status(
            artifact.expected_bytes,
            artifact.expected_files,
            artifact.physical.as_ref(),
        )
    {
        return Err(join_invalid(
            "A joined artifact entry has a stale measurement status.",
        ));
    }
    Ok(())
}

fn checked_add(left: u64, right: u64) -> UseResult<u64> {
    left.checked_add(right).ok_or_else(join_limit)
}

fn join_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.artifact_reachability.join_invalid", message)
}

fn join_limit() -> UseError {
    UseError::new(
        "use.artifact_reachability.join_limit_exceeded",
        "The joined artifact reachability inventory exceeds its accounting bounds.",
    )
}
