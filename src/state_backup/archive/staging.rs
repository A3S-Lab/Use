use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};

use super::{sync_parent, COPY_BUFFER_BYTES};
use crate::state_backup::{
    state_backup_invalid, state_backup_io, state_backup_limit, StateBackupEntry, StateBackupRoot,
    MAX_STATE_BACKUP_ENTRIES, MAX_STATE_BACKUP_FILE_BYTES,
};

pub(super) struct PendingCandidate {
    pub(super) file: File,
    pub(super) partial: PathBuf,
    pub(super) destination: PathBuf,
}

pub(super) fn prepare_candidate(
    destination: &Path,
    candidate_root: &Path,
    entry: &StateBackupEntry,
) -> UseResult<Option<PendingCandidate>> {
    let parent = destination
        .parent()
        .ok_or_else(|| state_backup_invalid("A restore candidate path has no parent directory."))?;
    ensure_candidate_directory_chain(candidate_root, parent)?;
    let partial = partial_candidate_path(destination)?;
    match std::fs::symlink_metadata(destination) {
        Ok(_) if file_matches_entry(destination, entry)? => {
            remove_stale_candidate(&partial)?;
            return Ok(None);
        }
        Ok(_) => {
            return Err(state_backup_invalid(
                "An existing restore candidate differs from its reviewed evidence.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(state_backup_io(format!(
                "A restore candidate cannot be inspected: {error}"
            )))
        }
    }
    remove_stale_candidate(&partial)?;
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .map_err(|error| {
            state_backup_io(format!(
                "A partial restore candidate cannot be created: {error}"
            ))
        })?;
    Ok(Some(PendingCandidate {
        file,
        partial,
        destination: destination.to_path_buf(),
    }))
}

pub(super) fn ensure_candidate_root(root: &Path) -> UseResult<()> {
    let parent = root
        .parent()
        .ok_or_else(|| state_backup_invalid("A restore candidate root has no parent directory."))?;
    std::fs::create_dir_all(parent).map_err(|error| {
        state_backup_io(format!(
            "A restore candidate parent cannot be created: {error}"
        ))
    })?;
    let parent_metadata = std::fs::symlink_metadata(parent).map_err(|error| {
        state_backup_io(format!(
            "A restore candidate parent cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&parent_metadata)
        || !parent_metadata.is_dir()
    {
        return Err(state_backup_invalid(
            "A restore candidate parent is not an owned directory.",
        ));
    }
    match std::fs::symlink_metadata(root) {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(())
        }
        Ok(_) => Err(state_backup_invalid(
            "A restore candidate root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir(root).map_err(|error| {
                state_backup_io(format!(
                    "A restore candidate root cannot be created: {error}"
                ))
            })?;
            sync_parent(root)
        }
        Err(error) => Err(state_backup_io(format!(
            "A restore candidate root cannot be inspected: {error}"
        ))),
    }
}

fn ensure_candidate_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(state_backup_invalid(
            "A restore candidate path escapes its exact staging root.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in directory.strip_prefix(root).unwrap().components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => {
                return Err(state_backup_invalid(
                    "A restore candidate directory chain is not owned.",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|error| {
                    state_backup_io(format!(
                        "A restore candidate directory cannot be created: {error}"
                    ))
                })?;
                sync_parent(&current)?;
            }
            Err(error) => {
                return Err(state_backup_io(format!(
                    "A restore candidate directory cannot be inspected: {error}"
                )))
            }
        }
    }
    Ok(())
}

fn partial_candidate_path(destination: &Path) -> UseResult<PathBuf> {
    let mut name = destination
        .file_name()
        .ok_or_else(|| state_backup_invalid("A restore candidate has no file name."))?
        .to_os_string();
    name.push(".partial");
    Ok(destination.with_file_name(name))
}

fn remove_stale_candidate(path: &Path) -> UseResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() <= MAX_STATE_BACKUP_FILE_BYTES =>
        {
            make_writable(path, metadata.permissions())?;
            std::fs::remove_file(path).map_err(|error| {
                state_backup_io(format!(
                    "A stale partial restore candidate cannot be removed: {error}"
                ))
            })?;
            sync_parent(path)
        }
        Ok(_) => Err(state_backup_invalid(
            "A partial restore candidate is not a bounded owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(state_backup_io(format!(
            "A partial restore candidate cannot be inspected: {error}"
        ))),
    }
}

pub(super) fn set_candidate_permissions(path: &Path, entry: &StateBackupEntry) -> UseResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        state_backup_io(format!(
            "A staged restore candidate cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(state_backup_invalid(
            "A staged restore candidate is not an owned regular file.",
        ));
    }
    // Keep a write-capable handle across the permission change. Windows
    // rejects FlushFileBuffers on a handle opened after the read-only bit is
    // set, while a pre-existing write handle can durably flush the metadata.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            state_backup_io(format!(
                "Restore candidate permissions cannot be prepared: {error}"
            ))
        })?;
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = entry.unix_mode {
            permissions.set_mode(mode);
        } else {
            permissions.set_readonly(entry.read_only);
        }
    }
    #[cfg(not(unix))]
    permissions.set_readonly(entry.read_only);
    std::fs::set_permissions(path, permissions).map_err(|error| {
        state_backup_io(format!(
            "Restore candidate permissions cannot be preserved: {error}"
        ))
    })?;
    file.sync_all().map_err(|error| {
        state_backup_io(format!(
            "Restore candidate permissions cannot be synchronized: {error}"
        ))
    })
}

