//! Reopens a staged Control restore after durable activation has begun.

use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;
use tokio::fs;

use super::super::{ControlPayloadOwnerRegistry, ControlPayloadSnapshotBinding};
use super::control_restore::StagedControlStoreRestore;
use super::control_restore_activation::{
    require_no_live_sidecars, validate_database, validate_evidence,
};
use super::control_restore_evidence::ControlCandidateEvidence;
use super::control_restore_filesystem::{
    optional_regular_file, optional_regular_file_length, read_exact_owned, require_no_sidecars,
    validate_directory, validate_entries, CANDIDATE_FILE, EVIDENCE_FILE, MAX_EVIDENCE_BYTES,
};
use super::restore::{restore_staging_invalid, restore_staging_io};
use crate::control_store::filesystem::CONTROL_STORE_DATABASE_FILE;

pub(super) async fn reopen(
    registry: &ControlPayloadOwnerRegistry,
    snapshot_descriptor_digest: &str,
    binding: &ControlPayloadSnapshotBinding,
    state_root: &Path,
    staging_directory: &Path,
    maintenance: &StateMaintenanceGuard,
) -> UseResult<StagedControlStoreRestore> {
    if !maintenance.is_exclusive_for(state_root) {
        return Err(restore_staging_invalid(
            "Control restore replay requires the exact target's exclusive maintenance guard.",
        ));
    }
    validate_directory(state_root).await?;
    validate_directory(staging_directory).await?;
    validate_entries(staging_directory).await?;
    require_no_sidecars(staging_directory).await?;
    require_no_live_sidecars(state_root).await?;

    let evidence_path = staging_directory.join(EVIDENCE_FILE);
    let evidence_length = optional_regular_file_length(&evidence_path)
        .await?
        .ok_or_else(|| {
            restore_staging_invalid("The reopened Control restore has no candidate evidence.")
        })?;
    let evidence_bytes =
        read_exact_owned(&evidence_path, evidence_length, MAX_EVIDENCE_BYTES).await?;
    let evidence: ControlCandidateEvidence =
        serde_json::from_slice(&evidence_bytes).map_err(|_| {
            restore_staging_invalid("The reopened Control candidate evidence is invalid JSON.")
        })?;
    if evidence.canonical_bytes(MAX_EVIDENCE_BYTES)? != evidence_bytes {
        return Err(restore_staging_invalid(
            "The reopened Control candidate evidence is not canonically encoded.",
        ));
    }

    let physical_root = fs::canonicalize(state_root)
        .await
        .map_err(|error| restore_staging_io("resolve reopened Control root", error))?;
    validate_directory(&physical_root).await?;
    let physical_staging = fs::canonicalize(staging_directory)
        .await
        .map_err(|error| restore_staging_io("resolve reopened Control staging", error))?;
    validate_directory(&physical_staging).await?;
    if physical_staging == physical_root || !physical_staging.starts_with(&physical_root) {
        return Err(restore_staging_invalid(
            "The reopened Control staging directory escapes its state root.",
        ));
    }

    let candidate = staging_directory.join(CANDIDATE_FILE);
    let live = state_root.join(CONTROL_STORE_DATABASE_FILE);
    let candidate_exists = optional_regular_file(&candidate).await?;
    let live_exists = optional_regular_file(&live).await?;
    let database = match (candidate_exists, live_exists) {
        (true, false) => physical_staging.join(CANDIDATE_FILE),
        (false, true) => physical_root.join(CONTROL_STORE_DATABASE_FILE),
        _ => {
            return Err(restore_staging_invalid(
                "The reopened Control restore boundary is ambiguous or missing.",
            ))
        }
    };

    let staged = StagedControlStoreRestore {
        registry: registry.clone(),
        snapshot_descriptor_digest: snapshot_descriptor_digest.to_owned(),
        binding: binding.clone(),
        state_root: state_root.to_path_buf(),
        staging_directory: staging_directory.to_path_buf(),
        candidate,
        evidence,
    };
    validate_evidence(&staged).await?;
    validate_database(
        registry,
        snapshot_descriptor_digest,
        binding,
        &staged.evidence,
        &database,
    )
    .await?;
    validate_evidence(&staged).await?;
    validate_entries(staging_directory).await?;
    require_no_sidecars(staging_directory).await?;
    require_no_live_sidecars(state_root).await?;
    Ok(staged)
}
