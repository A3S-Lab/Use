use std::path::{Path, PathBuf};

use a3s_use_core::{PluginPlanningBundle, UseError, UseResult, VerifiedPluginCatalogRecord};
use tempfile::TempDir;
use tokio::fs;
use tough::{Prefix, Repository, TargetName};

use crate::package::io_error;

use super::target_cache::{persist_verified_target, stage_cached_target};
use super::{hex_lower, RemoteRegistryAccess, ResolvedRemotePackage, REGISTRY_METADATA_KEY};

/// Verified repository state retained until its exact target is downloaded.
pub struct PreparedRemotePackage {
    repository: Repository,
    target_name: TargetName,
    resolved: ResolvedRemotePackage,
    verified_catalog: VerifiedPluginCatalogRecord,
    datastore: PathBuf,
    access: RemoteRegistryAccess,
}

impl std::fmt::Debug for PreparedRemotePackage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRemotePackage")
            .field("resolved", &self.resolved)
            .finish_non_exhaustive()
    }
}

impl PreparedRemotePackage {
    pub(super) fn new(
        repository: Repository,
        target_name: TargetName,
        resolved: ResolvedRemotePackage,
        verified_catalog: VerifiedPluginCatalogRecord,
        datastore: PathBuf,
        access: RemoteRegistryAccess,
    ) -> Self {
        Self {
            repository,
            target_name,
            resolved,
            verified_catalog,
            datastore,
            access,
        }
    }

    pub fn resolved(&self) -> &ResolvedRemotePackage {
        &self.resolved
    }

    pub fn verified_catalog(&self) -> &VerifiedPluginCatalogRecord {
        &self.verified_catalog
    }

    /// Download and verify only the small executable planning target.
    ///
    /// Static schema-v3 packages have no planning target and return `None`.
    /// A package with executable surfaces resolves one exact separately signed
    /// TUF target.
    pub async fn load_planning_bundle(&self) -> UseResult<Option<PluginPlanningBundle>> {
        let catalog = &self.verified_catalog;
        let Some(expected) = catalog.record.planning.as_ref() else {
            return Ok(None);
        };
        let target_name = TargetName::new(expected.target_name.clone()).map_err(|_| {
            planning_target_error("The catalog-v3 planning target name is invalid.")
        })?;
        if target_name.raw() != target_name.resolved() {
            return Err(planning_target_error(
                "The catalog-v3 planning target path is not portable.",
            ));
        }
        let target = self
            .repository
            .all_targets()
            .find(|(name, _)| *name == &target_name)
            .map(|(_, target)| target)
            .ok_or_else(|| {
                planning_target_error(
                    "The catalog-v3 planning target is absent from signed TUF metadata.",
                )
            })?;
        let signed_digest = format!("sha256:{}", hex_lower(target.hashes.sha256.as_ref()));
        if target.length != expected.length
            || signed_digest != expected.sha256
            || target.custom.contains_key(REGISTRY_METADATA_KEY)
        {
            return Err(planning_target_error(
                "The signed TUF planning target does not match the catalog evidence.",
            ));
        }

        let digest = expected.sha256.trim_start_matches("sha256:");
        let (temporary, path) = match self.access {
            RemoteRegistryAccess::Refreshed => {
                download_and_cache_target(
                    &self.repository,
                    &target_name,
                    &self.datastore,
                    expected.length,
                    digest,
                    true,
                )
                .await?
            }
            RemoteRegistryAccess::Cached => {
                stage_cached_target(&self.datastore, "planning-v1.json", expected.length, digest)
                    .await?
            }
        };
        let bytes = fs::read(&path)
            .await
            .map_err(|error| io_error("read staged plugin planning target", &path, error))?;
        drop(temporary);
        PluginPlanningBundle::from_catalog_target(&bytes, catalog)
            .map(Some)
            .map_err(|error| {
                planning_target_error(format!(
                    "The signed plugin planning bundle is invalid: {}",
                    error.message
                ))
            })
    }

    pub async fn download(self) -> UseResult<DownloadedRemotePackage> {
        let planning_bundle = self.load_planning_bundle().await?;
        let (temporary, path) = match self.access {
            RemoteRegistryAccess::Refreshed => {
                download_and_cache_target(
                    &self.repository,
                    &self.target_name,
                    &self.datastore,
                    self.resolved.length,
                    &self.resolved.sha256,
                    false,
                )
                .await?
            }
            RemoteRegistryAccess::Cached => {
                stage_cached_target(
                    &self.datastore,
                    &self.resolved.archive_name,
                    self.resolved.length,
                    &self.resolved.sha256,
                )
                .await?
            }
        };
        Ok(DownloadedRemotePackage {
            path,
            resolved: self.resolved,
            verified_catalog: self.verified_catalog,
            planning_bundle,
            _temporary: temporary,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_and_cache_target(
    repository: &Repository,
    target_name: &TargetName,
    datastore: &Path,
    expected_length: u64,
    expected_sha256: &str,
    planning: bool,
) -> UseResult<(TempDir, PathBuf)> {
    let temporary = tokio::task::spawn_blocking(tempfile::tempdir)
        .await
        .map_err(|error| {
            target_download_error(
                planning,
                format!("Failed to create the target staging task: {error}"),
            )
        })?
        .map_err(|error| {
            target_download_error(
                planning,
                format!("Failed to create target staging: {error}"),
            )
        })?;
    repository
        .save_target(target_name, temporary.path(), Prefix::None)
        .await
        .map_err(|error| {
            target_download_error(
                planning,
                format!(
                    "Failed to download and verify TUF target '{}': {error}",
                    target_name.raw()
                ),
            )
        })?;
    let path = temporary.path().join(target_name.resolved());
    persist_verified_target(datastore, &path, expected_length, expected_sha256).await?;
    Ok((temporary, path))
}

fn target_download_error(planning: bool, message: impl Into<String>) -> UseError {
    if planning {
        planning_target_error(message)
    } else {
        UseError::new("use.extension.registry_download_failed", message)
    }
}

fn planning_target_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_planning_target_invalid", message)
}

/// One downloaded archive kept alive through extension activation.
#[derive(Debug)]
pub struct DownloadedRemotePackage {
    path: PathBuf,
    resolved: ResolvedRemotePackage,
    verified_catalog: VerifiedPluginCatalogRecord,
    planning_bundle: Option<PluginPlanningBundle>,
    _temporary: TempDir,
}

impl DownloadedRemotePackage {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn resolved(&self) -> &ResolvedRemotePackage {
        &self.resolved
    }

    pub fn verified_catalog(&self) -> &VerifiedPluginCatalogRecord {
        &self.verified_catalog
    }

    pub fn planning_bundle(&self) -> Option<&PluginPlanningBundle> {
        self.planning_bundle.as_ref()
    }
}
