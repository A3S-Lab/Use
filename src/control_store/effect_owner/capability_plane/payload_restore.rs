//! One maintenance-fenced restore boundary for the Capability payload owners.
//!
//! The Gateway catalog and the descriptor-snapshot store are independent
//! immutable owners, but a recovered Capability plane is only useful when the
//! two reviewed inventories are admitted together.  This coordinator binds
//! their child plans to one canonical digest, preflights both source sets and
//! clean targets while holding one installation-wide exclusive fence, and
//! then replays each owner in a fixed order.  The individual owner activation
//! boundaries remain durable and no-clobber; if a process stops between them,
//! retrying the same coordinator plan observes the already-published owner as
//! an exact no-op and completes the remaining owner.

use std::path::Path;

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::descriptor_snapshot::{
    ControlCapabilityDescriptorSnapshot, ControlCapabilityDescriptorSnapshotRestorePlan,
    ControlCapabilityDescriptorSnapshotRestoreResult,
    ControlCapabilityDescriptorSnapshotRestoreVerification,
    ControlCapabilityDescriptorSnapshotStore,
};
use crate::capability_catalog_store::{
    CapabilityGatewayCatalogRestorePlan, CapabilityGatewayCatalogRestoreResult,
    CapabilityGatewayCatalogStore,
};

pub(in crate::control_store) const CONTROL_CAPABILITY_PAYLOAD_RESTORE_PLAN_SCHEMA: &str =
    "a3s.use.control-capability-payload-restore-plan.v1";
pub(in crate::control_store) const CONTROL_CAPABILITY_PAYLOAD_RESTORE_RESULT_SCHEMA: &str =
    "a3s.use.control-capability-payload-restore-result.v1";
const DIGEST_DOMAIN: &[u8] = b"a3s.use.control-capability-payload-restore.v1\0";
const ERROR_INVALID: &str = "use.control.capability_payload_restore_invalid";
const MAX_PLAN_BYTES: usize = 8 * 1024 * 1024;

/// Canonical plan binding the exact catalog and descriptor-snapshot child
/// plans reviewed for one installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadRestorePlan {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) catalog_plan: CapabilityGatewayCatalogRestorePlan,
    pub(in crate::control_store) descriptor_snapshot_plan:
        ControlCapabilityDescriptorSnapshotRestorePlan,
    pub(in crate::control_store) catalog_plan_digest: String,
    pub(in crate::control_store) descriptor_snapshot_plan_digest: String,
}

impl ControlCapabilityPayloadRestorePlan {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            coordinator_invalid("The Capability payload restore installation is invalid.")
        })?;
        self.catalog_plan.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The catalog child plan is invalid: {}",
                error.message
            ))
        })?;
        self.descriptor_snapshot_plan.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The descriptor snapshot child plan is invalid: {}",
                error.message
            ))
        })?;
        if self.schema != CONTROL_CAPABILITY_PAYLOAD_RESTORE_PLAN_SCHEMA
            || self.catalog_plan.installation != self.installation
            || self.descriptor_snapshot_plan.installation != self.installation
            || !valid_sha256(&self.catalog_plan_digest)
            || !valid_sha256(&self.descriptor_snapshot_plan_digest)
            || self.catalog_plan.descriptor_digest()? != self.catalog_plan_digest
            || self.descriptor_snapshot_plan.descriptor_digest()?
                != self.descriptor_snapshot_plan_digest
        {
            return Err(coordinator_invalid(
                "The Capability payload child plans are foreign or rebound.",
            ));
        }
        let bytes = canonical_json(self)?;
        if bytes.is_empty() || bytes.len() > MAX_PLAN_BYTES {
            return Err(coordinator_invalid(
                "The Capability payload restore plan exceeds its byte bound.",
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

/// Bounded evidence returned after both Capability payload owners have
/// completed or replayed their child activation boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadRestoreResult {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) changed: bool,
    pub(in crate::control_store) catalog: CapabilityGatewayCatalogRestoreResult,
    pub(in crate::control_store) descriptor_snapshot:
        ControlCapabilityDescriptorSnapshotRestoreResult,
}

impl ControlCapabilityPayloadRestoreResult {
    pub(in crate::control_store) fn validate(
        &self,
        plan: &ControlCapabilityPayloadRestorePlan,
        plan_digest: &str,
    ) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            coordinator_invalid("The Capability payload result installation is invalid.")
        })?;
        self.catalog.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The catalog child result is invalid: {}",
                error.message
            ))
        })?;
        self.descriptor_snapshot.validate().map_err(|error| {
            coordinator_invalid(format!(
                "The descriptor snapshot child result is invalid: {}",
                error.message
            ))
        })?;
        if self.schema != CONTROL_CAPABILITY_PAYLOAD_RESTORE_RESULT_SCHEMA
            || self.installation != plan.installation
            || self.plan_digest != plan_digest
            || self.catalog.installation != plan.installation
            || self.descriptor_snapshot.installation != plan.installation
            || self.catalog.plan_digest != plan.catalog_plan_digest
            || self.descriptor_snapshot.plan_digest != plan.descriptor_snapshot_plan_digest
            || self.catalog.inventory_digest != plan.catalog_plan.inventory_digest
            || self.descriptor_snapshot.inventory_digest
                != plan.descriptor_snapshot_plan.inventory_digest
            || self.changed != (self.catalog.changed || self.descriptor_snapshot.changed)
        {
            return Err(coordinator_invalid(
                "The Capability payload result differs from its reviewed child plans.",
            ));
        }
        Ok(())
    }
}

