use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{secure_file, target_cache_error, validated_evidence, MAX_REMOTE_ARCHIVE_BYTES};
use crate::package::{io_error, sync_parent_directory, unique_suffix};

pub(in crate::remote) const TARGET_OBSERVATION_SCHEMA: &str =
    "a3s.use.registry-target-observation.v1";
const MAX_TARGET_OBSERVATION_BYTES: u64 = 4 * 1024;

/// Source-scoped evidence that one signed target was admitted by digest.
///
/// The record contains no target bytes and is not trust authority. TUF remains
/// the source authority, while the global Artifact Store owns immutable bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::remote) struct TargetObservationRecord {
    pub(in crate::remote) schema: String,
    pub(in crate::remote) target_digest: String,
    pub(in crate::remote) expected_bytes: u64,
}

pub(in crate::remote) fn observation_path(cache_directory: &Path, sha256: &str) -> PathBuf {
    cache_directory.join(format!("{sha256}.json"))
}

pub(in crate::remote) async fn read_observation(
    cache_directory: &Path,
    sha256: &str,
    expected_length: u64,
) -> UseResult<Option<TargetObservationRecord>> {
    let sha256 = validated_evidence(expected_length, sha256)?;
    read_observation_path(
        &observation_path(cache_directory, &sha256),
        &sha256,
        Some(expected_length),
    )
    .await
}

pub(in crate::remote) async fn read_observation_path(
    path: &Path,
    sha256: &str,
    expected_length: Option<u64>,
) -> UseResult<Option<TargetObservationRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("inspect Registry target observation", path, error)),
    };
    validate_observation_metadata(path, &metadata)?;
    let mut file = observation_open_options()
        .open(path)
        .await
        .map_err(|error| io_error("open Registry target observation", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened Registry target observation", path, error))?;
    validate_observation_metadata(path, &opened)?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    (&mut file)
        .take(MAX_TARGET_OBSERVATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read Registry target observation", path, error))?;
    if bytes.len() as u64 != opened.len() {
        return Err(observation_invalid(
            path,
            "The Registry target observation changed while it was read.",
        ));
    }
    let record: TargetObservationRecord = serde_json::from_slice(&bytes).map_err(|error| {
        observation_invalid(
            path,
            format!("The Registry target observation is invalid JSON: {error}"),
        )
    })?;
    validate_record(path, &record, sha256, expected_length)?;
    let canonical = serde_json::to_vec(&record).map_err(|error| {
        observation_invalid(
            path,
            format!("Failed to canonicalize the Registry target observation: {error}"),
        )
    })?;
    if bytes != canonical {
        return Err(observation_invalid(
            path,
            "The Registry target observation encoding is not canonical.",
        ));
    }
    Ok(Some(record))
}

pub(in crate::remote) async fn write_observation(
    cache_directory: &Path,
    sha256: &str,
    expected_length: u64,
) -> UseResult<()> {
    let sha256 = validated_evidence(expected_length, sha256)?;
    if read_observation(cache_directory, &sha256, expected_length)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let record = TargetObservationRecord {
        schema: TARGET_OBSERVATION_SCHEMA.to_owned(),
        target_digest: format!("sha256:{sha256}"),
        expected_bytes: expected_length,
    };
    let bytes = serde_json::to_vec(&record).map_err(|error| {
        target_cache_error(
            "use.extension.registry_target_cache_invalid",
            format!("Failed to encode the Registry target observation: {error}"),
        )
    })?;
    let target = observation_path(cache_directory, &sha256);
    let temporary = cache_directory.join(format!(".target-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| io_error("create Registry target observation", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        drop(file);
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write Registry target observation",
            &temporary,
            error,
        ));
    }
    if let Err(error) = secure_file(&file, &temporary).await {
        drop(file);
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    if let Err(error) = file.sync_all().await {
        drop(file);
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "sync Registry target observation",
            &temporary,
            error,
        ));
    }
    drop(file);
    let temporary_for_error = temporary.clone();
    let target_for_worker = target.clone();
    let persisted = match tokio::task::spawn_blocking(move || {
        crate::atomic_file::persist_temporary_noclobber_blocking(temporary, &target_for_worker)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&temporary_for_error).await;
            return Err(target_cache_error(
                "use.extension.registry_target_cache_invalid",
                format!("Registry target observation publication did not complete: {error}"),
            ));
        }
    };
    if let Err(error) = persisted {
        let _ = fs::remove_file(&temporary_for_error).await;
        return Err(io_error(
            "publish Registry target observation",
            &target,
            error,
        ));
    }
    sync_parent_directory(cache_directory, "Registry target observation").await
}

fn validate_record(
    path: &Path,
    record: &TargetObservationRecord,
    sha256: &str,
    expected_length: Option<u64>,
) -> UseResult<()> {
    if record.schema != TARGET_OBSERVATION_SCHEMA
        || record.target_digest != format!("sha256:{sha256}")
        || record.expected_bytes == 0
        || record.expected_bytes > MAX_REMOTE_ARCHIVE_BYTES
        || expected_length.is_some_and(|length| length != record.expected_bytes)
    {
        return Err(observation_invalid(
            path,
            "The Registry target observation does not match its digest and length identity.",
        ));
    }
    Ok(())
}

fn validate_observation_metadata(path: &Path, metadata: &std::fs::Metadata) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_TARGET_OBSERVATION_BYTES
    {
        return Err(observation_invalid(
            path,
            "The Registry target observation is not a bounded owned file.",
        ));
    }
    Ok(())
}

fn observation_open_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    options
}

fn observation_invalid(path: &Path, message: impl Into<String>) -> UseError {
    target_cache_error("use.extension.registry_target_cache_invalid", message)
        .with_detail("path", path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn observations_are_canonical_idempotent_and_path_free() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("verified-targets/sha256");
        fs::create_dir_all(&cache).await.unwrap();
        let sha256 = "a".repeat(64);
        write_observation(&cache, &sha256, 42).await.unwrap();
        write_observation(&cache, &sha256, 42).await.unwrap();

        let record = read_observation(&cache, &sha256, 42)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.schema, TARGET_OBSERVATION_SCHEMA);
        let bytes = fs::read(observation_path(&cache, &sha256)).await.unwrap();
        assert!(!String::from_utf8(bytes)
            .unwrap()
            .contains(temporary.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn observations_reject_unknown_fields_and_identity_drift() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path();
        let sha256 = "b".repeat(64);
        let path = observation_path(cache, &sha256);
        fs::write(
            &path,
            format!(
                "{{\"schema\":\"{TARGET_OBSERVATION_SCHEMA}\",\"targetDigest\":\"sha256:{sha256}\",\"expectedBytes\":42,\"path\":\"secret\"}}"
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_observation(cache, &sha256, 42).await.unwrap_err().code,
            "use.extension.registry_target_cache_invalid"
        );

        fs::write(
            &path,
            format!(
                "{{\"schema\":\"{TARGET_OBSERVATION_SCHEMA}\",\"targetDigest\":\"sha256:{sha256}\",\"expectedBytes\":{}}}",
                MAX_REMOTE_ARCHIVE_BYTES + 1
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_observation_path(&path, &sha256, None)
                .await
                .unwrap_err()
                .code,
            "use.extension.registry_target_cache_invalid"
        );
    }
}
