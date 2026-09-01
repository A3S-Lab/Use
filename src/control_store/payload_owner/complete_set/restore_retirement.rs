//! Crash-safe retirement of complete-restore staging payloads.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use tokio::fs;

use super::restore::{
    restore_activation_invalid, restore_activation_io, ControlInstallationRestoreAttempt,
    RestoreComponent, MAX_RESTORE_ATTEMPT_BYTES,
};
use super::restore_activation::{self, ControlInstallationRestoreResult};
use super::restore_activation_filesystem::ACTIVATION_FILE;
use super::restore_activation_storage::sync_directory;
use super::restore_filesystem::{self, ATTEMPT_FILE};

const MAX_RETIREMENT_ENTRIES: usize = 500_000;
const MAX_RETIREMENT_DEPTH: usize = 64;

pub(in crate::control_store) fn validate_terminal_receipt_blocking(
    attempt: &Path,
) -> UseResult<InstallationId> {
    let before = inspect_terminal_receipt_blocking(attempt)?;
    let attempt_bytes = read_terminal_file_blocking(
        &attempt.join(ATTEMPT_FILE),
        *before.get(ATTEMPT_FILE).ok_or_else(|| {
            restore_activation_invalid(
                "The terminal complete restore receipt omits its attempt descriptor.",
            )
        })?,
        MAX_RESTORE_ATTEMPT_BYTES as u64,
    )?;
    let descriptor = ControlInstallationRestoreAttempt::decode_canonical(&attempt_bytes)?;
    let activation_bytes = read_terminal_file_blocking(
        &attempt.join(ACTIVATION_FILE),
        *before.get(ACTIVATION_FILE).ok_or_else(|| {
            restore_activation_invalid(
                "The terminal complete restore receipt omits its activation journal.",
            )
        })?,
        super::restore_activation_filesystem::MAX_ACTIVATION_BYTES,
    )?;
    let activation =
        super::restore_activation::ControlInstallationRestoreActivation::decode_canonical(
            &activation_bytes,
            descriptor.descriptor_digest(),
        )?;
    if !activation.is_complete() {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt has an incomplete activation journal.",
        ));
    }
    activation.completed_result(descriptor.descriptor_digest())?;
    if inspect_terminal_receipt_blocking(attempt)? != before {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt changed while it was inspected.",
        ));
    }
    Ok(descriptor.installation().clone())
}

fn inspect_terminal_receipt_blocking(attempt: &Path) -> UseResult<BTreeMap<String, u64>> {
    let metadata = std::fs::symlink_metadata(attempt)
        .map_err(|error| restore_activation_io("inspect terminal restore receipt", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt is not an owned directory.",
        ));
    }
    let mut inventory = BTreeMap::new();
    for entry in std::fs::read_dir(attempt)
        .map_err(|error| restore_activation_io("read terminal restore receipt", error))?
    {
        let entry = entry
            .map_err(|error| restore_activation_io("read terminal restore receipt entry", error))?;
        let name = entry.file_name().into_string().map_err(|_| {
            restore_activation_invalid(
                "The terminal complete restore receipt contains a non-UTF-8 entry.",
            )
        })?;
        if name != ATTEMPT_FILE && name != ACTIVATION_FILE {
            return Err(restore_activation_invalid(
                "The terminal complete restore receipt contains staging or unknown evidence.",
            ));
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
            restore_activation_io("inspect terminal restore receipt entry", error)
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || !metadata.is_file()
            || inventory.insert(name, metadata.len()).is_some()
        {
            return Err(restore_activation_invalid(
                "A terminal complete restore receipt entry is not one owned regular file.",
            ));
        }
    }
    if inventory.len() != 2
        || !inventory.contains_key(ATTEMPT_FILE)
        || !inventory.contains_key(ACTIVATION_FILE)
    {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt is incomplete.",
        ));
    }
    Ok(inventory)
}

