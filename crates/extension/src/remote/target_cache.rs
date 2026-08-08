use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::package::{activate_temporary_file, io_error, sync_parent_directory, unique_suffix};

use super::target_cache_inventory::{
    admit_target_write, ensure_staging_capacity, inspect_cache, prune_cache,
};
use super::{
    normalize_sha256, TrustedRegistry, VerifiedTargetCachePolicy, VerifiedTargetCachePruneResult,
    VerifiedTargetCacheUsage, MAX_REMOTE_ARCHIVE_BYTES, VERIFIED_TARGET_CACHE_SCHEMA_VERSION,
};

const VERIFIED_TARGETS_DIRECTORY: &str = "verified-targets";
const SHA256_DIRECTORY: &str = "sha256";
const TARGET_CACHE_LOCK: &str = ".target-cache.lock";

struct TargetCacheLock(File);

impl Drop for TargetCacheLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) async fn persist_verified_target(
    datastore: &Path,
    source: &Path,
    expected_length: u64,
    expected_sha256: &str,
    policy: VerifiedTargetCachePolicy,
) -> UseResult<()> {
    let expected_sha256 = validated_evidence(expected_length, expected_sha256)?;
    let _lock = acquire_target_cache_lock(datastore, true)?;
    let cache_directory = ensure_cache_directory(datastore).await?;
    let target = cache_directory.join(&expected_sha256);
    match fs::symlink_metadata(&target).await {
        Ok(metadata) => {
            validate_regular_metadata(
                &target,
                &metadata,
                expected_length,
                "use.extension.registry_target_cache_invalid",
                "The verified target cache entry is not a bounded regular file.",
            )?;
            verify_file(
                &target,
                None,
                expected_length,
                &expected_sha256,
                "use.extension.registry_target_cache_invalid",
            )
            .await?;
            admit_target_write(&cache_directory, &expected_sha256, expected_length, policy).await?;
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect verified target cache", &target, error)),
    }

    admit_target_write(&cache_directory, &expected_sha256, expected_length, policy).await?;

    let temporary = cache_directory.join(format!(".target-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut output = options
        .open(&temporary)
        .await
        .map_err(|error| io_error("create verified target cache", &temporary, error))?;
    if let Err(error) = verify_file(
        source,
        Some(&mut output),
        expected_length,
        &expected_sha256,
        "use.extension.registry_target_invalid",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    if let Err(error) = output.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error("sync verified target cache", &temporary, error));
    }
    drop(output);
    secure_file(&temporary).await?;
    if let Err(error) =
        activate_temporary_file(temporary.clone(), target, "activate verified target cache").await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(&cache_directory, "verified target cache").await
}

pub(super) async fn stage_cached_target(
    datastore: &Path,
    file_name: &str,
    expected_length: u64,
    expected_sha256: &str,
    policy: VerifiedTargetCachePolicy,
) -> UseResult<(TempDir, PathBuf)> {
    let expected_sha256 = validated_evidence(expected_length, expected_sha256)?;
    validate_staging_file_name(file_name)?;
    let _lock = acquire_target_cache_lock(datastore, false)?;
    let cache_directory = existing_cache_directory(datastore).await?;
    let source = cache_directory.join(&expected_sha256);
    let metadata = fs::symlink_metadata(&source).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            target_cache_error(
                "use.extension.registry_target_cache_missing",
                format!(
                    "Verified target '{}' is not available in the local Registry cache.",
                    expected_sha256
                ),
            )
        } else {
            io_error("inspect cached Registry target", &source, error)
        }
    })?;
    validate_regular_metadata(
        &source,
        &metadata,
        expected_length,
        "use.extension.registry_target_cache_invalid",
        "The cached Registry target is not a bounded regular file.",
    )?;

    let temporary = tokio::task::spawn_blocking(tempfile::tempdir)
        .await
        .map_err(|error| {
            target_cache_error(
                "use.extension.registry_target_cache_invalid",
                format!("Failed to create cached target staging task: {error}"),
            )
        })?
        .map_err(|error| {
            target_cache_error(
                "use.extension.registry_target_cache_invalid",
                format!("Failed to create cached target staging: {error}"),
            )
        })?;
    ensure_staging_capacity(temporary.path(), expected_length, policy).await?;
    let target = temporary.path().join(file_name);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut output = options
        .open(&target)
        .await
        .map_err(|error| io_error("create cached target staging file", &target, error))?;
    verify_file(
        &source,
        Some(&mut output),
        expected_length,
        &expected_sha256,
        "use.extension.registry_target_cache_invalid",
    )
    .await?;
    output
        .sync_all()
        .await
        .map_err(|error| io_error("sync cached target staging file", &target, error))?;
    drop(output);
    Ok((temporary, target))
}

