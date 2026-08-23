use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;

use super::storage::{read_optional_json, sync_directory, validate_directory_chain};
use super::{
    operation_invalid, operation_io, StateRestoreOperation, StateRestoreOperationStatus,
    StateRestoreOperationStore, MAX_OPERATION_BYTES, MAX_OPERATION_COUNT,
};

const PRUNING_PREFIX: &str = ".pruning-";

pub(super) struct HistoryInventory {
    pub(super) operations: Vec<StateRestoreOperation>,
    pub(super) retained_directories: usize,
    pub(super) unrecorded_directories: usize,
}

pub(super) async fn inspect(store: &StateRestoreOperationStore) -> UseResult<HistoryInventory> {
    let Some(entries) = read_root_entries(store).await? else {
        return Ok(HistoryInventory {
            operations: Vec::new(),
            retained_directories: 0,
            unrecorded_directories: 0,
        });
    };
    let retained_directories = entries.len();
    let mut operations = Vec::new();
    let mut unrecorded_directories = 0usize;
    for (name, directory) in entries {
        if let Some(segment) = pruning_segment(&name) {
            validate_pruning_directory(&directory, segment).await?;
            read_pruning_operation(&directory, segment).await?;
            unrecorded_directories += 1;
            continue;
        }
        let segment = canonical_segment(&name).ok_or_else(|| {
            operation_invalid("A restore operation directory name is not a canonical digest.")
        })?;
        validate_journal_directory(&directory, true).await?;
        let digest = format!("sha256:{segment}");
        let operation: Option<StateRestoreOperation> = read_optional_json(
            &directory.join("operation.json"),
            MAX_OPERATION_BYTES,
            "state restore operation",
        )
        .await?;
        match operation {
            Some(operation) => {
                operation.validate()?;
                if operation.plan_digest != digest {
                    return Err(operation_invalid(
                        "A restore operation diagnostic does not match its owned path.",
                    ));
                }
                operations.push(operation);
            }
            None => unrecorded_directories += 1,
        }
    }
    Ok(HistoryInventory {
        operations,
        retained_directories,
        unrecorded_directories,
    })
}

pub(super) async fn load_for_mutation(
    store: &StateRestoreOperationStore,
) -> UseResult<Vec<StateRestoreOperation>> {
    recover_pruning(store).await?;
    let Some(entries) = read_root_entries(store).await? else {
        return Ok(Vec::new());
    };
    let mut operations = Vec::with_capacity(entries.len());
    for (name, directory) in entries {
        let segment = canonical_segment(&name).ok_or_else(|| {
            operation_invalid("A restore operation directory name is not a canonical digest.")
        })?;
        validate_journal_directory(&directory, true).await?;
        let digest = format!("sha256:{segment}");
        let operation = store.load(&digest).await?.ok_or_else(|| {
            operation_invalid("A restore operation directory has no durable journal.")
        })?;
        validate_journal_directory(&directory, false).await?;
        operations.push(operation);
    }
    operations.sort_by(|left, right| left.plan_digest.cmp(&right.plan_digest));
    Ok(operations)
}

pub(super) async fn reserve(
    store: &StateRestoreOperationStore,
    requested_digest: &str,
) -> UseResult<()> {
    recover_pruning(store).await?;
    let requested_directory = store.operation_directory(requested_digest)?;
    let Some(entries) = read_root_entries(store).await? else {
        return Ok(());
    };
    let mut requested_exists = false;
    let mut operations = Vec::new();
    for (name, directory) in &entries {
        let segment = canonical_segment(name).ok_or_else(|| {
            operation_invalid("A restore operation directory name is not a canonical digest.")
        })?;
        validate_journal_directory(directory, true).await?;
        if *directory == requested_directory {
            requested_exists = true;
            continue;
        }
        let digest = format!("sha256:{segment}");
        let operation = store.load(&digest).await?.ok_or_else(|| {
            operation_invalid("A retained restore operation has no durable journal.")
        })?;
        validate_journal_directory(directory, false).await?;
        operations.push(operation);
    }
    if requested_exists || entries.len() < MAX_OPERATION_COUNT {
        return Ok(());
    }
    if operations
        .iter()
        .any(|operation| operation.status != StateRestoreOperationStatus::Completed)
    {
        return Err(operation_invalid(
            "A new restore cannot prune nonterminal operation evidence.",
        ));
    }
    let oldest = operations
        .into_iter()
        .min_by(|left, right| {
            left.completed_at_ms
                .cmp(&right.completed_at_ms)
                .then_with(|| left.started_at_ms.cmp(&right.started_at_ms))
                .then_with(|| left.plan_digest.cmp(&right.plan_digest))
        })
        .ok_or_else(|| operation_invalid("No terminal restore history is available to prune."))?;
    let source = store.operation_directory(&oldest.plan_digest)?;
    validate_journal_directory(&source, false).await?;
    let segment = oldest.plan_digest.strip_prefix("sha256:").ok_or_else(|| {
        operation_invalid("A retained restore operation has an invalid plan digest.")
    })?;
    let tombstone = store.root.join(format!("{PRUNING_PREFIX}{segment}"));
    match fs::symlink_metadata(&tombstone).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(operation_invalid(
                "A restore history pruning tombstone already exists.",
            ))
        }
        Err(error) => {
            return Err(operation_io(
                "inspect restore history pruning tombstone",
                &tombstone,
                error,
            ))
        }
    }
    let rename_source = source.clone();
    let rename_target = tombstone.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::rename_path_with_windows_retry_blocking(&rename_source, &rename_target)
    })
    .await
    .map_err(|error| {
        operation_invalid(format!(
            "Restore history pruning worker did not complete: {error}"
        ))
    })?
    .map_err(|error| operation_io("activate restore history pruning", &source, error))?;
    sync_directory(&store.root).await?;
    finish_pruning(store, &tombstone, segment).await
}

