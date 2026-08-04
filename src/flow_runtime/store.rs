use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{PlanQualifiedSurfaceRef, PluginPackageId, PluginSurfaceKind, UseResult};
use a3s_use_extension::ExtensionPaths;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::model::{flow_error, valid_machine_id, valid_segment};
use super::FlowRuntimeBinding;

pub const MAX_FLOW_RUNTIME_GENERATIONS: usize = 32;
const MAX_BINDING_BYTES: u64 = 256 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 64;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Symlink-safe durable store retaining preflight evidence by exact package
/// generation so blue-green upgrades never hide the last published Flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRuntimeBindingStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl FlowRuntimeBindingStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("bindings").join("flow"),
            state_root,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::new(paths.state_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn put(&self, binding: &FlowRuntimeBinding) -> UseResult<bool> {
        binding.validate()?;
        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(binding.scope_id(), binding.surface())?;
        ensure_owned_directory(&self.state_root, Some(&directory)).await?;
        let path = binding_path(&directory, binding.generation());
        if let Some(current) = read_optional_binding(&path).await? {
            if current == *binding {
                return Ok(false);
            }
            return Err(store_error(
                "use.plugin.flow_binding_conflict",
                "One A3S Flow generation has conflicting immutable preflight evidence.",
            ));
        }
        if count_generations(&directory).await? >= MAX_FLOW_RUNTIME_GENERATIONS {
            return Err(store_error(
                "use.plugin.flow_binding_limit_exceeded",
                format!(
                    "The A3S Flow binding reached its retained-generation limit of {MAX_FLOW_RUNTIME_GENERATIONS}; receipt-owned cleanup is required before another generation is prepared."
                ),
            ));
        }
        write_binding(&path, binding).await?;
        Ok(true)
    }

    pub async fn get(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
        generation: u64,
    ) -> UseResult<Option<FlowRuntimeBinding>> {
        if generation == 0 {
            return Err(invalid_path_identity());
        }
        let directory = self.surface_directory(scope_id, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(None);
        }
        let path = binding_path(&directory, generation);
        let Some(binding) = read_optional_binding(&path).await? else {
            return Ok(None);
        };
        validate_ownership(&binding, scope_id, surface, generation)?;
        Ok(Some(binding))
    }

    pub async fn remove(&self, expected: &FlowRuntimeBinding) -> UseResult<bool> {
        expected.validate()?;
        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(expected.scope_id(), expected.surface())?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(false);
        }
        let path = binding_path(&directory, expected.generation());
        let Some(current) = read_optional_binding(&path).await? else {
            return Ok(false);
        };
        if current != *expected {
            return Err(store_error(
                "use.plugin.flow_binding_ownership_changed",
                "The A3S Flow binding changed before removal and was preserved.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove A3S Flow binding", &path, error))?;
        sync_parent(path.parent()).await?;
        Ok(true)
    }

    fn surface_directory(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<PathBuf> {
        validate_path_identity(scope_id, surface)?;
        let package_id = PluginPackageId::parse(surface.package_id.clone())?;
        let (publisher, package) = package_id
            .as_str()
            .split_once('/')
            .ok_or_else(invalid_path_identity)?;
        let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
        Ok(self
            .root
            .join(scope_digest)
            .join(publisher)
            .join(package)
            .join(format!("flow-{}", surface.surface.id)))
    }

    async fn acquire_lock(&self) -> UseResult<StdFile> {
        fs::create_dir_all(&self.state_root)
            .await
            .map_err(|error| path_error("create A3S Flow state root", &self.state_root, error))?;
        validate_directory(&self.state_root).await?;
        ensure_owned_directory(&self.state_root, Some(&self.root)).await?;
        let lock_path = self.root.join(".store.lock");
        match fs::symlink_metadata(&lock_path).await {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_path_identity())
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(path_error(
                    "inspect A3S Flow binding lock",
                    &lock_path,
                    error,
                ))
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
                "use.plugin.flow_binding_io",
                format!(
                    "Failed to acquire A3S Flow binding lock '{}': blocking task failed: {error}",
                    error_path.display()
                ),
            )
        })?
        .map_err(|error| path_error("acquire A3S Flow binding lock", &error_path, error))
    }
}

