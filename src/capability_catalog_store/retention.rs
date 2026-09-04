//! Explicit, plan-bound retention for immutable Gateway catalog payloads.
//!
//! The payload store deliberately has no mutable current pointer. Retention
//! therefore cannot infer which generations are live: the lifecycle authority
//! must name every digest that remains protected, review the resulting plan,
//! and apply that exact plan under the store mutation lock.

use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

#[cfg(feature = "extensions")]
use a3s_use_extension::StateMaintenanceLock;

use super::{
    metadata_is_link_or_reparse_point, path_error, path_invalid, read_catalog_at,
    CapabilityGatewayCatalogStore, MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS, MAX_DIRECTORY_ENTRIES,
};

/// Canonical schema for one reviewed catalog-retention plan.
pub const CAPABILITY_GATEWAY_CATALOG_RETENTION_PLAN_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-retention-plan.v1";
/// Canonical schema for one completed catalog-retention result.
pub const CAPABILITY_GATEWAY_CATALOG_RETENTION_RESULT_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-retention-result.v1";

const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;
const ERROR_INVALID: &str = "use.plugin.capability_gateway_catalog_retention_invalid";
const ERROR_STALE: &str = "use.plugin.capability_gateway_catalog_retention_stale";
const ERROR_OUTCOME_UNKNOWN: &str =
    "use.plugin.capability_gateway_catalog_retention_outcome_unknown";

/// One immutable catalog named by a retention plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRetentionEntry {
    pub digest: String,
    pub generation: u64,
    pub revision: String,
}

impl CapabilityGatewayCatalogRetentionEntry {
    fn validate(&self) -> UseResult<()> {
        super::validate_digest(&self.digest)?;
        super::validate_revision(&self.revision)?;
        Ok(())
    }
}

/// An immutable inventory partition reviewed before destructive retention.
///
/// `retain` is the complete protected set supplied by the lifecycle owner;
/// every record in `remove` is selected explicitly by the plan builder as the
/// complement of that set. A non-empty inventory must retain at least one
/// record, so an accidental empty protection set cannot erase an installation
/// in one call. A host that is intentionally deleting an installation can
/// remove the store as part of its higher-level uninstall protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRetentionPlan {
    pub schema: String,
    pub installation: InstallationId,
    pub before_record_count: u64,
    pub before_inventory_digest: String,
    pub remove: Vec<CapabilityGatewayCatalogRetentionEntry>,
    pub retain: Vec<CapabilityGatewayCatalogRetentionEntry>,
}

impl CapabilityGatewayCatalogRetentionPlan {
    /// Validate structural invariants without consulting the filesystem.
    pub fn validate(&self) -> UseResult<()> {
        self.installation.validate()?;
        let before_record_count = usize::try_from(self.before_record_count).unwrap_or(usize::MAX);
        if self.schema != CAPABILITY_GATEWAY_CATALOG_RETENTION_PLAN_SCHEMA
            || before_record_count > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS
        {
            return Err(retention_invalid(
                "The catalog-retention plan schema or record bound is invalid.",
            ));
        }
        super::validate_digest(&self.before_inventory_digest)?;
        let mut all = Vec::with_capacity(self.remove.len().saturating_add(self.retain.len()));
        all.extend(self.remove.iter().cloned());
        all.extend(self.retain.iter().cloned());
        if all.len() != before_record_count {
            return Err(retention_invalid(
                "The catalog-retention plan count does not match its inventory partition.",
            ));
        }
        validate_entries(&self.remove)?;
        validate_entries(&self.retain)?;
        let mut digests = BTreeSet::new();
        for entry in &all {
            if !digests.insert(entry.digest.as_str()) {
                return Err(retention_invalid(
                    "The catalog-retention plan repeats or overlaps a digest.",
                ));
            }
        }
        if self.before_record_count > 0 && self.retain.is_empty() {
            return Err(retention_invalid(
                "A non-empty catalog inventory must retain at least one record.",
            ));
        }
        all.sort_by(|left, right| left.digest.cmp(&right.digest));
        if !all.windows(2).all(|pair| pair[0].digest < pair[1].digest) {
            return Err(retention_invalid(
                "The catalog-retention plan inventory is not canonically ordered.",
            ));
        }
        let bytes = serde_json::to_vec(self).map_err(|error| {
            retention_invalid(format!(
                "The catalog-retention plan cannot be encoded: {error}"
            ))
        })?;
        if bytes.len() > MAX_PLAN_BYTES {
            return Err(retention_invalid(
                "The catalog-retention plan exceeds its byte bound.",
            ));
        }
        Ok(())
    }

