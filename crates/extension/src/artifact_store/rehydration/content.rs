use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::{
    rehydration_plan_invalid, rehydration_plan_mismatch, rehydration_state_invalid,
    ArtifactRehydrationPlan,
};
use crate::artifact_store::blob::blob_open_options;
use crate::artifact_store::{artifact_store_error, ArtifactKind, ArtifactStore, CONTENT_DIRECTORY};
use crate::package::{
    copy_package_exact, io_error, remove_dir_all_with_windows_retry,
    remove_file_with_windows_retry, sync_parent_directory,
};

const REHYDRATION_STAGING: &str = ".artifact-staging-rehydration.tmp";
const REHYDRATION_RETIRED: &str = ".artifact-staging-rehydration-retired.tmp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactEvidence {
    pub(super) digest: String,
    pub(super) content_bytes: u64,
    pub(super) content_files: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RehydrationStorageProjection {
    pub(super) removed_before_write_bytes: u64,
    pub(super) added_bytes: u64,
}

impl ArtifactStore {
    pub(super) async fn candidate_evidence(
        &self,
        kind: ArtifactKind,
        candidate: &Path,
    ) -> UseResult<ArtifactEvidence> {
        let canonical_candidate = fs::canonicalize(candidate).await.map_err(|error| {
            io_error(
                "resolve Artifact Store rehydration candidate",
                candidate,
                error,
            )
        })?;
        let canonical_store = fs::canonicalize(self.root()).await.map_err(|error| {
            io_error(
                "resolve Artifact Store root for rehydration",
                self.root(),
                error,
            )
        })?;
        if canonical_candidate.starts_with(&canonical_store) {
            return Err(rehydration_plan_invalid(
                "An Artifact Store rehydration candidate must be independent of the store being repaired.",
            ));
        }
        measure_artifact(kind, &canonical_candidate).await
    }

    pub(super) async fn require_canonical_replacement(
        &self,
        plan: &ArtifactRehydrationPlan,
        container: &Path,
    ) -> UseResult<()> {
        let evidence = measure_artifact(plan.kind, &container.join(CONTENT_DIRECTORY)).await?;
        require_plan_candidate(plan, &evidence)
    }

    pub(super) async fn rehydrate_physical_content(
        &self,
        plan: &ArtifactRehydrationPlan,
        container: &Path,
        _sha256: &str,
        candidate: &Path,
    ) -> UseResult<()> {
        let content = container.join(CONTENT_DIRECTORY);
        let staging = container.join(REHYDRATION_STAGING);
        let retired = container.join(REHYDRATION_RETIRED);
        let mut content_evidence = optional_artifact(plan.kind, &content).await?;
        let mut retired_evidence = optional_artifact(plan.kind, &retired).await?;

        if let Some(evidence) = &retired_evidence {
            require_quarantined_content(plan, evidence)?;
        }
        match content_evidence.as_ref() {
            Some(evidence) if matches_replacement(plan, evidence) => {}
            Some(evidence) => {
                require_quarantined_content(plan, evidence)?;
                if retired_evidence.is_some() {
                    return Err(rehydration_state_invalid(
                        "Artifact rehydration found duplicate corrupt canonical and retired content.",
                    ));
                }
                ensure_candidate_staging(plan, candidate, &staging).await?;
                rename_owned_path(
                    content.clone(),
                    retired.clone(),
                    "retire corrupt artifact content",
                )
                .await?;
                sync_parent_directory(container, "retired artifact rehydration content").await?;
                retired_evidence = Some(evidence.clone());
                content_evidence = None;
            }
            None if retired_evidence.is_none() => {
                return Err(rehydration_state_invalid(
                    "Artifact rehydration found neither canonical nor retired corruption evidence.",
                ));
            }
            None => {}
        }

        if content_evidence.is_none() {
            ensure_candidate_staging(plan, candidate, &staging).await?;
            rename_owned_path(
                staging.clone(),
                content.clone(),
                "publish rehydrated artifact content",
            )
            .await?;
            sync_parent_directory(container, "rehydrated artifact content").await?;
        } else if let Some(staging_evidence) = optional_artifact(plan.kind, &staging).await? {
            require_plan_candidate(plan, &staging_evidence)?;
            remove_artifact(plan.kind, staging).await?;
        }

        self.require_canonical_replacement(plan, container).await?;
        if retired_evidence.is_some() {
            remove_artifact(plan.kind, retired).await?;
            sync_parent_directory(container, "retired artifact rehydration content").await?;
        }
        Ok(())
    }
}

