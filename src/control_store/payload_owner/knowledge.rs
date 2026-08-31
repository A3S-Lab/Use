use std::path::PathBuf;

use a3s_use_core::{PlanQualifiedSurfaceRef, UseError, UseResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerLimits, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding, ControlPayloadSnapshotEvidence, ControlPayloadSnapshotReceipt,
    ControlPayloadSnapshotSession,
};
use crate::control_store::model::valid_sha256;
use crate::okf_knowledge::{
    OkfKnowledgeBackupManifest, OkfKnowledgeBinding, OkfKnowledgeStoragePolicy,
    SqliteOkfKnowledgeAdapter, VerifiedOkfKnowledgeBackup,
};

mod inventory;

use inventory::knowledge_inventory_digest;

pub(in crate::control_store) const CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-knowledge-payload-snapshot.v1";
const CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_DOMAIN: &[u8] =
    b"a3s.use.control-knowledge-payload-snapshot.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlKnowledgePayloadState {
    Absent,
    Archive {
        backup: Box<OkfKnowledgeBackupManifest>,
        archive_bytes: u64,
        archive_sha256: String,
    },
}

/// Path-free manifest for the scope-local OKF SQLite payload belonging to one
/// exact Control export. The archive path remains caller-owned and is never
/// authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlKnowledgePayloadSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) payload: ControlKnowledgePayloadState,
    pub(in crate::control_store) retained_bindings: u64,
    pub(in crate::control_store) selected_surfaces: u64,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlKnowledgePayloadSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        created_at_ms: u64,
        payload: ControlKnowledgePayloadState,
        retained_bindings: u64,
        selected_surfaces: u64,
        inventory_digest: String,
    ) -> UseResult<Self> {
        let mut manifest = Self {
            schema: CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA.to_string(),
            binding,
            created_at_ms,
            payload,
            retained_bindings,
            selected_surfaces,
            inventory_digest,
            descriptor_digest: String::new(),
        };
        manifest.descriptor_digest = manifest.expected_descriptor_digest()?;
        manifest.validate(registry, &manifest.binding)?;
        Ok(manifest)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        let limits = knowledge_contract(registry)?;
        self.binding.validate(registry)?;
        if self.schema != CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA
            || &self.binding != expected_binding
            || self.created_at_ms == 0
            || !valid_sha256(&self.inventory_digest)
            || !valid_sha256(&self.descriptor_digest)
        {
            return Err(knowledge_error(
                "The Control Knowledge payload manifest is invalid or was rebound.",
            ));
        }
        match &self.payload {
            ControlKnowledgePayloadState::Absent => {
                if self.retained_bindings != 0
                    || self.selected_surfaces != 0
                    || self.inventory_digest
                        != knowledge_inventory_digest(&self.binding.installation, &[], &[])?
                {
                    return Err(knowledge_error(
                        "An absent Control Knowledge payload carries non-empty inventory evidence.",
                    ));
                }
            }
            ControlKnowledgePayloadState::Archive {
                backup,
                archive_bytes,
                archive_sha256,
            } => {
                backup.validate().map_err(wrap_knowledge_error)?;
                let expected_bindings = backup
                    .storage
                    .retained_projections
                    .checked_add(backup.storage.removed_tombstones)
                    .and_then(|count| u64::try_from(count).ok())
                    .ok_or_else(|| {
                        knowledge_error("The Control Knowledge binding count overflowed.")
                    })?;
                if backup.scope != self.binding.installation
                    || backup.created_at_ms != self.created_at_ms
                    || *archive_bytes == 0
                    || *archive_bytes > limits.max_payload_bytes
                    || !valid_sha256(archive_sha256)
                    || self.retained_bindings != expected_bindings
                    || self.selected_surfaces > self.retained_bindings
                {
                    return Err(knowledge_error(
                        "The Control Knowledge archive evidence is inconsistent or out of bounds.",
                    ));
                }
            }
        }
        if self.expected_descriptor_digest()? != self.descriptor_digest {
            return Err(knowledge_error(
                "The Control Knowledge payload manifest is invalid or was rebound.",
            ));
        }
        let bytes = canonical_json(self).map_err(|error| {
            knowledge_error(format!(
                "Failed to encode the canonical Control Knowledge payload manifest: {error}"
            ))
        })?;
        if bytes.is_empty()
            || u64::try_from(bytes.len())
                .ok()
                .is_none_or(|length| length > limits.max_manifest_bytes)
        {
            return Err(knowledge_error(
                "The Control Knowledge payload manifest exceeds its registered byte bound.",
            ));
        }
        Ok(())
    }

    fn canonical_bytes(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<Vec<u8>> {
        self.validate(registry, expected_binding)?;
        canonical_json(self).map_err(|error| {
            knowledge_error(format!(
                "Failed to encode the canonical Control Knowledge payload manifest: {error}"
            ))
        })
    }

    fn expected_descriptor_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            created_at_ms: u64,
            payload: &'a ControlKnowledgePayloadState,
            retained_bindings: u64,
            selected_surfaces: u64,
            inventory_digest: &'a str,
        }

        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            created_at_ms: self.created_at_ms,
            payload: &self.payload,
            retained_bindings: self.retained_bindings,
            selected_surfaces: self.selected_surfaces,
            inventory_digest: &self.inventory_digest,
        })
        .map_err(|error| {
            knowledge_error(format!(
                "Failed to encode the Control Knowledge payload descriptor: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlKnowledgePayloadSnapshot {
    pub(in crate::control_store) manifest: ControlKnowledgePayloadSnapshotManifest,
    pub(in crate::control_store) receipt: ControlPayloadSnapshotReceipt,
}

impl ControlKnowledgePayloadSnapshot {
    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        self.manifest.validate(registry, expected_binding)?;
        self.receipt.validate(registry, expected_binding)?;
        let manifest_bytes = self.manifest.canonical_bytes(registry, expected_binding)?;
        let manifest_bytes = u64::try_from(manifest_bytes.len()).map_err(|_| {
            knowledge_error("The Control Knowledge manifest byte count overflowed.")
        })?;
        let (file_count, byte_count) = match &self.manifest.payload {
            ControlKnowledgePayloadState::Absent => (0, 0),
            ControlKnowledgePayloadState::Archive { archive_bytes, .. } => (1, *archive_bytes),
        };
        if self.receipt.owner != ControlPayloadOwnerId::KnowledgePayload
            || self.receipt.owner_manifest_digest != self.manifest.descriptor_digest
            || self.receipt.inventory_digest != self.manifest.inventory_digest
            || self.receipt.manifest_bytes != manifest_bytes
            || self.receipt.file_count != file_count
            || self.receipt.byte_count != byte_count
        {
            return Err(knowledge_error(
                "The Control Knowledge payload receipt does not match its owner manifest.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) async fn verify_offline(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
        archive_path: Option<PathBuf>,
    ) -> UseResult<VerifiedControlKnowledgePayloadSnapshot> {
        self.validate(registry, expected_binding)?;
        let verified = match (&self.manifest.payload, archive_path) {
            (ControlKnowledgePayloadState::Absent, None) => None,
            (
                ControlKnowledgePayloadState::Archive {
                    backup,
                    archive_bytes,
                    archive_sha256,
                },
                Some(path),
            ) => {
                let limits = knowledge_contract(registry)?;
                let before = SqliteOkfKnowledgeAdapter::backup_archive_evidence(
                    path.clone(),
                    limits.max_payload_bytes,
                )
                .await
                .map_err(wrap_knowledge_error)?;
                if before.0 != *archive_bytes || before.1 != *archive_sha256 {
                    return Err(knowledge_error(
                        "The Control Knowledge archive differs from its manifest evidence.",
                    ));
                }
                let inspected = SqliteOkfKnowledgeAdapter::inspect_backup_for_restore(
                    path.clone(),
                    &expected_binding.installation,
                )
                .await
                .map_err(wrap_knowledge_error)?;
                let after = SqliteOkfKnowledgeAdapter::backup_archive_evidence(
                    path,
                    limits.max_payload_bytes,
                )
                .await
                .map_err(wrap_knowledge_error)?;
                if before != after || inspected.manifest != **backup {
                    return Err(knowledge_error(
                        "The Control Knowledge archive changed during offline verification.",
                    ));
                }
                validate_verified_inventory(&self.manifest, &inspected)?;
                Some(inspected)
            }
            _ => {
                return Err(knowledge_error(
                    "The Control Knowledge archive presence does not match its manifest.",
                ))
            }
        };
        Ok(VerifiedControlKnowledgePayloadSnapshot { backup: verified })
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlKnowledgePayloadSnapshot {
    backup: Option<VerifiedOkfKnowledgeBackup>,
}

impl VerifiedControlKnowledgePayloadSnapshot {
    pub(in crate::control_store) fn bindings(&self) -> &[OkfKnowledgeBinding] {
        self.backup
            .as_ref()
            .map_or(&[], |backup| backup.bindings.as_slice())
    }

    pub(in crate::control_store) fn selected(&self) -> &[(PlanQualifiedSurfaceRef, u64)] {
        self.backup
            .as_ref()
            .map_or(&[], |backup| backup.selected.as_slice())
    }
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_knowledge(
        &self,
        policy: OkfKnowledgeStoragePolicy,
        destination: PathBuf,
        created_at_ms: u64,
    ) -> UseResult<ControlKnowledgePayloadSnapshot> {
        let limits = knowledge_contract(self.registry())?;
        let adapter = SqliteOkfKnowledgeAdapter::with_policy(
            self.state_root().to_path_buf(),
            self.binding().installation.clone(),
            policy,
        )?;
        let backup = adapter
            .backup_if_present_under_maintenance(
                self.maintenance(),
                &self.binding().installation,
                destination.clone(),
                created_at_ms,
                limits.max_payload_bytes,
            )
            .await?;
        let (payload, bindings, selected) = match backup {
            Some(backup) => {
                let before = SqliteOkfKnowledgeAdapter::backup_archive_evidence(
                    destination.clone(),
                    limits.max_payload_bytes,
                )
                .await?;
                let verified = SqliteOkfKnowledgeAdapter::inspect_backup_for_restore(
                    destination.clone(),
                    &self.binding().installation,
                )
                .await?;
                let after = SqliteOkfKnowledgeAdapter::backup_archive_evidence(
                    destination,
                    limits.max_payload_bytes,
                )
                .await?;
                if before != after || verified.manifest != backup {
                    return Err(knowledge_error(
                        "The new Control Knowledge archive changed before it could be registered.",
                    ));
                }
                let payload = ControlKnowledgePayloadState::Archive {
                    backup: Box::new(backup),
                    archive_bytes: before.0,
                    archive_sha256: before.1,
                };
                (payload, verified.bindings, verified.selected)
            }
            None => (ControlKnowledgePayloadState::Absent, Vec::new(), Vec::new()),
        };
        let inventory_digest =
            knowledge_inventory_digest(&self.binding().installation, &bindings, &selected)?;
        let retained_bindings = u64::try_from(bindings.len())
            .map_err(|_| knowledge_error("The Control Knowledge binding count overflowed."))?;
        let selected_surfaces = u64::try_from(selected.len())
            .map_err(|_| knowledge_error("The Control Knowledge selection count overflowed."))?;
        let manifest = ControlKnowledgePayloadSnapshotManifest::new(
            self.registry(),
            self.binding().clone(),
            created_at_ms,
            payload,
            retained_bindings,
            selected_surfaces,
            inventory_digest.clone(),
        )?;
        let manifest_bytes = manifest.canonical_bytes(self.registry(), self.binding())?;
        let (file_count, byte_count) = match &manifest.payload {
            ControlKnowledgePayloadState::Absent => (0, 0),
            ControlKnowledgePayloadState::Archive { archive_bytes, .. } => (1, *archive_bytes),
        };
        let receipt = self.receipt(
            ControlPayloadOwnerId::KnowledgePayload,
            ControlPayloadSnapshotEvidence::new(
                manifest.descriptor_digest.clone(),
                inventory_digest,
                u64::try_from(manifest_bytes.len()).map_err(|_| {
                    knowledge_error("The Control Knowledge manifest byte count overflowed.")
                })?,
                file_count,
                byte_count,
            ),
        )?;
        let snapshot = ControlKnowledgePayloadSnapshot { manifest, receipt };
        snapshot.validate(self.registry(), self.binding())?;
        Ok(snapshot)
    }
}

fn validate_verified_inventory(
    manifest: &ControlKnowledgePayloadSnapshotManifest,
    verified: &VerifiedOkfKnowledgeBackup,
) -> UseResult<()> {
    let retained_bindings = u64::try_from(verified.bindings.len())
        .map_err(|_| knowledge_error("The verified Knowledge binding count overflowed."))?;
    let selected_surfaces = u64::try_from(verified.selected.len())
        .map_err(|_| knowledge_error("The verified Knowledge selection count overflowed."))?;
    let inventory_digest = knowledge_inventory_digest(
        &manifest.binding.installation,
        &verified.bindings,
        &verified.selected,
    )?;
    if retained_bindings != manifest.retained_bindings
        || selected_surfaces != manifest.selected_surfaces
        || inventory_digest != manifest.inventory_digest
    {
        return Err(knowledge_error(
            "The verified Knowledge inventory differs from its Control owner manifest.",
        ));
    }
    Ok(())
}

fn knowledge_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::KnowledgePayload)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(knowledge_error(
            "The Control Knowledge payload owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA {
        return Err(knowledge_error(
            "The Control Knowledge payload owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

fn wrap_knowledge_error(error: UseError) -> UseError {
    knowledge_error(format!(
        "Control Knowledge payload verification failed: {}",
        error.message
    ))
}

fn knowledge_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.knowledge_payload_snapshot_invalid",
        message,
    )
}
