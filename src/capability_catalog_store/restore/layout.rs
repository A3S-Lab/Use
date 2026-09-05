use std::io;
use std::path::Path;

use tokio::fs;

use super::{
    metadata_is_link_or_reparse_point, restore_invalid, restore_io, valid_hex, ACTIVATION_FILE,
    ACTIVATION_PARTIAL_FILE, CANDIDATE_DIRECTORY, CATALOG_LOCK, CATALOG_RETENTION_JOURNAL,
    CATALOG_STAGING, STAGING_PREFIX,
};
use crate::capability_catalog_store::{validate_existing_directory, validate_store_layout};

pub(super) async fn validate_candidate_layout(candidate: &Path) -> super::UseResult<()> {
    validate_store_layout(candidate)
        .await
        .map_err(super::wrap_restore_error)?;
    for name in [CATALOG_LOCK, CATALOG_RETENTION_JOURNAL] {
        if optional_path_exists(&candidate.join(name)).await? {
            return Err(restore_invalid(
                "The catalog restore candidate contains live-operation evidence.",
            ));
        }
    }
    let staging = candidate.join(CATALOG_STAGING);
    if !validate_existing_directory(&staging).await? {
        return Err(restore_invalid(
            "The catalog restore candidate is missing its owned staging directory.",
        ));
    }
    let mut entries = fs::read_dir(&staging)
        .await
        .map_err(|error| restore_io("read catalog restore candidate staging", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("inspect catalog restore candidate staging", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The catalog restore candidate contains residual staging evidence.",
        ));
    }
    Ok(())
}

async fn optional_path_exists(path: &Path) -> super::UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(restore_io("inspect catalog restore candidate", error)),
    }
}

pub(super) async fn validate_staging_layout(staging: &Path) -> super::UseResult<()> {
    validate_existing_directory(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read catalog restore staging directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read catalog restore staging entry", error))?
    {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| restore_invalid("Catalog restore staging names must be valid UTF-8."))?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_io("inspect catalog restore staging entry", error))?;
        let valid = match name.as_str() {
            CANDIDATE_DIRECTORY => metadata.is_dir(),
            ACTIVATION_FILE | ACTIVATION_PARTIAL_FILE => metadata.is_file(),
            _ => false,
        };
        if metadata_is_link_or_reparse_point(&metadata) || !valid {
            return Err(restore_invalid(
                "The catalog restore staging directory contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

pub(super) async fn reject_foreign_staging(parent: &Path, expected: &Path) -> super::UseResult<()> {
    let mut entries = fs::read_dir(parent)
        .await
        .map_err(|error| restore_io("read catalog restore owner parent", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read catalog restore owner parent entry", error))?
    {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| restore_invalid("Catalog restore staging names must be valid UTF-8."))?;
        if !name.starts_with(STAGING_PREFIX) {
            continue;
        }
        let suffix = name
            .strip_prefix(STAGING_PREFIX)
            .filter(|value| valid_hex(value, 64))
            .ok_or_else(|| restore_invalid("A catalog restore staging name is invalid."))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .await
            .map_err(|error| restore_io("inspect catalog restore staging owner", error))?;
        if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(restore_invalid(
                "A catalog restore staging owner is not an owned directory.",
            ));
        }
        if path != expected {
            return Err(restore_invalid(format!(
                "A different catalog restore plan is still staged ({suffix})."
            )));
        }
        validate_staging_layout(&path).await?;
    }
    Ok(())
}
