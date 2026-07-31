use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::UseResult;
use fs2::FileExt;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::workspace_grant::{record_error, store_error, StoredWorkspaceGrant};

const MAX_WORKSPACE_GRANT_RECORD_BYTES: u64 = 1024 * 1024;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) async fn acquire_lock(state_root: &Path, root: &Path) -> UseResult<StdFile> {
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_io_error("create workspace grant state root", state_root, error))?;
    validate_directory(state_root).await?;
    ensure_owned_directory(state_root, Some(root)).await?;
    let lock_path = root.join(".store.lock");
    match fs::symlink_metadata(&lock_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(store_error(
                "use.plugin.grant_store.path_invalid",
                "The workspace grant store lock is not an owned regular file.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(path_io_error(
                "inspect workspace grant store lock",
                &lock_path,
                error,
            ));
        }
    }
    let error_path = lock_path.clone();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.plugin.grant_store.io",
            format!(
                "Failed to acquire workspace grant lock '{}': blocking task failed: {error}",
                error_path.display()
            ),
        )
    })?
    .map_err(|error| path_io_error("acquire workspace grant store lock", &error_path, error))
}

pub(super) async fn ensure_owned_directory(root: &Path, parent: Option<&Path>) -> UseResult<()> {
    let parent = parent.ok_or_else(invalid_path)?;
    if !parent.starts_with(root) {
        return Err(invalid_path());
    }
    let relative = parent.strip_prefix(root).map_err(|_| invalid_path())?;
    let mut current = root.to_path_buf();
    validate_directory(&current).await?;
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_io_error(
                    "create workspace grant directory",
                    &current,
                    error,
                ));
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_existing_directory_chain(
    root: &Path,
    parent: Option<&Path>,
) -> UseResult<bool> {
    let parent = parent.ok_or_else(invalid_path)?;
    if !parent.starts_with(root) {
        return Err(invalid_path());
    }
    let relative = parent.strip_prefix(root).map_err(|_| invalid_path())?;
    let mut current = root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => {
                return Err(store_error(
                    "use.plugin.grant_store.path_invalid",
                    format!(
                        "Workspace grant directory '{}' is not an owned real directory.",
                        current.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_io_error(
                    "inspect workspace grant directory",
                    &current,
                    error,
                ));
            }
        }
    }
    Ok(true)
}

pub(super) async fn read_optional_record(path: &Path) -> UseResult<Option<StoredWorkspaceGrant>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(path_io_error("inspect workspace grant record", path, error));
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_WORKSPACE_GRANT_RECORD_BYTES
    {
        return Err(record_error(format!(
            "Workspace grant record '{}' is not a bounded regular file.",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_io_error("read workspace grant record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_WORKSPACE_GRANT_RECORD_BYTES {
        return Err(record_error(
            "A workspace grant record changed outside its size bound while reading.",
        ));
    }
    let record = serde_json::from_slice::<StoredWorkspaceGrant>(&bytes).map_err(|error| {
        record_error(format!(
            "Workspace grant record '{}' is invalid JSON: {error}",
            path.display()
        ))
    })?;
    record.validate().map_err(|error| {
        record_error(format!(
            "Workspace grant record '{}' failed validation: {}",
            path.display(),
            error.message
        ))
    })?;
    Ok(Some(record))
}

pub(super) async fn write_record(path: &Path, record: &StoredWorkspaceGrant) -> UseResult<()> {
    record.validate()?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| {
        record_error(format!(
            "Failed to encode the workspace grant record: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_WORKSPACE_GRANT_RECORD_BYTES {
        return Err(record_error(
            "The workspace grant record exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(invalid_path)?;
    let temporary = parent.join(format!(".grant-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| path_io_error("create temporary workspace grant", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_io_error(
            "write temporary workspace grant",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_io_error(
            "sync temporary workspace grant",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) =
        activate_temporary_file(temporary.clone(), path.to_path_buf(), "grant record").await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(Some(parent), "grant record").await
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_io_error("inspect workspace grant directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(store_error(
            "use.plugin.grant_store.path_invalid",
            format!(
                "Workspace grant directory '{}' is not an owned real directory.",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(super) async fn activate_temporary_file(
    temporary: PathBuf,
    target: PathBuf,
    label: &str,
) -> UseResult<()> {
    let error_target = target.clone();
    let error_label = label.to_string();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.plugin.grant_store.io",
            format!(
                "Failed to activate workspace grant {error_label} '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| {
        path_io_error(
            &format!("activate workspace grant {label}"),
            &error_target,
            error,
        )
    })
}

#[cfg(unix)]
pub(super) async fn sync_parent_directory(parent: Option<&Path>, label: &str) -> UseResult<()> {
    let parent = parent.ok_or_else(invalid_path)?;
    fs::File::open(parent)
        .await
        .map_err(|error| path_io_error("open workspace grant directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| {
            path_io_error(
                &format!("sync workspace grant {label} directory"),
                parent,
                error,
            )
        })
}

#[cfg(not(unix))]
pub(super) async fn sync_parent_directory(_parent: Option<&Path>, _label: &str) -> UseResult<()> {
    Ok(())
}

pub(super) fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn invalid_path() -> a3s_use_core::UseError {
    store_error(
        "use.plugin.grant_store.path_invalid",
        "A workspace grant record path is invalid.",
    )
}

fn path_io_error(action: &str, path: &Path, error: io::Error) -> a3s_use_core::UseError {
    store_error(
        "use.plugin.grant_store.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
