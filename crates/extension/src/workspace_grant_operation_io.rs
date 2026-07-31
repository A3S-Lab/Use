use std::io;
use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::workspace_grant_io::{activate_temporary_file, sync_parent_directory, unique_suffix};
use super::workspace_grant_operation::{operation_state_error, WorkspaceGrantOperationJournal};

const MAX_WORKSPACE_GRANT_OPERATION_BYTES: u64 = 8 * 1024 * 1024;

pub(super) async fn read_optional_operation(
    path: &Path,
) -> UseResult<Option<WorkspaceGrantOperationJournal>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(operation_io_error(
                "inspect workspace grant operation journal",
                path,
                error,
            ));
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_WORKSPACE_GRANT_OPERATION_BYTES
    {
        return Err(operation_invalid(
            "The workspace grant operation journal is not a bounded regular file.",
        ));
    }
    let bytes = fs::read(path).await.map_err(|error| {
        operation_io_error("read workspace grant operation journal", path, error)
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_WORKSPACE_GRANT_OPERATION_BYTES {
        return Err(operation_invalid(
            "The workspace grant operation journal changed outside its size bound while reading.",
        ));
    }
    let journal =
        serde_json::from_slice::<WorkspaceGrantOperationJournal>(&bytes).map_err(|error| {
            operation_invalid(format!(
                "The workspace grant operation journal is invalid JSON: {error}"
            ))
        })?;
    journal.validate().map_err(|error| {
        operation_invalid(format!(
            "The workspace grant operation journal failed validation: {}",
            error.message
        ))
    })?;
    Ok(Some(journal))
}

pub(super) async fn write_operation(
    path: &Path,
    journal: &WorkspaceGrantOperationJournal,
) -> UseResult<()> {
    journal.validate()?;
    let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
        operation_invalid(format!(
            "Failed to encode workspace grant operation journal: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_WORKSPACE_GRANT_OPERATION_BYTES {
        return Err(operation_invalid(
            "The workspace grant operation journal exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        operation_invalid("The workspace grant operation journal path has no parent.")
    })?;
    let temporary = parent.join(format!(".operation-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options.open(&temporary).await.map_err(|error| {
        operation_io_error(
            "create temporary workspace grant operation journal",
            &temporary,
            error,
        )
    })?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(operation_io_error(
            "write temporary workspace grant operation journal",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(operation_io_error(
            "sync temporary workspace grant operation journal",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) =
        activate_temporary_file(temporary.clone(), path.to_path_buf(), "operation journal").await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(Some(parent), "operation journal").await
}

fn operation_invalid(message: impl Into<String>) -> UseError {
    operation_state_error("use.plugin.grant_operation.invalid", message)
}

fn operation_io_error(action: &str, path: &Path, error: io::Error) -> UseError {
    operation_state_error(
        "use.plugin.grant_operation.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