pub(super) async fn inspect_registry_target_cache(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedTargetCacheUsage> {
    super::catalog::validate_target_cache_registry_identity(registry).await?;
    ensure_datastore_directory(registry.datastore()).await?;
    let _lock = acquire_target_cache_lock(registry.datastore(), false)?;
    let cache_directory = ensure_cache_directory(registry.datastore()).await?;
    let stats = inspect_cache(&cache_directory).await?;
    Ok(usage(registry, stats))
}

pub(super) async fn prune_registry_target_cache(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedTargetCachePruneResult> {
    super::catalog::validate_target_cache_registry_identity(registry).await?;
    ensure_datastore_directory(registry.datastore()).await?;
    let _lock = acquire_target_cache_lock(registry.datastore(), true)?;
    let cache_directory = ensure_cache_directory(registry.datastore()).await?;
    let before = usage(registry, inspect_cache(&cache_directory).await?);
    let removed = prune_cache(&cache_directory, registry.target_cache_policy()).await?;
    let after = usage(registry, inspect_cache(&cache_directory).await?);
    Ok(VerifiedTargetCachePruneResult {
        schema_version: VERIFIED_TARGET_CACHE_SCHEMA_VERSION,
        before,
        after,
        removed_target_entries: removed.target_entries,
        removed_target_bytes: removed.target_bytes,
        removed_stale_entries: removed.stale_entries,
        removed_stale_bytes: removed.stale_bytes,
    })
}

fn usage(
    registry: &TrustedRegistry,
    stats: super::target_cache_inventory::CacheStats,
) -> VerifiedTargetCacheUsage {
    VerifiedTargetCacheUsage {
        schema_version: VERIFIED_TARGET_CACHE_SCHEMA_VERSION,
        registry_name: registry.name().to_owned(),
        registry_url: registry.base_url().to_string(),
        target_entries: stats.target_entries,
        target_bytes: stats.target_bytes,
        stale_entries: stats.stale_entries,
        stale_bytes: stats.stale_bytes,
        available_bytes: stats.available_bytes,
        policy: registry.target_cache_policy(),
    }
}

async fn verify_file(
    path: &Path,
    mut output: Option<&mut fs::File>,
    expected_length: u64,
    expected_sha256: &str,
    error_code: &'static str,
) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect Registry target", path, error))?;
    validate_regular_metadata(
        path,
        &metadata,
        expected_length,
        error_code,
        "The Registry target is not a bounded regular file.",
    )?;
    let mut input = fs::File::open(path)
        .await
        .map_err(|error| io_error("open Registry target", path, error))?;
    let opened_metadata = input
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened Registry target", path, error))?;
    validate_regular_metadata(
        path,
        &opened_metadata,
        expected_length,
        error_code,
        "The opened Registry target changed before verification.",
    )?;

    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("read Registry target", path, error))?;
        if read == 0 {
            break;
        }
        length = length.checked_add(read as u64).ok_or_else(|| {
            target_cache_error(
                error_code,
                "The Registry target length overflowed its bound.",
            )
        })?;
        if length > expected_length || length > MAX_REMOTE_ARCHIVE_BYTES {
            return Err(target_cache_error(
                error_code,
                "The Registry target exceeds its signed length.",
            ));
        }
        digest.update(&buffer[..read]);
        if let Some(destination) = output.as_deref_mut() {
            destination
                .write_all(&buffer[..read])
                .await
                .map_err(|error| io_error("write verified Registry target", path, error))?;
        }
    }
    let actual_sha256 = format!("{:x}", digest.finalize());
    if length != expected_length || actual_sha256 != expected_sha256 {
        return Err(target_cache_error(
            error_code,
            "The Registry target does not match its signed length and SHA-256 digest.",
        )
        .with_detail("expectedLength", expected_length.to_string())
        .with_detail("actualLength", length.to_string())
        .with_detail("expectedSha256", expected_sha256.to_owned())
        .with_detail("actualSha256", actual_sha256));
    }
    Ok(())
}

