use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;
use tokio::fs;

use super::super::{ControlPayloadOwnerRegistry, ControlPayloadSnapshotBinding};
use super::control_restore::StagedControlStoreRestore;
use super::control_restore_filesystem::{
    cleanup_quiescent_sidecars, hash_owned_file, optional_regular_file,
    optional_regular_file_length, publish_noclobber, read_exact_owned, require_no_sidecars,
    sync_directory, validate_directory, validate_entries, CANDIDATE_FILE, EVIDENCE_FILE,
    EVIDENCE_PARTIAL_FILE, MAX_EVIDENCE_BYTES,
};
use super::control_restore_result::ControlStoreRestoreResult;
use super::restore::{restore_staging_invalid, restore_staging_io};
use crate::control_store::executor::ControlStoreExecutor;
use crate::control_store::filesystem::CONTROL_STORE_DATABASE_FILE;

impl StagedControlStoreRestore {
    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlStoreRestoreResult> {
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_staging_invalid(
                "Control activation requires the exact target's exclusive maintenance guard.",
            ));
        }
        if self.candidate != self.staging_directory.join(CANDIDATE_FILE) {
            return Err(restore_staging_invalid(
                "The Control candidate path differs from its fixed staging location.",
            ));
        }
        validate_directory(&self.state_root).await?;
        validate_directory(&self.staging_directory).await?;
        validate_entries(&self.staging_directory).await?;
        validate_evidence(self).await?;
        require_no_sidecars(&self.staging_directory).await?;
        require_no_live_sidecars(&self.state_root).await?;

        let physical_root = fs::canonicalize(&self.state_root)
            .await
            .map_err(|error| restore_staging_io("resolve Control activation root", error))?;
        validate_directory(&physical_root).await?;
        let physical_staging = fs::canonicalize(&self.staging_directory)
            .await
            .map_err(|error| restore_staging_io("resolve Control activation staging", error))?;
        validate_directory(&physical_staging).await?;
        if physical_staging == physical_root || !physical_staging.starts_with(&physical_root) {
            return Err(restore_staging_invalid(
                "The Control activation staging directory escapes its state root.",
            ));
        }

        let live = self.state_root.join(CONTROL_STORE_DATABASE_FILE);
        let candidate_exists = optional_regular_file(&self.candidate).await?;
        let live_exists = optional_regular_file(&live).await?;
        match (candidate_exists, live_exists) {
            (true, false) => {
                let physical_candidate = physical_staging.join(CANDIDATE_FILE);
                validate_database(
                    &self.registry,
                    &self.snapshot_descriptor_digest,
                    &self.binding,
                    &self.evidence,
                    &physical_candidate,
                )
                .await?;
                publish_noclobber(
                    physical_candidate,
                    physical_root.join(CONTROL_STORE_DATABASE_FILE),
                )
                .await?;
                sync_directory(&physical_staging).await?;
                sync_directory(&physical_root).await?;
            }
            (false, true) => {}
            (true, true) => {
                return Err(restore_staging_invalid(
                    "The Control candidate and live database both exist.",
                ))
            }
            (false, false) => {
                return Err(restore_staging_invalid(
                    "The Control candidate and live database are both missing.",
                ))
            }
        }

        if optional_regular_file(&self.candidate).await? || !optional_regular_file(&live).await? {
            return Err(restore_staging_invalid(
                "The Control activation did not converge to one live database.",
            ));
        }
        validate_database(
            &self.registry,
            &self.snapshot_descriptor_digest,
            &self.binding,
            &self.evidence,
            &physical_root.join(CONTROL_STORE_DATABASE_FILE),
        )
        .await?;
        validate_evidence(self).await?;
        validate_entries(&self.staging_directory).await?;
        require_no_sidecars(&self.staging_directory).await?;
        require_no_live_sidecars(&self.state_root).await?;
        ControlStoreRestoreResult::new(
            &self.registry,
            &self.snapshot_descriptor_digest,
            &self.binding,
            &self.evidence,
        )
    }
}

async fn validate_evidence(staged: &StagedControlStoreRestore) -> UseResult<()> {
    let expected = staged.evidence.canonical_bytes(MAX_EVIDENCE_BYTES)?;
    let path = staged.staging_directory.join(EVIDENCE_FILE);
    let Some(length) = optional_regular_file_length(&path).await? else {
        return Err(restore_staging_invalid(
            "The Control activation has no durable candidate evidence.",
        ));
    };
    if optional_regular_file(&staged.staging_directory.join(EVIDENCE_PARTIAL_FILE)).await?
        || read_exact_owned(&path, length, MAX_EVIDENCE_BYTES).await? != expected
    {
        return Err(restore_staging_invalid(
            "The Control activation candidate evidence was changed or rebound.",
        ));
    }
    Ok(())
}

async fn validate_database(
    registry: &ControlPayloadOwnerRegistry,
    snapshot_descriptor_digest: &str,
    binding: &ControlPayloadSnapshotBinding,
    evidence: &super::control_restore_evidence::ControlCandidateEvidence,
    database: &Path,
) -> UseResult<()> {
    let executor = ControlStoreExecutor::new().map_err(|error| {
        restore_staging_invalid(format!(
            "Failed to start Control activation verifier: {}",
            error.message
        ))
    })?;
    let exported = executor
        .export(database.to_path_buf(), binding.installation.clone())
        .await
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The Control activation database failed semantic verification: {}",
                error.message
            ))
        })?
        .into_bytes();
    binding
        .verify_control_export(registry, &exported)
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The Control activation database differs from its bound export: {}",
                error.message
            ))
        })?;
    cleanup_quiescent_sidecars(database).await?;
    let (database_bytes, database_sha256) = hash_owned_file(database).await?;
    evidence.validate_exact(
        registry,
        snapshot_descriptor_digest,
        binding,
        database_bytes,
        &database_sha256,
    )
}

async fn require_no_live_sidecars(state_root: &Path) -> UseResult<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        if optional_regular_file(&state_root.join(format!("{CONTROL_STORE_DATABASE_FILE}{suffix}")))
            .await?
        {
            return Err(restore_staging_invalid(
                "The live Control target retained an operational SQLite sidecar.",
            ));
        }
    }
    Ok(())
}