pub(super) fn require_replacement(
    kind: ArtifactKind,
    digest: &str,
    evidence: &ArtifactEvidence,
) -> UseResult<()> {
    if evidence.digest != digest || (kind == ArtifactKind::Blob && evidence.content_files != 1) {
        return Err(artifact_store_error(
            "use.artifact_store.rehydration_candidate_mismatch",
            "The independently supplied candidate does not match the artifact digest.",
        )
        .with_detail("expectedDigest", digest.to_owned())
        .with_detail("actualDigest", evidence.digest.clone()));
    }
    Ok(())
}

pub(super) async fn rehydration_storage_projection(
    plan: &ArtifactRehydrationPlan,
    container: &Path,
) -> UseResult<RehydrationStorageProjection> {
    let content = optional_artifact(plan.kind, &container.join(CONTENT_DIRECTORY)).await?;
    let staging = optional_artifact(plan.kind, &container.join(REHYDRATION_STAGING)).await?;
    if content
        .as_ref()
        .is_some_and(|evidence| matches_replacement(plan, evidence))
        || staging
            .as_ref()
            .is_some_and(|evidence| matches_replacement(plan, evidence))
    {
        return Ok(RehydrationStorageProjection {
            removed_before_write_bytes: 0,
            added_bytes: 0,
        });
    }
    Ok(RehydrationStorageProjection {
        removed_before_write_bytes: staging.map_or(0, |evidence| evidence.content_bytes),
        added_bytes: plan.replacement_content_bytes,
    })
}

pub(super) fn require_plan_candidate(
    plan: &ArtifactRehydrationPlan,
    evidence: &ArtifactEvidence,
) -> UseResult<()> {
    if !matches_replacement(plan, evidence) {
        return Err(artifact_store_error(
            "use.artifact_store.rehydration_candidate_mismatch",
            "The rehydration candidate changed after the exact plan was reviewed.",
        ));
    }
    Ok(())
}

fn matches_replacement(plan: &ArtifactRehydrationPlan, evidence: &ArtifactEvidence) -> bool {
    evidence.digest == plan.digest
        && evidence.content_bytes == plan.replacement_content_bytes
        && evidence.content_files == plan.replacement_content_files
}

fn require_quarantined_content(
    plan: &ArtifactRehydrationPlan,
    evidence: &ArtifactEvidence,
) -> UseResult<()> {
    if evidence.digest != plan.quarantined_observed_digest
        || evidence.content_bytes != plan.quarantined_content_bytes
        || evidence.content_files != plan.quarantined_content_files
    {
        return Err(rehydration_state_invalid(
            "The quarantined artifact bytes changed during rehydration.",
        ));
    }
    Ok(())
}

async fn optional_artifact(kind: ArtifactKind, path: &Path) -> UseResult<Option<ArtifactEvidence>> {
    match fs::symlink_metadata(path).await {
        Ok(_) => measure_artifact(kind, path).await.map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(
            "inspect Artifact Store rehydration content",
            path,
            error,
        )),
    }
}

async fn measure_artifact(kind: ArtifactKind, path: &Path) -> UseResult<ArtifactEvidence> {
    match kind {
        ArtifactKind::Blob => measure_blob(path).await,
        ArtifactKind::ExpandedPackage => {
            let metadata = fs::symlink_metadata(path).await.map_err(|error| {
                io_error(
                    "inspect expanded-package rehydration candidate",
                    path,
                    error,
                )
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                return Err(rehydration_state_invalid(
                    "An expanded-package rehydration candidate must be an owned directory.",
                ));
            }
            let fingerprint = crate::digest::package_fingerprint(path).await?;
            Ok(ArtifactEvidence {
                digest: format!("sha256:{}", fingerprint.sha256),
                content_bytes: fingerprint.byte_count,
                content_files: fingerprint.file_count,
            })
        }
    }
}

async fn measure_blob(path: &Path) -> UseResult<ArtifactEvidence> {
    let mut file = blob_open_options()
        .open(path)
        .await
        .map_err(|error| io_error("open Blob rehydration candidate", path, error))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened Blob rehydration candidate", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
    {
        return Err(rehydration_state_invalid(
            "A Blob rehydration candidate must be a non-empty owned regular file.",
        ));
    }
    let expected_length = metadata.len();
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("read Blob rehydration candidate", path, error))?;
        if read == 0 {
            break;
        }
        length = length.checked_add(read as u64).ok_or_else(|| {
            rehydration_state_invalid("A Blob rehydration candidate length overflowed.")
        })?;
        if length > expected_length {
            return Err(rehydration_state_invalid(
                "A Blob rehydration candidate grew while it was hashed.",
            ));
        }
        digest.update(&buffer[..read]);
    }
    let final_metadata = file
        .metadata()
        .await
        .map_err(|error| io_error("reinspect Blob rehydration candidate", path, error))?;
    if length != expected_length || final_metadata.len() != expected_length {
        return Err(rehydration_state_invalid(
            "A Blob rehydration candidate changed while it was hashed.",
        ));
    }
    Ok(ArtifactEvidence {
        digest: format!("sha256:{:x}", digest.finalize()),
        content_bytes: length,
        content_files: 1,
    })
}

