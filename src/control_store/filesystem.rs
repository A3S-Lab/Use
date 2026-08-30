use std::io;
use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

pub(super) const CONTROL_STORE_DATABASE_FILE: &str = "control.sqlite3";
pub(super) const CONTROL_STORE_RESTORE_FILE: &str = ".control-restore.sqlite3";
const CONTROL_STORE_WAL_FILE: &str = "control.sqlite3-wal";
const CONTROL_STORE_SHM_FILE: &str = "control.sqlite3-shm";
const CONTROL_STORE_JOURNAL_FILE: &str = "control.sqlite3-journal";
const OPERATIONAL_LOCK_FILES: &[&str] = &[".installation-mutation.lock", ".maintenance.lock"];

pub(super) async fn prepare_initialization(
    state_root: &Path,
    database_path: &Path,
) -> UseResult<()> {
    validate_database_identity(state_root, database_path)?;
    let inventory = inspect_root(state_root).await?;
    if !inventory.database && (inventory.wal || inventory.shared_memory || inventory.journal) {
        return Err(path_error(
            "Control Store sidecar state exists without its database.",
        ));
    }
    Ok(())
}

pub(super) async fn require_initialized(state_root: &Path, database_path: &Path) -> UseResult<()> {
    validate_database_identity(state_root, database_path)?;
    let inventory = inspect_root(state_root).await?;
    if !inventory.database {
        return Err(UseError::new(
            "use.control_store.not_initialized",
            "The installation Control Store has not been initialized.",
        ));
    }
    Ok(())
}

pub(super) async fn validate_initialized(state_root: &Path, database_path: &Path) -> UseResult<()> {
    require_initialized(state_root, database_path).await
}

pub(super) async fn prepare_clean_restore(
    state_root: &Path,
    database_path: &Path,
) -> UseResult<std::path::PathBuf> {
    validate_database_identity(state_root, database_path)?;
    let inventory = inspect_root(state_root).await?;
    if inventory.database || inventory.wal || inventory.shared_memory || inventory.journal {
        return Err(UseError::new(
            "use.control_store.restore_target_not_empty",
            "A Control Store restore requires a clean installation state root.",
        ));
    }
    let physical_root = physical_root(state_root).await?;
    Ok(physical_root.join(CONTROL_STORE_RESTORE_FILE))
}

pub(super) async fn activate_clean_restore(
    state_root: &Path,
    database_path: &Path,
) -> UseResult<()> {
    validate_database_identity(state_root, database_path)?;
    let staging = state_root.join(CONTROL_STORE_RESTORE_FILE);
    validate_regular_file(&staging).await?;
    for sidecar in restore_sidecars(state_root) {
        if fs::symlink_metadata(&sidecar).await.is_ok() {
            return Err(path_error(
                "A staged Control Store restore retained an operational sidecar.",
            ));
        }
    }
    match fs::symlink_metadata(database_path).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(UseError::new(
                "use.control_store.restore_target_not_empty",
                "The Control Store restore target appeared before activation.",
            ))
        }
        Err(error) => {
            return Err(io_error(
                "inspect Control Store restore target",
                database_path,
                error,
            ))
        }
    }
    fs::rename(&staging, database_path).await.map_err(|error| {
        io_error(
            "activate staged Control Store restore",
            database_path,
            error,
        )
    })?;
    sync_directory(state_root).await?;
    validate_initialized(state_root, database_path).await
}

