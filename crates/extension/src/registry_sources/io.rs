use std::fs::{File, OpenOptions};
use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::package::{
    activate_temporary_file, io_error, lock_is_contended, sync_parent_directory, unique_suffix,
};
use crate::remote::MAX_BOOTSTRAP_ROOT_BYTES;
use crate::ExtensionPaths;

use super::acl::{decode, RegistrySourcesDocument};

const MAX_REGISTRY_SOURCES_ACL_BYTES: u64 = 256 * 1024;

pub(super) struct RegistrySourcesLock(File);

impl RegistrySourcesLock {
    pub(super) fn acquire(paths: &ExtensionPaths) -> UseResult<Self> {
        let path = paths.registry_sources_lock_path();
        let parent = path
            .parent()
            .ok_or_else(|| source_io_error("Registry source lock path has no parent directory."))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| io_error("create Registry source state directory", parent, error))?;
        validate_directory_sync(parent)?;
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(source_io_error(
                    "Registry source lock must be a regular owned file.",
                ));
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_error("open Registry source lock", &path, error))?;
        file.try_lock_exclusive().map_err(|error| {
            if lock_is_contended(&error) {
                UseError::new(
                    "use.extension.registry_sources_busy",
                    "Another process is mutating Registry source configuration.",
                )
            } else {
                io_error("acquire Registry source lock", &path, error)
            }
        })?;
        Ok(Self(file))
    }
}

impl Drop for RegistrySourcesLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) async fn load(paths: &ExtensionPaths) -> UseResult<RegistrySourcesDocument> {
    let path = paths.registry_sources_path();
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RegistrySourcesDocument::default())
        }
        Err(error) => {
            return Err(io_error(
                "inspect Registry source configuration",
                &path,
                error,
            ))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(source_io_error(
            "Registry source configuration must be a regular owned file.",
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_REGISTRY_SOURCES_ACL_BYTES {
        return Err(source_io_error(format!(
            "Registry source configuration must contain between 1 and {MAX_REGISTRY_SOURCES_ACL_BYTES} bytes."
        )));
    }
    let bytes = fs::read(&path)
        .await
        .map_err(|error| io_error("read Registry source configuration", &path, error))?;
    let input = std::str::from_utf8(&bytes).map_err(|error| {
        source_io_error(format!(
            "Registry source configuration must be UTF-8 A3S ACL: {error}"
        ))
    })?;
    decode(input, paths)
}

pub(super) async fn write(
    paths: &ExtensionPaths,
    document: &RegistrySourcesDocument,
) -> UseResult<()> {
    let path = paths.registry_sources_path();
    let parent = path
        .parent()
        .ok_or_else(|| {
            source_io_error("Registry source configuration path has no parent directory.")
        })?
        .to_path_buf();
    fs::create_dir_all(&parent)
        .await
        .map_err(|error| io_error("create Registry source state directory", &parent, error))?;
    validate_directory(&parent).await?;
    let bytes = document.encode().into_bytes();
    if bytes.is_empty() || bytes.len() as u64 > MAX_REGISTRY_SOURCES_ACL_BYTES {
        return Err(source_io_error(
            "Generated Registry source configuration exceeds its storage bound.",
        ));
    }
    let temporary = parent.join(format!(".registries-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| io_error("create Registry source configuration", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write Registry source configuration",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "sync Registry source configuration",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path,
        "activate Registry source configuration",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(&parent, "Registry source configuration").await
}

pub(super) async fn import_trusted_root(
    paths: &ExtensionPaths,
    source: &Path,
    root_sha256: &str,
) -> UseResult<()> {
    if !source.is_absolute() {
        return Err(source_io_error(
            "An imported Registry trusted root path must be absolute.",
        ));
    }
    let metadata = fs::symlink_metadata(source)
        .await
        .map_err(|error| io_error("inspect imported Registry trusted root", source, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(source_io_error(
            "An imported Registry trusted root must be a regular file, not a link or reparse point.",
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_BOOTSTRAP_ROOT_BYTES {
        return Err(source_io_error(format!(
            "An imported Registry trusted root must contain between 1 and {MAX_BOOTSTRAP_ROOT_BYTES} bytes."
        )));
    }
    let bytes = fs::read(source)
        .await
        .map_err(|error| io_error("read imported Registry trusted root", source, error))?;
    verify_trusted_root_bytes(&bytes, root_sha256)?;
    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
        source_io_error(format!(
            "An imported Registry trusted root must be valid JSON: {error}"
        ))
    })?;

    let target = paths.registry_trusted_root_path(root_sha256)?;
    let parent = target
        .parent()
        .ok_or_else(|| {
            source_io_error("Managed Registry trusted root path has no parent directory.")
        })?
        .to_path_buf();
    fs::create_dir_all(&parent).await.map_err(|error| {
        io_error(
            "create managed Registry trusted-root directory",
            &parent,
            error,
        )
    })?;
    validate_directory(&parent).await?;
    match fs::symlink_metadata(&target).await {
        Ok(_) => return validate_managed_trusted_root(&target, root_sha256).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(io_error(
                "inspect managed Registry trusted root",
                &target,
                error,
            ))
        }
    }
    let temporary = parent.join(format!(".root-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| io_error("create managed Registry trusted root", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write managed Registry trusted root",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "sync managed Registry trusted root",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        target,
        "activate managed Registry trusted root",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(&parent, "managed Registry trusted root").await
}

pub(super) async fn validate_managed_trusted_root(path: &Path, root_sha256: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect managed Registry trusted root", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(source_io_error(
            "A managed Registry trusted root must be a regular file, not a link or reparse point.",
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_BOOTSTRAP_ROOT_BYTES {
        return Err(source_io_error(
            "A managed Registry trusted root is outside its one MiB bound.",
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| io_error("read managed Registry trusted root", path, error))?;
    verify_trusted_root_bytes(&bytes, root_sha256)
}

fn verify_trusted_root_bytes(bytes: &[u8], expected: &str) -> UseResult<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(UseError::new(
            "use.extension.registry_root_mismatch",
            "Imported Registry trusted root does not match its pinned SHA-256.",
        )
        .with_detail("expected", expected.to_owned())
        .with_detail("actual", actual))
    }
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect Registry source state directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        Err(source_io_error(
            "Registry source state must use a real owned directory.",
        ))
    } else {
        Ok(())
    }
}

fn validate_directory_sync(path: &Path) -> UseResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_error("inspect Registry source state directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        Err(source_io_error(
            "Registry source state must use a real owned directory.",
        ))
    } else {
        Ok(())
    }
}

fn source_io_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_sources_invalid", message)
}
