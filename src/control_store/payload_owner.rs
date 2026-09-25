//! Inactive, path-free contracts for payloads that intentionally remain
//! outside the Control Store transaction.
//!
//! This module registers identities, fixed backup policies, safety bounds,
//! and canonical snapshot evidence. Coordinated Control-backed
//! `state_backup` inventory admits only the Control export leaf plus these
//! registered owner live locations (see
//! [`backup_admits_control_installation_path`]). Owner-native complete-set
//! snapshot/restore remains the stronger portable archive path; production
//! restore wiring continues to converge on that registry.

use a3s_use_core::{UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};

mod capability_payload;
mod complete_set;
mod host_projection;
mod knowledge;
mod observations;
mod registry;
mod restore_coordinator;
mod runtime_plans;
mod session;
mod snapshot;

#[cfg(test)]
pub(in crate::control_store) use capability_payload::{
    ControlCapabilityPayloadEntry, ControlCapabilityPayloadEntryKind,
    ControlCapabilityPayloadRestoreResult, ControlCapabilityPayloadRestoreState,
    ControlCapabilityPayloadSnapshot, ControlCapabilityPayloadState,
    VerifiedControlCapabilityPayloadSnapshot, CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA,
};
pub(in crate::control_store) use complete_set::validate_terminal_receipt_blocking;
#[cfg(test)]
pub(in crate::control_store) use complete_set::{
    ControlInstallationSnapshotManifest, ControlStoreRestoreResult,
    StagedControlInstallationRestore, VerifiedControlInstallationSnapshot,
};
#[cfg(test)]
pub(in crate::control_store) use host_projection::{
    ControlHostProjectionEntryKind, ControlHostProjectionRestoreResult,
    ControlHostProjectionRestoreState, ControlHostProjectionSnapshot, ControlHostProjectionState,
    StagedControlHostProjectionRestore, VerifiedControlHostProjectionSnapshot,
    CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA,
};
#[cfg(test)]
pub(in crate::control_store) use knowledge::{
    ControlKnowledgePayloadRestoreResult, ControlKnowledgePayloadRestoreState,
    ControlKnowledgePayloadSnapshot, ControlKnowledgePayloadState,
    VerifiedControlKnowledgePayloadSnapshot, CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA,
};
#[cfg(test)]
pub(in crate::control_store) use observations::{
    ControlObservationPayloadEntryKind, ControlObservationPayloadRestoreResult,
    ControlObservationPayloadRestoreState, ControlObservationPayloadSnapshot,
    ControlObservationPayloadState, StagedControlObservationPayloadRestore,
    VerifiedControlObservationPayloadSnapshot, CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA,
};
pub(in crate::control_store) use registry::ControlPayloadOwnerRegistry;
#[cfg(test)]
pub(in crate::control_store) use restore_coordinator::{
    ControlRestoreCoordinatorRestoreResult, ControlRestoreCoordinatorRestoreState,
    ControlRestoreCoordinatorSnapshot, ControlRestoreCoordinatorState,
    StagedControlRestoreCoordinatorRestore, VerifiedControlRestoreCoordinatorSnapshot,
    CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA,
};
#[cfg(test)]
pub(in crate::control_store) use runtime_plans::{
    ControlRuntimePlanPayloadEntry, ControlRuntimePlanPayloadRestoreResult,
    ControlRuntimePlanPayloadRestoreState, ControlRuntimePlanPayloadSnapshot,
    ControlRuntimePlanPayloadState, VerifiedControlRuntimePlanPayloadSnapshot,
    CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA,
};
pub(in crate::control_store) use session::{
    ControlPayloadSnapshotBinding, ControlPayloadSnapshotSession,
};
#[cfg(test)]
pub(in crate::control_store) use snapshot::CONTROL_PAYLOAD_SNAPSHOT_RECEIPT_SCHEMA;
pub(in crate::control_store) use snapshot::{
    ControlPayloadSnapshotEvidence, ControlPayloadSnapshotReceipt, ControlPayloadSnapshotSet,
};

