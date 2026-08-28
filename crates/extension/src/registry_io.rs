use a3s_use_core::{UseError, UseResult};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::package::{activate_temporary_file, io_error, sync_parent_directory, unique_suffix};
use super::registry::ExtensionRegistrySnapshot;
use super::ExtensionPaths;

pub(super) async fn read_registry_snapshot(
    paths: &ExtensionPaths,
) -> UseResult<ExtensionRegistrySnapshot> {
    let path = paths.registry_snapshot_path();
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ExtensionRegistrySnapshot::empty(paths.installation().clone())
        }
        Err(error) => return Err(io_error("read extension registry snapshot", &path, error)),
    };
    let snapshot: ExtensionRegistrySnapshot = serde_json::from_slice(&bytes).map_err(|error| {
        UseError::new(
            "use.extension.registry_invalid",
            format!(
                "Invalid extension registry snapshot '{}': {error}",
                path.display()
            ),
        )
    })?;
    snapshot.validate()?;
    if snapshot.installation != *paths.installation() {
        return Err(UseError::new(
            "use.extension.registry_scope_mismatch",
            "The extension Registry snapshot belongs to a different installation.",
        ));
    }
    Ok(snapshot)
}

pub(super) async fn write_registry_snapshot(
    paths: &ExtensionPaths,
    snapshot: &ExtensionRegistrySnapshot,
) -> UseResult<()> {
    snapshot.validate()?;
    if snapshot.installation != *paths.installation() {
        return Err(UseError::new(
            "use.extension.registry_scope_mismatch",
            "The extension Registry snapshot belongs to a different installation.",
        ));
    }
    let path = paths.registry_snapshot_path();
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.extension.registry_invalid",
            "The extension registry snapshot has no parent directory.",
        )
    })?;
    fs::create_dir_all(parent)
        .await
        .map_err(|error| io_error("create extension registry directory", parent, error))?;
    let temporary = parent.join(format!(".registry-{}.tmp", unique_suffix()));
    let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| {
        UseError::new(
            "use.extension.registry_invalid",
            format!("Failed to encode extension registry snapshot: {error}"),
        )
    })?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| io_error("create temporary extension registry", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write extension registry snapshot",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "sync extension registry snapshot",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path.to_path_buf(),
        "activate extension registry snapshot",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(parent, "extension registry").await
}