    /// Return the canonical digest that must be confirmed before apply.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        canonical_digest(self)
    }
}

/// Bounded result of applying one exact retention plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRetentionResult {
    pub schema: String,
    pub installation: InstallationId,
    pub plan_digest: String,
    pub changed: bool,
    pub removed: Vec<CapabilityGatewayCatalogRetentionEntry>,
    pub retained_record_count: u64,
}

impl CapabilityGatewayCatalogStore {
    /// Build a retention plan while holding the same mutation boundary used by
    /// publication. `retain_digests` must name every payload that the
    /// lifecycle authority still protects, including generations held by
    /// active or draining sessions.
    pub async fn plan_retention(
        &self,
        retain_digests: &[String],
    ) -> UseResult<CapabilityGatewayCatalogRetentionPlan> {
        let retain_digests = validate_requested_digests(retain_digests)?;
        #[cfg(feature = "extensions")]
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        let _mutation = self.acquire_mutation().await?;
        super::validate_store_layout(&self.root).await?;
        let records = self.scan_records().await?;
        build_plan(self.installation.clone(), records, &retain_digests)
    }

    /// Apply only a previously reviewed retention plan whose canonical digest
    /// matches `expected_plan_digest`. The inventory is re-read under the
    /// mutation lock before any file is removed; a concurrent publication or
    /// tamper therefore produces a stale error instead of a best-effort prune.
    pub async fn apply_retention(
        &self,
        plan: &CapabilityGatewayCatalogRetentionPlan,
        expected_plan_digest: &str,
    ) -> UseResult<CapabilityGatewayCatalogRetentionResult> {
        plan.validate()?;
        super::validate_digest(expected_plan_digest)?;
        if plan.installation != self.installation {
            return Err(retention_invalid(
                "The catalog-retention plan belongs to another installation.",
            ));
        }
        let actual_plan_digest = plan.descriptor_digest()?;
        if actual_plan_digest != expected_plan_digest {
            return Err(retention_stale(
                "The confirmed catalog-retention plan digest does not match its payload.",
            ));
        }

        #[cfg(feature = "extensions")]
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        let _mutation = self.acquire_mutation().await?;
        super::validate_store_layout(&self.root).await?;
        let records = self.scan_records().await?;
        let current = entries_from_records(&records)?;
        if current == plan.retain {
            return Ok(CapabilityGatewayCatalogRetentionResult {
                schema: CAPABILITY_GATEWAY_CATALOG_RETENTION_RESULT_SCHEMA.to_owned(),
                installation: self.installation.clone(),
                plan_digest: actual_plan_digest,
                changed: false,
                removed: Vec::new(),
                retained_record_count: u64::try_from(current.len()).map_err(|_| {
                    retention_outcome_unknown(
                        "The retained catalog count exceeds the platform range.",
                        &[],
                    )
                })?,
            });
        }
        if inventory_digest(&current)? != plan.before_inventory_digest {
            return Err(retention_stale(
                "The catalog inventory changed after the retention plan was reviewed.",
            ));
        }
        if current.len() != usize::try_from(plan.before_record_count).unwrap_or(usize::MAX)
            || !same_partition(&current, plan)
        {
            return Err(retention_stale(
                "The catalog inventory no longer matches the retention plan.",
            ));
        }

        for entry in &plan.remove {
            let target = self.path_for_digest(&entry.digest)?;
            verify_record_before_remove(&target, entry).await?;
        }

        let mut removed = Vec::new();
        for entry in &plan.remove {
            let target = self.path_for_digest(&entry.digest)?;
            if let Err(error) = fs::remove_file(&target).await {
                return Err(retention_outcome_unknown(
                    format!("A reviewed catalog record could not be removed: {error}"),
                    &removed,
                ));
            }
            removed.push(entry.clone());
            let parent = target.parent().ok_or_else(path_invalid)?;
            if let Err(error) = super::sync_directory(parent).await {
                return Err(retention_outcome_unknown(
                    format!(
                        "A catalog record was removed, but shard durability could not be confirmed: {}",
                        error.message
                    ),
                    &removed,
                ));
            }
        }

        let after = self.scan_records().await.map_err(|error| {
            retention_outcome_unknown(
                format!(
                    "Catalog retention changed records, but the resulting inventory could not be verified: {}",
                    error.message
                ),
                &removed,
            )
        })?;
        let after_entries = entries_from_records(&after).map_err(|error| {
            retention_outcome_unknown(
                format!(
                    "The retained catalog inventory could not be decoded: {}",
                    error.message
                ),
                &removed,
            )
        })?;
        if after_entries != plan.retain {
            return Err(retention_outcome_unknown(
                "The catalog inventory changed during retention apply.",
                &removed,
            ));
        }
        Ok(CapabilityGatewayCatalogRetentionResult {
            schema: CAPABILITY_GATEWAY_CATALOG_RETENTION_RESULT_SCHEMA.to_owned(),
            installation: self.installation.clone(),
            plan_digest: actual_plan_digest,
            changed: !removed.is_empty(),
            removed,
            retained_record_count: u64::try_from(after_entries.len()).map_err(|_| {
                retention_outcome_unknown(
                    "The retained catalog count exceeds the platform range.",
                    &[],
                )
            })?,
        })
    }
}

