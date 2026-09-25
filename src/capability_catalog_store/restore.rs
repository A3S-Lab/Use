//! Plan-bound clean-target restore for immutable Gateway catalogs.
//!
//! Catalog records are projections, not lifecycle authority. Restore therefore
//! accepts an exact caller-supplied record set, binds it to a canonical review
//! digest, and publishes the complete owner directory with one no-clobber
//! rename. Existing live owner state is never merged or replaced.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseError, UseResult};
use a3s_use_extension::StateMaintenanceLock;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::mutation::{
    configure_no_follow_async, ensure_directory_exists, ensure_owned_directory_chain,
    file_identity, sync_directory, validate_existing_directory, validate_regular_file,
    write_new_record,
};
use super::{
    canonical_catalog_bytes, metadata_is_link_or_reparse_point, path_for_digest, scan_records,
    validate_store_layout, CapabilityGatewayCatalogStore, CATALOG_LOCK, CATALOG_RETENTION_JOURNAL,
    CATALOG_STAGING, MAX_CAPABILITY_GATEWAY_CATALOG_BYTES, MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS,
};

/// Canonical schema for one reviewed clean-target catalog restore.
pub const CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-restore-plan.v1";
/// Canonical schema for one completed clean-target catalog restore.
pub const CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-restore-result.v1";

const INVENTORY_DOMAIN: &[u8] = b"a3s.use.capability-gateway-catalog-restore-inventory.v1\0";
const STAGING_PREFIX: &str = ".catalog-restore-";
const CANDIDATE_DIRECTORY: &str = "candidate";
const ACTIVATION_FILE: &str = "activation.json";
const ACTIVATION_PARTIAL_FILE: &str = "activation.json.partial";
const ACTIVATION_SCHEMA: &str = "a3s.use.capability-gateway-catalog-restore-activation.v1";
const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_ACTIVATION_BYTES: usize = 64 * 1024;
const MAX_RESTORE_BYTES: u64 = MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
    .saturating_mul(MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS as u64);
const ERROR_INVALID: &str = "use.plugin.capability_gateway_catalog_restore_invalid";
const ERROR_TARGET_NOT_EMPTY: &str =
    "use.plugin.capability_gateway_catalog_restore_target_not_empty";

mod layout;

use layout::{reject_foreign_staging, validate_candidate_layout, validate_staging_layout};

/// One immutable catalog named by a clean-target restore plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRestoreEntry {
    pub digest: String,
    pub generation: u64,
    pub revision: String,
    pub byte_count: u64,
}

impl CapabilityGatewayCatalogRestoreEntry {
    fn validate(&self) -> UseResult<()> {
        valid_digest(&self.digest)?;
        super::validate_revision(&self.revision).map_err(|_| {
            restore_invalid("A catalog restore entry contains an invalid revision.")
        })?;
        if self.byte_count == 0 || self.byte_count > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES {
            return Err(restore_invalid(
                "A catalog restore entry exceeds its canonical byte bound.",
            ));
        }
        Ok(())
    }
}

/// Exact path-free record set approved for a clean owner restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRestorePlan {
    pub schema: String,
    pub installation: InstallationId,
    pub record_count: u64,
    pub byte_count: u64,
    pub inventory_digest: String,
    pub records: Vec<CapabilityGatewayCatalogRestoreEntry>,
}

impl CapabilityGatewayCatalogRestorePlan {
    /// Validate the immutable plan without consulting live state.
    pub fn validate(&self) -> UseResult<()> {
        self.installation
            .validate()
            .map_err(|_| restore_invalid("The catalog restore plan installation is invalid."))?;
        if self.schema != CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA
            || self.records.len() > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS
            || self.record_count != u64::try_from(self.records.len()).unwrap_or(u64::MAX)
            || !valid_sha256(&self.inventory_digest)
        {
            return Err(restore_invalid(
                "The catalog restore plan identity or record count is invalid.",
            ));
        }
        let mut byte_count = 0_u64;
        let mut previous = None;
        for record in &self.records {
            record.validate()?;
            if previous.is_some_and(|digest| digest >= record.digest.as_str()) {
                return Err(restore_invalid(
                    "Catalog restore records are duplicated or not canonically ordered.",
                ));
            }
            previous = Some(record.digest.as_str());
            byte_count = byte_count
                .checked_add(record.byte_count)
                .ok_or_else(|| restore_invalid("Catalog restore byte accounting overflowed."))?;
        }
        if self.byte_count != byte_count
            || self.byte_count > MAX_RESTORE_BYTES
            || self.inventory_digest != inventory_digest(&self.records)?
        {
            return Err(restore_invalid(
                "The catalog restore plan inventory accounting is invalid.",
            ));
        }
        let bytes = canonical_json(self, "catalog restore plan")?;
        if bytes.is_empty() || bytes.len() > MAX_PLAN_BYTES {
            return Err(restore_invalid(
                "The catalog restore plan exceeds its canonical byte bound.",
            ));
        }
        Ok(())
    }

