use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use serde::Serialize;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{operation_invalid, operation_io};

pub(super) async fn discard_unpublished_temporary_json(path: &Path, maximum: u64) -> UseResult<()> {
    let temporary = temporary_json_path(path)?;
    match fs::symlink_metadata(&temporary).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() <= maximum =>
        {
            fs::remove_file(&temporary).await.map_err(|error| {
                operation_io(
                    "discard unpublished temporary restore marker",
                    &temporary,
                    error,
                )
            })?;
            sync_parent(&temporary).await
        }
        Ok(_) => Err(operation_invalid(
            "An unpublished temporary restore marker is not an owned bounded file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(operation_io(
            "inspect unpublished temporary restore marker",
            &temporary,
            error,
        )),
    }
}

pub(super) async fn ensure_owned_directory(path: &Path) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(())
        }
        Ok(_) => Err(operation_invalid(
            "A whole-installation restore directory is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .await
                .map_err(|error| operation_io("create restore directory", path, error))?;
            sync_parent(path).await
        }
        Err(error) => Err(operation_io("inspect restore directory", path, error)),
    }
}

pub(super) async fn validate_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(operation_invalid(
            "A whole-installation restore path escapes the state root.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in directory.strip_prefix(root).unwrap().components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .await
            .map_err(|error| operation_io("inspect restore directory chain", &current, error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(operation_invalid(
                "A whole-installation restore directory chain is not owned.",
            ));
        }
    }
    Ok(())
}

pub(super) async fn read_optional_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    maximum: u64,
    label: &str,
) -> UseResult<Option<T>> {
    let before = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(operation_io(&format!("inspect {label}"), path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&before)
        || !before.is_file()
        || before.len() == 0
        || before.len() > maximum
    {
        return Err(operation_invalid(format!(
            "The {label} is not a bounded owned regular file."
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| operation_io(&format!("read {label}"), path, error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| operation_io(&format!("reinspect {label}"), path, error))?;
    if after.len() != before.len()
        || a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || bytes.len() as u64 != before.len()
    {
        return Err(operation_invalid(format!(
            "The {label} changed while it was read."
        )));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| operation_invalid(format!("The {label} is invalid JSON.")))
}

pub(super) async fn write_json<T: Serialize>(
    path: &Path,
    value: &T,
    maximum: u64,
) -> UseResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        operation_invalid(format!(
            "Whole-installation restore evidence cannot be encoded: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > maximum {
        return Err(operation_invalid(
            "Encoded whole-installation restore evidence exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        operation_invalid("A whole-installation restore evidence path has no parent.")
    })?;
    let temporary = temporary_json_path(path)?;
    remove_temporary(&temporary, maximum).await?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| operation_io("create temporary restore evidence", &temporary, error))?;
    file.write_all(&bytes)
        .await
        .map_err(|error| operation_io("write temporary restore evidence", &temporary, error))?;
    file.sync_all()
        .await
        .map_err(|error| operation_io("sync temporary restore evidence", &temporary, error))?;
    drop(file);
    let source = temporary.clone();
    let target = path.to_path_buf();
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_replace_blocking(source, &target)
    })
    .await
    .map_err(|error| {
        operation_invalid(format!(
            "Restore evidence activation worker did not complete: {error}"
        ))
    })?
    .map_err(|error| operation_io("activate restore evidence", &error_target, error))?;
    sync_directory(parent).await
}

pub(super) async fn recover_temporary_json(path: &Path, maximum: u64) -> UseResult<()> {
    let temporary = temporary_json_path(path)?;
    let temp_metadata = match fs::symlink_metadata(&temporary).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(operation_io(
                "inspect temporary restore evidence",
                &temporary,
                error,
            ))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&temp_metadata)
        || !temp_metadata.is_file()
        || temp_metadata.len() == 0
        || temp_metadata.len() > maximum
    {
        return Err(operation_invalid(
            "Temporary whole-installation restore evidence is not a bounded owned file.",
        ));
    }
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            fs::remove_file(&temporary).await.map_err(|error| {
                operation_io(
                    "remove obsolete temporary restore evidence",
                    &temporary,
                    error,
                )
            })?;
        }
        Ok(_) => {
            return Err(operation_invalid(
                "Whole-installation restore evidence target is not an owned file.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::rename(&temporary, path)
                .await
                .map_err(|error| operation_io("recover temporary restore evidence", path, error))?;
        }
        Err(error) => return Err(operation_io("inspect restore evidence target", path, error)),
    }
    sync_parent(path).await
}

async fn remove_temporary(path: &Path, maximum: u64) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() <= maximum =>
        {
            fs::remove_file(path)
                .await
                .map_err(|error| operation_io("remove temporary restore evidence", path, error))?;
            sync_parent(path).await
        }
        Ok(_) => Err(operation_invalid(
            "Temporary whole-installation restore evidence is not an owned file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(operation_io(
            "inspect temporary restore evidence",
            path,
            error,
        )),
    }
}

fn temporary_json_path(path: &Path) -> UseResult<PathBuf> {
    let mut name = path
        .file_name()
        .ok_or_else(|| operation_invalid("Restore evidence has no file name."))?
        .to_os_string();
    name.push(".tmp");
    Ok(path.with_file_name(name))
}

async fn sync_parent(path: &Path) -> UseResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| operation_invalid("A restore path has no parent directory."))?;
    sync_directory(parent).await
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| operation_io("open restore directory", path, error))?
        .sync_all()
        .await
        .map_err(|error| operation_io("sync restore directory", path, error))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
