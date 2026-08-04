use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind, UseError, UseResult};
use a3s_use_extension::ExtensionPaths;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::receipt::RuntimeBindingReceipt;

const MAX_BINDING_RECEIPT_BYTES: u64 = 256 * 1024;
const MAX_BINDING_DIRECTORY_ENTRIES: usize = 64;
pub const MAX_RUNTIME_BINDING_GENERATIONS: usize = 32;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBindingStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl RuntimeBindingStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("bindings").join("runtime"),
            state_root,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::new(paths.state_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn put(&self, receipt: &RuntimeBindingReceipt) -> UseResult<bool> {
        receipt.validate()?;
        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(receipt.scope_id(), receipt.surface())?;
        ensure_owned_directory(&self.root, Some(&directory)).await?;
        let retained = generations(&directory).await?;
        let path = binding_path(&directory, receipt.generation());
        if let Some(current) = read_optional_receipt(&path).await? {
            validate_ownership(
                &current,
                receipt.scope_id(),
                receipt.surface(),
                receipt.generation(),
            )?;
            if current == *receipt {
                return Ok(false);
            }
            validate_same_generation_replacement(&current, receipt)?;
        } else if retained.len() >= MAX_RUNTIME_BINDING_GENERATIONS {
            return Err(generation_limit_error());
        }
        write_receipt(&path, receipt).await?;
        Ok(true)
    }

    /// Return the newest retained receipt for inventory and diagnostics.
    /// Lifecycle and capability decisions must use [`Self::get_generation`]
    /// with the generation selected by their immutable package evidence.
    pub async fn get(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<Option<RuntimeBindingReceipt>> {
        let directory = self.surface_directory(scope_id, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(None);
        }
        let Some(generation) = generations(&directory).await?.into_iter().next_back() else {
            return Ok(None);
        };
        self.get_generation(scope_id, surface, generation).await
    }

    /// Read one exact retained Runtime generation. Candidate preparation and
    /// prior-generation retirement must never infer ownership from the newest
    /// receipt for a surface.
    pub async fn get_generation(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
        generation: u64,
    ) -> UseResult<Option<RuntimeBindingReceipt>> {
        if generation == 0 {
            return Err(invalid_path_identity());
        }
        let directory = self.surface_directory(scope_id, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(None);
        }
        let path = binding_path(&directory, generation);
        let Some(receipt) = read_optional_receipt(&path).await? else {
            return Ok(None);
        };
        validate_ownership(&receipt, scope_id, surface, generation)?;
        Ok(Some(receipt))
    }

    pub async fn remove(&self, expected: &RuntimeBindingReceipt) -> UseResult<bool> {
        expected.validate()?;
        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(expected.scope_id(), expected.surface())?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(false);
        }
        let path = binding_path(&directory, expected.generation());
        let Some(current) = read_optional_receipt(&path).await? else {
            return Ok(false);
        };
        if current != *expected {
            return Err(store_error(
                "use.plugin.runtime.binding_ownership_changed",
                "The Runtime binding changed before removal and was preserved.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove Runtime binding receipt", &path, error))?;
        sync_parent(path.parent()).await?;
        Ok(true)
    }

    fn surface_directory(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<PathBuf> {
        validate_path_identity(scope_id, surface)?;
        let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
        let mut segments = surface.package_id.split('/');
        let publisher = segments.next().ok_or_else(invalid_path_identity)?;
        let package = segments.next().ok_or_else(invalid_path_identity)?;
        let kind = match surface.surface.kind {
            PluginSurfaceKind::Tool => "tool",
            PluginSurfaceKind::Mcp => "mcp",
            PluginSurfaceKind::Flow
            | PluginSurfaceKind::Okf
            | PluginSurfaceKind::Skill
            | PluginSurfaceKind::Ui => return Err(invalid_path_identity()),
        };
        Ok(self
            .root
            .join(scope_digest)
            .join(publisher)
            .join(package)
            .join(format!("{kind}-{}", surface.surface.id)))
    }

    async fn acquire_lock(&self) -> UseResult<StdFile> {
        fs::create_dir_all(&self.state_root)
            .await
            .map_err(|error| {
                path_error("create Runtime binding state root", &self.state_root, error)
            })?;
        validate_directory(&self.state_root).await?;
        ensure_owned_directory(&self.state_root, Some(&self.root)).await?;
        let lock_path = self.root.join(".store.lock");
        match fs::symlink_metadata(&lock_path).await {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(store_error(
                    "use.plugin.runtime.binding_path_invalid",
                    "The Runtime binding store lock is not an owned regular file.",
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(path_error(
                    "inspect Runtime binding lock",
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
                "use.plugin.runtime.binding_io",
                format!(
                    "Failed to acquire Runtime binding lock '{}': blocking task failed: {error}",
                    error_path.display()
                ),
            )
        })?
        .map_err(|error| path_error("acquire Runtime binding lock", &error_path, error))
    }
}

fn validate_same_generation_replacement(
    current: &RuntimeBindingReceipt,
    next: &RuntimeBindingReceipt,
) -> UseResult<()> {
    if current.generation() != next.generation() {
        return Err(store_error(
            "use.plugin.runtime.binding_ownership_mismatch",
            "A Runtime binding receipt was stored under a different generation path.",
        ));
    }
    match (current, next) {
        (RuntimeBindingReceipt::Service(current), RuntimeBindingReceipt::Service(next))
            if same_service_generation(current, next)
                && next.observation_revision > current.observation_revision =>
        {
            Ok(())
        }
        _ => Err(store_error(
            "use.plugin.runtime.binding_conflict",
            "A Runtime binding generation has conflicting immutable content.",
        )),
    }
}

fn validate_ownership(
    receipt: &RuntimeBindingReceipt,
    scope_id: &str,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> UseResult<()> {
    if receipt.scope_id() != scope_id
        || receipt.surface() != surface
        || receipt.generation() != generation
    {
        return Err(store_error(
            "use.plugin.runtime.binding_ownership_mismatch",
            "A Runtime binding receipt does not match its scope, surface, and generation path.",
        ));
    }
    Ok(())
}

fn binding_path(directory: &Path, generation: u64) -> PathBuf {
    directory.join(format!("{generation:020}.json"))
}

async fn generations(directory: &Path) -> UseResult<std::collections::BTreeSet<u64>> {
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| path_error("read Runtime binding directory", directory, error))?;
    let mut values = std::collections::BTreeSet::new();
    let mut entries_seen = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read Runtime binding entry", directory, error))?
    {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_BINDING_DIRECTORY_ENTRIES {
            return Err(store_error(
                "use.plugin.runtime.binding_limit_exceeded",
                "The Runtime binding directory exceeds its entry bound.",
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid_path_identity)?;
        if name.starts_with(".binding-") && name.ends_with(".tmp") {
            let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
                path_error("inspect temporary Runtime binding", &entry.path(), error)
            })?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() > MAX_BINDING_RECEIPT_BYTES
            {
                return Err(invalid_path_identity());
            }
            continue;
        }
        let Some(stem) = name.strip_suffix(".json") else {
            return Err(invalid_path_identity());
        };
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| path_error("inspect Runtime binding receipt", &entry.path(), error))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_BINDING_RECEIPT_BYTES
        {
            return Err(invalid_path_identity());
        }
        if stem.len() != 20 || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_path_identity());
        }
        let generation = stem.parse::<u64>().map_err(|_| invalid_path_identity())?;
        if generation == 0 || format!("{generation:020}.json") != name {
            return Err(invalid_path_identity());
        }
        if !values.insert(generation) {
            return Err(invalid_path_identity());
        }
    }
    if values.len() > MAX_RUNTIME_BINDING_GENERATIONS {
        return Err(generation_limit_error());
    }
    Ok(values)
}

fn same_service_generation(
    current: &super::model::RuntimeServiceBindingReceipt,
    next: &super::model::RuntimeServiceBindingReceipt,
) -> bool {
    current.surface == next.surface
        && current.package_digest == next.package_digest
        && current.scope_id == next.scope_id
        && current.descriptor_digest == next.descriptor_digest
        && current.provider_id == next.provider_id
        && current.provider_build_id == next.provider_build_id
        && current.capability_digest == next.capability_digest
        && current.enforcement == next.enforcement
        && current.unit_id == next.unit_id
        && current.generation == next.generation
        && current.spec_digest == next.spec_digest
        && current.semantics_profile_digest == next.semantics_profile_digest
        && current.runtime_started_at_ms == next.runtime_started_at_ms
        && current.contract == next.contract
}

async fn ensure_owned_directory(root: &Path, parent: Option<&Path>) -> UseResult<()> {
    let parent = parent.ok_or_else(|| {
        store_error(
            "use.plugin.runtime.binding_path_invalid",
            "A Runtime binding path has no parent directory.",
        )
    })?;
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
                    "create Runtime binding directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect Runtime binding directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(store_error(
            "use.plugin.runtime.binding_path_invalid",
            format!(
                "Runtime binding directory '{}' is not an owned real directory.",
                path.display()
            ),
        ));
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
            Ok(_) => {
                return Err(store_error(
                    "use.plugin.runtime.binding_path_invalid",
                    format!(
                        "Runtime binding directory '{}' is not an owned real directory.",
                        current.display()
                    ),
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect Runtime binding directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn read_optional_receipt(path: &Path) -> UseResult<Option<RuntimeBindingReceipt>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect Runtime binding receipt", path, error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BINDING_RECEIPT_BYTES
    {
        return Err(store_error(
            "use.plugin.runtime.binding_receipt_invalid",
            format!(
                "Runtime binding receipt '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Runtime binding receipt", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_RECEIPT_BYTES {
        return Err(store_error(
            "use.plugin.runtime.binding_receipt_invalid",
            "A Runtime binding receipt changed outside its size bound while reading.",
        ));
    }
    let receipt = serde_json::from_slice::<RuntimeBindingReceipt>(&bytes).map_err(|error| {
        store_error(
            "use.plugin.runtime.binding_receipt_invalid",
            format!(
                "Runtime binding receipt '{}' is invalid JSON: {error}",
                path.display()
            ),
        )
    })?;
    receipt.validate()?;
    Ok(Some(receipt))
}

async fn write_receipt(path: &Path, receipt: &RuntimeBindingReceipt) -> UseResult<()> {
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| {
        store_error(
            "use.plugin.runtime.binding_receipt_invalid",
            format!("Failed to encode Runtime binding receipt: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_RECEIPT_BYTES {
        return Err(store_error(
            "use.plugin.runtime.binding_receipt_invalid",
            "The Runtime binding receipt exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(invalid_path_identity)?;
    let temporary = parent.join(format!(".binding-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary Runtime binding", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "write temporary Runtime binding",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "sync temporary Runtime binding",
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

async fn activate_temporary(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.plugin.runtime.binding_io",
            format!(
                "Failed to activate Runtime binding '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("activate Runtime binding", &error_target, error))
}

#[cfg(unix)]
async fn sync_parent(parent: Option<&Path>) -> UseResult<()> {
    let parent = parent.ok_or_else(invalid_path_identity)?;
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open Runtime binding directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync Runtime binding directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: Option<&Path>) -> UseResult<()> {
    Ok(())
}

fn validate_path_identity(scope_id: &str, surface: &PlanQualifiedSurfaceRef) -> UseResult<()> {
    let package_segments = surface.package_id.split('/').collect::<Vec<_>>();
    if !super::model::valid_machine_id(scope_id)
        || surface.package_id.len() > 128
        || package_segments.len() != 2
        || package_segments
            .iter()
            .any(|segment| !super::model::valid_surface_segment(segment))
        || !super::model::valid_surface_segment(&surface.surface.id)
        || !matches!(
            surface.surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )
    {
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

fn invalid_path_identity() -> UseError {
    store_error(
        "use.plugin.runtime.binding_path_invalid",
        "A Runtime binding scope or surface identity is invalid.",
    )
}

fn generation_limit_error() -> UseError {
    store_error(
        "use.plugin.runtime.binding_limit_exceeded",
        format!(
            "The Runtime binding reached its retained-generation limit of {MAX_RUNTIME_BINDING_GENERATIONS}; receipt-owned cleanup is required before another generation is prepared."
        ),
    )
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_error(
        "use.plugin.runtime.binding_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
