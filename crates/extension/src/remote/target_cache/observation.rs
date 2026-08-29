use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::{record, target_cache_error, validated_evidence};
use crate::ArtifactStore;

/// Path-free observation of one exact digest-bound Registry target.
///
/// `Complete` means a canonical source observation references an exact-length
/// owned global blob. Observation does not rehash the blob and is never
/// download, apply, or recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedTargetObservation {
    pub registry_name: String,
    pub target_digest: String,
    pub expected_bytes: u64,
    pub retained_bytes: u64,
    pub status: VerifiedTargetObservationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifiedTargetObservationStatus {
    Missing,
    Partial,
    Complete,
}

pub(in crate::remote) async fn observe_target_cache_entry(
    datastore: &Path,
    artifact_store: &ArtifactStore,
    registry_name: &str,
    expected_length: u64,
    expected_sha256: &str,
) -> UseResult<VerifiedTargetObservation> {
    super::super::validate_registry_name(registry_name)?;
    if !datastore.is_absolute() {
        return Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            "The observed Registry target cache path must be absolute.",
        ));
    }
    let digest = validated_evidence(expected_length, expected_sha256)?;
    let missing = || VerifiedTargetObservation {
        registry_name: registry_name.to_owned(),
        target_digest: format!("sha256:{digest}"),
        expected_bytes: expected_length,
        retained_bytes: 0,
        status: VerifiedTargetObservationStatus::Missing,
    };
    let Some(cache) = optional_cache_directory(datastore).await? else {
        return Ok(missing());
    };
    let target_path = record::observation_path(&cache, &digest);
    let partial_path = cache.join(format!(".target-{digest}.part"));
    let target = optional_metadata(&target_path).await?;
    let partial = optional_metadata(&partial_path).await?;
    if target.is_some() {
        record::read_observation(&cache, &digest, expected_length)
            .await?
            .ok_or_else(|| {
                target_cache_error(
                    "use.extension.registry_target_cache_invalid",
                    "The Registry target observation disappeared during inspection.",
                )
            })?;
        if !artifact_store
            .observe_blob(&digest, expected_length)
            .await?
        {
            return Err(target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "A Registry target observation references a missing global artifact blob.",
            ));
        }
        if let Some(metadata) = partial {
            validate_partial_observation(&metadata, expected_length)?;
        }
        return Ok(VerifiedTargetObservation {
            retained_bytes: expected_length,
            status: VerifiedTargetObservationStatus::Complete,
            ..missing()
        });
    }
    if let Some(metadata) = partial {
        validate_partial_observation(&metadata, expected_length)?;
        return Ok(VerifiedTargetObservation {
            retained_bytes: metadata.len(),
            status: VerifiedTargetObservationStatus::Partial,
            ..missing()
        });
    }
    Ok(missing())
}

fn validate_partial_observation(
    metadata: &std::fs::Metadata,
    expected_length: u64,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() > expected_length
    {
        return Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            "The observed partial Registry target is not a bounded regular file.",
        ));
    }
    Ok(())
}

async fn optional_cache_directory(datastore: &Path) -> UseResult<Option<PathBuf>> {
    let Some(()) = optional_real_directory(datastore, "Registry datastore").await? else {
        return Ok(None);
    };
    let targets = datastore.join("verified-targets");
    let Some(()) = optional_real_directory(&targets, "verified target cache").await? else {
        return Ok(None);
    };
    let cache = targets.join("sha256");
    let Some(()) = optional_real_directory(&cache, "SHA-256 target cache").await? else {
        return Ok(None);
    };
    Ok(Some(cache))
}

async fn optional_real_directory(path: &Path, label: &str) -> UseResult<Option<()>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(observation_io_error(label, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            format!("The observed {label} must be a real directory."),
        ));
    }
    Ok(Some(()))
}

async fn optional_metadata(path: &Path) -> UseResult<Option<std::fs::Metadata>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(observation_io_error("Registry target cache entry", error)),
    }
}

fn observation_io_error(label: &str, error: std::io::Error) -> UseError {
    target_cache_error(
        "use.extension.registry_target_cache_invalid",
        format!("Failed to inspect the observed {label}: {error}"),
    )
}