fn validated_evidence(expected_length: u64, expected_sha256: &str) -> UseResult<String> {
    if expected_length == 0 || expected_length > MAX_REMOTE_ARCHIVE_BYTES {
        return Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            "The verified target cache evidence has an invalid length.",
        ));
    }
    normalize_sha256(expected_sha256, "verified target cache")
}

fn validate_regular_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    expected_length: u64,
    error_code: &'static str,
    message: &'static str,
) -> UseResult<()> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != expected_length
        || metadata.len() > MAX_REMOTE_ARCHIVE_BYTES
    {
        return Err(
            target_cache_error(error_code, message).with_detail("path", path.display().to_string())
        );
    }
    Ok(())
}

async fn ensure_cache_directory(datastore: &Path) -> UseResult<PathBuf> {
    let targets = datastore.join(VERIFIED_TARGETS_DIRECTORY);
    ensure_real_directory(&targets, "verified target cache").await?;
    let sha256 = targets.join(SHA256_DIRECTORY);
    ensure_real_directory(&sha256, "SHA-256 target cache").await?;
    Ok(sha256)
}

async fn ensure_datastore_directory(datastore: &Path) -> UseResult<()> {
    fs::create_dir_all(datastore)
        .await
        .map_err(|error| io_error("create Registry datastore", datastore, error))?;
    inspect_real_directory(datastore, "Registry datastore").await
}

async fn existing_cache_directory(datastore: &Path) -> UseResult<PathBuf> {
    let targets = datastore.join(VERIFIED_TARGETS_DIRECTORY);
    inspect_real_directory(&targets, "verified target cache").await?;
    let sha256 = targets.join(SHA256_DIRECTORY);
    inspect_real_directory(&sha256, "SHA-256 target cache").await?;
    Ok(sha256)
}

async fn ensure_real_directory(path: &Path, label: &str) -> UseResult<()> {
    fs::create_dir_all(path)
        .await
        .map_err(|error| io_error(&format!("create {label}"), path, error))?;
    inspect_real_directory(path, label).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|error| io_error(&format!("secure {label}"), path, error))?;
    }
    Ok(())
}

async fn inspect_real_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            target_cache_error(
                "use.extension.registry_target_cache_missing",
                format!("The {label} does not exist."),
            )
        } else {
            io_error(&format!("inspect {label}"), path, error)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            format!("The {label} must be a real directory."),
        ));
    }
    Ok(())
}

fn acquire_target_cache_lock(datastore: &Path, exclusive: bool) -> UseResult<TargetCacheLock> {
    let path = datastore.join(TARGET_CACHE_LOCK);
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "The verified target cache lock must be a regular file.",
            ));
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error("open verified target cache lock", &path, error))?;
    let result = if exclusive {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    };
    result.map_err(|error| {
        UseError::new(
            "use.extension.registry_busy",
            format!(
                "Another process is accessing the verified Registry target cache '{}': {error}",
                datastore.display()
            ),
        )
    })?;
    Ok(TargetCacheLock(file))
}

fn validate_staging_file_name(file_name: &str) -> UseResult<()> {
    let mut components = Path::new(file_name).components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !file_name.is_empty();
    if valid {
        Ok(())
    } else {
        Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            "The cached Registry target staging name is invalid.",
        ))
    }
}

#[cfg(unix)]
async fn secure_file(path: &Path) -> UseResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(|error| io_error("secure verified target cache file", path, error))
}

#[cfg(not(unix))]
async fn secure_file(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn target_cache_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
