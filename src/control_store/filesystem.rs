use std::io;
use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

pub(super) const CONTROL_STORE_DATABASE_FILE: &str = "control.sqlite3";
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

fn path_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.path_invalid", message)
}

fn io_error(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.control_store.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
