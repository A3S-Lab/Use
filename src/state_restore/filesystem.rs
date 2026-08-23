use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::ExtensionPaths;
use sha2::{Digest, Sha256};

use super::journal::StateRestoreOperation;
use super::{
    maybe_test_crash, StateRestoreAction, StateRestoreActionKind, StateRestoreFileEvidence,
};
use crate::state_backup::{
    stage_state_restore_entries, validate_state_restore_entries, StateBackupEntry, StateBackupRoot,
    MAX_STATE_BACKUP_ENTRIES, MAX_STATE_BACKUP_FILE_BYTES,
};

const COPY_BUFFER_BYTES: usize = 128 * 1024;

pub(super) async fn stage_candidates(
    paths: &ExtensionPaths,
    backup_path: &Path,
    operation: &StateRestoreOperation,
) -> UseResult<()> {
    operation.validate()?;
    let selected = operation
        .plan
        .actions
        .iter()
        .filter(|action| {
            matches!(
                action.action,
                StateRestoreActionKind::Add | StateRestoreActionKind::Replace
            )
        })
        .map(|action| {
            action.after_entry().ok_or_else(|| {
                filesystem_invalid("A restore publication action has no candidate evidence.")
            })
        })
        .collect::<UseResult<Vec<StateBackupEntry>>>()?;
    let roots = candidate_roots(paths, &operation.plan_digest)?;
    let staged = stage_state_restore_entries(
        backup_path,
        operation.plan.backup.clone(),
        selected.clone(),
        roots.data.clone(),
        roots.state.clone(),
    )
    .await;
    if let Err(stage_error) = staged {
        validate_state_restore_entries(
            operation.plan.backup.clone(),
            selected,
            roots.data,
            roots.state,
        )
        .await
        .map_err(|_| stage_error)?;
    }
    maybe_test_crash("candidates-staged");
    Ok(())
}

pub(super) async fn apply_actions(
    paths: &ExtensionPaths,
    operation: &StateRestoreOperation,
) -> UseResult<()> {
    operation.validate()?;
    let paths = paths.clone();
    let operation = operation.clone();
    tokio::task::spawn_blocking(move || apply_actions_blocking(&paths, &operation))
        .await
        .map_err(|error| {
            filesystem_invalid(format!(
                "The restore publication worker did not complete: {error}"
            ))
        })?
}

pub(super) async fn remove_candidates(
    paths: &ExtensionPaths,
    operation: &StateRestoreOperation,
) -> UseResult<()> {
    operation.validate()?;
    let paths = paths.clone();
    let operation = operation.clone();
    tokio::task::spawn_blocking(move || remove_candidates_blocking(&paths, &operation))
        .await
        .map_err(|error| {
            filesystem_invalid(format!(
                "The restore candidate cleanup worker did not complete: {error}"
            ))
        })?
}

pub(super) fn validate_candidates_absent(
    paths: &ExtensionPaths,
    operation: &StateRestoreOperation,
) -> UseResult<()> {
    for root in candidate_roots(paths, &operation.plan_digest)?.as_array() {
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(filesystem_invalid(
                    "A completed restore still retains a hidden candidate root.",
                ))
            }
            Err(error) => {
                return Err(filesystem_io(
                    "inspect completed restore candidate root",
                    root,
                    error,
                ))
            }
        }
    }
    Ok(())
}

fn apply_actions_blocking(
    paths: &ExtensionPaths,
    operation: &StateRestoreOperation,
) -> UseResult<()> {
    let roots = candidate_roots(paths, &operation.plan_digest)?;
    for (index, action) in operation.plan.actions.iter().enumerate() {
        action.validate()?;
        let live_root = live_root(paths, action.root);
        let target = live_root.join(&action.path);
        match action.action {
            StateRestoreActionKind::Retain => {
                if optional_evidence(&target)?.as_ref() != action.after.as_ref() {
                    return Err(filesystem_invalid(
                        "A retained live file differs from the reviewed restore evidence.",
                    ));
                }
            }
            StateRestoreActionKind::Remove => apply_remove(&target, action, index)?,
            StateRestoreActionKind::Add | StateRestoreActionKind::Replace => {
                let candidate_root = roots.for_root(action.root);
                let candidate = candidate_root.join(&action.path);
                apply_publication(
                    live_root,
                    &target,
                    candidate_root,
                    &candidate,
                    action,
                    index,
                )?;
            }
        }
    }
    Ok(())
}

fn apply_remove(target: &Path, action: &StateRestoreAction, index: usize) -> UseResult<()> {
    let before = action.before.as_ref().ok_or_else(|| {
        filesystem_invalid("A restore removal action has no prior file evidence.")
    })?;
    match optional_evidence(target)? {
        None => Ok(()),
        Some(current) if current == *before || same_content(&current, before) => {
            remove_owned_file(target)?;
            maybe_test_crash(&format!("action-{index}-target-removed"));
            Ok(())
        }
        Some(_) => Err(filesystem_invalid(
            "A restore removal target differs from its reviewed prior evidence.",
        )),
    }
}

