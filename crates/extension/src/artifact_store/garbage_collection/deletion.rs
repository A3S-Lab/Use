use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;

use super::{
    garbage_collection_state_invalid, ArtifactGarbageCollectionEntry,
    ArtifactGarbageCollectionRecord,
};
use crate::artifact_store::quarantine::{
    validate_quarantine_metadata, QUARANTINE_RECORD, QUARANTINE_TEMPORARY,
};
use crate::artifact_store::rehydration::{
    validate_rehydration_metadata, REHYDRATION_PREPARED_RECORD, REHYDRATION_PREPARED_TEMPORARY,
    REHYDRATION_RECORD, REHYDRATION_TEMPORARY,
};
use crate::artifact_store::{
    validate_lock_metadata, ArtifactKind, ArtifactStore, ARTIFACT_STAGING_PREFIX, BLOBS_DIRECTORY,
    CONTENT_DIRECTORY, EXPANDED_PACKAGES_DIRECTORY, MAX_ARTIFACT_CONTAINER_ENTRIES,
    MAX_ARTIFACT_TREE_ENTRIES, MUTATION_LOCK, SHA256_DIRECTORY,
};
use crate::package::{io_error, remove_dir_all_with_windows_retry, sync_parent_directory};

const TOMBSTONE_PREFIX: &str = ".artifact-gc-";
const TOMBSTONE_SUFFIX: &str = ".tmp";

pub(super) async fn retire_artifact(
    store: &ArtifactStore,
    expected: &ArtifactGarbageCollectionEntry,
    plan_digest: &str,
) -> UseResult<()> {
    let (_, container) = store.garbage_collection_container(expected.kind, &expected.digest)?;
    let tombstone = tombstone_path(&container, &expected.digest, plan_digest)?;
    let container_present =
        optional_owned_directory(&container, "garbage-collection target").await?;
    let tombstone_present =
        optional_owned_directory(&tombstone, "garbage-collection tombstone").await?;
    match (container_present, tombstone_present) {
        (true, true) => {
            return Err(garbage_collection_state_invalid(
                "A garbage-collection target and its tombstone both exist.",
            ))
        }
        (true, false) => {
            let actual = store
                .inspect_garbage_collection_entry(expected.kind, &expected.digest)
                .await?;
            if &actual != expected {
                return Err(super::garbage_collection_plan_mismatch(
                    "A reviewed Artifact Store target changed before atomic retirement.",
                ));
            }
            rename_container(&container, &tombstone).await?;
            sync_parent(&container).await?;
        }
        (false, true) | (false, false) => {}
    }

    if optional_owned_directory(&tombstone, "garbage-collection tombstone").await? {
        validate_tombstone_residual(&tombstone, expected.kind).await?;
        remove_dir_all_with_windows_retry(
            tombstone.clone(),
            "remove Artifact Store garbage-collection tombstone",
        )
        .await?;
        sync_parent(&tombstone).await?;
    }
    if optional_owned_directory(&container, "garbage-collection target").await?
        || optional_owned_directory(&tombstone, "garbage-collection tombstone").await?
    {
        return Err(garbage_collection_state_invalid(
            "A reviewed Artifact Store target was not fully retired.",
        ));
    }
    Ok(())
}

pub(super) async fn require_no_tombstones(
    store: &ArtifactStore,
    record: &ArtifactGarbageCollectionRecord,
) -> UseResult<()> {
    require_no_tombstones_at_root(store.root(), record).await
}

pub(super) async fn require_no_tombstones_at_root(
    root: &Path,
    record: &ArtifactGarbageCollectionRecord,
) -> UseResult<()> {
    record.validate()?;
    for artifact in &record.plan.artifacts {
        let sha256 = artifact.digest.strip_prefix("sha256:").ok_or_else(|| {
            garbage_collection_state_invalid("A garbage-collection target digest is invalid.")
        })?;
        let shard = sha256.get(..2).ok_or_else(|| {
            garbage_collection_state_invalid("A garbage-collection target digest is invalid.")
        })?;
        let tier = match artifact.kind {
            ArtifactKind::Blob => BLOBS_DIRECTORY,
            ArtifactKind::ExpandedPackage => EXPANDED_PACKAGES_DIRECTORY,
        };
        let container = root
            .join(tier)
            .join(SHA256_DIRECTORY)
            .join(shard)
            .join(sha256);
        let tombstone = tombstone_path(&container, &artifact.digest, &record.plan_digest)?;
        if path_exists_owned(&tombstone, "garbage-collection tombstone").await? {
            return Err(garbage_collection_state_invalid(
                "A completed Artifact Store garbage collection retains a target tombstone.",
            ));
        }
    }
    Ok(())
}

fn tombstone_path(container: &Path, digest: &str, plan_digest: &str) -> UseResult<PathBuf> {
    let artifact_sha256 = digest.strip_prefix("sha256:").ok_or_else(|| {
        garbage_collection_state_invalid("A garbage-collection target digest is invalid.")
    })?;
    let plan_sha256 = plan_digest.strip_prefix("sha256:").ok_or_else(|| {
        garbage_collection_state_invalid("A garbage-collection plan digest is invalid.")
    })?;
    let parent = container.parent().ok_or_else(|| {
        garbage_collection_state_invalid("A garbage-collection target has no owned shard.")
    })?;
    Ok(parent.join(format!(
        "{TOMBSTONE_PREFIX}{artifact_sha256}-{plan_sha256}{TOMBSTONE_SUFFIX}"
    )))
}

