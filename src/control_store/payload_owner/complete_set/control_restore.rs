//! Qualification-only construction of one exact Control database candidate.

use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;
use tokio::fs;

use super::super::ControlPayloadSnapshotBinding;
use super::control_restore_evidence::ControlCandidateEvidence;
use super::control_restore_filesystem::{
    any_sidecar, cleanup_quiescent_sidecars, ensure_owned_directory, hash_owned_file,
    optional_regular_file, publish_evidence, publish_noclobber, remove_partial_family,
    require_no_sidecars, sync_directory, validate_directory, validate_entries, CANDIDATE_FILE,
    EVIDENCE_FILE, EVIDENCE_PARTIAL_FILE, MAX_EVIDENCE_BYTES, PARTIAL_FILE,
};
use super::restore::{restore_staging_invalid, restore_staging_io};
use super::ControlPayloadOwnerRegistry;
use crate::control_store::executor::ControlStoreExecutor;

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlStoreRestore {
    pub(super) registry: ControlPayloadOwnerRegistry,
    pub(super) snapshot_descriptor_digest: String,
    pub(super) binding: ControlPayloadSnapshotBinding,
    pub(super) state_root: PathBuf,
    pub(super) staging_directory: PathBuf,
    pub(super) candidate: PathBuf,
    pub(super) evidence: ControlCandidateEvidence,
}

impl StagedControlStoreRestore {
    pub(super) fn candidate_path(&self) -> &Path {
        &self.candidate
    }
}

pub(super) async fn stage(
    registry: &ControlPayloadOwnerRegistry,
    snapshot_descriptor_digest: &str,
    binding: &ControlPayloadSnapshotBinding,
    control_export: &[u8],
    state_root: &Path,
    staging_directory: &Path,
    maintenance: &StateMaintenanceGuard,
) -> UseResult<StagedControlStoreRestore> {
    if !maintenance.is_exclusive_for(state_root) {
        return Err(restore_staging_invalid(
            "Control candidate staging requires the exact target's exclusive maintenance guard.",
        ));
    }
    let verified = binding
        .verify_control_export(registry, control_export)
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The complete restore Control export is invalid: {}",
                error.message
            ))
        })?;
    ensure_owned_directory(state_root, staging_directory).await?;
    validate_entries(staging_directory).await?;
    let physical_directory = fs::canonicalize(staging_directory)
        .await
        .map_err(|error| restore_staging_io("resolve Control candidate directory", error))?;
    validate_directory(&physical_directory).await?;
    let candidate = staging_directory.join(CANDIDATE_FILE);
    let physical_candidate = physical_directory.join(CANDIDATE_FILE);
    let partial = physical_directory.join(PARTIAL_FILE);
    let candidate_exists = optional_regular_file(&candidate).await?;
    let partial_exists = optional_regular_file(&staging_directory.join(PARTIAL_FILE)).await?;
    let evidence_exists = optional_regular_file(&staging_directory.join(EVIDENCE_FILE)).await?;
    let evidence_partial_exists =
        optional_regular_file(&staging_directory.join(EVIDENCE_PARTIAL_FILE)).await?;

    if candidate_exists {
        if partial_exists {
            return Err(restore_staging_invalid(
                "The complete restore Control candidate has conflicting partial state.",
            ));
        }
    } else {
        if evidence_exists || evidence_partial_exists {
            return Err(restore_staging_invalid(
                "Control candidate evidence exists without its database.",
            ));
        }
        if partial_exists || any_sidecar(staging_directory).await? {
            remove_partial_family(staging_directory).await?;
        }
        let executor = ControlStoreExecutor::new().map_err(|error| {
            restore_staging_invalid(format!(
                "Failed to start Control candidate worker: {}",
                error.message
            ))
        })?;
        let restore = executor
            .restore(
                partial.clone(),
                binding.installation.clone(),
                verified.export,
            )
            .await;
        if let Err(error) = restore {
            remove_partial_family(staging_directory).await?;
            return Err(restore_staging_invalid(format!(
                "Failed to build Control restore candidate: {}",
                error.message
            )));
        }
        verify_exact_candidate(&executor, &partial, binding, control_export).await?;
        fs::OpenOptions::new()
            .write(true)
            .open(&partial)
            .await
            .map_err(|error| restore_staging_io("open Control candidate for sync", error))?
            .sync_all()
            .await
            .map_err(|error| restore_staging_io("sync Control candidate", error))?;
        publish_noclobber(partial, physical_candidate.clone()).await?;
        sync_directory(&physical_directory).await?;
    }

    require_no_sidecars(staging_directory).await?;
    let executor = ControlStoreExecutor::new().map_err(|error| {
        restore_staging_invalid(format!(
            "Failed to start Control verification worker: {}",
            error.message
        ))
    })?;
    let (database_bytes, database_sha256) =
        verify_exact_candidate(&executor, &physical_candidate, binding, control_export).await?;
    let evidence = ControlCandidateEvidence::new(
        registry,
        snapshot_descriptor_digest,
        binding,
        database_bytes,
        database_sha256,
    )?;
    publish_evidence(
        staging_directory,
        &evidence.canonical_bytes(MAX_EVIDENCE_BYTES)?,
    )
    .await?;
    validate_entries(staging_directory).await?;
    require_no_sidecars(staging_directory).await?;
    Ok(StagedControlStoreRestore {
        registry: registry.clone(),
        snapshot_descriptor_digest: snapshot_descriptor_digest.to_owned(),
        binding: binding.clone(),
        state_root: state_root.to_path_buf(),
        staging_directory: staging_directory.to_path_buf(),
        candidate,
        evidence,
    })
}

async fn verify_exact_candidate(
    executor: &ControlStoreExecutor,
    candidate: &Path,
    binding: &ControlPayloadSnapshotBinding,
    expected_export: &[u8],
) -> UseResult<(u64, String)> {
    let exported = executor
        .export(candidate.to_path_buf(), binding.installation.clone())
        .await
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The staged Control database failed verification: {}",
                error.message
            ))
        })?
        .into_bytes();
    if exported != expected_export {
        return Err(restore_staging_invalid(
            "The staged Control database does not reproduce the exact bound export.",
        ));
    }
    cleanup_quiescent_sidecars(candidate).await?;
    hash_owned_file(candidate).await
}
