//! Inactive, path-free contracts for payloads that intentionally remain
//! outside the Control Store transaction.
//!
//! This module registers identities, fixed backup policies, safety bounds,
//! and canonical snapshot evidence. The Knowledge owner has a qualified
//! snapshot, offline verifier, and clean-target staged restore/activation. The
//! planning-and-diagnostic owner has the same qualified boundary for terminal
//! records. No owner participates in the production state-backup path yet. The
//! remaining adapters and complete-set coordinator must be complete before the
//! coordinated authority cutover.

use a3s_use_core::{UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};

mod knowledge;
mod observations;
mod registry;
mod session;
mod snapshot;

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
    HostProtocolProjection,
    KnowledgePayload,
    PlanningAndDiagnosticObservations,
    RestoreCoordinator,
}

impl ControlPayloadOwnerId {
    pub(in crate::control_store) const ALL: [Self; 5] = [
        Self::ArtifactStore,
        Self::HostProtocolProjection,
        Self::KnowledgePayload,
        Self::PlanningAndDiagnosticObservations,
        Self::RestoreCoordinator,
    ];

    pub(in crate::control_store) const SNAPSHOTTED: [Self; 4] = [
        Self::HostProtocolProjection,
        Self::KnowledgePayload,
        Self::PlanningAndDiagnosticObservations,
        Self::RestoreCoordinator,
    ];

    pub(in crate::control_store) const fn as_str(self) -> &'static str {
        match self {
            Self::ArtifactStore => "artifact-store",
            Self::HostProtocolProjection => "host-protocol-projection",
            Self::KnowledgePayload => "knowledge-payload",
            Self::PlanningAndDiagnosticObservations => "planning-and-diagnostic-observations",
            Self::RestoreCoordinator => "restore-coordinator",
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
            Self::HostProtocolProjection => ControlPayloadBackupPolicy::RegisteredProjection,
            Self::KnowledgePayload => ControlPayloadBackupPolicy::OwnerSnapshot,
            Self::PlanningAndDiagnosticObservations => {
                ControlPayloadBackupPolicy::RegisteredTerminalSnapshot
            }
            Self::RestoreCoordinator => ControlPayloadBackupPolicy::ExcludeActiveRegisterTerminal,
        }
    }
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
