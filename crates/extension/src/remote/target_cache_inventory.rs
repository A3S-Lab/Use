use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

use crate::package::{io_error, sync_parent_directory};

use super::{VerifiedTargetCachePolicy, MAX_REMOTE_ARCHIVE_BYTES};

const MAX_SCANNED_TARGET_CACHE_ENTRIES: u64 = 100_000;

#[derive(Debug)]
struct TargetEntry {
    path: PathBuf,
    digest: String,
    bytes: u64,
    modified_key: u128,
}

#[derive(Debug)]
struct StaleEntry {
    path: PathBuf,
    bytes: u64,
}

#[derive(Debug, Default)]
struct CacheInventory {
    targets: Vec<TargetEntry>,
    stale: Vec<StaleEntry>,
    target_bytes: u64,
    stale_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CacheStats {
    pub target_entries: u64,
    pub target_bytes: u64,
    pub stale_entries: u64,
    pub stale_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RemovedCacheStats {
    pub target_entries: u64,
    pub target_bytes: u64,
    pub stale_entries: u64,
    pub stale_bytes: u64,
}

pub(super) async fn inspect_cache(cache_directory: &Path) -> UseResult<CacheStats> {
    let inventory = scan_cache(cache_directory).await?;
    Ok(stats(&inventory, available_space(cache_directory).await?))
}

pub(super) async fn admit_target_write(
    cache_directory: &Path,
    digest: &str,
    expected_length: u64,
    policy: VerifiedTargetCachePolicy,
) -> UseResult<RemovedCacheStats> {
    enforce_policy(cache_directory, policy, Some((digest, expected_length))).await
}

pub(super) async fn prune_cache(
    cache_directory: &Path,
    policy: VerifiedTargetCachePolicy,
) -> UseResult<RemovedCacheStats> {
    enforce_policy(cache_directory, policy, None).await
}

pub(super) async fn ensure_staging_capacity(
    staging_directory: &Path,
    expected_length: u64,
    policy: VerifiedTargetCachePolicy,
) -> UseResult<()> {
    ensure_target_fits_policy(expected_length, policy)?;
    let required = expected_length
        .checked_add(policy.min_free_bytes())
        .ok_or_else(|| storage_error("The target staging space requirement overflowed."))?;
    let available = available_space(staging_directory).await?;
    if available < required {
        return Err(
            storage_error("The target download does not have enough staging disk space.")
                .with_detail("availableBytes", available.to_string())
                .with_detail("requiredBytes", required.to_string())
                .with_detail("targetBytes", expected_length.to_string())
                .with_detail("minimumFreeBytes", policy.min_free_bytes().to_string()),
        );
    }
    Ok(())
}

async fn enforce_policy(
    cache_directory: &Path,
    policy: VerifiedTargetCachePolicy,
    incoming: Option<(&str, u64)>,
) -> UseResult<RemovedCacheStats> {
    if let Some((_, expected_length)) = incoming {
        ensure_target_fits_policy(expected_length, policy)?;
    }
    let mut inventory = scan_cache(cache_directory).await?;
    let available = available_space(cache_directory).await?;
    let incoming_is_present = incoming
        .is_some_and(|(digest, _)| inventory.targets.iter().any(|entry| entry.digest == digest));
    let incoming_bytes = incoming
        .filter(|_| !incoming_is_present)
        .map_or(0, |(_, bytes)| bytes);
    let incoming_entries = u64::from(incoming.is_some() && !incoming_is_present);
    let protected_digest = incoming.map(|(digest, _)| digest);

    let protected_bytes = incoming.map_or(0, |(digest, expected_length)| {
        inventory
            .targets
            .iter()
            .find(|entry| entry.digest == digest)
            .map_or(expected_length, |entry| entry.bytes)
    });
    if protected_bytes > policy.max_bytes() {
        return Err(policy_exceeded(
            "The verified target cannot fit within the configured cache byte limit.",
            policy,
            protected_bytes,
        ));
    }

    let mut candidates = inventory
        .targets
        .iter()
        .filter(|entry| protected_digest != Some(entry.digest.as_str()))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.modified_key
            .cmp(&right.modified_key)
            .then_with(|| left.digest.cmp(&right.digest))
    });

    let required_available = match incoming {
        None => policy.min_free_bytes(),
        Some(_) if incoming_bytes == 0 => 0,
        Some(_) => incoming_bytes
            .checked_add(policy.min_free_bytes())
            .ok_or_else(|| storage_error("The target cache space requirement overflowed."))?,
    };
    let maximum_reclaimable = inventory
        .stale_bytes
        .checked_add(
            candidates
                .iter()
                .try_fold(0_u64, |total, entry| total.checked_add(entry.bytes))
                .ok_or_else(|| cache_invalid("The target cache byte inventory overflowed."))?,
        )
        .ok_or_else(|| cache_invalid("The reclaimable target cache bytes overflowed."))?;
    if available.saturating_add(maximum_reclaimable) < required_available {
        return Err(storage_error(
            "The verified target cache cannot reclaim enough disk space for the target.",
        )
        .with_detail("availableBytes", available.to_string())
        .with_detail("reclaimableBytes", maximum_reclaimable.to_string())
        .with_detail("requiredBytes", required_available.to_string()));
    }

