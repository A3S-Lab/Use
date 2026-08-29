use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::Path;

use a3s_use_core::UseResult;
use fs2::FileExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{inventory_invalid, inventory_io};

pub(super) async fn read_bounded_json<T>(path: &Path, max_bytes: u64, label: &str) -> UseResult<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let bytes = read_bounded_bytes(path, max_bytes, label).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| inventory_invalid(format!("The {label} contains invalid JSON: {error}")))
}

pub(super) async fn read_bounded_bytes(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> UseResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| inventory_io(&format!("inspect {label}"), path, error))?;
    validate_bounded_file(path, &metadata, max_bytes, label)?;
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
        .map_err(|error| inventory_io(&format!("open {label}"), path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| inventory_io(&format!("inspect opened {label}"), path, error))?;
    validate_bounded_file(path, &opened, max_bytes, label)?;
    file.rewind()
        .await
        .map_err(|error| inventory_io(&format!("seek {label}"), path, error))?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| inventory_io(&format!("read {label}"), path, error))?;
    if bytes.len() as u64 != opened.len() {
        return Err(inventory_invalid(format!(
            "The {label} changed while it was read."
        )));
    }
    Ok(bytes)
}

pub(super) async fn require_owned_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| inventory_io(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(
            inventory_invalid(format!("The {label} must be an owned directory."))
                .with_detail("path", path.display().to_string()),
        );
    }
    Ok(())
}

pub(super) async fn require_bounded_file(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| inventory_io(&format!("inspect {label}"), path, error))?;
    validate_bounded_file(path, &metadata, max_bytes, label)
}

pub(super) async fn require_owned_file(path: &Path, max_bytes: u64, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| inventory_io(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > max_bytes
    {
        return Err(inventory_invalid(format!(
            "The {label} must be a bounded owned regular file."
        ))
        .with_detail("path", path.display().to_string()));
    }
    Ok(())
}

pub(super) async fn acquire_existing_owned_lock_shared(
    path: &Path,
    label: &str,
) -> UseResult<StdFile> {
    require_owned_file(path, 4 * 1024, label).await?;
    let lock_path = path.to_path_buf();
    let error_path = lock_path.clone();
    let owned_label = label.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut options = StdOpenOptions::new();
        options.create(false).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_SHARE_WRITE: u32 = 0x0000_0002;
            options
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let file = options
            .open(&lock_path)
            .map_err(|error| inventory_io(&format!("open {owned_label}"), &lock_path, error))?;
        let metadata = file.metadata().map_err(|error| {
            inventory_io(&format!("inspect opened {owned_label}"), &lock_path, error)
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || !metadata.is_file()
            || metadata.len() > 4 * 1024
        {
            return Err(inventory_invalid(format!(
                "The {owned_label} must remain a bounded owned regular file while opened."
            ))
            .with_detail("path", lock_path.display().to_string()));
        }
        FileExt::lock_shared(&file)
            .map_err(|error| inventory_io(&format!("lock {owned_label}"), &lock_path, error))?;
        Ok(file)
    })
    .await
    .map_err(|error| {
        inventory_io(
            &format!("join {label} task"),
            &error_path,
            io::Error::other(error),
        )
    })?
}

pub(super) async fn optional_owned_directory(path: &Path, label: &str) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(inventory_invalid(format!(
            "The {label} must be an owned directory."
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(inventory_io(&format!("inspect {label}"), path, error)),
    }
}

pub(super) async fn owned_metadata(path: &Path, label: &str) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| inventory_io(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || (!metadata.is_dir() && !metadata.is_file())
    {
        return Err(
            inventory_invalid(format!("The {label} is not an owned file or directory."))
                .with_detail("path", path.display().to_string()),
        );
    }
    Ok(metadata)
}

fn validate_bounded_file(
    path: &Path,
    metadata: &std::fs::Metadata,
    max_bytes: u64,
    label: &str,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(inventory_invalid(format!(
            "The {label} must be a bounded owned regular file."
        ))
        .with_detail("path", path.display().to_string()));
    }
    Ok(())
}

pub(super) fn entry_name(entry: &fs::DirEntry, label: &str) -> UseResult<String> {
    entry.file_name().into_string().map_err(|_| {
        inventory_invalid(format!("The {label} contains a non-UTF-8 entry name."))
            .with_detail("path", entry.path().display().to_string())
    })
}
