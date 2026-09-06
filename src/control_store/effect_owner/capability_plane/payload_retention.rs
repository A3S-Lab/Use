//! One maintenance-fenced retention boundary for the Capability payload owners.
//!
//! Catalog records and descriptor snapshots are independent immutable stores,
//! but lifecycle retention must not prune one owner while a reviewed plan for
//! the other owner is already stale. This coordinator binds both child plans
//! to one digest, preflights both inventories under one exclusive maintenance
//! fence, and then applies them in a deterministic catalog → descriptor
//! order. A small durable coordinator journal records the cross-owner phase;
//! retrying after a process stop therefore resumes the reviewed operation
//! without asking a caller to guess which owner may already have changed.

use std::path::Path;

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceGuard, StateMaintenanceLock};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[path = "payload_retention_journal.rs"]
mod journal;

use super::descriptor_snapshot::{
    ControlCapabilityDescriptorSnapshotRetentionPlan,
    ControlCapabilityDescriptorSnapshotRetentionResult, ControlCapabilityDescriptorSnapshotStore,
};
use crate::capability_catalog_store::{
    CapabilityGatewayCatalogRetentionPlan, CapabilityGatewayCatalogRetentionResult,
    CapabilityGatewayCatalogStore,
};

pub(in crate::control_store) const CONTROL_CAPABILITY_PAYLOAD_RETENTION_PLAN_SCHEMA: &str =
    "a3s.use.control-capability-payload-retention-plan.v1";
pub(in crate::control_store) const CONTROL_CAPABILITY_PAYLOAD_RETENTION_RESULT_SCHEMA: &str =
    "a3s.use.control-capability-payload-retention-result.v1";
pub(in crate::control_store) const CONTROL_CAPABILITY_PAYLOAD_RETENTION_JOURNAL_SCHEMA: &str =
    "a3s.use.control-capability-payload-retention-journal.v1";
const DIGEST_DOMAIN: &[u8] = b"a3s.use.control-capability-payload-retention.v1\0";
const ERROR_INVALID: &str = "use.control.capability_payload_retention_invalid";
const ERROR_STALE: &str = "use.control.capability_payload_retention_stale";
const ERROR_JOURNAL_IO: &str = "use.control.capability_payload_retention_journal_io";
const MAX_PLAN_BYTES: usize = 8 * 1024 * 1024;

/// Canonical binding of the exact catalog and descriptor-snapshot retention
/// plans reviewed for one installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadRetentionPlan {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) catalog_plan: CapabilityGatewayCatalogRetentionPlan,
    pub(in crate::control_store) descriptor_snapshot_plan:
        ControlCapabilityDescriptorSnapshotRetentionPlan,
    pub(in crate::control_store) catalog_plan_digest: String,
    pub(in crate::control_store) descriptor_snapshot_plan_digest: String,
}