    let mut retained_bytes = inventory
        .target_bytes
        .checked_add(incoming_bytes)
        .ok_or_else(|| cache_invalid("The retained target cache bytes overflowed."))?;
    let mut retained_entries = (inventory.targets.len() as u64)
        .checked_add(incoming_entries)
        .ok_or_else(|| cache_invalid("The retained target cache entries overflowed."))?;
    let mut projected_available = available.saturating_add(inventory.stale_bytes);
    let mut selected_targets = Vec::new();
    for entry in candidates {
        if retained_bytes <= policy.max_bytes()
            && retained_entries <= policy.max_entries()
            && projected_available >= required_available
        {
            break;
        }
        retained_bytes = retained_bytes
            .checked_sub(entry.bytes)
            .ok_or_else(|| cache_invalid("The retained target cache bytes underflowed."))?;
        retained_entries = retained_entries
            .checked_sub(1)
            .ok_or_else(|| cache_invalid("The retained target cache entries underflowed."))?;
        projected_available = projected_available.saturating_add(entry.bytes);
        selected_targets.push(entry.path.clone());
    }
    if retained_bytes > policy.max_bytes() || retained_entries > policy.max_entries() {
        return Err(policy_exceeded(
            "The verified target cache cannot satisfy its configured retention limits.",
            policy,
            retained_bytes,
        ));
    }
    if projected_available < required_available {
        return Err(storage_error(
            "The verified target cache cannot satisfy its minimum free-space reserve.",
        ));
    }

    let mut removed = RemovedCacheStats::default();
    for entry in inventory.stale.drain(..) {
        remove_cache_file(&entry.path).await?;
        removed.stale_entries += 1;
        removed.stale_bytes = removed.stale_bytes.saturating_add(entry.bytes);
    }
    for path in selected_targets {
        let entry = inventory
            .targets
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| cache_invalid("The selected cache entry disappeared from inventory."))?;
        remove_cache_file(&entry.path).await?;
        removed.target_entries += 1;
        removed.target_bytes = removed.target_bytes.saturating_add(entry.bytes);
    }
    if removed.target_entries > 0 || removed.stale_entries > 0 {
        sync_parent_directory(cache_directory, "verified target cache pruning").await?;
    }
    if required_available > 0 {
        let actual_available = available_space(cache_directory).await?;
        if actual_available < required_available {
            return Err(storage_error(
                "The verified target cache free space changed during admission.",
            )
            .with_detail("availableBytes", actual_available.to_string())
            .with_detail("requiredBytes", required_available.to_string()));
        }
    }
    Ok(removed)
}

async fn scan_cache(cache_directory: &Path) -> UseResult<CacheInventory> {
    let mut inventory = CacheInventory::default();
    let mut scanned_entries = 0_u64;
    let mut entries = fs::read_dir(cache_directory)
        .await
        .map_err(|error| io_error("read verified target cache", cache_directory, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("read verified target cache entry", cache_directory, error))?
    {
        scanned_entries = scanned_entries
            .checked_add(1)
            .ok_or_else(|| cache_invalid("The target cache entry count overflowed."))?;
        if scanned_entries > MAX_SCANNED_TARGET_CACHE_ENTRIES {
            return Err(cache_invalid(format!(
                "The verified target cache exceeds the {MAX_SCANNED_TARGET_CACHE_ENTRIES}-entry scan limit."
            )));
        }
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| {
            cache_invalid("The verified target cache contains a non-UTF-8 entry name.")
        })?;
        let metadata = fs::symlink_metadata(&path)
            .await
            .map_err(|error| io_error("inspect verified target cache entry", &path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(
                cache_invalid("Every verified target cache entry must be a regular file.")
                    .with_detail("path", path.display().to_string()),
            );
        }
        if valid_digest_name(&name) {
            if metadata.len() == 0 || metadata.len() > MAX_REMOTE_ARCHIVE_BYTES {
                return Err(cache_invalid(
                    "A verified target cache entry has an invalid bounded length.",
                )
                .with_detail("path", path.display().to_string())
                .with_detail("length", metadata.len().to_string()));
            }
            inventory.target_bytes = inventory
                .target_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| cache_invalid("The target cache byte inventory overflowed."))?;
            inventory.targets.push(TargetEntry {
                path,
                digest: name,
                bytes: metadata.len(),
                modified_key: modified_key(metadata.modified().ok()),
            });
        } else if valid_temporary_name(&name) {
            inventory.stale_bytes = inventory
                .stale_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| cache_invalid("The stale cache byte inventory overflowed."))?;
            inventory.stale.push(StaleEntry {
                path,
                bytes: metadata.len(),
            });
        } else {
            return Err(
                cache_invalid("The verified target cache contains an unexpected entry.")
                    .with_detail("path", path.display().to_string()),
            );
        }
    }
    Ok(inventory)
}