pub(super) fn file_matches_entry(path: &Path, entry: &StateBackupEntry) -> UseResult<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(state_backup_io(format!(
                "A restore candidate cannot be inspected: {error}"
            )))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != entry.length
        || metadata.permissions().readonly() != entry.read_only
        || candidate_unix_mode(&metadata) != entry.unix_mode
    {
        return Ok(false);
    }
    let mut file = File::open(path).map_err(|error| {
        state_backup_io(format!("A restore candidate cannot be opened: {error}"))
    })?;
    let mut digest = Sha256::new();
    let mut remaining = entry.length;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = remaining.min(buffer.len() as u64) as usize;
        let count = file.read(&mut buffer[..requested]).map_err(|error| {
            state_backup_io(format!("A restore candidate cannot be read: {error}"))
        })?;
        if count == 0 {
            return Ok(false);
        }
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut extra = [0u8; 1];
    if file.read(&mut extra).map_err(|error| {
        state_backup_io(format!("A restore candidate cannot be finished: {error}"))
    })? != 0
    {
        return Ok(false);
    }
    Ok(format!("sha256:{:x}", digest.finalize()) == entry.sha256)
}

pub(super) fn validate_candidate_tree(
    root: &Path,
    root_kind: StateBackupRoot,
    expected: &BTreeMap<(StateBackupRoot, String), StateBackupEntry>,
) -> UseResult<()> {
    let expected_count = expected
        .keys()
        .filter(|(entry_root, _)| *entry_root == root_kind)
        .count();
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound && expected_count == 0 => {
            return Ok(())
        }
        Err(error) => {
            return Err(state_backup_io(format!(
                "A restore candidate root cannot be inspected: {error}"
            )))
        }
    };
    if expected_count == 0
        || a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_dir()
    {
        return Err(state_backup_invalid(
            "A restore candidate root does not match its selected inventory.",
        ));
    }
    let mut stack = vec![(root.to_path_buf(), PathBuf::new())];
    let mut observed = BTreeSet::new();
    let mut count = 0u64;
    while let Some((directory, relative)) = stack.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            state_backup_io(format!(
                "A restore candidate directory cannot be read: {error}"
            ))
        })?;
        for item in entries {
            let item = item.map_err(|error| {
                state_backup_io(format!("A restore candidate entry cannot be read: {error}"))
            })?;
            count = count.checked_add(1).ok_or_else(|| {
                state_backup_limit("The restore candidate entry count overflowed.")
            })?;
            if count > MAX_STATE_BACKUP_ENTRIES {
                return Err(state_backup_limit(
                    "The restore candidate tree exceeds its entry bound.",
                ));
            }
            let name = item.file_name().into_string().map_err(|_| {
                state_backup_invalid("A restore candidate path is not valid UTF-8.")
            })?;
            let child_relative = relative.join(name);
            let portable = child_relative
                .components()
                .map(|component| {
                    component.as_os_str().to_str().ok_or_else(|| {
                        state_backup_invalid("A restore candidate path is not valid UTF-8.")
                    })
                })
                .collect::<UseResult<Vec<_>>>()?
                .join("/");
            crate::state_backup::inventory::validate_portable_path(&portable)?;
            let path = item.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                state_backup_io(format!("A restore candidate cannot be inspected: {error}"))
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(state_backup_invalid(
                    "A restore candidate tree contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                stack.push((path, child_relative));
                continue;
            }
            if !metadata.is_file() {
                return Err(state_backup_invalid(
                    "A restore candidate tree contains a special filesystem entry.",
                ));
            }
            let key = (root_kind, portable);
            let entry = expected.get(&key).ok_or_else(|| {
                state_backup_invalid("A restore candidate tree contains an unselected file.")
            })?;
            if !observed.insert(key) || !file_matches_entry(&path, entry)? {
                return Err(state_backup_invalid(
                    "A restore candidate file differs from its selected evidence.",
                ));
            }
        }
    }
    if observed.len() != expected_count {
        return Err(state_backup_invalid(
            "The restore candidate inventory is incomplete.",
        ));
    }
    Ok(())
}

fn make_writable(path: &Path, mut permissions: std::fs::Permissions) -> UseResult<()> {
    if permissions.readonly() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions).map_err(|error| {
            state_backup_io(format!(
                "A restore candidate cannot be made removable: {error}"
            ))
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn candidate_unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn candidate_unix_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}
