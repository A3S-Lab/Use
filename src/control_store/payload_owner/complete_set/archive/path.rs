use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{metadata_is_link_or_reparse_point, UseResult};

use super::super::{snapshot_exists, snapshot_io, snapshot_path_invalid};

pub(in crate::control_store::payload_owner::complete_set) fn resolve_destination(
    destination: PathBuf,
    owned_roots: &[PathBuf],
) -> UseResult<PathBuf> {
    let file_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            snapshot_path_invalid("The complete snapshot destination has no file name.")
        })?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent).map_err(|error| {
        snapshot_io(format!(
            "resolve the complete snapshot destination directory: {error}"
        ))
    })?;
    let parent_metadata = std::fs::symlink_metadata(&parent).map_err(|error| {
        snapshot_io(format!(
            "inspect the complete snapshot destination directory: {error}"
        ))
    })?;
    if metadata_is_link_or_reparse_point(&parent_metadata) || !parent_metadata.is_dir() {
        return Err(snapshot_path_invalid(
            "The complete snapshot destination parent is not an owned directory.",
        ));
    }
    let resolved = parent.join(file_name);
    for owned_root in owned_roots {
        if resolved.starts_with(canonical_or_absolute(owned_root)?) {
            return Err(snapshot_path_invalid(
                "A complete snapshot must be written outside the Use data and state roots.",
            ));
        }
    }
    match std::fs::symlink_metadata(&resolved) {
        Ok(_) => Err(snapshot_exists()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(resolved),
        Err(error) => Err(snapshot_io(format!(
            "inspect the complete snapshot destination: {error}"
        ))),
    }
}

pub(in crate::control_store::payload_owner::complete_set) fn publish(
    temporary: tempfile::NamedTempFile,
    destination: &Path,
) -> UseResult<()> {
    a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, destination).map_err(
        |error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                snapshot_exists()
            } else {
                snapshot_io(format!("publish the complete snapshot archive: {error}"))
            }
        },
    )?;
    sync_parent(destination)
}

fn canonical_or_absolute(path: &Path) -> UseResult<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|error| {
                        snapshot_io(format!(
                            "resolve the current directory for an owned root: {error}"
                        ))
                    })?
                    .join(path)
            };
            normalize_lexical(&absolute)
        }
        Err(error) => Err(snapshot_io(format!(
            "resolve a Use-owned root for complete snapshot isolation: {error}"
        ))),
    }
}

fn normalize_lexical(path: &Path) -> UseResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(snapshot_path_invalid(
                        "A Use-owned root cannot be normalized safely.",
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        snapshot_path_invalid("The complete snapshot destination has no parent directory.")
    })?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            snapshot_io(format!(
                "synchronize complete snapshot destination directory: {error}"
            ))
        })
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> UseResult<()> {
    Ok(())
}