async fn ensure_candidate_staging(
    plan: &ArtifactRehydrationPlan,
    candidate: &Path,
    staging: &Path,
) -> UseResult<()> {
    if let Some(evidence) = optional_artifact(plan.kind, staging).await? {
        if matches_replacement(plan, &evidence) {
            return Ok(());
        }
        remove_artifact(plan.kind, staging.to_path_buf()).await?;
    }
    match plan.kind {
        ArtifactKind::Blob => copy_blob_candidate(candidate, staging, plan).await?,
        ArtifactKind::ExpandedPackage => {
            fs::create_dir(staging).await.map_err(|error| {
                io_error(
                    "create expanded-package rehydration staging",
                    staging,
                    error,
                )
            })?;
            if let Err(error) = copy_package_exact(
                candidate,
                staging,
                plan.replacement_content_bytes,
                plan.replacement_content_files,
            )
            .await
            {
                let _ = remove_dir_all_with_windows_retry(
                    staging.to_path_buf(),
                    "remove failed expanded-package rehydration staging",
                )
                .await;
                return Err(error);
            }
            sync_package_directories(staging).await?;
        }
    }
    let evidence = measure_artifact(plan.kind, staging).await?;
    require_plan_candidate(plan, &evidence)?;
    if let Some(parent) = staging.parent() {
        sync_parent_directory(parent, "Artifact Store rehydration staging").await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_package_directories(root: &Path) -> UseResult<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    let mut entries = 0_usize;
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        let mut children = fs::read_dir(&directory)
            .await
            .map_err(|error| io_error("read rehydration staging directory", &directory, error))?;
        while let Some(entry) = children.next_entry().await.map_err(|error| {
            io_error(
                "read rehydration staging directory entry",
                &directory,
                error,
            )
        })? {
            entries = entries.checked_add(1).ok_or_else(|| {
                rehydration_state_invalid("Rehydration staging directory count overflowed.")
            })?;
            if entries > crate::artifact_store::MAX_ARTIFACT_TREE_ENTRIES {
                return Err(rehydration_state_invalid(
                    "Rehydration staging exceeds the Artifact Store tree bound.",
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
                io_error("inspect rehydration staging directory entry", &path, error)
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(rehydration_state_invalid(
                    "Rehydration staging contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_parent_directory(&directory, "expanded-package rehydration staging").await?;
    }
    Ok(())
}

#[cfg(not(unix))]
async fn sync_package_directories(_root: &Path) -> UseResult<()> {
    Ok(())
}

async fn copy_blob_candidate(
    candidate: &Path,
    staging: &Path,
    plan: &ArtifactRehydrationPlan,
) -> UseResult<()> {
    let mut input = blob_open_options()
        .open(candidate)
        .await
        .map_err(|error| io_error("open Blob rehydration source", candidate, error))?;
    input
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|error| io_error("seek Blob rehydration source", candidate, error))?;
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staging)
        .await
        .map_err(|error| io_error("create Blob rehydration staging", staging, error))?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("read Blob rehydration source", candidate, error))?;
        if read == 0 {
            break;
        }
        length = length.checked_add(read as u64).ok_or_else(|| {
            rehydration_state_invalid("Blob rehydration staging length overflowed.")
        })?;
        if length > plan.replacement_content_bytes {
            return Err(rehydration_plan_mismatch(
                "The Blob rehydration candidate grew after review.",
            ));
        }
        digest.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|error| io_error("write Blob rehydration staging", staging, error))?;
    }
    let observed = format!("sha256:{:x}", digest.finalize());
    if length != plan.replacement_content_bytes || observed != plan.digest {
        return Err(rehydration_plan_mismatch(
            "The Blob rehydration candidate changed after review.",
        ));
    }
    output
        .sync_all()
        .await
        .map_err(|error| io_error("sync Blob rehydration staging", staging, error))
}

async fn remove_artifact(kind: ArtifactKind, path: PathBuf) -> UseResult<()> {
    match kind {
        ArtifactKind::Blob => {
            remove_file_with_windows_retry(path, "remove Artifact Store rehydration content").await
        }
        ArtifactKind::ExpandedPackage => {
            remove_dir_all_with_windows_retry(path, "remove Artifact Store rehydration content")
                .await
        }
    }
}

async fn rename_owned_path(
    source: PathBuf,
    target: PathBuf,
    action: &'static str,
) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        crate::rename_path_with_windows_retry_blocking(&source, &target)
    })
    .await
    .map_err(|error| {
        rehydration_state_invalid(format!(
            "Artifact Store rehydration worker did not complete: {error}"
        ))
    })?
    .map_err(|error| io_error(action, &error_target, error))
}
