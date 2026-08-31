use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    knowledge_inventory_digest, ControlKnowledgePayloadSnapshot, ControlKnowledgePayloadState,
    VerifiedControlKnowledgePayloadSnapshot,
};
use crate::control_store::model::valid_sha256;
use crate::control_store::payload_owner::{
    canonical_json, ControlPayloadOwnerRegistry, ControlPayloadSnapshotBinding,
};
use crate::okf_knowledge::{
    OkfKnowledgeBackupManifest, OkfKnowledgeBinding, OkfKnowledgeStoragePolicy,
    SqliteOkfKnowledgeAdapter,
};

const RESTORE_RESULT_SCHEMA: &str = "a3s.use.control-knowledge-payload-restore-result.v1";
const RESTORE_RESULT_DOMAIN: &[u8] = b"a3s.use.control-knowledge-payload-restore-result.v1\0";
const CANDIDATE_FILE: &str = "knowledge.sqlite3";
const PARTIAL_FILE: &str = "knowledge.sqlite3.partial";
const MAX_RESULT_BYTES: usize = 128 * 1024;

mod filesystem;

use filesystem::{
    activate_candidate, ensure_owned_directory, inspect_live_payload_layout, optional_regular_file,
    stage_database, validate_staging_entries, LiveKnowledgePayloadLayout,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlKnowledgePayloadRestoreState {
    Absent,
    Database {
        database_bytes: u64,
        database_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlKnowledgePayloadRestoreResult {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    inventory_digest: String,
    pub(in crate::control_store) payload: ControlKnowledgePayloadRestoreState,
    descriptor_digest: String,
}

impl ControlKnowledgePayloadRestoreResult {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlKnowledgePayloadSnapshot,
    ) -> UseResult<Self> {
        let payload = match &snapshot.manifest.payload {
            ControlKnowledgePayloadState::Absent => ControlKnowledgePayloadRestoreState::Absent,
            ControlKnowledgePayloadState::Archive { backup, .. } => {
                ControlKnowledgePayloadRestoreState::Database {
                    database_bytes: backup.database_bytes,
                    database_sha256: backup.database_sha256.clone(),
                }
            }
        };
        let mut result = Self {
            schema: RESTORE_RESULT_SCHEMA.to_owned(),
            binding: snapshot.manifest.binding.clone(),
            owner_manifest_digest: snapshot.manifest.descriptor_digest.clone(),
            inventory_digest: snapshot.manifest.inventory_digest.clone(),
            payload,
            descriptor_digest: String::new(),
        };
        result.descriptor_digest = result.expected_digest()?;
        result.validate_for_snapshot(registry, snapshot)?;
        Ok(result)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        self.binding.validate(registry)?;
        let payload_valid = match &self.payload {
            ControlKnowledgePayloadRestoreState::Absent => true,
            ControlKnowledgePayloadRestoreState::Database {
                database_bytes,
                database_sha256,
            } => {
                *database_bytes > 0
                    && *database_bytes <= crate::okf_knowledge::MAX_BACKUP_DATABASE_BYTES
                    && valid_sha256(database_sha256)
            }
        };
        if self.schema != RESTORE_RESULT_SCHEMA
            || !valid_sha256(&self.owner_manifest_digest)
            || !valid_sha256(&self.inventory_digest)
            || !payload_valid
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_invalid(
                "The Control Knowledge restore result is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn validate_for_snapshot(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlKnowledgePayloadSnapshot,
    ) -> UseResult<()> {
        self.validate(registry)?;
        snapshot.validate(registry, &snapshot.manifest.binding)?;
        let payload_matches = match (&self.payload, &snapshot.manifest.payload) {
            (ControlKnowledgePayloadRestoreState::Absent, ControlKnowledgePayloadState::Absent) => {
                true
            }
            (
                ControlKnowledgePayloadRestoreState::Database {
                    database_bytes,
                    database_sha256,
                },
                ControlKnowledgePayloadState::Archive { backup, .. },
            ) => {
                *database_bytes == backup.database_bytes
                    && database_sha256 == &backup.database_sha256
            }
            _ => false,
        };
        if self.binding != snapshot.manifest.binding
            || self.owner_manifest_digest != snapshot.manifest.descriptor_digest
            || self.inventory_digest != snapshot.manifest.inventory_digest
            || !payload_matches
        {
            return Err(restore_invalid(
                "The Control Knowledge restore result differs from its exact owner snapshot.",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            owner_manifest_digest: &'a str,
            inventory_digest: &'a str,
            payload: &'a ControlKnowledgePayloadRestoreState,
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            owner_manifest_digest: &self.owner_manifest_digest,
            inventory_digest: &self.inventory_digest,
            payload: &self.payload,
        })
        .map_err(|error| restore_invalid(format!("Failed to encode restore result: {error}")))?;
        if bytes.is_empty() || bytes.len() > MAX_RESULT_BYTES {
            return Err(restore_invalid(
                "The Control Knowledge restore result exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(RESTORE_RESULT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlKnowledgePayloadRestore {
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlKnowledgePayloadSnapshot,
    bindings: Vec<OkfKnowledgeBinding>,
    selected: Vec<(a3s_use_core::PlanQualifiedSurfaceRef, u64)>,
    state_root: PathBuf,
    adapter: SqliteOkfKnowledgeAdapter,
    staging_directory: PathBuf,
    candidate: Option<PathBuf>,
}

impl VerifiedControlKnowledgePayloadSnapshot {
    pub(in crate::control_store) async fn stage_clean_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        policy: OkfKnowledgeStoragePolicy,
    ) -> UseResult<StagedControlKnowledgePayloadRestore> {
        self.validate_restore_policy(policy)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        let adapter = self.restore_adapter(&state_root, &staging_directory, policy)?;
        let _maintenance = StateMaintenanceLock::new(&state_root)
            .acquire_shared()
            .await
            .map_err(wrap_restore_error)?;
        self.stage_clean_restore_inner(state_root, staging_directory, policy, adapter)
            .await
    }

    pub(in crate::control_store) async fn stage_clean_restore_under_exclusive(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        policy: OkfKnowledgeStoragePolicy,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlKnowledgePayloadRestore> {
        self.validate_restore_policy(policy)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        let adapter = self.restore_adapter(&state_root, &staging_directory, policy)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Control Knowledge staging requires the exact target's exclusive maintenance guard.",
            ));
        }
        self.stage_clean_restore_inner(state_root, staging_directory, policy, adapter)
            .await
    }

    pub(in crate::control_store) fn validate_restore_policy(
        &self,
        policy: OkfKnowledgeStoragePolicy,
    ) -> UseResult<()> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if let Some(backup) = &self.backup {
            if backup.manifest.validate().map_err(wrap_restore_error)? != policy {
                return Err(restore_invalid(
                    "The Knowledge backup storage policy differs from the restore target policy.",
                ));
            }
        }
        Ok(())
    }

    fn restore_adapter(
        &self,
        state_root: &Path,
        staging_directory: &Path,
        policy: OkfKnowledgeStoragePolicy,
    ) -> UseResult<SqliteOkfKnowledgeAdapter> {
        let adapter = SqliteOkfKnowledgeAdapter::with_policy(
            state_root.to_path_buf(),
            self.snapshot.manifest.binding.installation.clone(),
            policy,
        )?;
        let live_payload_root = adapter.root().parent().ok_or_else(|| {
            restore_invalid("The configured Knowledge payload root has no state-owned parent.")
        })?;
        if staging_directory == state_root
            || !staging_directory.starts_with(state_root)
            || staging_directory.starts_with(live_payload_root)
        {
            return Err(restore_invalid(
                "The Control Knowledge restore candidate is outside its staging boundary or inside the live payload root.",
            ));
        }
        Ok(adapter)
    }

    async fn stage_clean_restore_inner(
        &self,
        state_root: PathBuf,
        staging_directory: PathBuf,
        policy: OkfKnowledgeStoragePolicy,
        adapter: SqliteOkfKnowledgeAdapter,
    ) -> UseResult<StagedControlKnowledgePayloadRestore> {
        ensure_owned_directory(&state_root, &staging_directory).await?;
        validate_staging_entries(&staging_directory).await?;

        let candidate = match &self.backup {
            None => {
                if !matches!(
                    self.snapshot.manifest.payload,
                    ControlKnowledgePayloadState::Absent
                ) {
                    return Err(restore_invalid(
                        "The verified Knowledge payload omitted its required database.",
                    ));
                }
                if optional_regular_file(&staging_directory.join(CANDIDATE_FILE)).await?
                    || optional_regular_file(&staging_directory.join(PARTIAL_FILE)).await?
                {
                    return Err(restore_invalid(
                        "An absent Knowledge payload has unexpected staged database bytes.",
                    ));
                }
                None
            }
            Some(backup) => {
                if backup.manifest.validate().map_err(wrap_restore_error)? != policy {
                    return Err(restore_invalid(
                        "The Knowledge backup storage policy differs from the restore target policy.",
                    ));
                }
                let candidate = staging_directory.join(CANDIDATE_FILE);
                stage_database(
                    &adapter,
                    &backup.database_path,
                    &candidate,
                    &backup.manifest,
                )
                .await?;
                validate_inventory(
                    &adapter,
                    &candidate,
                    &backup.manifest,
                    &backup.bindings,
                    &backup.selected,
                )
                .await?;
                Some(candidate)
            }
        };
        validate_staging_entries(&staging_directory).await?;
        Ok(StagedControlKnowledgePayloadRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            bindings: self.bindings().to_vec(),
            selected: self.selected().to_vec(),
            state_root,
            adapter,
            staging_directory,
            candidate,
        })
    }
}

impl StagedControlKnowledgePayloadRestore {
    pub(in crate::control_store) fn candidate_path(&self) -> Option<&Path> {
        self.candidate.as_deref()
    }

    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlKnowledgePayloadRestoreResult> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Control Knowledge activation requires the exact target's exclusive maintenance guard.",
            ));
        }
        validate_staging_entries(&self.staging_directory).await?;
        let staged_candidate = self.staging_directory.join(CANDIDATE_FILE);
        let staged_partial = self.staging_directory.join(PARTIAL_FILE);
        let candidate_exists = optional_regular_file(&staged_candidate).await?;
        if optional_regular_file(&staged_partial).await?
            || self.candidate.is_none() && candidate_exists
            || self
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate != &staged_candidate)
        {
            return Err(restore_invalid(
                "The Knowledge restore staging state differs from its verified target.",
            ));
        }
        let scope = &self.snapshot.manifest.binding.installation;
        let live = inspect_live_payload_layout(&self.adapter, scope).await?;

        match (
            &self.snapshot.manifest.payload,
            &self.candidate,
            candidate_exists,
            live,
        ) {
            (
                ControlKnowledgePayloadState::Absent,
                None,
                false,
                LiveKnowledgePayloadLayout::Absent,
            ) => {}
            (ControlKnowledgePayloadState::Absent, None, false, _) => {
                return Err(restore_target_not_empty())
            }
            (
                ControlKnowledgePayloadState::Archive { backup, .. },
                Some(candidate),
                true,
                LiveKnowledgePayloadLayout::Absent | LiveKnowledgePayloadLayout::Empty,
            ) => {
                validate_inventory(
                    &self.adapter,
                    candidate,
                    backup,
                    &self.bindings,
                    &self.selected,
                )
                .await?;
                let guard = self
                    .adapter
                    .restore_database_guard(scope)
                    .await
                    .map_err(wrap_restore_error)?;
                if optional_regular_file(guard.path()).await? {
                    return Err(restore_target_not_empty());
                }
                activate_candidate(candidate, guard.path(), &self.staging_directory).await?;
                validate_inventory(
                    &self.adapter,
                    guard.path(),
                    backup,
                    &self.bindings,
                    &self.selected,
                )
                .await?;
                if !matches!(
                    inspect_live_payload_layout(&self.adapter, scope).await?,
                    LiveKnowledgePayloadLayout::Database(path) if path == guard.path()
                ) {
                    return Err(restore_invalid(
                        "The activated Knowledge payload layout differs from its exact target.",
                    ));
                }
            }
            (
                ControlKnowledgePayloadState::Archive { backup, .. },
                Some(_),
                false,
                LiveKnowledgePayloadLayout::Database(live),
            ) => {
                validate_inventory(&self.adapter, &live, backup, &self.bindings, &self.selected)
                    .await
                    .map_err(|_| restore_target_not_empty())?;
            }
            (
                ControlKnowledgePayloadState::Archive { .. },
                Some(_),
                true,
                LiveKnowledgePayloadLayout::Database(_),
            ) => return Err(restore_target_not_empty()),
            _ => {
                return Err(restore_invalid(
                    "The staged Knowledge payload does not match its snapshot target.",
                ))
            }
        }
        ControlKnowledgePayloadRestoreResult::new(&self.registry, &self.snapshot)
    }
}

