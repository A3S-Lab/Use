use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{PlanQualifiedSurfaceRef, UseResult};
use fs2::FileExt;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{
    invalid_path_identity, path_error, record_error, store_error, validate_ownership,
    OkfKnowledgeBinding, MAX_OKF_KNOWLEDGE_GENERATIONS,
};

const MAX_BINDING_BYTES: u64 = 256 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 64;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) async fn acquire_lock(state_root: &Path, root: &Path) -> UseResult<StdFile> {
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_error("create OKF Knowledge state root", state_root, error))?;
    validate_directory(state_root).await?;
    ensure_owned_directory(state_root, Some(root)).await?;
    let lock_path = root.join(".store.lock");
    match fs::symlink_metadata(&lock_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(store_error(
                "use.okf.knowledge_binding_path_invalid",
                "The OKF Knowledge binding lock is not an owned regular file.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(path_error(
                "inspect OKF Knowledge binding lock",
                &lock_path,
                error,
            ));
        }
    }
    let error_path = lock_path.clone();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.okf.knowledge_binding_io",
            format!(
                "Failed to acquire OKF Knowledge binding lock '{}': blocking task failed: {error}",
                error_path.display()
            ),
        )
    })?
    .map_err(|error| path_error("acquire OKF Knowledge binding lock", &error_path, error))
}

pub(super) async fn read_bindings(
    directory: &Path,
    scope_id: &str,
    surface: &PlanQualifiedSurfaceRef,
) -> UseResult<Vec<OkfKnowledgeBinding>> {
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| path_error("read OKF Knowledge binding directory", directory, error))?;
    let mut records = Vec::new();
    let mut entry_count = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read OKF Knowledge binding entry", directory, error))?
    {
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_DIRECTORY_ENTRIES {
            return Err(store_error(
                "use.okf.knowledge_binding_limit_exceeded",
                "The OKF Knowledge binding directory exceeds its entry bound.",
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid_path_identity)?;
        if name.starts_with(".binding-") && name.ends_with(".tmp") {
            validate_ignored_temporary(&entry.path()).await?;
            continue;
        }
        let generation = parse_generation_filename(name)?;
        let path = entry.path();
        let binding = read_optional_binding(&path).await?.ok_or_else(|| {
            path_error(
                "read OKF Knowledge binding",
                &path,
                io::Error::from(io::ErrorKind::NotFound),
            )
        })?;
        validate_ownership(&binding, scope_id, surface, generation)?;
        records.push(binding);
    }
    records.sort_by_key(|record| record.receipt.generation);
    if records.len() > MAX_OKF_KNOWLEDGE_GENERATIONS {
        return Err(store_error(
            "use.okf.knowledge_binding_limit_exceeded",
            "The OKF Knowledge binding exceeds its retained-generation bound.",
        ));
    }
    Ok(records)
}

pub(super) fn binding_path(directory: &Path, generation: u64) -> PathBuf {
    directory.join(format!("{generation:020}.json"))
}

pub(super) async fn read_optional_binding(path: &Path) -> UseResult<Option<OkfKnowledgeBinding>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect OKF Knowledge binding", path, error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BINDING_BYTES
    {
        return Err(record_error(format!(
            "OKF Knowledge binding '{}' is not a bounded regular file.",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read OKF Knowledge binding", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_BYTES {
        return Err(record_error(
            "An OKF Knowledge binding changed outside its size bound while reading.",
        ));
    }
    let binding = serde_json::from_slice::<OkfKnowledgeBinding>(&bytes).map_err(|error| {
        record_error(format!(
            "OKF Knowledge binding '{}' is invalid JSON: {error}",
            path.display()
        ))
    })?;
    binding.validate().map_err(|error| {
        record_error(format!(
            "OKF Knowledge binding '{}' is invalid: {}",
            path.display(),
            error.message
        ))
    })?;
    Ok(Some(binding))
}

pub(super) async fn write_binding(path: &Path, binding: &OkfKnowledgeBinding) -> UseResult<()> {
    let bytes = serde_json::to_vec_pretty(binding).map_err(|error| {
        record_error(format!("Failed to encode OKF Knowledge binding: {error}"))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_BYTES {
        return Err(record_error(
            "The OKF Knowledge binding exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(invalid_path_identity)?;
    let temporary = parent.join(format!(".binding-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary OKF Knowledge binding", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "write temporary OKF Knowledge binding",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "sync temporary OKF Knowledge binding",
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

pub(super) async fn ensure_owned_directory(root: &Path, parent: Option<&Path>) -> UseResult<()> {
    let parent = parent.ok_or_else(invalid_path_identity)?;
    if !parent.starts_with(root) {
        return Err(invalid_path_identity());
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| invalid_path_identity())?;
    let mut current = root.to_path_buf();
    validate_directory(&current).await?;
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create OKF Knowledge binding directory",
                    &current,
                    error,
                ));
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_existing_directory_chain(
    root: &Path,
    parent: Option<&Path>,
) -> UseResult<bool> {
    let parent = parent.ok_or_else(invalid_path_identity)?;
    if !parent.starts_with(root) {
        return Err(invalid_path_identity());
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| invalid_path_identity())?;
    let mut current = root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => return Err(invalid_path_identity()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect OKF Knowledge binding directory",
                    &current,
                    error,
                ));
            }
        }
    }
    Ok(true)
}

async fn validate_ignored_temporary(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect temporary OKF Knowledge binding", path, error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_BINDING_BYTES
    {
        return Err(invalid_path_identity());
    }
    Ok(())
}

fn parse_generation_filename(name: &str) -> UseResult<u64> {
    let Some(stem) = name.strip_suffix(".json") else {
        return Err(invalid_path_identity());
    };
    if stem.len() != 20 || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_path_identity());
    }
    let generation = stem.parse::<u64>().map_err(|_| invalid_path_identity())?;
    if generation == 0 || format!("{generation:020}.json") != name {
        return Err(invalid_path_identity());
    }
    Ok(generation)
}

async fn activate_temporary(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.okf.knowledge_binding_io",
            format!(
                "Failed to activate OKF Knowledge binding '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("activate OKF Knowledge binding", &error_target, error))
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect OKF Knowledge binding directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_path_identity());
    }
    Ok(())
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

#[cfg(unix)]
async fn sync_parent(parent: Option<&Path>) -> UseResult<()> {
    let parent = parent.ok_or_else(invalid_path_identity)?;
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open OKF Knowledge binding directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync OKF Knowledge binding directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: Option<&Path>) -> UseResult<()> {
    Ok(())
}
