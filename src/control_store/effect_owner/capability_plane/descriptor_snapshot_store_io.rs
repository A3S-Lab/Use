//! Filesystem helpers for the Capability descriptor-snapshot store.

use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{InstallationId, UseResult};
use fs2::FileExt;
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::*;
use super::{
    LOCK_RETRY, LOCK_WAIT, MAX_DIRECTORY_ENTRIES, MAX_STAGING_BYTES, SNAPSHOT_DIRECTORY,
    SNAPSHOT_LOCK, SNAPSHOT_RETENTION_JOURNAL, SNAPSHOT_STAGING,
};


pub(crate) async fn write_new_record(root: &Path, target: &Path, bytes: &[u8]) -> UseResult<()> {
    let staging = root.join(SNAPSHOT_STAGING);
    ensure_owned_directory_chain(root, &staging).await?;
    let key_digest = target
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(path_invalid)?;
    let temporary = staging.join(format!(".{key_digest}.tmp"));
    prepare_staging_file(&temporary, bytes).await?;
    sync_directory(&staging).await?;
    match tokio_fs::hard_link(&temporary, target).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let Some(current) = super::read_snapshot_at(target, &format!("sha256:{key_digest}")).await?
            else {
                return Err(snapshot_conflict());
            };
            if super::encode_snapshot(&current)? != bytes {
                return Err(snapshot_conflict());
            }
            sync_directory(root).await?;
            retire_staging(root, &format!("sha256:{key_digest}")).await?;
            return Ok(());
        }
        Err(error) => return Err(path_error("publish descriptor snapshot", target, error)),
    }
    let Some(current) = super::read_snapshot_at(target, &format!("sha256:{key_digest}")).await? else {
        return Err(snapshot_conflict());
    };
    if super::encode_snapshot(&current)? != bytes {
        return Err(snapshot_conflict());
    }
    sync_directory(root).await?;
    retire_staging(root, &format!("sha256:{key_digest}")).await
}

pub(crate) async fn prepare_staging_file(path: &Path, bytes: &[u8]) -> UseResult<()> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if metadata_is_link(&metadata)
                || !metadata.is_file()
                || metadata.len() as usize > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES
            {
                return Err(snapshot_conflict());
            }
            let current = tokio_fs::read(path)
                .await
                .map_err(|error| path_error("read descriptor snapshot staging", path, error))?;
            if current != bytes {
                tokio_fs::remove_file(path).await.map_err(|error| {
                    path_error("retire descriptor snapshot staging", path, error)
                })?;
                sync_directory(path.parent().ok_or_else(path_invalid)?).await?;
            } else {
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(path_error(
                "inspect descriptor snapshot staging",
                path,
                error,
            ))
        }
    }
    let mut options = tokio_fs::OpenOptions::new();
    options.create_new(true).write(true);
    configure_no_follow(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("create descriptor snapshot staging", path, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = tokio_fs::remove_file(path).await;
        return Err(path_error("write descriptor snapshot staging", path, error));
    }
    drop(file);
    Ok(())
}

pub(crate) async fn retire_staging(root: &Path, key_digest: &str) -> UseResult<()> {
    let hex = key_digest
        .strip_prefix("sha256:")
        .ok_or_else(path_invalid)?;
    let path = root.join(SNAPSHOT_STAGING).join(format!(".{hex}.tmp"));
    match tokio_fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if metadata_is_link(&metadata) || !metadata.is_file() {
                return Err(path_invalid());
            }
            tokio_fs::remove_file(&path)
                .await
                .map_err(|error| path_error("retire descriptor snapshot staging", &path, error))?;
            sync_directory(path.parent().ok_or_else(path_invalid)?).await
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error(
            "inspect descriptor snapshot staging",
            &path,
            error,
        )),
    }
}