/// Installation-scoped coordinator for the two immutable Capability payload
/// owners. Construction checks that both owners point at exactly one state
/// root and installation before any filesystem operation is attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityPayloadRestoreCoordinator {
    catalog_store: CapabilityGatewayCatalogStore,
    descriptor_snapshot_store: ControlCapabilityDescriptorSnapshotStore,
}

impl ControlCapabilityPayloadRestoreCoordinator {
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

    /// Build one path-free plan from the exact source sets for both owners.
    pub(in crate::control_store) fn plan_clean_restore(
        &self,
        catalogs: &[CapabilityGatewayCatalog],
        snapshots: &[ControlCapabilityDescriptorSnapshot],
    ) -> UseResult<ControlCapabilityPayloadRestorePlan> {
        let catalog_plan = self.catalog_store.plan_clean_restore(catalogs)?;
        let descriptor_snapshot_plan = self
            .descriptor_snapshot_store
            .plan_clean_restore(snapshots)?;
        let plan = ControlCapabilityPayloadRestorePlan {
            schema: CONTROL_CAPABILITY_PAYLOAD_RESTORE_PLAN_SCHEMA.to_owned(),
            installation: self.installation().clone(),
            catalog_plan_digest: catalog_plan.descriptor_digest()?,
            descriptor_snapshot_plan_digest: descriptor_snapshot_plan.descriptor_digest()?,
            catalog_plan,
            descriptor_snapshot_plan,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Preflight and apply both child plans under one exclusive maintenance
    /// guard. Source and target validation for both owners completes before
    /// the first publication. Child owners then activate in fixed catalog →
    /// descriptor order; each child boundary is independently replayable.
    pub(in crate::control_store) async fn apply_clean_restore(
        &self,
        plan: &ControlCapabilityPayloadRestorePlan,
        catalogs: &[CapabilityGatewayCatalog],
        snapshots: &[ControlCapabilityDescriptorSnapshot],
        expected_plan_digest: &str,
        verification: ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
    ) -> UseResult<ControlCapabilityPayloadRestoreResult> {
        plan.validate()?;
        if !valid_sha256(expected_plan_digest) {
            return Err(coordinator_invalid(
                "The Capability payload restore plan digest is invalid.",
            ));
        }
        if plan.descriptor_digest()? != expected_plan_digest {
            return Err(coordinator_invalid(
                "The confirmed Capability payload restore plan differs from its payload.",
            ));
        }

        let _maintenance = StateMaintenanceLock::new(self.state_root())
            .acquire_exclusive()
            .await?;

        // Validate both source sets and both clean targets before either owner
        // can publish payload bytes. This is the key cross-owner invariant; a
        // bad signed snapshot or occupied second target cannot leave a newly
        // published first owner behind.
        self.catalog_store
            .validate_clean_restore_source_under_maintenance(
                &plan.catalog_plan,
                catalogs,
                &plan.catalog_plan_digest,
            )?;
        self.descriptor_snapshot_store
            .validate_clean_restore_source_under_maintenance(
                &plan.descriptor_snapshot_plan,
                snapshots,
                &plan.descriptor_snapshot_plan_digest,
                &verification,
            )?;
        self.catalog_store
            .ensure_clean_restore_target_under_maintenance(
                &plan.catalog_plan,
                &plan.catalog_plan_digest,
            )
            .await?;
        self.descriptor_snapshot_store
            .ensure_clean_restore_target_under_maintenance(
                &plan.descriptor_snapshot_plan,
                &plan.descriptor_snapshot_plan_digest,
            )
            .await?;

        let catalog = self
            .catalog_store
            .apply_clean_restore_under_maintenance(
                &plan.catalog_plan,
                catalogs,
                &plan.catalog_plan_digest,
            )
            .await?;
        let descriptor_snapshot = self
            .descriptor_snapshot_store
            .apply_clean_restore_under_maintenance(
                &plan.descriptor_snapshot_plan,
                snapshots,
                &plan.descriptor_snapshot_plan_digest,
                verification,
            )
            .await?;
        let result = ControlCapabilityPayloadRestoreResult {
            schema: CONTROL_CAPABILITY_PAYLOAD_RESTORE_RESULT_SCHEMA.to_owned(),
            installation: self.installation().clone(),
            plan_digest: expected_plan_digest.to_owned(),
            changed: catalog.changed || descriptor_snapshot.changed,
            catalog,
            descriptor_snapshot,
        };
        result.validate(plan, expected_plan_digest)?;
        Ok(result)
    }
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        coordinator_invalid(format!(
            "Failed to encode the Capability payload restore plan: {error}"
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