async fn validate_inventory(
    adapter: &SqliteOkfKnowledgeAdapter,
    path: &Path,
    manifest: &OkfKnowledgeBackupManifest,
    bindings: &[OkfKnowledgeBinding],
    selected: &[(a3s_use_core::PlanQualifiedSurfaceRef, u64)],
) -> UseResult<()> {
    let inventory = adapter
        .inspect_staged_restore_database(path, manifest)
        .await
        .map_err(wrap_restore_error)?;
    if inventory.bindings != bindings
        || inventory.selected != selected
        || knowledge_inventory_digest(&manifest.scope, bindings, selected)?
            != knowledge_inventory_digest(
                &manifest.scope,
                &inventory.bindings,
                &inventory.selected,
            )?
    {
        return Err(restore_invalid(
            "The staged Knowledge restore inventory differs from its verified snapshot.",
        ));
    }
    Ok(())
}

fn wrap_restore_error(error: UseError) -> UseError {
    restore_invalid(format!(
        "Knowledge restore verification failed: {}",
        error.message
    ))
}

fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.knowledge_payload_restore_target_not_empty",
        "The clean-target Knowledge restore refuses to replace existing payload state.",
    )
}

fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.knowledge_payload_restore_invalid",
        message,
    )
}

fn restore_io(action: &str, error: io::Error) -> UseError {
    restore_invalid(format!("Failed to {action}: {error}"))
}