impl ControlCapabilityPayloadRetentionPlan {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            coordinator_invalid("The Capability payload retention installation is invalid.")
        })?;
        self.catalog_plan.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The catalog retention child plan is invalid: {}",
                error.message
            ))
        })?;
        self.descriptor_snapshot_plan.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The descriptor snapshot retention child plan is invalid: {}",
                error.message
            ))
        })?;
        if self.schema != CONTROL_CAPABILITY_PAYLOAD_RETENTION_PLAN_SCHEMA
            || self.catalog_plan.installation != self.installation
            || self.descriptor_snapshot_plan.installation != self.installation
            || !valid_sha256(&self.catalog_plan_digest)
            || !valid_sha256(&self.descriptor_snapshot_plan_digest)
            || self.catalog_plan.descriptor_digest()? != self.catalog_plan_digest
            || self.descriptor_snapshot_plan.descriptor_digest()?
                != self.descriptor_snapshot_plan_digest
        {
            return Err(coordinator_invalid(
                "The Capability payload retention child plans are foreign or rebound.",
            ));
        }
        let bytes = canonical_json(self)?;
        if bytes.is_empty() || bytes.len() > MAX_PLAN_BYTES {
            return Err(coordinator_invalid(
                "The Capability payload retention plan exceeds its byte bound.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        let bytes = canonical_json(self)?;
        let mut digest = Sha256::new();
        digest.update(DIGEST_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

/// Evidence returned after both child owners complete or replay their exact
/// retention journals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadRetentionResult {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) changed: bool,
    pub(in crate::control_store) catalog: CapabilityGatewayCatalogRetentionResult,
    pub(in crate::control_store) descriptor_snapshot:
        ControlCapabilityDescriptorSnapshotRetentionResult,
}

impl ControlCapabilityPayloadRetentionResult {
    pub(in crate::control_store) fn validate(
        &self,
        plan: &ControlCapabilityPayloadRetentionPlan,
        plan_digest: &str,
    ) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            coordinator_invalid("The Capability payload retention result installation is invalid.")
        })?;
        self.catalog.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The catalog retention child result is invalid: {}",
                error.message
            ))
        })?;
        self.descriptor_snapshot.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The descriptor snapshot retention child result is invalid: {}",
                error.message
            ))
        })?;
        let catalog_removed = plan.catalog_plan.remove.as_slice();
        let descriptor_removed = plan.descriptor_snapshot_plan.remove.as_slice();
        let catalog_removed_matches = self.catalog.removed.as_slice() == catalog_removed
            || (self.catalog.removed.is_empty() && !self.catalog.changed);
        let descriptor_removed_matches = self.descriptor_snapshot.removed.as_slice()
            == descriptor_removed
            || (self.descriptor_snapshot.removed.is_empty() && !self.descriptor_snapshot.changed);
        if self.schema != CONTROL_CAPABILITY_PAYLOAD_RETENTION_RESULT_SCHEMA
            || self.installation != plan.installation
            || self.plan_digest != plan_digest
            || self.catalog.installation != plan.installation
            || self.descriptor_snapshot.installation != plan.installation
            || self.catalog.plan_digest != plan.catalog_plan_digest
            || self.descriptor_snapshot.plan_digest != plan.descriptor_snapshot_plan_digest
            || !catalog_removed_matches
            || !descriptor_removed_matches
            || self.catalog.retained_record_count
                != u64::try_from(plan.catalog_plan.retain.len()).unwrap_or(u64::MAX)
            || self.descriptor_snapshot.retained_record_count
                != u64::try_from(plan.descriptor_snapshot_plan.retain.len()).unwrap_or(u64::MAX)
            || self.changed != (self.catalog.changed || self.descriptor_snapshot.changed)
        {
            return Err(coordinator_invalid(
                "The Capability payload retention result differs from its reviewed child plans.",
            ));
        }
        Ok(())
    }
}

/// Installation-scoped coordinator for the two immutable Capability payload
/// owners. Construction rejects a mixed installation or state root before any
/// retention operation can acquire a lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityPayloadRetentionCoordinator {
    catalog_store: CapabilityGatewayCatalogStore,
    descriptor_snapshot_store: ControlCapabilityDescriptorSnapshotStore,
}

impl ControlCapabilityPayloadRetentionCoordinator {
    pub(in crate::control_store) fn new(
        catalog_store: CapabilityGatewayCatalogStore,
        descriptor_snapshot_store: ControlCapabilityDescriptorSnapshotStore,
    ) -> UseResult<Self> {
        if catalog_store.installation() != descriptor_snapshot_store.installation()
            || catalog_store.state_root() != descriptor_snapshot_store.state_root()
        {
            return Err(coordinator_invalid(
                "The Capability payload owners do not share one installation root.",
            ));
        }
        Ok(Self {
            catalog_store,
            descriptor_snapshot_store,
        })
    }