fn read_terminal_file_blocking(path: &Path, expected: u64, maximum: u64) -> UseResult<Vec<u8>> {
    if expected == 0 || expected > maximum {
        return Err(restore_activation_invalid(
            "A terminal complete restore receipt file exceeds its byte bound.",
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| restore_activation_io("read terminal restore receipt file", error))?;
    let after = std::fs::symlink_metadata(path)
        .map_err(|error| restore_activation_io("reinspect terminal restore receipt file", error))?;
    if bytes.len() as u64 != expected
        || a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != expected
    {
        return Err(restore_activation_invalid(
            "A terminal complete restore receipt file changed while it was read.",
        ));
    }
    Ok(bytes)
}

pub(super) async fn finish(
    state_root: &Path,
    attempt: &Path,
    attempt_bytes: &[u8],
    attempt_digest: &str,
) -> UseResult<ControlInstallationRestoreResult> {
    preflight(state_root, attempt, attempt_bytes, attempt_digest).await?;
    let result = restore_activation::complete(state_root, attempt, attempt_digest).await?;
    retire_components(state_root, attempt, attempt_bytes, attempt_digest).await?;
    let durable =
        validate_terminal_receipt(state_root, attempt, attempt_bytes, attempt_digest).await?;
    if durable != result {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt differs from its completed result.",
        ));
    }
    Ok(durable)
}

async fn preflight(
    state_root: &Path,
    attempt: &Path,
    attempt_bytes: &[u8],
    attempt_digest: &str,
) -> UseResult<()> {
    require_fixed_attempt(state_root, attempt)?;
    restore_filesystem::validate_attempt_evidence(attempt, attempt_bytes).await?;
    let activation = restore_activation::load(state_root, attempt, attempt_digest).await?;
    if !activation.is_complete() {
        return Err(restore_activation_invalid(
            "Complete restore staging cannot retire before every owner checkpoint.",
        ));
    }
    let components = inspect_retirement_inventory(attempt).await?;
    if restore_activation::marker_exists(state_root).await?
        && components.len() != RestoreComponent::ALL.len()
    {
        return Err(restore_activation_invalid(
            "Complete restore staging is incomplete before marker retirement.",
        ));
    }
    for component in components {
        validate_cleanup_tree(&component).await?;
    }
    restore_filesystem::validate_attempt_evidence(attempt, attempt_bytes).await?;
    let durable = restore_activation::load(state_root, attempt, attempt_digest).await?;
    if !durable.is_complete() {
        return Err(restore_activation_invalid(
            "The complete restore journal changed while staging retirement was preflighted.",
        ));
    }
    Ok(())
}

async fn retire_components(
    state_root: &Path,
    attempt: &Path,
    attempt_bytes: &[u8],
    attempt_digest: &str,
) -> UseResult<()> {
    for component in RestoreComponent::ALL {
        restore_filesystem::validate_attempt_evidence(attempt, attempt_bytes).await?;
        let activation = restore_activation::load(state_root, attempt, attempt_digest).await?;
        if !activation.is_complete() {
            return Err(restore_activation_invalid(
                "The complete restore journal became nonterminal during staging retirement.",
            ));
        }
        let component_path = attempt.join(component.staging_directory_name());
        match fs::symlink_metadata(&component_path).await {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(restore_activation_io(
                    "inspect complete restore staging retirement target",
                    error,
                ))
            }
            Ok(_) => validate_cleanup_tree(&component_path).await?,
        }
        let worker_path = component_path.clone();
        tokio::task::spawn_blocking(move || {
            a3s_use_extension::remove_dir_all_with_windows_retry_blocking(&worker_path)
        })
        .await
        .map_err(|error| {
            restore_activation_invalid(format!(
                "The complete restore staging retirement worker did not complete: {error}"
            ))
        })?
        .map_err(|error| {
            restore_activation_io("retire complete restore staging component", error)
        })?;
        sync_directory(attempt).await?;
        restore_activation::maybe_test_crash(&format!("{}-staging-retired", component.label()));
    }
    Ok(())
}

