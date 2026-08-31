use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use tokio::fs;

#[derive(Debug, Clone, Copy)]
pub(crate) enum LockMode {
    Shared,
    Exclusive,
}

pub(crate) struct ScopeDatabaseGuard {
    pub(super) path: PathBuf,
    _lock: StdFile,
}

impl ScopeDatabaseGuard {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) async fn prepare_scope_database(
    state_root: &Path,
    root: &Path,
    scope_directory: &Path,
    mode: LockMode,
) -> UseResult<ScopeDatabaseGuard> {
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| io_error("create Knowledge state root", state_root, error))?;
    ensure_owned_directory(state_root, root).await?;
    ensure_owned_directory(state_root, scope_directory).await?;

    let lock_path = scope_directory.join(".knowledge.lock");
    validate_optional_regular_file(&lock_path).await?;
    let error_path = lock_path.clone();
    let lock = tokio::task::spawn_blocking(move || -> io::Result<StdFile> {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        match mode {
            LockMode::Shared => FileExt::lock_shared(&file)?,
            LockMode::Exclusive => FileExt::lock_exclusive(&file)?,
        }
        Ok(file)
    })
    .await
    .map_err(|error| {
        database_error(format!(
            "Failed to acquire Knowledge database lock '{}': blocking task failed: {error}",
            error_path.display()
        ))
    })?
    .map_err(|error| io_error("acquire Knowledge database lock", &error_path, error))?;

    validate_existing_directory(state_root).await?;
    validate_existing_directory(root).await?;
    validate_existing_directory(scope_directory).await?;
    let path = scope_directory.join("knowledge.sqlite3");
    for candidate in [
        path.clone(),
        scope_directory.join("knowledge.sqlite3-wal"),
        scope_directory.join("knowledge.sqlite3-shm"),
    ] {
        validate_optional_regular_file(&candidate).await?;
    }
    Ok(ScopeDatabaseGuard { path, _lock: lock })
}

/// Resolve an existing scope database without creating directories, lock
/// files, or an empty database. The caller must already hold the installation
/// maintenance fence exclusively, so no second per-scope lock is acquired.
pub(crate) async fn existing_scope_database_under_maintenance(
    state_root: &Path,
    root: &Path,
    scope_directory: &Path,
) -> UseResult<Option<PathBuf>> {
    if !root.starts_with(state_root) || !scope_directory.starts_with(root) {
        return Err(path_error(
            "The Knowledge database directory escapes the configured state root.",
        ));
    }
    validate_existing_directory(state_root).await?;
    if !validate_optional_directory_chain(state_root, scope_directory).await? {
        return Ok(None);
    }

    let lock = scope_directory.join(".knowledge.lock");
    optional_regular_file_exists(&lock).await?;
    let path = scope_directory.join("knowledge.sqlite3");
    let database_exists = optional_regular_file_exists(&path).await?;
    let wal_exists =
        optional_regular_file_exists(&scope_directory.join("knowledge.sqlite3-wal")).await?;
    let shm_exists =
        optional_regular_file_exists(&scope_directory.join("knowledge.sqlite3-shm")).await?;
    if !database_exists {
        if wal_exists || shm_exists {
            return Err(path_error(
                "Knowledge database sidecars exist without their owned database.",
            ));
        }
        return Ok(None);
    }
    Ok(Some(path))
}

async fn ensure_owned_directory(state_root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(state_root) {
        return Err(path_error(
            "The Knowledge database directory escapes the configured state root.",
        ));
    }
    validate_existing_directory(state_root).await?;
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| path_error("The Knowledge database directory is not state-owned."))?;
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(io_error(
                    "create Knowledge database directory",
                    &current,
                    error,
                ));
            }
        }
        validate_existing_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect Knowledge database directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(path_error(format!(
            "Knowledge database directory '{}' is not an owned directory.",
            path.display()
        )));
    }
    Ok(())
}

async fn validate_optional_directory_chain(state_root: &Path, target: &Path) -> UseResult<bool> {
    let relative = target
        .strip_prefix(state_root)
        .map_err(|_| path_error("The Knowledge database directory is not state-owned."))?;
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => {
                return Err(path_error(format!(
                    "Knowledge database directory '{}' is not an owned directory.",
                    current.display()
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(io_error(
                    "inspect Knowledge database directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn validate_optional_regular_file(path: &Path) -> UseResult<()> {
    optional_regular_file_exists(path).await.map(|_| ())
}

async fn optional_regular_file_exists(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(true)
        }
        Ok(_) => Err(path_error(format!(
            "Knowledge database file '{}' is not an owned regular file.",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("inspect Knowledge database file", path, error)),
    }
}

fn path_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_database_path_invalid", message)
}

fn io_error(action: &str, path: &Path, error: io::Error) -> UseError {
    database_error(format!("Failed to {action} '{}': {error}", path.display()))
}

fn database_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_database_io", message)
}
