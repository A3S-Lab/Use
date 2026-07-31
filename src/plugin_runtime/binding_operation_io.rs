use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{PluginOperationPlan, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::binding_operation::{operation_error, RuntimeBindingOperationJournal};
use super::store::{
    activate_temporary, ensure_owned_directory, path_error, sync_parent, unique_suffix,
    validate_existing_directory_chain, RuntimeBindingStore,
};

const MAX_RUNTIME_BINDING_OPERATION_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn operation_path(
    store: &RuntimeBindingStore,
    scope_id: &str,
    operation_id: &str,
) -> UseResult<PathBuf> {
    if !super::model::valid_machine_id(scope_id) {
        return Err(operation_error(
            "A Runtime binding operation scope path identity is invalid.",
        ));
    }
    PluginOperationPlan::validate_operation_id(operation_id)
        .map_err(|_| operation_error("A Runtime binding operation path identity is invalid."))?;
    let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
    let operation_digest = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
    Ok(store
        .root()
        .join(".operations")
        .join(scope_digest)
        .join(format!("{operation_digest}.json")))
}

pub(super) async fn read_optional_operation(
    store: &RuntimeBindingStore,
    path: &Path,
) -> UseResult<Option<RuntimeBindingOperationJournal>> {
    if !validate_existing_directory_chain(store.state_root(), path.parent()).await? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(path_error(
                "inspect Runtime binding operation journal",
                path,
                error,
            ))
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RUNTIME_BINDING_OPERATION_BYTES
    {
        return Err(operation_error(format!(
            "Runtime binding operation journal '{}' is not a bounded regular file.",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Runtime binding operation journal", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_RUNTIME_BINDING_OPERATION_BYTES {
        return Err(operation_error(
            "A Runtime binding operation journal changed outside its size bound while reading.",
        ));
    }
    let journal =
        serde_json::from_slice::<RuntimeBindingOperationJournal>(&bytes).map_err(|error| {
            operation_error(format!(
                "Runtime binding operation journal '{}' is invalid JSON: {error}",
                path.display()
            ))
        })?;
    journal
        .validate()
        .map_err(|_| operation_error("The Runtime binding operation journal is invalid."))?;
    Ok(Some(journal))
}

pub(super) async fn write_operation(
    store: &RuntimeBindingStore,
    path: &Path,
    journal: &RuntimeBindingOperationJournal,
) -> UseResult<()> {
    journal.validate()?;
    let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
        operation_error(format!(
            "Failed to encode Runtime binding operation journal: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_RUNTIME_BINDING_OPERATION_BYTES {
        return Err(operation_error(
            "The Runtime binding operation journal exceeds its storage bound.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| operation_error("A Runtime binding operation path has no parent."))?;
    ensure_owned_directory(store.root(), Some(parent)).await?;
    let temporary = parent.join(format!(".operation-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options.open(&temporary).await.map_err(|error| {
        path_error(
            "create temporary Runtime binding operation journal",
            &temporary,
            error,
        )
    })?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "write temporary Runtime binding operation journal",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "sync temporary Runtime binding operation journal",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(Some(parent)).await
}