fn build_plan(
    installation: InstallationId,
    records: Vec<(String, CapabilityGatewayCatalog)>,
    retain_digests: &BTreeSet<String>,
) -> UseResult<CapabilityGatewayCatalogRetentionPlan> {
    let entries = entries_from_records(&records)?;
    if entries.len() > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS {
        return Err(retention_invalid(
            "The catalog inventory exceeds its retention bound.",
        ));
    }
    if !entries.is_empty() && retain_digests.is_empty() {
        return Err(retention_invalid(
            "Retention requires at least one protected digest for a non-empty inventory.",
        ));
    }
    let known = entries
        .iter()
        .map(|entry| entry.digest.as_str())
        .collect::<BTreeSet<_>>();
    if retain_digests
        .iter()
        .any(|digest| !known.contains(digest.as_str()))
    {
        return Err(retention_stale(
            "A requested protected catalog digest is not present in the inventory.",
        ));
    }
    let (retain, remove): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|entry| retain_digests.contains(&entry.digest));
    let before_record_count =
        u64::try_from(retain.len().saturating_add(remove.len())).map_err(|_| {
            retention_invalid("The catalog inventory count exceeds the platform range.")
        })?;
    let plan = CapabilityGatewayCatalogRetentionPlan {
        schema: CAPABILITY_GATEWAY_CATALOG_RETENTION_PLAN_SCHEMA.to_owned(),
        installation,
        before_record_count,
        before_inventory_digest: inventory_digest_from_parts(&remove, &retain)?,
        remove,
        retain,
    };
    plan.validate()?;
    Ok(plan)
}