pub(crate) async fn scan_records(
    root: &Path,
    installation: &InstallationId,
) -> UseResult<Vec<ControlCapabilityDescriptorSnapshot>> {
    let mut entries = tokio_fs::read_dir(root)
        .await
        .map_err(|error| path_error("read descriptor snapshot store", root, error))?;
    let mut count = 0_usize;
    let mut records = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read descriptor snapshot entry", root, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_DIRECTORY_ENTRIES {
            return Err(snapshot_error(
                "The descriptor snapshot directory exceeds its bound.",
            ));
        }
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| snapshot_error("Descriptor snapshot names must be UTF-8."))?
            .to_owned();
        match name.as_str() {
            SNAPSHOT_LOCK => validate_regular_file(&entry.path()).await?,
            SNAPSHOT_RETENTION_JOURNAL => {
                super::retention::validate_journal_file(&entry.path()).await?;
            }
            SNAPSHOT_STAGING => validate_staging(&entry.path()).await?,
            _ if is_record_name(&name) => {
                let digest = format!("sha256:{}", name.trim_end_matches(".json"));
                let snapshot = super::read_snapshot_at(&entry.path(), &digest)
                    .await?
                    .ok_or_else(snapshot_conflict)?;
                installation
                    .ensure_same(&snapshot.key.installation)
                    .map_err(|_| {
                        snapshot_error("A descriptor snapshot belongs to another installation.")
                    })?;
                if snapshot.digest()? != digest {
                    return Err(snapshot_conflict());
                }
                records.push(snapshot);
            }
            _ => return Err(path_invalid()),
        }
    }
    if records.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
        return Err(snapshot_error(
            "The descriptor snapshot store exceeds its record bound.",
        ));
    }
    records.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(records)
}

pub(crate) async fn validate_staging(path: &Path) -> UseResult<()> {
    validate_directory(path).await?;
    let mut entries = tokio_fs::read_dir(path)
        .await
        .map_err(|error| path_error("read descriptor snapshot staging", path, error))?;
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read descriptor snapshot staging entry", path, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
            return Err(snapshot_error(
                "Descriptor snapshot staging exceeds its entry bound.",
            ));
        }
        let file_name = entry.file_name();
        let name = file_name
            .to_str()
            .ok_or_else(|| snapshot_error("Descriptor snapshot staging names must be UTF-8."))?;
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
        let metadata = tokio_fs::symlink_metadata(entry.path()).await.map_err(|error| {
            path_error("inspect descriptor snapshot staging", &entry.path(), error)
        })?;
        if metadata_is_link(&metadata)
            || !metadata.is_file()
            || metadata.len() as usize > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES
        {
            return Err(snapshot_conflict());
        }
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or_else(|| snapshot_error("Descriptor snapshot staging size overflowed."))?;
        if bytes > MAX_STAGING_BYTES {
            return Err(snapshot_error(
                "Descriptor snapshot staging exceeds its byte bound.",
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_record_name(name: &str) -> bool {
    let Some(hex) = name.strip_suffix(".json") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn path_for_digest(root: &Path, digest: &str) -> UseResult<PathBuf> {
    if !valid_sha256(digest) {
        return Err(path_invalid());
    }
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    Ok(root.join(format!("{hex}.json")))
}

pub(crate) async fn acquire_lock(root: &Path, mode: LockMode) -> UseResult<SnapshotLock> {
    let path = root.join(SNAPSHOT_LOCK);
    let mut options = StdOpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let path_for_open = path.clone();
    let file = tokio::task::spawn_blocking(move || options.open(path_for_open))
        .await
        .map_err(|error| snapshot_io(format!("Descriptor snapshot lock task failed: {error}")))?
        .map_err(|error| path_error("open descriptor snapshot lock", &path, error))?;
    validate_regular_file(&path).await?;
    let deadline = tokio::time::Instant::now() + LOCK_WAIT;
    let mut file = file;
    loop {
        let (returned, result) = tokio::task::spawn_blocking(move || {
            let result = match mode {
                LockMode::Shared => FileExt::try_lock_shared(&file),
                LockMode::Exclusive => FileExt::try_lock_exclusive(&file),
            };
            (file, result)
        })
        .await
        .map_err(|error| snapshot_io(format!("Descriptor snapshot lock task failed: {error}")))?;
        file = returned;
        match result {
            Ok(()) => return Ok(SnapshotLock(file)),
            Err(error) if lock_contended(&error) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(UseError::new(
                        SNAPSHOT_BUSY,
                        "Another process owns the descriptor snapshot store lock.",
                    ));
                }
                tokio::time::sleep(LOCK_RETRY.min(deadline.saturating_duration_since(now))).await;
            }
            Err(error) => return Err(path_error("lock descriptor snapshot store", &path, error)),
        }
    }
}

#[derive(Debug)]
pub(crate) struct SnapshotLock(StdFile);

impl Drop for SnapshotLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LockMode {
    Shared,
    Exclusive,
}

pub(crate) async fn ensure_directory_exists(path: &Path) -> UseResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut missing = Vec::new();
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match tokio_fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link(&metadata) || !metadata.is_dir() {
                    return Err(path_invalid());
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.to_path_buf())
            }
            Err(error) => {
                return Err(path_error(
                    "inspect descriptor snapshot root",
                    ancestor,
                    error,
                ))
            }
        }
    }
    while let Some(directory) = missing.pop() {
        match tokio_fs::create_dir(&directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create descriptor snapshot directory",
                    &directory,
                    error,
                ))
            }
        }
        validate_directory(&directory).await?;
    }
    Ok(())
}