async fn rename_container(source: &Path, target: &Path) -> UseResult<()> {
    let source_for_worker = source.to_path_buf();
    let target_for_worker = target.to_path_buf();
    let error_target = source.to_path_buf();
    tokio::task::spawn_blocking(move || {
        crate::atomic_file::rename_path_with_windows_retry_blocking(
            &source_for_worker,
            &target_for_worker,
        )
    })
    .await
    .map_err(|error| {
        garbage_collection_state_invalid(format!(
            "Artifact Store garbage-collection rename worker did not complete: {error}"
        ))
    })?
    .map_err(|error| {
        io_error(
            "atomically retire Artifact Store target",
            &error_target,
            error,
        )
    })
}

async fn sync_parent(path: &Path) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        garbage_collection_state_invalid("An Artifact Store GC path has no owned parent.")
    })?;
    sync_parent_directory(parent, "Artifact Store garbage-collection shard").await
}

async fn optional_owned_directory(path: &Path, label: &str) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(&format!("inspect {label}"), path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(garbage_collection_state_invalid(format!(
            "The {label} '{}' must be an owned directory.",
            path.display()
        )));
    }
    Ok(true)
}

async fn path_exists_owned(path: &Path, label: &str) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(&format!("inspect {label}"), path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
        return Err(garbage_collection_state_invalid(format!(
            "The {label} '{}' is a link or reparse point.",
            path.display()
        )));
    }
    Ok(true)
}

async fn validate_tombstone_residual(root: &Path, kind: ArtifactKind) -> UseResult<()> {
    optional_owned_directory(root, "garbage-collection tombstone").await?;
    let mut directory = fs::read_dir(root).await.map_err(|error| {
        io_error(
            "read Artifact Store garbage-collection tombstone",
            root,
            error,
        )
    })?;
    let mut immediate_entries = 0_usize;
    while let Some(entry) = directory.next_entry().await.map_err(|error| {
        io_error(
            "read Artifact Store garbage-collection tombstone entry",
            root,
            error,
        )
    })? {
        immediate_entries = immediate_entries.checked_add(1).ok_or_else(|| {
            garbage_collection_state_invalid(
                "A garbage-collection tombstone entry count overflowed.",
            )
        })?;
        if immediate_entries > MAX_ARTIFACT_CONTAINER_ENTRIES {
            return Err(garbage_collection_state_invalid(
                "A garbage-collection tombstone exceeds its bounded immediate inventory.",
            ));
        }
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| {
            garbage_collection_state_invalid(
                "A garbage-collection tombstone contains a non-UTF-8 entry name.",
            )
        })?;
        let metadata = owned_metadata(&path, "garbage-collection tombstone entry").await?;
        match name.as_str() {
            MUTATION_LOCK => validate_lock_metadata(&path, &metadata, "artifact mutation")?,
            QUARANTINE_RECORD => validate_quarantine_metadata(&path, &metadata, false)?,
            QUARANTINE_TEMPORARY => validate_quarantine_metadata(&path, &metadata, true)?,
            REHYDRATION_PREPARED_RECORD | REHYDRATION_RECORD => {
                validate_rehydration_metadata(&path, &metadata, false)?
            }
            REHYDRATION_PREPARED_TEMPORARY | REHYDRATION_TEMPORARY => {
                validate_rehydration_metadata(&path, &metadata, true)?
            }
            CONTENT_DIRECTORY => validate_content_residual(&path, &metadata, kind).await?,
            _ if name.starts_with(ARTIFACT_STAGING_PREFIX) => {
                validate_content_residual(&path, &metadata, kind).await?
            }
            _ => {
                return Err(garbage_collection_state_invalid(
                    "A garbage-collection tombstone contains an unowned entry.",
                ))
            }
        }
    }
    Ok(())
}

async fn validate_content_residual(
    path: &Path,
    metadata: &std::fs::Metadata,
    kind: ArtifactKind,
) -> UseResult<()> {
    match kind {
        ArtifactKind::Blob if metadata.is_file() => Ok(()),
        ArtifactKind::ExpandedPackage if metadata.is_dir() => validate_owned_tree(path).await,
        ArtifactKind::Blob => Err(garbage_collection_state_invalid(
            "A blob garbage-collection tombstone contains non-file content.",
        )),
        ArtifactKind::ExpandedPackage => Err(garbage_collection_state_invalid(
            "An expanded-package garbage-collection tombstone contains non-directory content.",
        )),
    }
}

async fn validate_owned_tree(root: &Path) -> UseResult<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0_usize;
    while let Some(directory_path) = pending.pop() {
        let mut directory = fs::read_dir(&directory_path).await.map_err(|error| {
            io_error(
                "read garbage-collection tombstone content",
                &directory_path,
                error,
            )
        })?;
        while let Some(entry) = directory.next_entry().await.map_err(|error| {
            io_error(
                "read garbage-collection tombstone content entry",
                &directory_path,
                error,
            )
        })? {
            entries = entries.checked_add(1).ok_or_else(|| {
                garbage_collection_state_invalid(
                    "A garbage-collection tombstone tree inventory overflowed.",
                )
            })?;
            if entries > MAX_ARTIFACT_TREE_ENTRIES {
                return Err(garbage_collection_state_invalid(
                    "A garbage-collection tombstone exceeds its bounded tree inventory.",
                ));
            }
            let path = entry.path();
            let metadata = owned_metadata(&path, "garbage-collection tombstone content").await?;
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(garbage_collection_state_invalid(
                    "A garbage-collection tombstone contains a special file.",
                ));
            }
        }
    }
    Ok(())
}

async fn owned_metadata(path: &Path, label: &str) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
        return Err(garbage_collection_state_invalid(format!(
            "The {label} '{}' is a link or reparse point.",
            path.display()
        )));
    }
    Ok(metadata)
}