fn apply_publication(
    live_root: &Path,
    target: &Path,
    candidate_root: &Path,
    candidate: &Path,
    action: &StateRestoreAction,
    index: usize,
) -> UseResult<()> {
    let after = action.after.as_ref().ok_or_else(|| {
        filesystem_invalid("A restore publication action has no candidate evidence.")
    })?;
    ensure_owned_root(live_root)?;
    ensure_directory_chain(
        live_root,
        target.parent().ok_or_else(|| {
            filesystem_invalid("A restore publication target has no parent directory.")
        })?,
    )?;
    validate_directory_chain(
        candidate_root,
        candidate
            .parent()
            .ok_or_else(|| filesystem_invalid("A restore candidate has no parent directory."))?,
    )?;

    let live = optional_evidence(target)?;
    let staged = optional_evidence(candidate)?;
    if live.as_ref() == Some(after) {
        match staged {
            None => return Ok(()),
            Some(staged) if staged == *after => {
                remove_owned_file(candidate)?;
                maybe_test_crash(&format!("action-{index}-duplicate-candidate-removed"));
                return Ok(());
            }
            Some(_) => {
                return Err(filesystem_invalid(
                    "A duplicate restore candidate differs from the published file.",
                ))
            }
        }
    }
    if staged.as_ref() != Some(after) {
        return Err(filesystem_invalid(
            "A restore candidate is missing or differs from its reviewed evidence.",
        ));
    }

    let prior_matches = match (&action.before, &live) {
        (None, None) => true,
        (Some(before), Some(current)) => current == before || same_content(current, before),
        (Some(_), None) => true,
        _ => false,
    };
    if !prior_matches {
        return Err(filesystem_invalid(
            "A restore publication target differs from its reviewed prior evidence.",
        ));
    }
    if live.is_some() {
        remove_owned_file(target)?;
        maybe_test_crash(&format!("action-{index}-target-removed"));
    }
    a3s_use_extension::rename_path_with_windows_retry_blocking(candidate, target)
        .map_err(|error| filesystem_io("publish restore candidate", target, error))?;
    sync_parent(candidate)?;
    if candidate.parent() != target.parent() {
        sync_parent(target)?;
    }
    maybe_test_crash(&format!("action-{index}-candidate-published"));
    if optional_evidence(target)?.as_ref() != Some(after) {
        return Err(filesystem_invalid(
            "A published restore candidate differs from its reviewed evidence.",
        ));
    }
    Ok(())
}

fn remove_candidates_blocking(
    paths: &ExtensionPaths,
    operation: &StateRestoreOperation,
) -> UseResult<()> {
    let roots = candidate_roots(paths, &operation.plan_digest)?;
    for (index, root) in roots.as_array().into_iter().enumerate() {
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(filesystem_io("inspect restore candidate root", root, error)),
            Ok(metadata) => {
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir()
                {
                    return Err(filesystem_invalid(
                        "A restore candidate root is not an owned directory.",
                    ));
                }
            }
        }
        validate_empty_tree(root)?;
        std::fs::remove_dir_all(root)
            .map_err(|error| filesystem_io("remove restore candidate root", root, error))?;
        sync_parent(root)?;
        maybe_test_crash(&format!("candidate-root-{index}-removed"));
    }
    validate_candidates_absent(paths, operation)
}

