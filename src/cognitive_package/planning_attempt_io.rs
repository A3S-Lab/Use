use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{PluginPackageId, UseError, UseResult};
use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::package_manager_error;

#[derive(Debug, Clone, Copy)]
pub(super) enum PlanningAttemptKind {
    Download,
    Resolution,
}

#[derive(Debug)]
pub(super) struct PackagePlanningLock {
    package_id: String,
    _file: StdFile,
}

impl PackagePlanningLock {
    pub(super) fn validates(&self, package_id: &str) -> bool {
        self.package_id == package_id
    }
}

pub(super) fn planning_lock_root(state_root: &Path) -> PathBuf {
    state_root.join("operations/package-downloads/locks")
}

pub(super) fn package_relative_path(
    package_id: &str,
    extension: &str,
    kind: PlanningAttemptKind,
) -> UseResult<PathBuf> {
    PluginPackageId::parse(package_id.to_owned()).map_err(|_| store_invalid(kind))?;
    let (publisher, package) = package_id
        .split_once('/')
        .ok_or_else(|| store_invalid(kind))?;
    Ok(Path::new(publisher).join(format!("{package}.{extension}")))
}

pub(super) async fn acquire_package_lock(
    state_root: &Path,
    package_id: &str,
    kind: PlanningAttemptKind,
) -> UseResult<PackagePlanningLock> {
    let path =
        planning_lock_root(state_root).join(package_relative_path(package_id, "lock", kind)?);
    let parent = path.parent().ok_or_else(|| store_invalid(kind))?;
    ensure_owned_directory(state_root, parent, kind).await?;
    match fs::symlink_metadata(&path).await {
        Ok(metadata)
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file() =>
        {
            return Err(store_invalid(kind))
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(path_error(
                kind,
                "inspect planning attempt lock",
                &path,
                error,
            ))
        }
    }
    let package_id = package_id.to_owned();
    let lock_package_id = package_id.clone();
    let file = tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| path_error(kind, "open planning attempt lock", &path, error))?;
        file.try_lock_exclusive().map_err(|error| {
            if lock_is_contended(&error) {
                busy_error(kind)
            } else {
                path_error(kind, "lock planning attempt", &path, error)
            }
        })?;
        Ok(file)
    })
    .await
    .map_err(|error| {
        package_manager_error(
            io_code(kind),
            format!("Failed to join the package planning lock task: {error}"),
        )
    })??;
    Ok(PackagePlanningLock {
        package_id: lock_package_id,
        _file: file,
    })
}

pub(super) async fn read_optional_json<T: DeserializeOwned>(
    path: &Path,
    max_bytes: u64,
    kind: PlanningAttemptKind,
) -> UseResult<Option<T>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error(kind, "inspect planning attempt", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(store_invalid(kind));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error(kind, "read planning attempt", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        return Err(store_invalid(kind));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| store_invalid(kind))
}

pub(super) async fn write_json<T: Serialize>(
    state_root: &Path,
    path: &Path,
    record: &T,
    max_bytes: u64,
    kind: PlanningAttemptKind,
) -> UseResult<()> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(store_invalid(kind));
    }
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| store_invalid(kind))?;
    if bytes.is_empty() || bytes.len().saturating_add(1) as u64 > max_bytes {
        return Err(store_invalid(kind));
    }
    let parent = path.parent().ok_or_else(|| store_invalid(kind))?;
    ensure_owned_directory(state_root, parent, kind).await?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{suffix}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| {
            path_error(kind, "create temporary planning attempt", &temporary, error)
        })?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        Ok::<_, io::Error>(())
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(kind, "commit planning attempt", path, error));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(temporary.clone(), path.to_path_buf(), kind).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(parent, kind).await
}

pub(super) async fn remove_file(
    state_root: &Path,
    path: &Path,
    kind: PlanningAttemptKind,
) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| store_invalid(kind))?;
    if !validate_existing_directory_chain(state_root, parent, kind).await? {
        return Err(store_invalid(kind));
    }
    fs::remove_file(path)
        .await
        .map_err(|error| path_error(kind, "remove planning attempt", path, error))?;
    sync_parent(parent, kind).await
}

pub(super) async fn validate_existing_directory_chain(
    state_root: &Path,
    directory: &Path,
    kind: PlanningAttemptKind,
) -> UseResult<bool> {
    if !directory.starts_with(state_root) {
        return Err(store_invalid(kind));
    }
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| store_invalid(kind))?;
    let mut current = state_root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(store_invalid(kind)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    kind,
                    "inspect planning attempt directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn activate_temporary_file(
    temporary: PathBuf,
    target: PathBuf,
    kind: PlanningAttemptKind,
) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        package_manager_error(
            io_code(kind),
            format!("Failed to join the package planning commit task: {error}"),
        )
    })?
    .map_err(|error| path_error(kind, "commit planning attempt", &error_target, error))
}

async fn ensure_owned_directory(
    state_root: &Path,
    directory: &Path,
    kind: PlanningAttemptKind,
) -> UseResult<()> {
    if !directory.starts_with(state_root) {
        return Err(store_invalid(kind));
    }
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_error(kind, "create planning state root", state_root, error))?;
    validate_directory(state_root, kind).await?;
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| store_invalid(kind))?;
    let mut current = state_root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    kind,
                    "create planning attempt directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current, kind).await?;
    }
    Ok(())
}

async fn validate_directory(path: &Path, kind: PlanningAttemptKind) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error(kind, "inspect planning attempt directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(store_invalid(kind));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path, kind: PlanningAttemptKind) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error(kind, "open planning attempt directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error(kind, "sync planning attempt directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path, _kind: PlanningAttemptKind) -> UseResult<()> {
    Ok(())
}

fn busy_error(kind: PlanningAttemptKind) -> UseError {
    match kind {
        PlanningAttemptKind::Download => package_manager_error(
            "use.plugin.package_download_attempt_busy",
            "Another pre-plan download attempt is active for this cognitive package.",
        ),
        PlanningAttemptKind::Resolution => package_manager_error(
            "use.plugin.package_resolution_attempt_busy",
            "Another pre-lock Registry resolution is active for this cognitive package.",
        ),
    }
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx reports sharing or lock violations without consistently
        // mapping either Windows error to WouldBlock.
        return matches!(error.raw_os_error(), Some(32 | 33));
    }
    #[cfg(not(windows))]
    false
}

fn io_code(kind: PlanningAttemptKind) -> &'static str {
    match kind {
        PlanningAttemptKind::Download => "use.plugin.package_download_attempt_io",
        PlanningAttemptKind::Resolution => "use.plugin.package_resolution_attempt_io",
    }
}

fn path_error(
    kind: PlanningAttemptKind,
    operation: &str,
    path: &Path,
    error: io::Error,
) -> UseError {
    package_manager_error(
        io_code(kind),
        format!("Failed to {operation} '{}': {error}", path.display()),
    )
}

pub(super) fn store_invalid(kind: PlanningAttemptKind) -> UseError {
    match kind {
        PlanningAttemptKind::Download => package_manager_error(
            "use.plugin.package_download_attempt_store_invalid",
            "The retained pre-plan package download evidence is invalid.",
        ),
        PlanningAttemptKind::Resolution => package_manager_error(
            "use.plugin.package_resolution_attempt_store_invalid",
            "The retained pre-lock Registry resolution evidence is invalid.",
        ),
    }
}