    /// Return the canonical digest that must be confirmed at apply time.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(digest(&canonical_json(self, "catalog restore plan")?))
    }
}

/// Bounded evidence for a completed or terminally replayed restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRestoreResult {
    pub schema: String,
    pub installation: InstallationId,
    pub plan_digest: String,
    pub inventory_digest: String,
    pub changed: bool,
    pub restored_record_count: u64,
    pub restored_byte_count: u64,
}

impl CapabilityGatewayCatalogRestoreResult {
    pub fn validate(&self) -> UseResult<()> {
        self.installation
            .validate()
            .map_err(|_| restore_invalid("The catalog restore result installation is invalid."))?;
        if self.schema != CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.inventory_digest)
            || self.restored_record_count > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS as u64
            || self.restored_byte_count > MAX_RESTORE_BYTES
            || (self.restored_record_count == 0 && self.restored_byte_count != 0)
        {
            return Err(restore_invalid(
                "The catalog restore result identity or accounting is invalid.",
            ));
        }
        Ok(())
    }
}

impl CapabilityGatewayCatalogStore {
    /// Build a path-free plan for an exact catalog record set.
    ///
    /// This is pure review evidence. Apply always re-derives the entries from
    /// the supplied canonical catalogs and refuses to merge them with any
    /// existing live owner directory.
    pub fn plan_clean_restore(
        &self,
        catalogs: &[CapabilityGatewayCatalog],
    ) -> UseResult<CapabilityGatewayCatalogRestorePlan> {
        let prepared = prepare_catalogs(self, catalogs)?;
        let records = prepared
            .iter()
            .map(|record| record.entry.clone())
            .collect::<Vec<_>>();
        let byte_count = records.iter().try_fold(0_u64, |total, record| {
            total
                .checked_add(record.byte_count)
                .ok_or_else(|| restore_invalid("Catalog restore byte accounting overflowed."))
        })?;
        let plan = CapabilityGatewayCatalogRestorePlan {
            schema: CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA.to_owned(),
            installation: self.installation.clone(),
            record_count: u64::try_from(records.len()).map_err(|_| {
                restore_invalid("The catalog restore record count exceeds the platform range.")
            })?,
            byte_count,
            inventory_digest: inventory_digest(&records)?,
            records,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Publish one reviewed catalog set only into a clean owner target.
    ///
    /// The complete candidate directory is verified before an activation
    /// marker is persisted, then moved into place with a no-clobber rename.
    /// A retry can recover both sides of that single publication boundary.
    pub async fn apply_clean_restore(
        &self,
        plan: &CapabilityGatewayCatalogRestorePlan,
        catalogs: &[CapabilityGatewayCatalog],
        expected_plan_digest: &str,
    ) -> UseResult<CapabilityGatewayCatalogRestoreResult> {
        plan.validate()?;
        valid_digest(expected_plan_digest)?;
        if plan.installation != self.installation {
            return Err(restore_invalid(
                "The catalog restore plan belongs to another installation.",
            ));
        }
        let plan_digest = plan.descriptor_digest()?;
        if plan_digest != expected_plan_digest {
            return Err(restore_invalid(
                "The confirmed catalog restore plan digest does not match its payload.",
            ));
        }
        let prepared = prepare_catalogs(self, catalogs)?;
        if prepared
            .iter()
            .map(|record| &record.entry)
            .ne(plan.records.iter())
        {
            return Err(restore_invalid(
                "The supplied catalog set differs from the reviewed restore plan.",
            ));
        }

        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_exclusive()
            .await?;
        self.apply_clean_restore_under_maintenance(plan, catalogs, &plan_digest)
            .await
    }

    /// Apply a previously prepared catalog restore while the caller owns the
    /// installation-wide exclusive maintenance fence. This is the internal
    /// composition seam used by the multi-owner capability restore coordinator;
    /// it deliberately does not acquire a second state lock.
    pub(crate) async fn apply_clean_restore_under_maintenance(
        &self,
        plan: &CapabilityGatewayCatalogRestorePlan,
        catalogs: &[CapabilityGatewayCatalog],
        plan_digest: &str,
    ) -> UseResult<CapabilityGatewayCatalogRestoreResult> {
        plan.validate()?;
        if plan.installation != self.installation {
            return Err(restore_invalid(
                "The catalog restore plan belongs to another installation.",
            ));
        }
        if plan.descriptor_digest()? != plan_digest {
            return Err(restore_invalid(
                "The catalog restore plan digest differs from its payload.",
            ));
        }
        #[cfg(feature = "extensions")]
        crate::control_store::ensure_capability_payload_retention_quiescent(&self.state_root)
            .await?;
        let prepared = prepare_catalogs(self, catalogs)?;
        if prepared
            .iter()
            .map(|record| &record.entry)
            .ne(plan.records.iter())
        {
            return Err(restore_invalid(
                "The prepared catalog set differs from the reviewed restore plan.",
            ));
        }
        ensure_directory_exists(&self.state_root).await?;
        let (state_root, root) = self.physical_paths().await?;
        let parent = root.parent().ok_or_else(|| {
            restore_invalid("The catalog restore target has no owned parent directory.")
        })?;
        ensure_owned_directory_chain(&state_root, parent).await?;
        let staging = staging_directory(parent, plan_digest)?;
        reject_foreign_staging(parent, &staging).await?;

        match inspect_live(self, &root).await? {
            LiveCatalogRoot::Absent => {}
            LiveCatalogRoot::Owned(current) if current == plan.records => {
                retire_completed_staging(self, &staging, plan, plan_digest).await?;
                return restore_result(plan, plan_digest.to_owned(), false);
            }
            LiveCatalogRoot::Owned(_) => return Err(restore_target_not_empty()),
        }
        if plan.records.is_empty() {
            reject_unexpected_staging(&staging).await?;
            return restore_result(plan, plan_digest.to_owned(), false);
        }

        prepare_staging(self, &state_root, &staging, &prepared, plan, plan_digest).await?;
        let candidate = staging.join(CANDIDATE_DIRECTORY);
        validate_candidate(self, &candidate, &plan.records).await?;
        if !recover_activation_marker(&staging, plan, plan_digest).await? {
            create_activation_marker(&staging, plan, plan_digest).await?;
        }
        validate_candidate(self, &candidate, &plan.records).await?;
        if !matches!(inspect_live(self, &root).await?, LiveCatalogRoot::Absent) {
            return Err(restore_target_not_empty());
        }
        publish_candidate(candidate, root.clone()).await?;
        let LiveCatalogRoot::Owned(current) = inspect_live(self, &root).await? else {
            return Err(restore_invalid(
                "The activated catalog owner directory is missing.",
            ));
        };
        if current != plan.records {
            return Err(restore_invalid(
                "The activated catalog owner inventory differs from its reviewed plan.",
            ));
        }
        retire_staging(&staging, plan, plan_digest).await?;
        restore_result(plan, plan_digest.to_owned(), true)
    }

    /// Validate the reviewed catalog source while an outer coordinator owns
    /// the installation-wide maintenance fence. This is deliberately
    /// side-effect free and lets a multi-owner restore reject source drift
    /// before any owner publishes bytes.
    pub(crate) fn validate_clean_restore_source_under_maintenance(
        &self,
        plan: &CapabilityGatewayCatalogRestorePlan,
        catalogs: &[CapabilityGatewayCatalog],
        plan_digest: &str,
    ) -> UseResult<()> {
        plan.validate()?;
        if plan.installation != self.installation {
            return Err(restore_invalid(
                "The catalog restore plan belongs to another installation.",
            ));
        }
        if plan.descriptor_digest()? != plan_digest {
            return Err(restore_invalid(
                "The catalog restore plan digest differs from its payload.",
            ));
        }
        let prepared = prepare_catalogs(self, catalogs)?;
        if prepared
            .iter()
            .map(|record| &record.entry)
            .ne(plan.records.iter())
        {
            return Err(restore_invalid(
                "The supplied catalog set differs from the reviewed restore plan.",
            ));
        }
        Ok(())
    }

    /// Check that a reviewed catalog target is either absent or already the
    /// exact requested inventory while the caller owns the maintenance fence.
    /// No payload bytes are written and no existing inventory is replaced.
    pub(crate) async fn ensure_clean_restore_target_under_maintenance(
        &self,
        plan: &CapabilityGatewayCatalogRestorePlan,
        plan_digest: &str,
    ) -> UseResult<()> {
        plan.validate()?;
        if plan.installation != self.installation {
            return Err(restore_invalid(
                "The catalog restore plan belongs to another installation.",
            ));
        }
        let expected_digest = plan.descriptor_digest()?;
        if expected_digest != plan_digest {
            return Err(restore_invalid(
                "The catalog restore plan digest differs from its payload.",
            ));
        }
        ensure_directory_exists(&self.state_root).await?;
        let (state_root, root) = self.physical_paths().await?;
        let parent = root.parent().ok_or_else(|| {
            restore_invalid("The catalog restore target has no owned parent directory.")
        })?;
        ensure_owned_directory_chain(&state_root, parent).await?;
        let staging = staging_directory(parent, plan_digest)?;
        reject_foreign_staging(parent, &staging).await?;
        match inspect_live(self, &root).await? {
            LiveCatalogRoot::Absent => {}
            LiveCatalogRoot::Owned(current) if current == plan.records => {}
            LiveCatalogRoot::Owned(_) => return Err(restore_target_not_empty()),
        }
        if plan.records.is_empty() {
            reject_unexpected_staging(&staging).await?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PreparedCatalog {
    entry: CapabilityGatewayCatalogRestoreEntry,
    bytes: Vec<u8>,
}

include!("restore_prepare.rs");

#[cfg(test)]
mod tests;
