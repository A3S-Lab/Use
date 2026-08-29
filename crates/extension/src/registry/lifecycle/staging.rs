use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

pub(super) use crate::artifact_store::ARTIFACT_STAGING_PREFIX;
use crate::artifact_store::{MAX_ARTIFACT_CONTAINER_ENTRIES, MAX_ARTIFACT_TREE_ENTRIES};
use crate::package::{io_error, remove_dir_all_with_windows_retry, sync_parent_directory};

/// Reclaim incomplete writes while the caller holds the digest mutation lock.
///
/// `ArtifactStore::acquire_expanded_package_mutation` creates and validates the
/// complete digest container before this function runs. Keeping that ownership
/// check in the store avoids a second path-creation implementation here.
pub(super) async fn reclaim_abandoned_artifact_staging(parent: &Path) -> UseResult<()> {
    let mut entries = fs::read_dir(parent)
        .await
        .map_err(|error| io_error("read expanded-package artifact container", parent, error))?;
    let mut entries_seen = 0_usize;
    let mut removed = false;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("read expanded-package artifact entry", parent, error))?
    {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_ARTIFACT_CONTAINER_ENTRIES {
            return Err(UseError::new(
                "use.artifact_store.inventory_limit_exceeded",
                "An expanded-package artifact container exceeds its bounded entry inventory.",
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            package_ownership_error(
                &entry.path(),
                "An expanded-package artifact container contains a non-UTF-8 entry name.",
            )
        })?;
        if !name.starts_with(ARTIFACT_STAGING_PREFIX) {
            continue;
        }
        let path = entry.path();
        validate_abandoned_staging_tree(&path).await?;
        remove_dir_all_with_windows_retry(path, "remove abandoned artifact staging").await?;
        removed = true;
    }
    if removed {
        sync_parent_directory(parent, "expanded-package artifact").await?;
    }
    Ok(())
}

async fn validate_abandoned_staging_tree(root: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(root)
        .await
        .map_err(|error| io_error("inspect abandoned artifact staging", root, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(package_ownership_error(
            root,
            "An abandoned artifact staging path is not an owned physical directory.",
        ));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut entries_seen = 0_usize;
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .await
            .map_err(|error| io_error("read abandoned artifact staging", &directory, error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("read abandoned artifact staging entry", &directory, error))?
        {
            entries_seen = entries_seen.saturating_add(1);
            if entries_seen > MAX_ARTIFACT_TREE_ENTRIES {
                return Err(UseError::new(
                    "use.artifact_store.inventory_limit_exceeded",
                    "An abandoned artifact staging tree exceeds its bounded entry inventory.",
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
                io_error("inspect abandoned artifact staging entry", &path, error)
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(package_ownership_error(
                    &path,
                    "An abandoned artifact staging tree contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(package_ownership_error(
                    &path,
                    "An abandoned artifact staging tree contains a special file.",
                ));
            }
        }
    }
    Ok(())
}

fn package_ownership_error(path: &Path, message: &str) -> UseError {
    UseError::new("use.artifact_store.ownership_invalid", message)
        .with_detail("path", path.display().to_string())
}
