//! Low-level mutation locking, directory ownership, and staging I/O for the
//! catalog payload store.

use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Component, Path};
use std::time::SystemTime;

use a3s_use_core::{metadata_is_link_or_reparse_point, UseResult};
use fs2::FileExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    catalog_conflict, digest_for_bytes, path_error, path_invalid, read_catalog_at, CATALOG_STAGING,
    LOCK_RETRY, LOCK_WAIT, MAX_CAPABILITY_GATEWAY_CATALOG_BYTES,
};

pub(super) struct MutationGuard(pub(super) StdFile);

impl Drop for MutationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) async fn write_new_record(root: &Path, target: &Path, bytes: &[u8]) -> UseResult<()> {
    let parent = target.parent().ok_or_else(path_invalid)?;
    let staging_root = root.join(CATALOG_STAGING);
    ensure_owned_directory_chain(root, parent).await?;
    ensure_owned_directory_chain(root, &staging_root).await?;
    let digest = digest_for_bytes(bytes)?;
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    let temporary = staging_root.join(format!(".{hex}.tmp"));
    prepare_staging_file(&temporary, bytes).await?;
    // Persist the recovery name before linking it into the immutable shard.
    // This makes a crash between file creation and publication replayable.
    sync_directory(&staging_root).await?;
    match fs::hard_link(&temporary, target).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let Some((_catalog, current)) = read_catalog_at(target, &digest).await? else {
                return Err(catalog_conflict());
            };
            sync_directory(parent).await?;
            retire_staging(root, &digest).await?;
            if current != bytes {
                return Err(catalog_conflict());
            }
            return Ok(());
        }
        Err(error) => {
            return Err(path_error("publish catalog record", target, error));
        }
    }
    let Some((_catalog, published)) = read_catalog_at(target, &digest).await? else {
        return Err(catalog_conflict());
    };
    if published != bytes {
        return Err(catalog_conflict());
    }
    sync_directory(parent).await?;
    retire_staging(root, &digest).await
}

async fn prepare_staging_file(path: &Path, bytes: &[u8]) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file()
                || metadata.len() > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
            {
                return Err(catalog_conflict());
            }
            if read_raw_file(path).await? == bytes {
                return Ok(());
            }
            // The deterministic staging name identifies the requested
            // digest. A regular, bounded file with different bytes is an
            // incomplete/tampered replay artifact; remove only that owned
            // artifact while holding the mutation lock, then write the
            // requested canonical bytes again.
            retire_staging_path(path).await?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect catalog staging file", path, error)),
    }
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    configure_no_follow_async(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("create catalog staging file", path, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(path).await;
        return Err(path_error("write catalog staging file", path, error));
    }
    drop(file);
    validate_regular_file(path).await?;
    if read_raw_file(path).await? != bytes {
        return Err(catalog_conflict());
    }
    Ok(())
}

async fn retire_staging_path(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog staging file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(path_invalid());
    }
    fs::remove_file(path)
        .await
        .map_err(|error| path_error("retire catalog staging file", path, error))?;
    sync_directory(path.parent().ok_or_else(path_invalid)?).await
}

pub(super) async fn retire_staging(root: &Path, digest: &str) -> UseResult<()> {
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    let path = root.join(CATALOG_STAGING).join(format!(".{hex}.tmp"));
    match fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(path_invalid());
            }
            retire_staging_path(&path).await
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error("inspect catalog staging file", &path, error)),
    }
}

async fn read_raw_file(path: &Path) -> UseResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog staging file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
    {
        return Err(catalog_conflict());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("open catalog staging file", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| path_error("inspect opened catalog staging file", path, error))?;
    let before = file_identity(&metadata);
    if metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
        || file_identity(&opened) != before
    {
        return Err(catalog_conflict());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_CAPABILITY_GATEWAY_CATALOG_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| path_error("read catalog staging file", path, error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("reinspect catalog staging file", path, error))?;
    if metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || bytes.len() as u64 != opened.len()
    {
        return Err(catalog_conflict());
    }
    Ok(bytes)
}

pub(super) async fn ensure_owned_directory_chain(root: &Path, target: &Path) -> UseResult<()> {
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
        let parent = current.clone();
        current.push(segment);
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {}
            Ok(_) => return Err(path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(path_error("create catalog directory", &current, error))
                    }
                }
                validate_directory(&current).await?;
                sync_directory(&parent).await?;
            }
            Err(error) => return Err(path_error("inspect catalog directory", &current, error)),
        }
    }
    Ok(())
}