pub(super) async fn remove_failed_restore(state_root: &Path) -> UseResult<()> {
    let mut paths = vec![state_root.join(CONTROL_STORE_RESTORE_FILE)];
    paths.extend(restore_sidecars(state_root));
    for path in paths {
        match fs::symlink_metadata(&path).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_file() =>
            {
                fs::remove_file(&path).await.map_err(|error| {
                    io_error("remove failed Control Store restore staging", &path, error)
                })?;
            }
            Ok(_) => {
                return Err(path_error(
                    "Failed Control Store restore staging is not an owned regular file.",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(io_error(
                    "inspect failed Control Store restore staging",
                    &path,
                    error,
                ))
            }
        }
    }
    Ok(())
}

/// Resolve platform path aliases before opening SQLite with `NOFOLLOW`.
///
/// macOS exposes temporary paths through `/var`, which is an operating-system
/// symlink to `/private/var`. SQLite intentionally rejects any symlink in a
/// `NOFOLLOW` filename, including ancestors. The maintenance boundary and root
/// inventory validate the logical root first; opening its physical equivalent
/// preserves final-database link protection without rejecting that platform
/// alias.
pub(super) async fn physical_database_path(
    state_root: &Path,
    database_path: &Path,
) -> UseResult<std::path::PathBuf> {
    validate_database_identity(state_root, database_path)?;
    validate_owned_directory(state_root).await?;
    Ok(physical_root(state_root)
        .await?
        .join(CONTROL_STORE_DATABASE_FILE))
}

async fn physical_root(state_root: &Path) -> UseResult<std::path::PathBuf> {
    validate_owned_directory(state_root).await?;
    let physical_root = fs::canonicalize(state_root).await.map_err(|error| {
        io_error(
            "resolve physical Control Store state root",
            state_root,
            error,
        )
    })?;
    validate_owned_directory(&physical_root).await?;
    Ok(physical_root)
}

fn restore_sidecars(state_root: &Path) -> [std::path::PathBuf; 3] {
    [
        state_root.join(format!("{CONTROL_STORE_RESTORE_FILE}-wal")),
        state_root.join(format!("{CONTROL_STORE_RESTORE_FILE}-shm")),
        state_root.join(format!("{CONTROL_STORE_RESTORE_FILE}-journal")),
    ]
}

#[derive(Debug, Default)]
struct RootInventory {
    database: bool,
    wal: bool,
    shared_memory: bool,
    journal: bool,
}

async fn inspect_root(state_root: &Path) -> UseResult<RootInventory> {
    validate_owned_directory(state_root).await?;
    let mut inventory = RootInventory::default();
    let mut entries = fs::read_dir(state_root)
        .await
        .map_err(|error| io_error("read Control Store state root", state_root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("read Control Store state entry", state_root, error))?
    {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| path_error("The Control Store state root contains a non-UTF-8 entry."))?;
        let path = entry.path();
        match name.as_str() {
            CONTROL_STORE_DATABASE_FILE => {
                validate_regular_file(&path).await?;
                inventory.database = true;
            }
            CONTROL_STORE_WAL_FILE => {
                validate_regular_file(&path).await?;
                inventory.wal = true;
            }
            CONTROL_STORE_SHM_FILE => {
                validate_regular_file(&path).await?;
                inventory.shared_memory = true;
            }
            CONTROL_STORE_JOURNAL_FILE => {
                validate_regular_file(&path).await?;
                inventory.journal = true;
            }
            name if OPERATIONAL_LOCK_FILES.contains(&name) => {
                validate_regular_file(&path).await?;
            }
            _ => {
                return Err(UseError::new(
                    "use.control_store.legacy_state_unsupported",
                    "The installation state root contains authority outside the inactive Control Store kernel.",
                )
                .with_detail("entry", name));
            }
        }
    }
    Ok(inventory)
}

fn validate_database_identity(state_root: &Path, database_path: &Path) -> UseResult<()> {
    if database_path != state_root.join(CONTROL_STORE_DATABASE_FILE) {
        return Err(path_error(
            "The Control Store database path is not owned by its installation state root.",
        ));
    }
    Ok(())
}

async fn validate_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect Control Store state root", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(path_error(
            "The Control Store state root is not an owned directory.",
        ));
    }
    Ok(())
}

async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect Control Store file", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(path_error(
            "A Control Store database, sidecar, or lock is not an owned regular file.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| io_error("open Control Store state root for sync", path, error))?
        .sync_all()
        .await
        .map_err(|error| io_error("sync Control Store state root", path, error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn path_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.path_invalid", message)
}

fn io_error(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.control_store.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
