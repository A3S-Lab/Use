use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

use super::{io_error, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};

#[cfg(test)]
pub(crate) async fn copy_package(source: &Path, target: &Path) -> UseResult<()> {
    copy_package_bounded(source, target, None).await
}

pub(crate) async fn copy_package_exact(
    source: &Path,
    target: &Path,
    expected_bytes: u64,
    expected_files: u64,
) -> UseResult<()> {
    if expected_bytes == 0 || expected_files == 0 {
        return Err(UseError::new(
            "use.extension.package_changed",
            "The expected package measurement is empty.",
        ));
    }
    copy_package_bounded(source, target, Some((expected_bytes, expected_files))).await
}

async fn copy_package_bounded(
    source: &Path,
    target: &Path,
    expected: Option<(u64, u64)>,
) -> UseResult<()> {
    // Package identity covers regular-file paths and bytes. Materialize parent
    // directories lazily so unhashed empty directories cannot enter the
    // content-addressed representation.
    let mut pending = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut entry_count = 0_usize;
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    while let Some((source_dir, target_dir)) = pending.pop() {
        let source_metadata = fs::symlink_metadata(&source_dir)
            .await
            .map_err(|error| io_error("inspect extension package directory", &source_dir, error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&source_metadata) {
            return Err(UseError::new(
                "use.extension.package_symlink",
                format!(
                    "Extension package directory '{}' is a link or reparse point.",
                    source_dir.display()
                ),
            ));
        }
        if !source_metadata.is_dir() {
            return Err(UseError::new(
                "use.extension.package_entry_invalid",
                format!(
                    "Extension package directory '{}' is not a directory.",
                    source_dir.display()
                ),
            ));
        }
        let mut entries = fs::read_dir(&source_dir)
            .await
            .map_err(|error| io_error("read extension package directory", &source_dir, error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("read extension package entry", &source_dir, error))?
        {
            entry_count = entry_count.saturating_add(1);
            if entry_count > MAX_PACKAGE_FILES {
                return Err(UseError::new(
                    "use.extension.package_too_large",
                    "The extension package exceeds the local installation limits.",
                ));
            }
            let source_path = entry.path();
            let target_path = target_dir.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path).await.map_err(|error| {
                io_error("inspect extension package entry", &source_path, error)
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(UseError::new(
                    "use.extension.package_symlink",
                    format!(
                        "Extension package entry '{}' is a link or reparse point.",
                        source_path.display()
                    ),
                ));
            }
            if metadata.is_dir() {
                pending.push((source_path, target_path));
            } else if metadata.is_file() {
                let next_files = files.saturating_add(1);
                let next_bytes = bytes.saturating_add(metadata.len());
                let exceeds_expected = expected.is_some_and(|(expected_bytes, expected_files)| {
                    next_files > expected_files || next_bytes > expected_bytes
                });
                if next_bytes > MAX_PACKAGE_BYTES || exceeds_expected {
                    return Err(UseError::new(
                        if exceeds_expected {
                            "use.extension.package_changed"
                        } else {
                            "use.extension.package_too_large"
                        },
                        if exceeds_expected {
                            "The extension package exceeds its prepared physical measurement."
                        } else {
                            "The extension package exceeds the local installation limits."
                        },
                    ));
                }
                let target_parent = target_path.parent().ok_or_else(|| {
                    UseError::new(
                        "use.extension.package_entry_invalid",
                        "A staged package file has no parent directory.",
                    )
                })?;
                fs::create_dir_all(target_parent).await.map_err(|error| {
                    io_error("create staged package directory", target_parent, error)
                })?;
                copy_package_file_exact(&source_path, &target_path, metadata.len()).await?;
                files = next_files;
                bytes = next_bytes;
            } else {
                return Err(UseError::new(
                    "use.extension.package_entry_invalid",
                    format!(
                        "Extension package entry '{}' is not a regular file or directory.",
                        source_path.display()
                    ),
                ));
            }
        }
    }
    if expected.is_some_and(|value| value != (bytes, files)) {
        return Err(UseError::new(
            "use.extension.package_changed",
            "The extension package no longer matches its prepared physical measurement.",
        ));
    }
    Ok(())
}

async fn copy_package_file_exact(
    source: &Path,
    target: &Path,
    expected_bytes: u64,
) -> UseResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut input = fs::File::open(source)
        .await
        .map_err(|error| io_error("open extension package file", source, error))?;
    let opened = input
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened extension package file", source, error))?;
    if !opened.is_file() || opened.len() != expected_bytes {
        return Err(UseError::new(
            "use.extension.package_changed",
            "The extension package file changed before it could be copied.",
        ));
    }
    let permissions = opened.permissions();
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .await
        .map_err(|error| io_error("create staged package file", target, error))?;
    let mut copied = 0_u64;
    // Keep the copy buffer off the async state-machine stack. Lifecycle
    // orchestration composes several futures and Windows test threads have a
    // comparatively small stack.
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("read extension package file", source, error))?;
        if read == 0 {
            break;
        }
        copied = copied.checked_add(read as u64).ok_or_else(|| {
            UseError::new(
                "use.extension.package_changed",
                "The extension package file length overflowed while copying.",
            )
        })?;
        if copied > expected_bytes {
            return Err(UseError::new(
                "use.extension.package_changed",
                "The extension package file grew while it was copied.",
            ));
        }
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|error| io_error("write staged package file", target, error))?;
    }
    if copied != expected_bytes {
        return Err(UseError::new(
            "use.extension.package_changed",
            "The extension package file changed while it was copied.",
        ));
    }
    fs::set_permissions(target, permissions)
        .await
        .map_err(|error| io_error("preserve staged package file permissions", target, error))?;
    output
        .sync_all()
        .await
        .map_err(|error| io_error("sync staged package file", target, error))?;
    Ok(())
}