/// Create a missing absolute directory path without traversing a symlinked
/// ancestor. `create_dir_all` is deliberately avoided because it follows an
/// intermediate link before this store can inspect it.
pub(super) async fn ensure_directory_exists(path: &Path) -> UseResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut missing = Vec::new();
    let mut existing = false;
    // Inspect every ancestor, not only the final path component. Otherwise an
    // intermediate symlink could redirect a seemingly missing state root
    // before this store has a chance to reject it.
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link_or_reparse_point(&metadata) {
                    // A configured state root may be reached through an
                    // operating-system alias (for example macOS `/var`).
                    // The final state-root component is still required to be
                    // link-free; aliases outside that boundary are resolved
                    // later by `physical_paths` before no-follow I/O.
                    if ancestor == path {
                        return Err(path_invalid());
                    }
                    let followed = fs::metadata(ancestor).await.map_err(|error| {
                        path_error("resolve catalog state-root alias", ancestor, error)
                    })?;
                    if !followed.is_dir() {
                        return Err(path_invalid());
                    }
                } else if !metadata.is_dir() {
                    return Err(path_invalid());
                }
                existing = true;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.to_path_buf());
            }
            Err(error) => return Err(path_error("inspect catalog state root", ancestor, error)),
        }
    }
    if !existing {
        return Err(path_invalid());
    }
    while let Some(directory) = missing.pop() {
        let parent = directory.parent().ok_or_else(path_invalid)?;
        match fs::create_dir(&directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(path_error("create catalog state root", &directory, error)),
        }
        validate_directory(&directory).await?;
        sync_directory(parent).await?;
    }
    Ok(())
}

pub(super) async fn validate_existing_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {
            Ok(true)
        }
        Ok(_) => Err(path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error("inspect catalog directory", path, error)),
    }
}

pub(super) async fn validate_existing_directory_chain(
    root: &Path,
    target: &Path,
) -> UseResult<bool> {
    if !target.starts_with(root) {
        return Err(path_invalid());
    }
    if !validate_existing_path_ancestors(root).await? {
        return Ok(false);
    }
    if !validate_existing_directory(root).await? {
        return Ok(false);
    }
    let relative = target.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(path_invalid());
        };
        current.push(segment);
        if !validate_existing_directory(&current).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) async fn validate_existing_path_ancestors(path: &Path) -> UseResult<bool> {
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
        match fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link_or_reparse_point(&metadata) {
                    if ancestor == path {
                        return Err(path_invalid());
                    }
                    let followed = fs::metadata(ancestor).await.map_err(|error| {
                        path_error("resolve catalog state-root alias", ancestor, error)
                    })?;
                    if !followed.is_dir() {
                        return Err(path_invalid());
                    }
                } else if !metadata.is_dir() {
                    return Err(path_invalid());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => complete = false,
            Err(error) => return Err(path_error("inspect catalog directory", ancestor, error)),
        }
    }
    Ok(complete)
}

pub(super) async fn validate_directory(path: &Path) -> UseResult<()> {
    if !validate_existing_directory(path).await? {
        return Err(path_invalid());
    }
    Ok(())
}

pub(super) async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(path_invalid());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

pub(super) fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
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

pub(super) fn configure_no_follow_async(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
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

#[derive(Debug, Clone, Copy)]
pub(super) enum MutationMode {
    Shared,
    Exclusive,
}

pub(super) fn acquire_lock_blocking(path: &Path, mode: MutationMode) -> io::Result<StdFile> {
    let started = std::time::Instant::now();
    let file = loop {
        let mut options = StdOpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        configure_no_follow_blocking(&mut options);
        let file = options.open(path)?;
        let result = match mode {
            MutationMode::Shared => FileExt::try_lock_shared(&file),
            MutationMode::Exclusive => FileExt::try_lock_exclusive(&file),
        };
        match result {
            Ok(()) => break file,
            Err(error) if lock_is_contended(&error) && started.elapsed() < LOCK_WAIT => {
                drop(file);
                std::thread::sleep(LOCK_RETRY);
            }
            Err(error) => return Err(error),
        }
    };
    Ok(file)
}

fn configure_no_follow_blocking(options: &mut StdOpenOptions) {
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
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| path_error("open catalog directory for sync", path, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync catalog directory", path, error))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