pub(crate) async fn ensure_owned_directory_chain(root: &Path, target: &Path) -> UseResult<()> {
    if !target.starts_with(root) {
        return Err(path_invalid());
    }
    ensure_directory_exists(root).await?;
    validate_directory(root).await?;
    let relative = target.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(path_invalid());
        };
        current.push(segment);
        match tokio_fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata_is_link(&metadata) && metadata.is_dir() => {}
            Ok(_) => return Err(path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match tokio_fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(path_error(
                            "create descriptor snapshot directory",
                            &current,
                            error,
                        ))
                    }
                }
                validate_directory(&current).await?;
            }
            Err(error) => {
                return Err(path_error(
                    "inspect descriptor snapshot directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(())
}

pub(crate) async fn path_ancestors_exist(path: &Path) -> UseResult<bool> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut complete = true;
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match tokio_fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link(&metadata) {
                    // A configured state root can be reached through an
                    // operating-system alias (for example macOS `/var`).
                    // Only a link at the configured path itself is invalid;
                    // aliases outside that ownership boundary are resolved
                    // and still required to denote directories.
                    if ancestor == path {
                        return Err(path_invalid());
                    }
                    let followed = tokio_fs::metadata(ancestor).await.map_err(|error| {
                        path_error(
                            "resolve descriptor snapshot state-root alias",
                            ancestor,
                            error,
                        )
                    })?;
                    if !followed.is_dir() {
                        return Err(path_invalid());
                    }
                } else if !metadata.is_dir() {
                    return Err(path_invalid());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => complete = false,
            Err(error) => {
                return Err(path_error(
                    "inspect descriptor snapshot root",
                    ancestor,
                    error,
                ))
            }
        }
    }
    Ok(complete)
}

pub(crate) async fn validate_existing_directory(path: &Path) -> UseResult<bool> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link(&metadata) && metadata.is_dir() => Ok(true),
        Ok(_) => Err(path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error(
            "inspect descriptor snapshot directory",
            path,
            error,
        )),
    }
}

pub(crate) async fn validate_directory(path: &Path) -> UseResult<()> {
    if !validate_existing_directory(path).await? {
        return Err(path_invalid());
    }
    Ok(())
}

pub(crate) async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = tokio_fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect descriptor snapshot file", path, error))?;
    if metadata_is_link(&metadata) || !metadata.is_file() {
        return Err(path_invalid());
    }
    Ok(())
}

pub(crate) fn metadata_is_link(metadata: &std::fs::Metadata) -> bool {
    a3s_use_core::metadata_is_link_or_reparse_point(metadata)
}

pub(super) fn configure_no_follow(options: &mut tokio_fs::OpenOptions) {
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

pub(crate) fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }
}

#[cfg(unix)]
pub(crate) async fn sync_directory(path: &Path) -> UseResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .read(true)
            .open(&path)
            .map_err(|error| {
                path_error("open descriptor snapshot directory for sync", &path, error)
            })?;
        file.sync_all()
            .map_err(|error| path_error("sync descriptor snapshot directory", &path, error))
    })
    .await
    .map_err(|error| {
        snapshot_io(format!(
            "Descriptor snapshot directory sync failed: {error}"
        ))
    })?
}

#[cfg(not(unix))]
pub(crate) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
