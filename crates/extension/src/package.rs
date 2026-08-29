use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::registry::ExtensionReceipt;
use super::state_maintenance::{StateMaintenanceGuard, StateMaintenanceLock};
use super::{ExtensionManifest, ExtensionPaths};

pub(crate) const MANIFEST_NAME: &str = "a3s-use-extension.acl";
pub(crate) const MAX_PACKAGE_FILES: usize = 10_000;
pub(crate) const MAX_PACKAGE_BYTES: u64 = 1_073_741_824;
pub(super) const MAX_ACTIVITY_HTML_BYTES: u64 = 2 * 1024 * 1024;
pub(super) const MAX_ACTIVITY_RESOURCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PACKAGE_README_BYTES: u64 = 2 * 1024 * 1024;
const REGISTRY_MUTATION_LOCK_WAIT: Duration = Duration::from_secs(2);
const REGISTRY_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

pub(crate) async fn read_manifest(package_root: &Path) -> UseResult<(ExtensionManifest, Vec<u8>)> {
    let path = package_root.join(MANIFEST_NAME);
    let bytes = fs::read(&path)
        .await
        .map_err(|error| io_error("read extension manifest", &path, error))?;
    let input = std::str::from_utf8(&bytes).map_err(|error| {
        UseError::new(
            "use.extension.manifest_invalid",
            format!("Extension manifest must be UTF-8: {error}"),
        )
    })?;
    Ok((ExtensionManifest::parse_acl(input)?, bytes))
}

pub(crate) async fn validate_surface_files(
    manifest: &ExtensionManifest,
    package_root: &Path,
) -> UseResult<()> {
    manifest.validate_package_root(package_root)?;
    let canonical_root = fs::canonicalize(package_root)
        .await
        .map_err(|error| io_error("resolve extension package root", package_root, error))?;
    validate_text_asset(
        "use.extension.readme_invalid",
        "Cognitive package README",
        "UTF-8 Markdown",
        &canonical_root,
        &package_root.join("README.md"),
        MAX_PACKAGE_README_BYTES,
    )
    .await
    .map_err(|error| {
        UseError::new(
            "use.extension.readme_invalid",
            format!(
                "Cognitive packages require a bounded UTF-8 README.md: {}",
                error.message
            ),
        )
    })?;
    super::surface_files::validate_named_surface_files(manifest, &canonical_root, package_root)
        .await?;
    Ok(())
}