const MAX_CONTROL_PAYLOAD_OWNER_FILES: u64 = 100_000;
const MAX_CONTROL_PAYLOAD_OWNER_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_CONTROL_PAYLOAD_OWNER_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CONTROL_PAYLOAD_SCHEMA_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlPayloadOwnerId {
    ArtifactStore,
    CapabilityPayload,
    HostProtocolProjection,
    KnowledgePayload,
    PlanningAndDiagnosticObservations,
    RestoreCoordinator,
    RuntimePlanPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlPayloadLiveLocation {
    StateRoot(&'static str),
    OperationRoot(&'static str),
}

const ARTIFACT_STORE_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] = &[];
const CAPABILITY_PAYLOAD_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] =
    &[ControlPayloadLiveLocation::StateRoot("capability-gateway")];
const HOST_PROJECTION_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] =
    &[ControlPayloadLiveLocation::StateRoot("plugin-host-manager")];
const KNOWLEDGE_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] =
    &[ControlPayloadLiveLocation::StateRoot("knowledge")];
const OBSERVATION_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] = &[
    ControlPayloadLiveLocation::OperationRoot("package-diagnostic-history"),
    ControlPayloadLiveLocation::OperationRoot("package-downloads"),
    ControlPayloadLiveLocation::OperationRoot("package-resolutions"),
];
const RESTORE_COORDINATOR_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] =
    &[ControlPayloadLiveLocation::OperationRoot("state-restores")];
const RUNTIME_PLAN_LIVE_LOCATIONS: &[ControlPayloadLiveLocation] =
    &[ControlPayloadLiveLocation::StateRoot("runtime-plans")];

impl ControlPayloadOwnerId {
    pub(in crate::control_store) const ALL: [Self; 7] = [
        Self::ArtifactStore,
        Self::CapabilityPayload,
        Self::HostProtocolProjection,
        Self::KnowledgePayload,
        Self::PlanningAndDiagnosticObservations,
        Self::RestoreCoordinator,
        Self::RuntimePlanPayload,
    ];

    pub(in crate::control_store) const SNAPSHOTTED: [Self; 6] = [
        Self::CapabilityPayload,
        Self::HostProtocolProjection,
        Self::KnowledgePayload,
        Self::PlanningAndDiagnosticObservations,
        Self::RestoreCoordinator,
        Self::RuntimePlanPayload,
    ];

    pub(in crate::control_store) const fn as_str(self) -> &'static str {
        match self {
            Self::ArtifactStore => "artifact-store",
            Self::CapabilityPayload => "capability-payload",
            Self::HostProtocolProjection => "host-protocol-projection",
            Self::KnowledgePayload => "knowledge-payload",
            Self::PlanningAndDiagnosticObservations => "planning-and-diagnostic-observations",
            Self::RestoreCoordinator => "restore-coordinator",
            Self::RuntimePlanPayload => "runtime-plan-payload",
        }
    }

    pub(in crate::control_store) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
    }

    pub(in crate::control_store) const fn backup_policy(self) -> ControlPayloadBackupPolicy {
        match self {
            Self::ArtifactStore => ControlPayloadBackupPolicy::ExcludedGlobal,
            Self::CapabilityPayload => ControlPayloadBackupPolicy::OwnerSnapshot,
            Self::HostProtocolProjection => ControlPayloadBackupPolicy::RegisteredProjection,
            Self::KnowledgePayload => ControlPayloadBackupPolicy::OwnerSnapshot,
            Self::PlanningAndDiagnosticObservations => {
                ControlPayloadBackupPolicy::RegisteredTerminalSnapshot
            }
            Self::RestoreCoordinator => ControlPayloadBackupPolicy::ExcludeActiveRegisterTerminal,
            Self::RuntimePlanPayload => ControlPayloadBackupPolicy::OwnerSnapshot,
        }
    }

    fn live_locations(self) -> &'static [ControlPayloadLiveLocation] {
        match self {
            Self::ArtifactStore => ARTIFACT_STORE_LIVE_LOCATIONS,
            Self::CapabilityPayload => CAPABILITY_PAYLOAD_LIVE_LOCATIONS,
            Self::HostProtocolProjection => HOST_PROJECTION_LIVE_LOCATIONS,
            Self::KnowledgePayload => KNOWLEDGE_LIVE_LOCATIONS,
            Self::PlanningAndDiagnosticObservations => OBSERVATION_LIVE_LOCATIONS,
            Self::RestoreCoordinator => RESTORE_COORDINATOR_LIVE_LOCATIONS,
            Self::RuntimePlanPayload => RUNTIME_PLAN_LIVE_LOCATIONS,
        }
    }

    pub(in crate::control_store) fn owner_for_state_root(name: &str) -> Option<Self> {
        Self::SNAPSHOTTED.into_iter().find(|owner| {
            owner
                .live_locations()
                .iter()
                .any(|location| matches!(location, ControlPayloadLiveLocation::StateRoot(value) if *value == name))
        })
    }

    pub(in crate::control_store) fn owner_for_operation_root(name: &str) -> Option<Self> {
        Self::SNAPSHOTTED.into_iter().find(|owner| {
            owner
                .live_locations()
                .iter()
                .any(|location| matches!(location, ControlPayloadLiveLocation::OperationRoot(value) if *value == name))
        })
    }
}

