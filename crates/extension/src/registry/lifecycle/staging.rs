use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

use crate::package::{io_error, sync_parent_directory, MAX_PACKAGE_FILES};

pub(super) const LIFECYCLE_STAGING_PREFIX: &str = ".lifecycle-staging-";

const MAX_LIFECYCLE_PACKAGE_DIRECTORY_ENTRIES: usize = 128;
const MAX_LIFECYCLE_STAGING_TREE_ENTRIES: usize = MAX_PACKAGE_FILES * 2;

pub(super) async fn prepare_lifecycle_package_parent(
    root: &Path,
    directory: &Path,
) -> UseResult<()> {
    ensure_owned_package_directory(root, directory).await?;
    remove_abandoned_staging_directories(directory).await
}

async fn ensure_owned_package_directory(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(package_ownership_error(
            directory,
            "The lifecycle package directory escapes its configured data root.",
        ));
    }
    fs::create_dir_all(root)
        .await
        .map_err(|error| io_error("create lifecycle data root", root, error))?;
    validate_owned_directory(root).await?;
    let relative = directory.strip_prefix(root).map_err(|_| {
        package_ownership_error(
            directory,
            "The lifecycle package directory escapes its configured data root.",
        )
    })?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        create_and_validate_directory(&current, "create lifecycle package directory").await?;
    }
    Ok(())
}

async fn create_and_validate_directory(path: &Path, action: &'static str) -> UseResult<()> {
    match fs::create_dir(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(action, path, error)),
    }
    validate_owned_directory(path).await
}

async fn validate_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect lifecycle package directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(package_ownership_error(
            path,
            "A lifecycle package directory is not an owned physical directory.",
        ));
    }
    Ok(())
}

async fn remove_abandoned_staging_directories(parent: &Path) -> UseResult<()> {
    let mut entries = fs::read_dir(parent)
        .await
        .map_err(|error| io_error("read lifecycle package directory", parent, error))?;
    let mut entries_seen = 0_usize;
    let mut removed = false;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("read lifecycle package entry", parent, error))?
    {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_LIFECYCLE_PACKAGE_DIRECTORY_ENTRIES {
            return Err(UseError::new(
                "use.extension.lifecycle_package_limit_exceeded",
                "The lifecycle package directory exceeds its bounded entry inventory.",
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            package_ownership_error(
                &entry.path(),
                "The lifecycle package directory contains a non-UTF-8 entry name.",
            )
        })?;
        if !name.starts_with(LIFECYCLE_STAGING_PREFIX) {
            continue;
        }
        let path = entry.path();
        validate_abandoned_staging_tree(&path).await?;
        fs::remove_dir_all(&path).await.map_err(|error| {
            io_error("remove abandoned lifecycle package staging", &path, error)
        })?;
        removed = true;
    }
    if removed {
        sync_parent_directory(parent, "lifecycle package").await?;
    }
    Ok(())
}

async fn validate_abandoned_staging_tree(root: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(root)
        .await
        .map_err(|error| io_error("inspect abandoned lifecycle package staging", root, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(package_ownership_error(
            root,
            "An abandoned lifecycle staging path is not an owned physical directory.",
        ));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut entries_seen = 0_usize;
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory).await.map_err(|error| {
            io_error(
                "read abandoned lifecycle package staging",
                &directory,
                error,
            )
        })?;
        while let Some(entry) = entries.next_entry().await.map_err(|error| {
            io_error(
                "read abandoned lifecycle package staging entry",
                &directory,
                error,
            )
        })? {
            entries_seen = entries_seen.saturating_add(1);
            if entries_seen > MAX_LIFECYCLE_STAGING_TREE_ENTRIES {
                return Err(UseError::new(
                    "use.extension.lifecycle_package_limit_exceeded",
                    "An abandoned lifecycle staging tree exceeds its bounded entry inventory.",
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
                io_error("inspect abandoned lifecycle package entry", &path, error)
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(package_ownership_error(
                    &path,
                    "An abandoned lifecycle staging tree contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(package_ownership_error(
                    &path,
                    "An abandoned lifecycle staging tree contains a special file.",
                ));
            }
        }
    }
    Ok(())
}

fn package_ownership_error(path: &Path, message: &str) -> UseError {
    UseError::new("use.extension.ownership_invalid", message)
        .with_detail("path", path.display().to_string())
}