async fn recover_pruning(store: &StateRestoreOperationStore) -> UseResult<()> {
    let Some(entries) = read_root_entries(store).await? else {
        return Ok(());
    };
    let pruning = entries
        .into_iter()
        .filter_map(|(name, directory)| {
            pruning_segment(&name).map(|segment| (segment.to_owned(), directory))
        })
        .collect::<Vec<_>>();
    if pruning.len() > 1 {
        return Err(operation_invalid(
            "Multiple restore history pruning tombstones are retained.",
        ));
    }
    if let Some((segment, directory)) = pruning.into_iter().next() {
        finish_pruning(store, &directory, &segment).await?;
    }
    Ok(())
}

async fn finish_pruning(
    store: &StateRestoreOperationStore,
    directory: &Path,
    segment: &str,
) -> UseResult<()> {
    validate_pruning_directory(directory, segment).await?;
    let journal = directory.join("operation.json");
    if read_pruning_operation(directory, segment).await?.is_some() {
        fs::remove_file(&journal)
            .await
            .map_err(|error| operation_io("remove pruned restore journal", &journal, error))?;
        sync_directory(directory).await?;
    }
    fs::remove_dir(directory).await.map_err(|error| {
        operation_io("remove restore history pruning tombstone", directory, error)
    })?;
    sync_directory(&store.root).await
}

async fn read_root_entries(
    store: &StateRestoreOperationStore,
) -> UseResult<Option<Vec<(String, PathBuf)>>> {
    let metadata = match fs::symlink_metadata(&store.root).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(operation_io(
                "inspect restore operation root",
                &store.root,
                error,
            ))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(operation_invalid(
            "The restore operation history root is not an owned directory.",
        ));
    }
    validate_directory_chain(&store.state_root, &store.root).await?;
    let mut reader = fs::read_dir(&store.root)
        .await
        .map_err(|error| operation_io("read restore operation root", &store.root, error))?;
    let mut entries = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| operation_io("read restore operation entry", &store.root, error))?
    {
        if entries.len() >= MAX_OPERATION_COUNT {
            return Err(operation_invalid(
                "The whole-installation restore operation history exceeds its bound.",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            operation_invalid("A restore operation directory name is not valid UTF-8.")
        })?;
        if canonical_segment(&name).is_none() && pruning_segment(&name).is_none() {
            return Err(operation_invalid(
                "A restore operation directory name is not canonical history evidence.",
            ));
        }
        let directory = entry.path();
        let metadata = fs::symlink_metadata(&directory).await.map_err(|error| {
            operation_io("inspect restore operation directory", &directory, error)
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(operation_invalid(
                "A restore operation history path is not an owned directory.",
            ));
        }
        entries.push((name, directory));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(Some(entries))
}

async fn validate_journal_directory(directory: &Path, allow_temporary: bool) -> UseResult<()> {
    validate_owned_directory(directory).await?;
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| operation_io("read restore operation directory", directory, error))?;
    let mut count = 0usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| operation_io("read restore operation directory entry", directory, error))?
    {
        count += 1;
        if count > 2 {
            return Err(operation_invalid(
                "A restore operation directory contains unknown evidence.",
            ));
        }
        let name = entry.file_name();
        if name != "operation.json" && !(allow_temporary && name == "operation.json.tmp") {
            return Err(operation_invalid(
                "A restore operation directory contains unknown evidence.",
            ));
        }
        validate_bounded_file(&entry.path()).await?;
    }
    Ok(())
}

async fn validate_pruning_directory(directory: &Path, segment: &str) -> UseResult<()> {
    if canonical_segment(segment).is_none() {
        return Err(operation_invalid(
            "A restore history pruning tombstone has an invalid identity.",
        ));
    }
    validate_owned_directory(directory).await?;
    let mut entries = fs::read_dir(directory).await.map_err(|error| {
        operation_io("read restore history pruning tombstone", directory, error)
    })?;
    let mut count = 0usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| operation_io("read restore history pruning entry", directory, error))?
    {
        count += 1;
        if count > 1 || entry.file_name() != "operation.json" {
            return Err(operation_invalid(
                "A restore history pruning tombstone contains unknown evidence.",
            ));
        }
        validate_bounded_file(&entry.path()).await?;
    }
    Ok(())
}

async fn read_pruning_operation(
    directory: &Path,
    segment: &str,
) -> UseResult<Option<StateRestoreOperation>> {
    let operation: Option<StateRestoreOperation> = read_optional_json(
        &directory.join("operation.json"),
        MAX_OPERATION_BYTES,
        "pruned state restore operation",
    )
    .await?;
    if let Some(operation) = &operation {
        operation.validate()?;
        if operation.status != StateRestoreOperationStatus::Completed
            || operation.plan_digest != format!("sha256:{segment}")
        {
            return Err(operation_invalid(
                "A restore history pruning tombstone is not exact terminal evidence.",
            ));
        }
    }
    Ok(operation)
}

async fn validate_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| operation_io("inspect restore history directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(operation_invalid(
            "A restore history path is not an owned directory.",
        ));
    }
    Ok(())
}

async fn validate_bounded_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| operation_io("inspect restore history evidence", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OPERATION_BYTES
    {
        return Err(operation_invalid(
            "Restore history evidence is not a bounded owned regular file.",
        ));
    }
    Ok(())
}

fn canonical_segment(value: &str) -> Option<&str> {
    (value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then_some(value)
}

fn pruning_segment(value: &str) -> Option<&str> {
    value
        .strip_prefix(PRUNING_PREFIX)
        .and_then(canonical_segment)
}