fn validate_ownership(
    binding: &FlowRuntimeBinding,
    scope_id: &str,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> UseResult<()> {
    if binding.scope_id() != scope_id
        || binding.surface() != surface
        || binding.generation() != generation
    {
        return Err(store_error(
            "use.plugin.flow_binding_ownership_mismatch",
            "An A3S Flow binding does not match its scope, surface, and generation path.",
        ));
    }
    Ok(())
}

fn validate_path_identity(scope_id: &str, surface: &PlanQualifiedSurfaceRef) -> UseResult<()> {
    if !valid_machine_id(scope_id)
        || PluginPackageId::parse(surface.package_id.clone()).is_err()
        || surface.surface.kind != PluginSurfaceKind::Flow
        || !valid_segment(&surface.surface.id)
    {
        return Err(invalid_path_identity());
    }
    Ok(())
}

fn binding_path(directory: &Path, generation: u64) -> PathBuf {
    directory.join(format!("{generation:020}.json"))
}

async fn count_generations(directory: &Path) -> UseResult<usize> {
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| path_error("read A3S Flow binding directory", directory, error))?;
    let mut generations = 0_usize;
    let mut entries_seen = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read A3S Flow binding entry", directory, error))?
    {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_DIRECTORY_ENTRIES {
            return Err(store_error(
                "use.plugin.flow_binding_limit_exceeded",
                "The A3S Flow binding directory exceeds its entry bound.",
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid_path_identity)?;
        if name.starts_with(".binding-") && name.ends_with(".tmp") {
            let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
                path_error("inspect temporary A3S Flow binding", &entry.path(), error)
            })?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() > MAX_BINDING_BYTES
            {
                return Err(invalid_path_identity());
            }
            continue;
        }
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
        generations = generations.saturating_add(1);
    }
    Ok(generations)
}

async fn read_optional_binding(path: &Path) -> UseResult<Option<FlowRuntimeBinding>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect A3S Flow binding", path, error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BINDING_BYTES
    {
        return Err(record_error(format!(
            "A3S Flow binding '{}' is not a bounded regular file.",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read A3S Flow binding", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_BYTES {
        return Err(record_error(
            "An A3S Flow binding changed outside its size bound while reading.",
        ));
    }
    let binding = serde_json::from_slice::<FlowRuntimeBinding>(&bytes).map_err(|error| {
        record_error(format!(
            "A3S Flow binding '{}' is invalid JSON: {error}",
            path.display()
        ))
    })?;
    binding.validate().map_err(|error| {
        record_error(format!(
            "A3S Flow binding '{}' is invalid: {}",
            path.display(),
            error.message
        ))
    })?;
    Ok(Some(binding))
}

async fn write_binding(path: &Path, binding: &FlowRuntimeBinding) -> UseResult<()> {
    let bytes = serde_json::to_vec_pretty(binding)
        .map_err(|error| record_error(format!("Failed to encode A3S Flow binding: {error}")))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_BYTES {
        return Err(record_error(
            "The A3S Flow binding exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(invalid_path_identity)?;
    let temporary = parent.join(format!(".binding-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary A3S Flow binding", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "write temporary A3S Flow binding",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "sync temporary A3S Flow binding",
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

async fn ensure_owned_directory(root: &Path, parent: Option<&Path>) -> UseResult<()> {
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
                    "create A3S Flow binding directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(root: &Path, parent: Option<&Path>) -> UseResult<bool> {
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
                    "inspect A3S Flow binding directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
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
            "use.plugin.flow_binding_io",
            format!(
                "Failed to activate A3S Flow binding '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("activate A3S Flow binding", &error_target, error))
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect A3S Flow binding directory", path, error))?;
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
        .map_err(|error| path_error("open A3S Flow binding directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync A3S Flow binding directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: Option<&Path>) -> UseResult<()> {
    Ok(())
}

fn invalid_path_identity() -> a3s_use_core::UseError {
    store_error(
        "use.plugin.flow_binding_path_invalid",
        "The A3S Flow binding path identity is invalid or not owned by the store.",
    )
}

fn record_error(message: impl Into<String>) -> a3s_use_core::UseError {
    store_error("use.plugin.flow_binding_record_invalid", message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> a3s_use_core::UseError {
    store_error(
        "use.plugin.flow_binding_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn store_error(code: &'static str, message: impl Into<String>) -> a3s_use_core::UseError {
    flow_error(code, message)
}
