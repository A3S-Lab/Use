use std::path::Path;

use a3s_use_core::{metadata_is_link_or_reparse_point, UseResult};
use tokio::fs;

use super::{
    catalog_conflict, path_error, path_invalid, store_invalid, validate_directory,
    validate_regular_file, CATALOG_LOCK, CATALOG_RETENTION_JOURNAL, CATALOG_STAGING,
    MAX_CAPABILITY_GATEWAY_CATALOG_BYTES, MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS,
    MAX_DIRECTORY_ENTRIES, MAX_RETENTION_JOURNAL_BYTES, MAX_STAGING_BYTES,
};

/// Validate the store's top-level namespace before interpreting any payload.
///
/// A content-addressed store has no safe meaning for an unexpected sibling:
/// accepting it would make an operator unable to distinguish state written by
/// this implementation from state written by another authority. Unknown
/// entries therefore fail closed, including links and reparse points.
pub(super) async fn validate_store_layout(root: &Path) -> UseResult<()> {
    validate_directory(root).await?;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read catalog store layout", root, error))?;
    let mut count = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read catalog store entry", root, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_DIRECTORY_ENTRIES {
            return Err(store_invalid(
                "The catalog store layout exceeds its entry bound.",
            ));
        }
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| store_invalid("Catalog store names must be valid UTF-8."))?
            .to_owned();
        let path = entry.path();
        match name.as_str() {
            CATALOG_LOCK => validate_regular_file(&path).await?,
            CATALOG_RETENTION_JOURNAL => validate_retention_journal(&path).await?,
            CATALOG_STAGING => validate_staging_layout(&path).await?,
            "sha256" => validate_directory(&path).await?,
            _ => return Err(path_invalid()),
        }
    }
    Ok(())
}

async fn validate_retention_journal(path: &Path) -> UseResult<()> {
    validate_regular_file(path).await?;
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog-retention journal", path, error))?;
    if metadata.len() == 0 || metadata.len() > MAX_RETENTION_JOURNAL_BYTES {
        return Err(store_invalid(
            "The catalog-retention journal exceeds its byte bound.",
        ));
    }
    Ok(())
}

async fn validate_staging_layout(path: &Path) -> UseResult<()> {
    validate_directory(path).await?;
    let mut entries = fs::read_dir(path)
        .await
        .map_err(|error| path_error("read catalog staging layout", path, error))?;
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read catalog staging entry", path, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS {
            return Err(store_invalid(
                "The catalog staging inventory exceeds its entry bound.",
            ));
        }
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| store_invalid("Catalog staging names must be valid UTF-8."))?
            .to_owned();
        let Some(hex) = name
            .strip_prefix('.')
            .and_then(|value| value.strip_suffix(".tmp"))
        else {
            return Err(path_invalid());
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(path_invalid());
        }
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)
            .await
            .map_err(|error| path_error("inspect catalog staging entry", &entry_path, error))?;
        if metadata_is_link_or_reparse_point(&metadata)
            || !metadata.is_file()
            || metadata.len() > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
        {
            return Err(catalog_conflict());
        }
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or_else(|| store_invalid("The catalog staging byte bound overflowed."))?;
        if bytes > MAX_STAGING_BYTES {
            return Err(store_invalid(
                "The catalog staging inventory exceeds its byte bound.",
            ));
        }
    }
    Ok(())
}