/// Whether one installation-state path is admissible in a Control-backed
/// coordinated backup inventory.
///
/// Admits only the Control export leaf and registered external payload-owner
/// live locations. Operational locks, Control SQLite sidecars, and derived
/// indexes may appear so the scanner can skip them; they are not portable
/// inventory. Legacy layout families (`extensions/`, `grants/`, …) are not
/// admitted even when `installation_state_layout` still lists them.
pub(crate) fn backup_admits_control_installation_path(relative: &str, is_directory: bool) -> bool {
    use crate::installation_state_layout;
    use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;

    let mut parts = relative.split('/');
    let Some(first) = parts.next().filter(|value| !value.is_empty()) else {
        return false;
    };
    let nested = parts.next().is_some();
    if !nested {
        if !is_directory {
            return first == super::snapshot_read::CONTROL_STORE_EXPORT_BACKUP_PATH
                || first == "control.sqlite3"
                || installation_state_layout::excluded_operational_state_file(first)
                || installation_state_layout::excluded_root_lock(first)
                || first == ACTIVE_STATE_RESTORE_MARKER;
        }
        if installation_state_layout::excluded_derived_root(first) || first == "operations" {
            return true;
        }
        return ControlPayloadOwnerId::owner_for_state_root(first).is_some();
    }
    if first == "operations" {
        let Some(operation) = relative.split('/').nth(1) else {
            return false;
        };
        return ControlPayloadOwnerId::owner_for_operation_root(operation).is_some();
    }
    ControlPayloadOwnerId::owner_for_state_root(first).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlPayloadBackupPolicy {
    ExcludedGlobal,
    OwnerSnapshot,
    RegisteredTerminalSnapshot,
    RegisteredProjection,
    ExcludeActiveRegisterTerminal,
}

impl ControlPayloadBackupPolicy {
    pub(in crate::control_store) const fn as_str(self) -> &'static str {
        match self {
            Self::ExcludedGlobal => "excluded-global",
            Self::OwnerSnapshot => "owner-snapshot",
            Self::RegisteredTerminalSnapshot => "registered-terminal-snapshot",
            Self::RegisteredProjection => "registered-projection",
            Self::ExcludeActiveRegisterTerminal => "exclude-active-register-terminal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPayloadOwnerLimits {
    pub(in crate::control_store) max_files: u64,
    pub(in crate::control_store) max_payload_bytes: u64,
    pub(in crate::control_store) max_manifest_bytes: u64,
}

impl ControlPayloadOwnerLimits {
    pub(in crate::control_store) fn new(
        max_files: u64,
        max_payload_bytes: u64,
        max_manifest_bytes: u64,
    ) -> UseResult<Self> {
        let limits = Self {
            max_files,
            max_payload_bytes,
            max_manifest_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    fn validate(self) -> UseResult<()> {
        if self.max_files == 0
            || self.max_files > MAX_CONTROL_PAYLOAD_OWNER_FILES
            || self.max_payload_bytes == 0
            || self.max_payload_bytes > MAX_CONTROL_PAYLOAD_OWNER_BYTES
            || self.max_manifest_bytes == 0
            || self.max_manifest_bytes > MAX_CONTROL_PAYLOAD_OWNER_MANIFEST_BYTES
        {
            return Err(registry_error(
                "Control payload owner limits are empty or exceed the global safety bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "registrationKind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlPayloadOwnerRegistration {
    ExcludedGlobal {
        owner: ControlPayloadOwnerId,
    },
    Snapshotted {
        owner: ControlPayloadOwnerId,
        backup_policy: ControlPayloadBackupPolicy,
        owner_snapshot_schema: String,
        limits: ControlPayloadOwnerLimits,
    },
}

impl ControlPayloadOwnerRegistration {
    pub(in crate::control_store) fn excluded_global(
        owner: ControlPayloadOwnerId,
    ) -> UseResult<Self> {
        let registration = Self::ExcludedGlobal { owner };
        registration.validate()?;
        Ok(registration)
    }

    pub(in crate::control_store) fn snapshotted(
        owner: ControlPayloadOwnerId,
        owner_snapshot_schema: impl Into<String>,
        limits: ControlPayloadOwnerLimits,
    ) -> UseResult<Self> {
        let registration = Self::Snapshotted {
            owner,
            backup_policy: owner.backup_policy(),
            owner_snapshot_schema: owner_snapshot_schema.into(),
            limits,
        };
        registration.validate()?;
        Ok(registration)
    }

    pub(in crate::control_store) const fn owner(&self) -> ControlPayloadOwnerId {
        match self {
            Self::ExcludedGlobal { owner } | Self::Snapshotted { owner, .. } => *owner,
        }
    }

    pub(in crate::control_store) const fn backup_policy(&self) -> ControlPayloadBackupPolicy {
        match self {
            Self::ExcludedGlobal { .. } => ControlPayloadBackupPolicy::ExcludedGlobal,
            Self::Snapshotted { backup_policy, .. } => *backup_policy,
        }
    }

    fn snapshot_contract(&self) -> Option<(&str, ControlPayloadOwnerLimits)> {
        match self {
            Self::ExcludedGlobal { .. } => None,
            Self::Snapshotted {
                owner_snapshot_schema,
                limits,
                ..
            } => Some((owner_snapshot_schema, *limits)),
        }
    }

    fn validate(&self) -> UseResult<()> {
        if self.backup_policy() != self.owner().backup_policy() {
            return Err(registry_error(
                "A Control payload owner registration changed its fixed backup policy.",
            ));
        }
        match self {
            Self::ExcludedGlobal { owner } => {
                if *owner != ControlPayloadOwnerId::ArtifactStore {
                    return Err(registry_error(
                        "Only the global Artifact Store may omit an installation snapshot.",
                    ));
                }
            }
            Self::Snapshotted {
                owner,
                owner_snapshot_schema,
                limits,
                ..
            } => {
                if *owner == ControlPayloadOwnerId::ArtifactStore
                    || !valid_schema(owner_snapshot_schema)
                {
                    return Err(registry_error(
                        "A snapshotted Control payload owner has an invalid identity or schema.",
                    ));
                }
                limits.validate()?;
            }
        }
        Ok(())
    }
}

fn valid_schema(value: &str) -> bool {
    value.len() <= MAX_CONTROL_PAYLOAD_SCHEMA_BYTES
        && value.starts_with("a3s.use.")
        && !value.ends_with('.')
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn canonical_json<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer)?;
    Ok(bytes)
}

fn registry_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.payload_registry_invalid", message)
}
