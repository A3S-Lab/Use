use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::registry::ExtensionReceipt;
use super::{ExtensionManifest, ExtensionPaths};

pub(crate) const MANIFEST_NAME: &str = "a3s-use-extension.acl";
pub(crate) const MAX_PACKAGE_FILES: usize = 10_000;
pub(crate) const MAX_PACKAGE_BYTES: u64 = 1_073_741_824;
const MAX_ACTIVITY_HTML_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ACTIVITY_RESOURCE_BYTES: u64 = 2 * 1024 * 1024;

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
    if let Some(cli) = &manifest.cli {
        validate_surface_file(
            "CLI executable",
            &canonical_root,
            &package_root.join(&cli.executable),
            true,
        )
        .await?;
    }
    if let Some(mcp) = &manifest.mcp {
        validate_surface_file(
            "MCP executable",
            &canonical_root,
            &package_root.join(&mcp.executable),
            true,
        )
        .await?;
    }
    if let Some(skill) = &manifest.skill {
        validate_surface_file(
            "Skill file",
            &canonical_root,
            &package_root.join(&skill.path),
            false,
        )
        .await?;
    }
    for activity in &manifest.contributes.activity_bar {
        validate_activity_text_asset(
            "Activity Bar entry",
            "HTML",
            &canonical_root,
            &package_root.join(&activity.entry),
            MAX_ACTIVITY_HTML_BYTES,
        )
        .await?;
        for style in &activity.styles {
            validate_activity_text_asset(
                "Activity Bar style",
                "CSS",
                &canonical_root,
                &package_root.join(style),
                MAX_ACTIVITY_RESOURCE_BYTES,
            )
            .await?;
        }
        for script in &activity.scripts {
            validate_activity_text_asset(
                "Activity Bar script",
                "JavaScript",
                &canonical_root,
                &package_root.join(script),
                MAX_ACTIVITY_RESOURCE_BYTES,
            )
            .await?;
        }
    }
    Ok(())
}

async fn validate_activity_text_asset(
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
            "use.extension.activity_asset_invalid",
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
            "use.extension.activity_asset_invalid",
            format!(
                "{label} '{}' must be UTF-8 {content_type}: {error}",
                path.display()
            ),
        )
    })?;
    Ok(())
}

async fn validate_surface_file(
    label: &str,
    canonical_root: &Path,
    path: &Path,
    require_executable: bool,
) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
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
            if metadata.file_type().is_symlink() {
                return Err(UseError::new(
                    "use.extension.package_symlink",
                    format!(
                        "Extension package entry '{}' is a symbolic link.",
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
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.extension.io",
            format!(
                "Failed to {action} '{}': atomic replacement task failed: {error}",
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
    package_id: &str,
    candidate: &Path,
) -> bool {
    candidate.is_absolute()
        && candidate.starts_with(paths.package_parent(package_id))
        && candidate.parent() == Some(paths.package_parent(package_id).as_path())
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

pub(crate) struct RegistryLock {
    file: std::fs::File,
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
            if error.kind() == std::io::ErrorKind::WouldBlock {
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
        Ok(Self { file })
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
