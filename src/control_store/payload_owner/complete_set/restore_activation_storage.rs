//! Bounded, no-follow storage primitives for the restore activation protocol.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::restore::{restore_activation_invalid, restore_activation_io};

pub(super) async fn write_synced_new(path: &Path, bytes: &[u8], label: &str) -> UseResult<()> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await
        .map_err(|error| restore_activation_io(&format!("create {label}"), error))?;
    file.write_all(bytes)
        .await
        .map_err(|error| restore_activation_io(&format!("write {label}"), error))?;
    file.flush()
        .await
        .map_err(|error| restore_activation_io(&format!("flush {label}"), error))?;
    file.sync_all()
        .await
        .map_err(|error| restore_activation_io(&format!("sync {label}"), error))?;
    drop(file);
    if read_bounded_file(path, bytes.len() as u64, label).await? != bytes {
        return Err(restore_activation_invalid(format!(
            "The {label} changed while it was written."
        )));
    }
    Ok(())
}

pub(super) async fn publish_noclobber(
    source: PathBuf,
    target: PathBuf,
    label: &str,
) -> UseResult<()> {
    let action = label.to_owned();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| {
        restore_activation_invalid(format!(
            "The {action} publication worker did not complete: {error}"
        ))
    })?
    .map_err(|error| restore_activation_io(&format!("publish {label}"), error))
}

pub(super) async fn remove_obsolete_temporary(path: &Path, maximum: u64) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() <= maximum =>
        {
            fs::remove_file(path)
                .await
                .map_err(|error| restore_activation_io("remove activation temporary", error))?;
            if let Some(parent) = path.parent() {
                sync_directory(parent).await?;
            }
            Ok(())
        }
        Ok(_) => Err(restore_activation_invalid(
            "The activation temporary is not an owned bounded regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_activation_io("inspect activation temporary", error)),
    }
}

pub(super) async fn optional_regular_file(path: &Path) -> UseResult<bool> {
    Ok(optional_regular_file_length(path).await?.is_some())
}

pub(super) async fn optional_regular_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_activation_invalid(
            "A complete restore activation path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_activation_io("inspect activation path", error)),
    }
}

pub(super) async fn read_bounded_file(
    path: &Path,
    maximum: u64,
    label: &str,
) -> UseResult<Vec<u8>> {
    let before = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_activation_io(&format!("inspect {label}"), error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&before)
        || !before.is_file()
        || before.len() == 0
        || before.len() > maximum
    {
        return Err(restore_activation_invalid(format!(
            "The {label} is not a bounded owned regular file."
        )));
    }
    let before_modified = before.modified().ok();
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let mut file = options
        .open(path)
        .await
        .map_err(|error| restore_activation_io(&format!("open {label}"), error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| restore_activation_io(&format!("inspect opened {label}"), error))?;
    if !opened.is_file() || opened.len() != before.len() {
        return Err(restore_activation_invalid(format!(
            "The opened {label} differs from its owned path evidence."
        )));
    }
    let capacity = usize::try_from(before.len())
        .map_err(|_| restore_activation_invalid(format!("The {label} length overflowed.")))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(before.len().saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_activation_io(&format!("read {label}"), error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_activation_io(&format!("reinspect {label}"), error))?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || (before_modified.is_some() && after.modified().ok() != before_modified)
    {
        return Err(restore_activation_invalid(format!(
            "The {label} changed while it was read."
        )));
    }
    Ok(bytes)
}

pub(super) async fn validate_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_activation_io(&format!("inspect {label}"), error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_activation_invalid(format!(
            "The {label} is not an owned directory."
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_activation_io("open activation directory for sync", error))?
        .sync_all()
        .await
        .map_err(|error| restore_activation_io("sync activation directory", error))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