pub(super) async fn validate_text_asset(
    error_code: &'static str,
    label: &str,
    content_type: &str,
    canonical_root: &Path,
    path: &Path,
    max_bytes: u64,
) -> UseResult<()> {
    validate_surface_file(label, canonical_root, path, false).await?;
    let metadata = fs::metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if metadata.len() == 0 || metadata.len() > max_bytes {
        return Err(UseError::new(
            error_code,
            format!(
                "{label} '{}' must contain between 1 byte and {max_bytes} bytes.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| io_error(&format!("read {label}"), path, error))?;
    std::str::from_utf8(&bytes).map_err(|error| {
        UseError::new(
            error_code,
            format!(
                "{label} '{}' must be UTF-8 {content_type}: {error}",
                path.display()
            ),
        )
    })?;
    Ok(())
}

pub(super) async fn validate_surface_file(
    label: &str,
    canonical_root: &Path,
    path: &Path,
    require_executable: bool,
) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(UseError::new(
            "use.extension.surface_invalid",
            format!(
                "{label} '{}' must be a regular package file.",
                path.display()
            ),
        ));
    }
    let canonical = fs::canonicalize(path)
        .await
        .map_err(|error| io_error(&format!("resolve {label}"), path, error))?;
    if !canonical.starts_with(canonical_root) {
        return Err(UseError::new(
            "use.extension.path_escape",
            format!("{label} '{}' escapes the package.", path.display()),
        ));
    }
    if require_executable && !is_executable(&metadata) {
        return Err(UseError::new(
            "use.extension.surface_not_executable",
            format!("{label} '{}' is not executable.", path.display()),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

pub(crate) async fn copy_package(source: &Path, target: &Path) -> UseResult<()> {
    let mut pending = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some((source_dir, target_dir)) = pending.pop() {
        let source_metadata = fs::symlink_metadata(&source_dir)
            .await
            .map_err(|error| io_error("inspect extension package directory", &source_dir, error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&source_metadata) {
            return Err(UseError::new(
                "use.extension.package_symlink",
                format!(
                    "Extension package directory '{}' is a link or reparse point.",
                    source_dir.display()
                ),
            ));
        }
        if !source_metadata.is_dir() {
            return Err(UseError::new(
                "use.extension.package_entry_invalid",
                format!(
                    "Extension package directory '{}' is not a directory.",
                    source_dir.display()
                ),
            ));
        }
        fs::create_dir_all(&target_dir)
            .await
            .map_err(|error| io_error("create staged package directory", &target_dir, error))?;
        let mut entries = fs::read_dir(&source_dir)
            .await
            .map_err(|error| io_error("read extension package directory", &source_dir, error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("read extension package entry", &source_dir, error))?
        {
            let source_path = entry.path();
            let target_path = target_dir.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path).await.map_err(|error| {
                io_error("inspect extension package entry", &source_path, error)
            })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(UseError::new(
                    "use.extension.package_symlink",
                    format!(
                        "Extension package entry '{}' is a link or reparse point.",
                        source_path.display()
                    ),
                ));
            }
            if metadata.is_dir() {
                pending.push((source_path, target_path));
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes.saturating_add(metadata.len());
                if files > MAX_PACKAGE_FILES || bytes > MAX_PACKAGE_BYTES {
                    return Err(UseError::new(
                        "use.extension.package_too_large",
                        "The extension package exceeds the local installation limits.",
                    ));
                }
                fs::copy(&source_path, &target_path)
                    .await
                    .map_err(|error| {
                        io_error("copy extension package file", &source_path, error)
                    })?;
            } else {
                return Err(UseError::new(
                    "use.extension.package_entry_invalid",
                    format!(
                        "Extension package entry '{}' is not a regular file or directory.",
                        source_path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) async fn write_receipt(path: &Path, receipt: &ExtensionReceipt) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.extension.receipt_invalid",
            "The extension receipt path has no parent directory.",
        )
    })?;
    fs::create_dir_all(parent)
        .await
        .map_err(|error| io_error("create extension receipt directory", parent, error))?;
    let temporary = parent.join(format!(".receipt-{}.tmp", unique_suffix()));
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| {
        UseError::new(
            "use.extension.receipt_invalid",
            format!("Failed to encode extension receipt: {error}"),
        )
    })?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| io_error("create temporary extension receipt", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error("write extension receipt", &temporary, error));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error("sync extension receipt", &temporary, error));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path.to_path_buf(),
        "activate extension receipt",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(parent, "extension receipt").await?;
    Ok(())
}

pub(crate) async fn activate_temporary_file(
    temporary: PathBuf,
    target: PathBuf,
    action: &'static str,
) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        crate::rename_path_with_windows_retry_blocking(&temporary, &target)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.extension.io",
            format!(
                "Failed to {action} '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| io_error(action, &error_target, error))
}

pub(crate) async fn remove_file_with_windows_retry(
    path: PathBuf,
    action: &'static str,
) -> UseResult<()> {
    let error_target = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::atomic_file::remove_file_with_windows_retry_blocking(&path)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.extension.io",
            format!(
                "Failed to {action} '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| io_error(action, &error_target, error))
}

pub(crate) async fn remove_dir_all_with_windows_retry(
    path: PathBuf,
    action: &'static str,
) -> UseResult<()> {
    let error_target = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::atomic_file::remove_dir_all_with_windows_retry_blocking(&path)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.extension.io",
            format!(
                "Failed to {action} '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| io_error(action, &error_target, error))
}

#[cfg(unix)]
pub(crate) async fn sync_parent_directory(parent: &Path, label: &str) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| io_error(&format!("open {label} directory"), parent, error))?
        .sync_all()
        .await
        .map_err(|error| io_error(&format!("sync {label} directory"), parent, error))
}

#[cfg(not(unix))]
pub(crate) async fn sync_parent_directory(_parent: &Path, _label: &str) -> UseResult<()> {
    Ok(())
}

pub(crate) fn owned_package_path(
    paths: &ExtensionPaths,
    candidate: &Path,
    package_sha256: &str,
) -> bool {
    candidate.is_absolute()
        && candidate
            == paths
                .artifact_store()
                .expanded_package_path_from_sha256(package_sha256)
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

pub(crate) fn io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.extension.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

pub(crate) fn lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx reports ERROR_SHARING_VIOLATION or ERROR_LOCK_VIOLATION,
        // neither of which Rust consistently maps to WouldBlock.
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    false
}

pub(crate) struct RegistryLock {
    file: std::fs::File,
    _maintenance: Option<StateMaintenanceGuard>,
}

impl RegistryLock {
    pub(crate) fn acquire(path: &Path) -> UseResult<Self> {
        let parent = path.parent().ok_or_else(|| {
            UseError::new(
                "use.extension.lock_invalid",
                "The extension registry lock has no parent directory.",
            )
        })?;
        std::fs::create_dir_all(parent)
            .map_err(|error| io_error("create extension state directory", parent, error))?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| io_error("open extension registry lock", path, error))?;
        file.try_lock_exclusive().map_err(|error| {
            if lock_is_contended(&error) {
                UseError::new(
                    "use.extension.busy",
                    "Another extension registry operation is in progress.",
                )
            } else {
                io_error("acquire extension registry lock", path, error)
            }
        })?;
        file.set_len(0)
            .map_err(|error| io_error("truncate extension registry lock", path, error))?;
        writeln!(file, "{}", std::process::id())
            .map_err(|error| io_error("write extension registry lock", path, error))?;
        Ok(Self {
            file,
            _maintenance: None,
        })
    }

    /// Wait briefly for a read-side crash reconciliation to release the
    /// Registry lock before reporting real writer contention.
    ///
    /// Snapshot repair intentionally uses [`Self::acquire`] so a reader never
    /// waits behind a lifecycle mutation. Mutations use this bounded async
    /// path because a Code watcher may own the lock for one short repair
    /// between two otherwise independent package operations.
    pub(crate) async fn acquire_for_mutation(paths: &ExtensionPaths) -> UseResult<Self> {
        let maintenance = StateMaintenanceLock::new(paths.state_root())
            .acquire_shared()
            .await?;
        let mut lock = Self::acquire_for_mutation_with_wait(
            &paths.registry_lock_path(),
            REGISTRY_MUTATION_LOCK_WAIT,
        )
        .await?;
        lock._maintenance = Some(maintenance);
        Ok(lock)
    }

    async fn acquire_for_mutation_with_wait(path: &Path, wait: Duration) -> UseResult<Self> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match Self::acquire(path) {
                Ok(lock) => return Ok(lock),
                Err(error) if error.code == "use.extension.busy" => {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(
                        REGISTRY_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                    )
                    .await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod registry_lock_tests {
    use super::*;

    #[tokio::test]
    async fn mutation_waits_for_a_transient_registry_reconciliation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state/.registry.lock");
        let transient = RegistryLock::acquire(&path).unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(75)).await;
            drop(transient);
        });

        let acquired =
            RegistryLock::acquire_for_mutation_with_wait(&path, Duration::from_millis(500)).await;

        release.await.unwrap();
        assert!(acquired.is_ok());
    }

    #[tokio::test]
    async fn mutation_reports_busy_after_the_bounded_wait() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state/.registry.lock");
        let _held = RegistryLock::acquire(&path).unwrap();

        let error =
            match RegistryLock::acquire_for_mutation_with_wait(&path, Duration::from_millis(75))
                .await
            {
                Ok(_) => panic!("a held Registry lock must remain busy after the wait bound"),
                Err(error) => error,
            };

        assert_eq!(error.code, "use.extension.busy");
    }
}

#[cfg(test)]
mod atomic_activation_tests {
    use super::*;

    #[tokio::test]
    async fn atomic_activation_replaces_a_target_beyond_the_legacy_windows_path_limit() {
        let temporary_root = tempfile::tempdir().unwrap();
        let parent = temporary_root
            .path()
            .join("a".repeat(100))
            .join("b".repeat(100));
        std::fs::create_dir_all(&parent).unwrap();
        let temporary = parent.join(format!(".{}.tmp", "c".repeat(64)));
        let target = parent.join("d".repeat(64));
        assert!(temporary.as_os_str().len() > 260);
        std::fs::write(&temporary, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();

        activate_temporary_file(
            temporary.clone(),
            target.clone(),
            "activate long-path test file",
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!temporary.exists());
    }
}
