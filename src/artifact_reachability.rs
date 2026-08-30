//! Global, path-free Artifact Store reachability evidence.
//!
//! Reference state remains owned by Registry sources, installations, and
//! durable operations. Physical state remains owned by the Artifact Store.
//! This module derives fresh immutable reference and joined views under the
//! global collection guard; it never creates a second mutable authority.

use std::collections::BTreeMap;

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{ArtifactCollectionGuard, ArtifactKind, RegistrySourceStore, UsePaths};
use serde::{Deserialize, Serialize};

mod installation;
mod joined;
mod quota;
mod rehydration;
#[cfg(test)]
mod tests;

pub use joined::{
    ArtifactMeasurementStatus, ArtifactPhysicalEvidence, ArtifactReachabilityEntry,
    ArtifactReachabilityInventory, ArtifactReferenceOwner, ArtifactStorageUsage,
    ARTIFACT_REACHABILITY_INVENTORY_SCHEMA,
};
pub use quota::{
    ArtifactStorageQuotaAssessment, ArtifactStorageQuotaPolicy,
    MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS,
};
pub use rehydration::{
    ArtifactRehydrationPlan, ArtifactRehydrationRecord, ArtifactRehydrationResult,
    ArtifactStoreMaintenance, ARTIFACT_REHYDRATION_PLAN_SCHEMA, ARTIFACT_REHYDRATION_RECORD_SCHEMA,
    ARTIFACT_REHYDRATION_RESULT_SCHEMA,
};

pub const ARTIFACT_REFERENCE_INVENTORY_SCHEMA: &str = "a3s.use.artifact-reference-inventory.v1";

