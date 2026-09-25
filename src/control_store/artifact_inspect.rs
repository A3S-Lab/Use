//! Control Store artifact-reference inspection for reachability inventory.
//!
//! Clean Control installations have no `installation-snapshot.json` or
//! `extensions/` receipts. Collectors must read committed generations and
//! nonterminal reviewed operations from Control as sole authority.

use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::{
    InstallationId, InstallationKind, InstallationSnapshot, PluginPackageLock, UseError, UseResult,
};
use a3s_use_extension::{ArtifactKind, StateMaintenanceLock};

use super::export;
use super::filesystem::{self, CONTROL_STORE_DATABASE_FILE};
use super::model::ControlOperationStatus;
use super::production::reject_legacy_authority_paths;
use super::schema;
use super::ControlStore;

/// One Artifact Store digest retained by Control authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ControlInstallationArtifactReference {
    pub kind: ArtifactKind,
    pub digest: String,
    /// True when the digest is selected by a committed generation snapshot.
    pub committed: bool,
    pub expected_bytes: Option<u64>,
    pub expected_files: Option<u64>,
}

/// Inspect Control-owned package artifact references for one installation root.
///
/// Fails closed when legacy authority leaves are present beside Control.
pub(crate) async fn inspect_installation_artifact_references(
    state_root: &Path,
    expected_kind: InstallationKind,
    expected_storage_key: &str,
) -> UseResult<(InstallationId, Vec<ControlInstallationArtifactReference>)> {
    reject_legacy_authority_paths(state_root)?;
    let database_path = state_root.join(CONTROL_STORE_DATABASE_FILE);
    let _maintenance = StateMaintenanceLock::new(state_root)
        .acquire_shared()
        .await?;
    filesystem::require_initialized(state_root, &database_path).await?;
    let physical = filesystem::physical_database_path(state_root, &database_path).await?;
    let physical_for_probe = physical.clone();
    let installation = tokio::task::spawn_blocking(move || {
        schema::probe_installation_identity(&physical_for_probe)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.control_store.artifact_inspect_failed",
            format!("Control Store identity probe worker failed: {error}"),
        )
    })??;
    if installation.kind != expected_kind || installation.storage_key()? != expected_storage_key {
        return Err(UseError::new(
            "use.control_store.identity_mismatch",
            "The Control Store installation identity does not match its state path.",
        ));
    }
    drop(_maintenance);

    let store = ControlStore::new(state_root, installation.clone())?;
    let bytes = store.export().await?;
    let verified = export::verify(&bytes, &installation)?;
    let references = project_artifact_references(
        &verified.export.authority.generations,
        &verified.export.authority.operations,
    )?;
    Ok((installation, references))
}

fn project_artifact_references(
    generations: &[super::model::ControlGeneration],
    operations: &[super::model::ControlOperationRecord],
) -> UseResult<Vec<ControlInstallationArtifactReference>> {
    let mut references = Vec::new();
    let mut seen = BTreeSet::new();
    for generation in generations {
        append_snapshot_references(&mut references, &mut seen, &generation.snapshot, true)?;
    }
    for operation in operations {
        match operation.status {
            ControlOperationStatus::Reviewed | ControlOperationStatus::EffectsPending => {}
            ControlOperationStatus::Completed
            | ControlOperationStatus::Cancelled
            | ControlOperationStatus::Rejected => continue,
        }
        if let Some(package_lock) = &operation.reviewed.envelope.package_lock {
            append_package_lock_references(&mut references, &mut seen, package_lock, false)?;
        }
        if let Some(package_lock) = &operation.reviewed.envelope.prior_package_lock {
            append_package_lock_references(&mut references, &mut seen, package_lock, false)?;
        }
    }
    Ok(references)
}

fn append_snapshot_references(
    references: &mut Vec<ControlInstallationArtifactReference>,
    seen: &mut BTreeSet<(bool, ArtifactKind, String)>,
    snapshot: &InstallationSnapshot,
    committed: bool,
) -> UseResult<()> {
    for selection in &snapshot.packages {
        let package = &selection.package.catalog.record.package;
        let digest = package.sha256.clone().ok_or_else(|| {
            UseError::new(
                "use.control_store.artifact_inspect_invalid",
                "A Control generation package omits its expanded digest.",
            )
        })?;
        push_unique(
            references,
            seen,
            ControlInstallationArtifactReference {
                kind: ArtifactKind::ExpandedPackage,
                digest,
                committed,
                expected_bytes: Some(package.expanded_bytes),
                expected_files: Some(package.file_count),
            },
        );
    }
    Ok(())
}

fn append_package_lock_references(
    references: &mut Vec<ControlInstallationArtifactReference>,
    seen: &mut BTreeSet<(bool, ArtifactKind, String)>,
    package_lock: &PluginPackageLock,
    committed: bool,
) -> UseResult<()> {
    package_lock.validate()?;
    for package in &package_lock.packages {
        let record = &package.catalog.record;
        let expanded_digest = record.package.sha256.clone().ok_or_else(|| {
            UseError::new(
                "use.control_store.artifact_inspect_invalid",
                "A Control operation package lock omits its expanded package digest.",
            )
        })?;
        push_unique(
            references,
            seen,
            ControlInstallationArtifactReference {
                kind: ArtifactKind::ExpandedPackage,
                digest: expanded_digest,
                committed,
                expected_bytes: Some(record.package.expanded_bytes),
                expected_files: Some(record.package.file_count),
            },
        );
        push_unique(
            references,
            seen,
            ControlInstallationArtifactReference {
                kind: ArtifactKind::Blob,
                digest: record.archive.sha256.clone(),
                committed,
                expected_bytes: Some(record.archive.length),
                expected_files: None,
            },
        );
        if let Some(planning) = &record.planning {
            push_unique(
                references,
                seen,
                ControlInstallationArtifactReference {
                    kind: ArtifactKind::Blob,
                    digest: planning.sha256.clone(),
                    committed,
                    expected_bytes: Some(planning.length),
                    expected_files: None,
                },
            );
        }
    }
    Ok(())
}

fn push_unique(
    references: &mut Vec<ControlInstallationArtifactReference>,
    seen: &mut BTreeSet<(bool, ArtifactKind, String)>,
    reference: ControlInstallationArtifactReference,
) {
    let key = (
        reference.committed,
        reference.kind,
        reference.digest.clone(),
    );
    if seen.insert(key) {
        references.push(reference);
    }
}