async fn validate_terminal_receipt(
    state_root: &Path,
    attempt: &Path,
    attempt_bytes: &[u8],
    attempt_digest: &str,
) -> UseResult<ControlInstallationRestoreResult> {
    require_fixed_attempt(state_root, attempt)?;
    restore_filesystem::validate_attempt_evidence(attempt, attempt_bytes).await?;
    if !inspect_retirement_inventory(attempt).await?.is_empty() {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt retains staging payloads.",
        ));
    }
    let activation = restore_activation::load(state_root, attempt, attempt_digest).await?;
    if restore_activation::marker_exists(state_root).await? || !activation.is_complete() {
        return Err(restore_activation_invalid(
            "The terminal complete restore receipt is not paired with a retired marker and complete journal.",
        ));
    }
    activation.completed_result(attempt_digest)
}

fn require_fixed_attempt(state_root: &Path, attempt: &Path) -> UseResult<()> {
    if attempt != state_root.join(restore_filesystem::ATTEMPT_DIRECTORY) {
        return Err(restore_activation_invalid(
            "Complete restore staging retirement is outside its fixed state-root location.",
        ));
    }
    Ok(())
}

async fn inspect_retirement_inventory(attempt: &Path) -> UseResult<Vec<PathBuf>> {
    let component_names = restore_filesystem::component_names()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut receipt_files = BTreeSet::new();
    let mut components = Vec::new();
    let mut entries = fs::read_dir(attempt).await.map_err(|error| {
        restore_activation_io("read complete restore retirement inventory", error)
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        restore_activation_io("read complete restore retirement inventory entry", error)
    })? {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_activation_invalid(
                "The complete restore retirement inventory contains a non-UTF-8 entry.",
            )
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
            restore_activation_io("inspect complete restore retirement inventory entry", error)
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
            return Err(restore_activation_invalid(
                "The complete restore retirement inventory contains a link or reparse point.",
            ));
        }
        if name == ATTEMPT_FILE || name == ACTIVATION_FILE {
            if !metadata.is_file() || !receipt_files.insert(name) {
                return Err(restore_activation_invalid(
                    "A complete restore terminal receipt path is not one owned regular file.",
                ));
            }
        } else if component_names.contains(name.as_str()) {
            if !metadata.is_dir() {
                return Err(restore_activation_invalid(
                    "A complete restore retirement component is not an owned directory.",
                ));
            }
            components.push(path);
        } else {
            return Err(restore_activation_invalid(
                "The complete restore retirement inventory contains unknown evidence.",
            ));
        }
    }
    if receipt_files != BTreeSet::from([ATTEMPT_FILE.to_owned(), ACTIVATION_FILE.to_owned()]) {
        return Err(restore_activation_invalid(
            "The complete restore retirement inventory omits terminal receipt evidence.",
        ));
    }
    components.sort();
    Ok(components)
}

async fn validate_cleanup_tree(root: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(root)
        .await
        .map_err(|error| restore_activation_io("inspect restore staging cleanup root", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_activation_invalid(
            "A complete restore staging cleanup root is not an owned directory.",
        ));
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut count = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_RETIREMENT_DEPTH {
            return Err(restore_activation_invalid(
                "A complete restore staging tree exceeds its cleanup depth bound.",
            ));
        }
        let mut entries = fs::read_dir(&directory)
            .await
            .map_err(|error| restore_activation_io("read restore staging cleanup tree", error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| restore_activation_io("read restore staging cleanup entry", error))?
        {
            count = count.checked_add(1).ok_or_else(|| {
                restore_activation_invalid("The complete restore cleanup entry count overflowed.")
            })?;
            if count > MAX_RETIREMENT_ENTRIES {
                return Err(restore_activation_invalid(
                    "A complete restore staging tree exceeds its cleanup entry bound.",
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
                restore_activation_io("inspect restore staging cleanup entry", error)
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(restore_activation_invalid(
                    "A complete restore staging tree contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                stack.push((path, depth + 1));
            } else if !metadata.is_file() {
                return Err(restore_activation_invalid(
                    "A complete restore staging tree contains a special filesystem entry.",
                ));
            }
        }
    }
    Ok(())
}