    #[allow(dead_code)]
    pub(in crate::control_store) fn from_extension_paths(
        paths: &ExtensionPaths,
    ) -> UseResult<Self> {
        Self::new(
            CapabilityGatewayCatalogStore::from_extension_paths(paths),
            ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths),
        )
    }

    pub(in crate::control_store) fn installation(&self) -> &InstallationId {
        self.catalog_store.installation()
    }

    pub(in crate::control_store) fn state_root(&self) -> &Path {
        self.catalog_store.state_root()
    }

    pub(in crate::control_store) fn catalog_store(&self) -> &CapabilityGatewayCatalogStore {
        &self.catalog_store
    }

    pub(in crate::control_store) fn descriptor_snapshot_store(
        &self,
    ) -> &ControlCapabilityDescriptorSnapshotStore {
        &self.descriptor_snapshot_store
    }

    /// Build one path-free retention plan from both current inventories.
    /// Each child plan is independently reviewed; apply performs the final
    /// cross-owner preflight under one exclusive fence.
    pub(in crate::control_store) async fn plan_retention(
        &self,
        catalog_retain_digests: &[String],
        descriptor_snapshot_retain_digests: &[String],
    ) -> UseResult<ControlCapabilityPayloadRetentionPlan> {
        ensure_no_pending_journal(self.state_root()).await?;
        let catalog_plan = self
            .catalog_store
            .plan_retention(catalog_retain_digests)
            .await?;
        let descriptor_snapshot_plan = self
            .descriptor_snapshot_store
            .plan_retention(descriptor_snapshot_retain_digests)
            .await?;
        let plan = ControlCapabilityPayloadRetentionPlan {
            schema: CONTROL_CAPABILITY_PAYLOAD_RETENTION_PLAN_SCHEMA.to_owned(),
            installation: self.installation().clone(),
            catalog_plan_digest: catalog_plan.descriptor_digest()?,
            descriptor_snapshot_plan_digest: descriptor_snapshot_plan.descriptor_digest()?,
            catalog_plan,
            descriptor_snapshot_plan,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Preflight both child inventories while holding one exclusive
    /// installation fence, then delete in deterministic catalog → descriptor
    /// order. The coordinator journal is created before the first unlink and
    /// advanced after catalog completion, so a restart can resume either
    /// phase using the exact reviewed plan.
    pub(in crate::control_store) async fn apply_retention(
        &self,
        plan: &ControlCapabilityPayloadRetentionPlan,
        expected_plan_digest: &str,
    ) -> UseResult<ControlCapabilityPayloadRetentionResult> {
        plan.validate()?;
        if !valid_sha256(expected_plan_digest) {
            return Err(coordinator_invalid(
                "The Capability payload retention plan digest is invalid.",
            ));
        }
        if plan.descriptor_digest()? != expected_plan_digest {
            return Err(coordinator_invalid(
                "The confirmed Capability payload retention plan differs from its payload.",
            ));
        }

        let _maintenance = StateMaintenanceLock::new(self.state_root())
            .acquire_exclusive()
            .await?;

        let existing_journal =
            journal::RetentionCoordinatorJournal::load_unbound(self.state_root()).await?;
        self.apply_retention_under_maintenance(plan, expected_plan_digest, existing_journal)
            .await
    }

    /// Apply a reviewed retention plan while the caller owns the exact
    /// installation-wide exclusive maintenance fence.
    ///
    /// This is the composition seam for lifecycle code that must re-check
    /// Control authority and the protected payload set immediately before
    /// deletion.  Accepting an already-held guard avoids recursively taking
    /// the same file lock and makes the authority check and destructive phase
    /// one indivisible boundary.
    pub(in crate::control_store) async fn apply_retention_with_exclusive_maintenance(
        &self,
        plan: &ControlCapabilityPayloadRetentionPlan,
        expected_plan_digest: &str,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlCapabilityPayloadRetentionResult> {
        plan.validate()?;
        if !valid_sha256(expected_plan_digest) || plan.descriptor_digest()? != expected_plan_digest
        {
            return Err(coordinator_invalid(
                "The confirmed Capability payload retention plan differs from its payload.",
            ));
        }
        if !maintenance.is_exclusive_for(self.state_root()) {
            return Err(coordinator_invalid(
                "Capability payload retention requires the installation's exclusive maintenance fence.",
            ));
        }
        let existing_journal =
            journal::RetentionCoordinatorJournal::load_unbound(self.state_root()).await?;
        self.apply_retention_under_maintenance(plan, expected_plan_digest, existing_journal)
            .await
    }

    /// Resume a pending cross-owner retention journal, if one exists. The
    /// journal itself is the authority for the exact plan after a restart;
    /// callers do not supply a replacement plan.
    pub(in crate::control_store) async fn recover_retention(
        &self,
    ) -> UseResult<Option<ControlCapabilityPayloadRetentionResult>> {
        let _maintenance = StateMaintenanceLock::new(self.state_root())
            .acquire_exclusive()
            .await?;
        let Some(journal) =
            journal::RetentionCoordinatorJournal::load_unbound(self.state_root()).await?
        else {
            return Ok(None);
        };
        let plan = journal.plan().clone();
        let digest = journal.plan_digest().to_owned();
        self.apply_retention_under_maintenance(&plan, &digest, Some(journal))
            .await
            .map(Some)
    }

    async fn apply_retention_under_maintenance(
        &self,
        plan: &ControlCapabilityPayloadRetentionPlan,
        expected_plan_digest: &str,
        existing_journal: Option<journal::RetentionCoordinatorJournal>,
    ) -> UseResult<ControlCapabilityPayloadRetentionResult> {
        plan.validate()?;
        if !valid_sha256(expected_plan_digest) || plan.descriptor_digest()? != expected_plan_digest
        {
            return Err(coordinator_invalid(
                "The confirmed Capability payload retention plan differs from its payload.",
            ));
        }

        // Bind an already-persisted coordinator journal before touching either
        // child journal. A caller cannot use a different plan to advance a
        // recovery left by an earlier process.
        let mut journal = match existing_journal {
            Some(journal) => {
                if journal.plan() != plan || journal.plan_digest() != expected_plan_digest {
                    return Err(coordinator_stale(
                        "The durable Capability payload retention journal belongs to another plan.",
                    ));
                }
                Some(journal)
            }
            None => None,
        };

        // Both target inventories are checked before the first unlink. A
        // stale second owner therefore cannot leave a newly-pruned first owner
        // behind.
        self.catalog_store
            .ensure_retention_target_under_maintenance(
                &plan.catalog_plan,
                &plan.catalog_plan_digest,
            )
            .await?;
        self.descriptor_snapshot_store
            .ensure_retention_target_under_maintenance(
                &plan.descriptor_snapshot_plan,
                &plan.descriptor_snapshot_plan_digest,
            )
            .await?;

        if journal.is_none()
            && (!plan.catalog_plan.remove.is_empty()
                || !plan.descriptor_snapshot_plan.remove.is_empty())
        {
            journal = Some(
                journal::RetentionCoordinatorJournal::create(
                    self.state_root(),
                    plan,
                    expected_plan_digest,
                )
                .await?,
            );
        }

        let catalog = self
            .catalog_store
            .apply_retention_under_maintenance(&plan.catalog_plan, plan.catalog_plan_digest.clone())
            .await?;
        if let Some(progress) = journal.as_mut() {
            if progress.is_prepared() {
                progress.mark_catalog_applied().await?;
            }
        }
        let descriptor_snapshot = self
            .descriptor_snapshot_store
            .apply_retention_under_maintenance(
                &plan.descriptor_snapshot_plan,
                &plan.descriptor_snapshot_plan_digest,
            )
            .await?;
        let result = ControlCapabilityPayloadRetentionResult {
            schema: CONTROL_CAPABILITY_PAYLOAD_RETENTION_RESULT_SCHEMA.to_owned(),
            installation: self.installation().clone(),
            plan_digest: expected_plan_digest.to_owned(),
            changed: catalog.changed || descriptor_snapshot.changed,
            catalog,
            descriptor_snapshot,
        };
        result.validate(plan, expected_plan_digest)?;
        if let Some(progress) = journal {
            progress.retire().await?;
        }
        Ok(result)
    }
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        coordinator_invalid(format!(
            "Failed to encode the Capability payload retention value: {error}"
        ))
    })?;
    Ok(bytes)
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn coordinator_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

fn coordinator_stale(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_STALE, message)
}

fn coordinator_journal_io(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_JOURNAL_IO, message)
}

/// Reject ordinary owner planning and publication while a cross-owner
/// retention operation is recoverable from disk. This check is made while a
/// caller holds its shared maintenance guard, so a coordinator cannot create
/// the journal concurrently with a permitted mutation.
pub(super) async fn ensure_no_pending_journal(state_root: &Path) -> UseResult<()> {
    if journal::RetentionCoordinatorJournal::has_pending(state_root).await? {
        return Err(coordinator_stale(
            "A Capability payload retention coordination journal is pending; resume that exact plan before another mutation.",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn seed_test_journal(
    state_root: &Path,
    plan: &ControlCapabilityPayloadRetentionPlan,
    plan_digest: &str,
    catalog_applied: bool,
) -> UseResult<()> {
    let mut journal =
        journal::RetentionCoordinatorJournal::create(state_root, plan, plan_digest).await?;
    if catalog_applied {
        journal.mark_catalog_applied().await?;
    }
    Ok(())
}