fn entries_from_records(
    records: &[(String, CapabilityGatewayCatalog)],
) -> UseResult<Vec<CapabilityGatewayCatalogRetentionEntry>> {
    let mut entries = records
        .iter()
        .map(|(digest, catalog)| CapabilityGatewayCatalogRetentionEntry {
            digest: digest.clone(),
            generation: catalog.generation(),
            revision: catalog.revision().to_owned(),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    validate_entries(&entries)?;
    Ok(entries)
}

fn validate_entries(entries: &[CapabilityGatewayCatalogRetentionEntry]) -> UseResult<()> {
    if entries.len() > MAX_DIRECTORY_ENTRIES {
        return Err(retention_invalid(
            "The catalog-retention inventory exceeds its entry bound.",
        ));
    }
    for entry in entries {
        entry.validate()?;
    }
    if !entries
        .windows(2)
        .all(|pair| pair[0].digest < pair[1].digest)
    {
        return Err(retention_invalid(
            "The catalog-retention entries are not canonically ordered.",
        ));
    }
    Ok(())
}

fn same_partition(
    current: &[CapabilityGatewayCatalogRetentionEntry],
    plan: &CapabilityGatewayCatalogRetentionPlan,
) -> bool {
    let mut expected = plan.remove.clone();
    expected.extend(plan.retain.clone());
    expected.sort_by(|left, right| left.digest.cmp(&right.digest));
    expected == current
}

fn validate_requested_digests(digests: &[String]) -> UseResult<BTreeSet<String>> {
    if digests.len() > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS {
        return Err(retention_invalid(
            "The protected catalog digest set exceeds its bound.",
        ));
    }
    let mut result = BTreeSet::new();
    for digest in digests {
        super::validate_digest(digest)?;
        if !result.insert(digest.clone()) {
            return Err(retention_invalid(
                "The protected catalog digest set contains a duplicate.",
            ));
        }
    }
    Ok(result)
}

fn inventory_digest(entries: &[CapabilityGatewayCatalogRetentionEntry]) -> UseResult<String> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| left.digest.cmp(&right.digest));
    inventory_digest_from_parts(&[], &sorted)
}

fn inventory_digest_from_parts(
    remove: &[CapabilityGatewayCatalogRetentionEntry],
    retain: &[CapabilityGatewayCatalogRetentionEntry],
) -> UseResult<String> {
    let mut all = remove.to_vec();
    all.extend(retain.to_vec());
    all.sort_by(|left, right| left.digest.cmp(&right.digest));
    canonical_digest(&all)
}

fn canonical_digest<T: Serialize>(value: &T) -> UseResult<String> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        retention_invalid(format!(
            "The catalog-retention value cannot be encoded: {error}"
        ))
    })?;
    if bytes.len() > MAX_PLAN_BYTES {
        return Err(retention_invalid(
            "The catalog-retention value exceeds its byte bound.",
        ));
    }
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

async fn verify_record_before_remove(
    path: &Path,
    expected: &CapabilityGatewayCatalogRetentionEntry,
) -> UseResult<()> {
    let Some((catalog, _bytes)) = read_catalog_at(path, &expected.digest).await? else {
        return Err(retention_stale(
            "A reviewed catalog record disappeared before retention apply.",
        ));
    };
    if catalog.generation() != expected.generation || catalog.revision() != expected.revision {
        return Err(retention_stale(
            "A reviewed catalog record changed before retention apply.",
        ));
    }
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog record before retention", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(retention_stale(
            "A reviewed catalog record is no longer an owned regular file.",
        ));
    }
    Ok(())
}

fn retention_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

fn retention_stale(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_STALE, message)
}

fn retention_outcome_unknown(
    message: impl Into<String>,
    removed: &[CapabilityGatewayCatalogRetentionEntry],
) -> UseError {
    UseError::new(ERROR_OUTCOME_UNKNOWN, message).with_detail(
        "removedDigests",
        removed
            .iter()
            .map(|entry| entry.digest.clone())
            .collect::<Vec<_>>(),
    )
}