fn stats(inventory: &CacheInventory, available_bytes: u64) -> CacheStats {
    CacheStats {
        target_entries: inventory.targets.len() as u64,
        target_bytes: inventory.target_bytes,
        stale_entries: inventory.stale.len() as u64,
        stale_bytes: inventory.stale_bytes,
        available_bytes,
    }
}

async fn available_space(path: &Path) -> UseResult<u64> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || fs2::available_space(&owned))
        .await
        .map_err(|error| {
            storage_error(format!(
                "Failed to inspect verified target cache disk space: {error}"
            ))
        })?
        .map_err(|error| io_error("inspect available disk space", path, error))
}

async fn remove_cache_file(path: &Path) -> UseResult<()> {
    fs::remove_file(path)
        .await
        .map_err(|error| io_error("remove verified target cache entry", path, error))
}

fn ensure_target_fits_policy(
    expected_length: u64,
    policy: VerifiedTargetCachePolicy,
) -> UseResult<()> {
    if expected_length > policy.max_bytes() {
        return Err(policy_exceeded(
            "The signed target exceeds the configured verified target cache byte limit.",
            policy,
            expected_length,
        ));
    }
    Ok(())
}

fn valid_digest_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_temporary_name(name: &str) -> bool {
    let Some(suffix) = name
        .strip_prefix(".target-")
        .and_then(|name| name.strip_suffix(".tmp"))
    else {
        return false;
    };
    !suffix.is_empty()
        && suffix.len() <= 80
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
}

fn modified_key(modified: Option<SystemTime>) -> u128 {
    modified
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos())
}

fn policy_exceeded(
    message: impl Into<String>,
    policy: VerifiedTargetCachePolicy,
    target_bytes: u64,
) -> UseError {
    UseError::new(
        "use.extension.registry_target_cache_policy_exceeded",
        message,
    )
    .with_detail("targetBytes", target_bytes.to_string())
    .with_detail("maxBytes", policy.max_bytes().to_string())
    .with_detail("maxEntries", policy.max_entries().to_string())
}

fn storage_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.extension.registry_target_cache_storage_insufficient",
        message,
    )
}

fn cache_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_target_cache_invalid", message)
}

#[cfg(test)]
mod tests {
    use std::fs::{File, FileTimes};
    use std::io::Write;
    use std::time::Duration;

    use super::*;

    #[test]
    fn cache_entry_names_are_exact_and_portable() {
        assert!(valid_digest_name(&"a".repeat(64)));
        assert!(!valid_digest_name(&"A".repeat(64)));
        assert!(!valid_digest_name(&"a".repeat(63)));
        assert!(valid_temporary_name(".target-123-456.tmp"));
        assert!(!valid_temporary_name(".target-../456.tmp"));
        assert!(!valid_temporary_name("unowned.tmp"));
    }

    #[tokio::test]
    async fn staging_capacity_overflow_fails_before_disk_inspection() {
        let policy = VerifiedTargetCachePolicy::new(1, 1, 1).unwrap();
        let error = ensure_staging_capacity(Path::new("."), u64::MAX, policy)
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            "use.extension.registry_target_cache_policy_exceeded"
        );
    }

    #[tokio::test]
    async fn prune_removes_stale_files_then_the_oldest_verified_target() {
        let temporary = tempfile::tempdir().unwrap();
        let old_path = temporary.path().join("a".repeat(64));
        let new_path = temporary.path().join("b".repeat(64));
        let stale_path = temporary.path().join(".target-123-456.tmp");
        for (path, body, modified) in [
            (
                &old_path,
                b"old".as_slice(),
                UNIX_EPOCH + Duration::from_secs(1),
            ),
            (
                &new_path,
                b"new".as_slice(),
                UNIX_EPOCH + Duration::from_secs(2),
            ),
            (
                &stale_path,
                b"stale".as_slice(),
                UNIX_EPOCH + Duration::from_secs(3),
            ),
        ] {
            let mut file = File::create(path).unwrap();
            file.write_all(body).unwrap();
            file.sync_all().unwrap();
            file.set_times(FileTimes::new().set_modified(modified))
                .unwrap();
        }
        let policy = VerifiedTargetCachePolicy::new(3, 1, 0).unwrap();

        let removed = prune_cache(temporary.path(), policy).await.unwrap();

        assert_eq!(removed.target_entries, 1);
        assert_eq!(removed.target_bytes, 3);
        assert_eq!(removed.stale_entries, 1);
        assert_eq!(removed.stale_bytes, 5);
        assert!(!old_path.exists());
        assert!(new_path.is_file());
        assert!(!stale_path.exists());
        let usage = inspect_cache(temporary.path()).await.unwrap();
        assert_eq!(usage.target_entries, 1);
        assert_eq!(usage.target_bytes, 3);
    }

    #[tokio::test]
    async fn inventory_rejects_unowned_entries() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("unowned"), b"x").unwrap();
        assert_eq!(
            inspect_cache(temporary.path()).await.unwrap_err().code,
            "use.extension.registry_target_cache_invalid"
        );
    }
}
