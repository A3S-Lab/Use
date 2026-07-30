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
        let path = self.binding_path(receipt.scope_id(), receipt.surface())?;
        ensure_owned_directory(&self.root, path.parent()).await?;
        if let Some(current) = read_optional_receipt(&path).await? {
            if current == *receipt {
                return Ok(false);
            }
            validate_replacement(&current, receipt)?;
        }
        write_receipt(&path, receipt).await?;
        Ok(true)
    }

    pub async fn get(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<Option<RuntimeBindingReceipt>> {
        let path = self.binding_path(scope_id, surface)?;
        if !validate_existing_directory_chain(&self.state_root, path.parent()).await? {
            return Ok(None);
        }
        let Some(receipt) = read_optional_receipt(&path).await? else {
            return Ok(None);
        };
        if receipt.scope_id() != scope_id || receipt.surface() != surface {
            return Err(store_error(
                "use.plugin.runtime.binding_ownership_mismatch",
                "A Runtime binding receipt does not match its scope and surface path.",
            ));
        }
        Ok(Some(receipt))
    }

    pub async fn remove(&self, expected: &RuntimeBindingReceipt) -> UseResult<bool> {
        expected.validate()?;
        let _lock = self.acquire_lock().await?;
        let path = self.binding_path(expected.scope_id(), expected.surface())?;
        if !validate_existing_directory_chain(&self.state_root, path.parent()).await? {
            return Ok(false);
        }
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

    fn binding_path(
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
            PluginSurfaceKind::Skill | PluginSurfaceKind::Ui => return Err(invalid_path_identity()),
        };
        Ok(self
            .root
            .join(scope_digest)
            .join(publisher)
            .join(package)
            .join(format!("{kind}-{}.json", surface.surface.id)))
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

fn validate_replacement(
    current: &RuntimeBindingReceipt,
    next: &RuntimeBindingReceipt,
) -> UseResult<()> {
    if current.generation() > next.generation() {
        return Err(store_error(
            "use.plugin.runtime.binding_stale",
            "A stale Runtime binding generation cannot replace the current receipt.",
        ));
    }
    if current.generation() < next.generation() {
        return Ok(());
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

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_error(
        "use.plugin.runtime.binding_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