fn validate_empty_tree(root: &Path) -> UseResult<()> {
    let mut stack = vec![root.to_path_buf()];
    let mut count = 0u64;
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| filesystem_io("read restore candidate directory", &directory, error))?
        {
            let entry = entry.map_err(|error| {
                filesystem_io("read restore candidate entry", &directory, error)
            })?;
            count = count.checked_add(1).ok_or_else(|| {
                filesystem_invalid("The restore candidate entry count overflowed.")
            })?;
            if count > MAX_STATE_BACKUP_ENTRIES {
                return Err(filesystem_invalid(
                    "The restore candidate tree exceeds its entry bound.",
                ));
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| filesystem_io("inspect restore candidate entry", &path, error))?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(filesystem_invalid(
                    "A restore candidate tree contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                stack.push(path);
            } else {
                return Err(filesystem_invalid(
                    "A published restore candidate tree still contains a file or special entry.",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct CandidateRoots {
    data: PathBuf,
    state: PathBuf,
}

impl CandidateRoots {
    fn for_root(&self, root: StateBackupRoot) -> &Path {
        match root {
            StateBackupRoot::Data => &self.data,
            StateBackupRoot::State => &self.state,
        }
    }

    fn as_array(&self) -> [&Path; 2] {
        [&self.data, &self.state]
    }
}

fn candidate_roots(paths: &ExtensionPaths, plan_digest: &str) -> UseResult<CandidateRoots> {
    let digest = plan_digest.strip_prefix("sha256:").filter(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    let digest = digest.ok_or_else(|| {
        filesystem_invalid("The restore plan digest cannot name its candidate roots.")
    })?;
    let name = format!(".state-restore-{digest}");
    Ok(CandidateRoots {
        data: paths.data_root().join(&name),
        state: paths.state_root().join(name),
    })
}

fn live_root(paths: &ExtensionPaths, root: StateBackupRoot) -> &Path {
    match root {
        StateBackupRoot::Data => paths.data_root(),
        StateBackupRoot::State => paths.state_root(),
    }
}

fn ensure_owned_root(root: &Path) -> UseResult<()> {
    std::fs::create_dir_all(root)
        .map_err(|error| filesystem_io("create Use-owned restore root", root, error))?;
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|error| filesystem_io("inspect Use-owned restore root", root, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(filesystem_invalid(
            "A Use-owned restore root is not an owned directory.",
        ));
    }
    Ok(())
}

fn ensure_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(filesystem_invalid(
            "A restore publication path escapes its Use-owned root.",
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
                return Err(filesystem_invalid(
                    "A restore publication directory chain is not owned.",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|error| {
                    filesystem_io("create restore publication directory", &current, error)
                })?;
                sync_parent(&current)?;
            }
            Err(error) => {
                return Err(filesystem_io(
                    "inspect restore publication directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(())
}

fn validate_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(filesystem_invalid(
            "A restore candidate path escapes its staging root.",
        ));
    }
    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|error| filesystem_io("inspect restore candidate root", root, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&root_metadata) || !root_metadata.is_dir() {
        return Err(filesystem_invalid(
            "A restore candidate root is not an owned directory.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in directory.strip_prefix(root).unwrap().components() {
        current.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            filesystem_io("inspect restore candidate directory", &current, error)
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(filesystem_invalid(
                "A restore candidate directory chain is not owned.",
            ));
        }
    }
    Ok(())
}

fn optional_evidence(path: &Path) -> UseResult<Option<StateRestoreFileEvidence>> {
    let before = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(filesystem_io("inspect restore file", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&before)
        || !before.is_file()
        || before.len() > MAX_STATE_BACKUP_FILE_BYTES
    {
        return Err(filesystem_invalid(
            "A restore file is not a bounded owned regular file.",
        ));
    }
    let mut file =
        File::open(path).map_err(|error| filesystem_io("open restore file", path, error))?;
    let mut remaining = before.len();
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = remaining.min(buffer.len() as u64) as usize;
        let count = file
            .read(&mut buffer[..requested])
            .map_err(|error| filesystem_io("read restore file", path, error))?;
        if count == 0 {
            return Err(filesystem_invalid(
                "A restore file changed while its evidence was read.",
            ));
        }
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut extra = [0u8; 1];
    if file
        .read(&mut extra)
        .map_err(|error| filesystem_io("finish restore file", path, error))?
        != 0
    {
        return Err(filesystem_invalid(
            "A restore file grew while its evidence was read.",
        ));
    }
    let after = std::fs::symlink_metadata(path)
        .map_err(|error| filesystem_io("reinspect restore file", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != before.len()
        || after.permissions().readonly() != before.permissions().readonly()
        || unix_mode(&after) != unix_mode(&before)
    {
        return Err(filesystem_invalid(
            "A restore file changed while its evidence was read.",
        ));
    }
    Ok(Some(StateRestoreFileEvidence {
        length: before.len(),
        sha256: format!("sha256:{:x}", digest.finalize()),
        read_only: before.permissions().readonly(),
        unix_mode: unix_mode(&before),
    }))
}

fn same_content(left: &StateRestoreFileEvidence, right: &StateRestoreFileEvidence) -> bool {
    left.length == right.length && left.sha256 == right.sha256
}

fn remove_owned_file(path: &Path) -> UseResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| filesystem_io("inspect restore removal target", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(filesystem_invalid(
            "A restore removal target is not an owned regular file.",
        ));
    }
    if metadata.permissions().readonly() {
        make_writable(path, metadata.permissions())?;
    }
    std::fs::remove_file(path)
        .map_err(|error| filesystem_io("remove restore target", path, error))?;
    sync_parent(path)
}

fn make_writable(path: &Path, mut permissions: std::fs::Permissions) -> UseResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)
        .map_err(|error| filesystem_io("make restore target removable", path, error))
}

fn sync_parent(path: &Path) -> UseResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| filesystem_invalid("A restore filesystem path has no parent."))?;
    sync_directory(parent)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> UseResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| filesystem_io("sync restore directory", path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

#[cfg(unix)]
fn unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn unix_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

fn filesystem_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.state_restore_filesystem_invalid", message)
}

fn filesystem_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.state_restore_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