pub(super) const MAX_ARTIFACT_REFERENCE_FACTS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactReferenceSource {
    RegistryObservation,
    InstallationSnapshot,
    CurrentReceipt,
    RetainedReceipt,
    PendingPackageGraph,
    PluginLifecycleOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReferenceEntry {
    pub kind: ArtifactKind,
    pub digest: String,
    pub source: ArtifactReferenceSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<InstallationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_files: Option<u64>,
    pub reference_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReferenceInventory {
    pub schema: String,
    pub entries: Vec<ArtifactReferenceEntry>,
}

#[derive(Debug, Clone)]
pub struct ArtifactReachabilityInspector {
    paths: UsePaths,
}

impl ArtifactReachabilityInspector {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(UsePaths::from_env()?))
    }

    pub fn new(paths: UsePaths) -> Self {
        Self { paths }
    }

    /// Freeze new durable references and derive a deterministic inventory from
    /// every Registry source, installation, and nonterminal operation.
    pub async fn inspect_references(&self) -> UseResult<ArtifactReferenceInventory> {
        let artifact_store = self.paths.artifact_store();
        let collection = artifact_store.acquire_collection().await?;
        self.inspect_references_under_collection(&collection).await
    }

    /// Join logical references and physical Artifact Store evidence in one
    /// guarded collection pass. New references and physical publication are
    /// frozen; concurrent retirement can only leave conservative extra owners.
    /// The result grants no deletion authority and does not claim that content
    /// bytes match their path digest.
    pub async fn inspect_reachability(&self) -> UseResult<ArtifactReachabilityInventory> {
        let artifact_store = self.paths.artifact_store();
        let collection = artifact_store.acquire_collection().await?;
        let references = self
            .inspect_references_under_collection(&collection)
            .await?;
        let physical = artifact_store.inspect_inventory(&collection).await?;
        joined::join(references, physical)
    }

    async fn inspect_references_under_collection(
        &self,
        collection: &ArtifactCollectionGuard,
    ) -> UseResult<ArtifactReferenceInventory> {
        let registry = RegistrySourceStore::new(self.paths.clone())
            .inspect_artifact_references(collection)
            .await?;
        let mut accumulator = ReferenceAccumulator::default();
        for reference in registry.references {
            accumulator.observe(RawArtifactReference {
                kind: ArtifactKind::Blob,
                digest: reference.digest,
                source: ArtifactReferenceSource::RegistryObservation,
                installation: None,
                expected_bytes: Some(reference.expected_bytes),
                expected_files: None,
            })?;
        }
        for reference in installation::inspect(&self.paths).await? {
            accumulator.observe(reference)?;
        }
        Ok(accumulator.finish())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RawArtifactReference {
    pub(crate) kind: ArtifactKind,
    pub(crate) digest: String,
    pub(crate) source: ArtifactReferenceSource,
    pub(crate) installation: Option<InstallationId>,
    pub(crate) expected_bytes: Option<u64>,
    pub(crate) expected_files: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReferenceKey {
    kind: ArtifactKind,
    digest: String,
    source: ArtifactReferenceSource,
    installation: Option<InstallationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ArtifactKey {
    kind: ArtifactKind,
    digest: String,
}

#[derive(Debug, Clone, Copy, Default)]
struct PhysicalExpectation {
    expected_bytes: Option<u64>,
    expected_files: Option<u64>,
}

#[derive(Debug, Default)]
struct ReferenceAccumulator {
    entries: BTreeMap<ReferenceKey, ArtifactReferenceEntry>,
    expectations: BTreeMap<ArtifactKey, PhysicalExpectation>,
    facts: u64,
}

impl ReferenceAccumulator {
    fn observe(&mut self, reference: RawArtifactReference) -> UseResult<()> {
        validate_reference(&reference)?;
        self.facts = self.facts.checked_add(1).ok_or_else(reference_limit)?;
        if self.facts > MAX_ARTIFACT_REFERENCE_FACTS {
            return Err(reference_limit());
        }
        let expectation = self
            .expectations
            .entry(ArtifactKey {
                kind: reference.kind,
                digest: reference.digest.clone(),
            })
            .or_default();
        expectation.expected_bytes =
            merge_expectation(expectation.expected_bytes, reference.expected_bytes, "byte")?;
        expectation.expected_files = merge_expectation(
            expectation.expected_files,
            reference.expected_files,
            "file-count",
        )?;
        let key = ReferenceKey {
            kind: reference.kind,
            digest: reference.digest.clone(),
            source: reference.source,
            installation: reference.installation.clone(),
        };
        match self.entries.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(ArtifactReferenceEntry {
                    kind: reference.kind,
                    digest: reference.digest,
                    source: reference.source,
                    installation: reference.installation,
                    expected_bytes: reference.expected_bytes,
                    expected_files: reference.expected_files,
                    reference_count: 1,
                });
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get_mut();
                current.reference_count = current
                    .reference_count
                    .checked_add(1)
                    .ok_or_else(reference_limit)?;
            }
        }
        Ok(())
    }

    fn finish(mut self) -> ArtifactReferenceInventory {
        for entry in self.entries.values_mut() {
            if let Some(expectation) = self.expectations.get(&ArtifactKey {
                kind: entry.kind,
                digest: entry.digest.clone(),
            }) {
                entry.expected_bytes = expectation.expected_bytes;
                entry.expected_files = expectation.expected_files;
            }
        }
        ArtifactReferenceInventory {
            schema: ARTIFACT_REFERENCE_INVENTORY_SCHEMA.to_owned(),
            entries: self.entries.into_values().collect(),
        }
    }
}

pub(super) fn validate_reference(reference: &RawArtifactReference) -> UseResult<()> {
    if !valid_artifact_digest(&reference.digest)
        || reference.expected_bytes == Some(0)
        || reference.expected_files == Some(0)
        || (reference.kind == ArtifactKind::Blob && reference.expected_files.is_some())
        || (reference.source == ArtifactReferenceSource::RegistryObservation
            && reference.installation.is_some())
        || (reference.source != ArtifactReferenceSource::RegistryObservation
            && reference.installation.is_none())
    {
        return Err(reference_invalid(
            "An artifact reference fact has invalid identity or physical expectations.",
        ));
    }
    if let Some(installation) = &reference.installation {
        installation.validate()?;
    }
    Ok(())
}

pub(super) fn merge_expectation(
    current: Option<u64>,
    candidate: Option<u64>,
    label: &str,
) -> UseResult<Option<u64>> {
    match (current, candidate) {
        (Some(left), Some(right)) if left != right => Err(reference_invalid(format!(
            "Artifact references disagree about their expected {label} evidence."
        ))),
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

pub(super) fn valid_artifact_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(crate) fn reference_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.artifact_reachability.reference_invalid", message)
}

fn reference_limit() -> UseError {
    UseError::new(
        "use.artifact_reachability.reference_limit_exceeded",
        "The global artifact reference inventory exceeds its bound.",
    )
}
