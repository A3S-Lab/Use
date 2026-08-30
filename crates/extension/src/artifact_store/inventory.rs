use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::quarantine::{
    inspect_container_state, validate_quarantine_metadata, QUARANTINE_RECORD, QUARANTINE_TEMPORARY,
};
use super::quota::{
    validate_policy_metadata, STORAGE_QUOTA_LOCK, STORAGE_QUOTA_POLICY_FILE,
    STORAGE_QUOTA_TEMPORARY_FILE,
};
use super::rehydration::{
    validate_rehydration_metadata, REHYDRATION_PREPARED_RECORD, REHYDRATION_PREPARED_TEMPORARY,
    REHYDRATION_RECORD, REHYDRATION_TEMPORARY,
};
use super::{
    artifact_store_error, validate_lock_metadata, validate_real_directory, validate_sha256,
    ArtifactCollectionGuard, ArtifactStore, ARTIFACT_STAGING_PREFIX, BLOBS_DIRECTORY,
    CONTENT_DIRECTORY, EXPANDED_PACKAGES_DIRECTORY, MAX_ARTIFACT_CONTAINER_ENTRIES,
    MAX_ARTIFACT_TREE_ENTRIES, MUTATION_LOCK, REACHABILITY_LOCK, SHA256_DIRECTORY,
};
use crate::package::{io_error, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};

/// Stable schema for a path-free physical Artifact Store inventory.
pub const ARTIFACT_STORE_INVENTORY_SCHEMA: &str = "a3s.use.artifact-store-inventory.v1";

const MAX_ARTIFACT_SHARDS: usize = 256;
/// Maximum digest containers accepted by one physical Artifact Store scan.
/// Downstream accounting and quota projections use the same bound.
pub const MAX_ARTIFACT_STORE_INVENTORY_ENTRIES: usize = 100_000;
const MAX_ARTIFACT_INVENTORY_TREE_ENTRIES: usize = 1_000_000;

/// Physical content tier in the global Artifact Store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Blob,
    ExpandedPackage,
}

/// Publication state inferred only from the owned physical layout.
///
/// `Complete` means that the canonical `content` object exists with the right
/// filesystem kind. It does not assert that the bytes match the path digest;
/// cryptographic verification belongs to the separate audit stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactPhysicalState {
    Complete,
    Incomplete,
}

/// One path-free physical fact about a content-addressed artifact container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactInventoryEntry {
    pub kind: ArtifactKind,
    pub digest: String,
    pub state: ArtifactPhysicalState,
    pub content_bytes: u64,
    pub content_files: u64,
    pub staging_entries: u64,
    pub staging_bytes: u64,
}

/// Deterministic, path-free physical inventory of the global Artifact Store.
///
/// This is physical evidence only. It deliberately carries neither inferred
/// reachability nor deletion authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactStoreInventory {
    pub schema: String,
    pub entries: Vec<ArtifactInventoryEntry>,
}

impl ArtifactStore {
    /// Inspect every owned physical artifact while global reference admission
    /// is frozen by the exact store-bound collection guard.
    pub async fn inspect_inventory(
        &self,
        collection: &ArtifactCollectionGuard,
    ) -> UseResult<ArtifactStoreInventory> {
        collection.ensure_store(self)?;
        self.scan_inventory_under_global_guard().await
    }

    /// The caller must own either the exclusive reachability collection guard,
    /// or shared reference admission plus the exclusive storage boundary.
    pub(super) async fn scan_inventory_under_global_guard(
        &self,
    ) -> UseResult<ArtifactStoreInventory> {
        validate_real_directory(self.root(), "Artifact Store root").await?;

        let mut budget = InventoryBudget::default();
        let mut entries = Vec::new();
        scan_store_root(self.root(), &mut budget, &mut entries).await?;
        entries.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.digest.cmp(&right.digest))
        });

        Ok(ArtifactStoreInventory {
            schema: ARTIFACT_STORE_INVENTORY_SCHEMA.to_owned(),
            entries,
        })
    }
}

#[derive(Debug, Default)]
struct InventoryBudget {
    artifacts: usize,
    filesystem_entries: usize,
}

impl InventoryBudget {
    fn observe_artifact(&mut self) -> UseResult<()> {
        self.artifacts = self
            .artifacts
            .checked_add(1)
            .ok_or_else(|| inventory_limit("The Artifact Store artifact inventory overflowed."))?;
        if self.artifacts > MAX_ARTIFACT_STORE_INVENTORY_ENTRIES {
            return Err(inventory_limit(
                "The Artifact Store exceeds its bounded artifact inventory.",
            ));
        }
        Ok(())
    }

    fn observe_filesystem_entry(&mut self) -> UseResult<()> {
        self.filesystem_entries = self.filesystem_entries.checked_add(1).ok_or_else(|| {
            inventory_limit("The Artifact Store filesystem inventory overflowed.")
        })?;
        if self.filesystem_entries > MAX_ARTIFACT_INVENTORY_TREE_ENTRIES {
            return Err(inventory_limit(
                "The Artifact Store exceeds its bounded filesystem inventory.",
            ));
        }
        Ok(())
    }
}

async fn scan_store_root(
    root: &Path,
    budget: &mut InventoryBudget,
    inventory: &mut Vec<ArtifactInventoryEntry>,
) -> UseResult<()> {
    let mut directory = fs::read_dir(root)
        .await
        .map_err(|error| io_error("read Artifact Store root", root, error))?;
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|error| io_error("read Artifact Store root entry", root, error))?
    {
        budget.observe_filesystem_entry()?;
        let path = entry.path();
        let name = entry_name(&entry, "Artifact Store root")?;
        let metadata = owned_metadata(&path, "Artifact Store root entry").await?;
        match name.as_str() {
            REACHABILITY_LOCK => {
                validate_lock_metadata(&path, &metadata, "global artifact reachability")?;
            }
            STORAGE_QUOTA_LOCK => {
                validate_lock_metadata(&path, &metadata, "Artifact Store quota")?;
            }
            STORAGE_QUOTA_POLICY_FILE => {
                validate_policy_metadata(&path, &metadata, false)?;
            }
            STORAGE_QUOTA_TEMPORARY_FILE => {
                validate_policy_metadata(&path, &metadata, true)?;
            }
            BLOBS_DIRECTORY => {
                validate_directory_metadata(&path, &metadata, "blob Artifact Store tier")?;
                scan_kind_root(&path, ArtifactKind::Blob, budget, inventory).await?;
            }
            EXPANDED_PACKAGES_DIRECTORY => {
                validate_directory_metadata(
                    &path,
                    &metadata,
                    "expanded-package Artifact Store tier",
                )?;
                scan_kind_root(&path, ArtifactKind::ExpandedPackage, budget, inventory).await?;
            }
            _ => {
                return Err(ownership_error(
                    &path,
                    "The Artifact Store root contains an unowned entry.",
                ));
            }
        }
    }
    Ok(())
}

async fn scan_kind_root(
    root: &Path,
    kind: ArtifactKind,
    budget: &mut InventoryBudget,
    inventory: &mut Vec<ArtifactInventoryEntry>,
) -> UseResult<()> {
    let mut directory = fs::read_dir(root)
        .await
        .map_err(|error| io_error("read Artifact Store tier", root, error))?;
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|error| io_error("read Artifact Store tier entry", root, error))?
    {
        budget.observe_filesystem_entry()?;
        let path = entry.path();
        let name = entry_name(&entry, "Artifact Store tier")?;
        let metadata = owned_metadata(&path, "Artifact Store tier entry").await?;
        if name != SHA256_DIRECTORY {
            return Err(ownership_error(
                &path,
                "An Artifact Store tier contains an unowned digest algorithm.",
            ));
        }
        validate_directory_metadata(&path, &metadata, "Artifact Store SHA-256 tier")?;
        scan_sha256_root(&path, kind, budget, inventory).await?;
    }
    Ok(())
}

async fn scan_sha256_root(
    root: &Path,
    kind: ArtifactKind,
    budget: &mut InventoryBudget,
    inventory: &mut Vec<ArtifactInventoryEntry>,
) -> UseResult<()> {
    let mut directory = fs::read_dir(root)
        .await
        .map_err(|error| io_error("read Artifact Store SHA-256 tier", root, error))?;
    let mut shards = 0_usize;
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|error| io_error("read Artifact Store SHA-256 shard", root, error))?
    {
        budget.observe_filesystem_entry()?;
        shards = shards
            .checked_add(1)
            .ok_or_else(|| inventory_limit("The Artifact Store shard inventory overflowed."))?;
        if shards > MAX_ARTIFACT_SHARDS {
            return Err(inventory_limit(
                "An Artifact Store SHA-256 tier exceeds its bounded shard inventory.",
            ));
        }

        let path = entry.path();
        let shard = entry_name(&entry, "Artifact Store SHA-256 tier")?;
        if !valid_shard(&shard) {
            return Err(ownership_error(
                &path,
                "An Artifact Store SHA-256 tier contains a non-canonical shard.",
            ));
        }
        let metadata = owned_metadata(&path, "Artifact Store SHA-256 shard").await?;
        validate_directory_metadata(&path, &metadata, "Artifact Store SHA-256 shard")?;
        scan_shard(&path, &shard, kind, budget, inventory).await?;
    }
    Ok(())
}

async fn scan_shard(
    root: &Path,
    shard: &str,
    kind: ArtifactKind,
    budget: &mut InventoryBudget,
    inventory: &mut Vec<ArtifactInventoryEntry>,
) -> UseResult<()> {
    let mut directory = fs::read_dir(root)
        .await
        .map_err(|error| io_error("read Artifact Store digest shard", root, error))?;
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|error| io_error("read Artifact Store digest container", root, error))?
    {
        budget.observe_filesystem_entry()?;
        budget.observe_artifact()?;
        let path = entry.path();
        let sha256 = entry_name(&entry, "Artifact Store digest shard")?;
        if !sha256.starts_with(shard) || validate_sha256(&sha256).is_err() {
            return Err(ownership_error(
                &path,
                "An Artifact Store shard contains a non-canonical digest container.",
            ));
        }
        let metadata = owned_metadata(&path, "Artifact Store digest container").await?;
        validate_directory_metadata(&path, &metadata, "Artifact Store digest container")?;
        inventory.push(scan_container(&path, &sha256, kind, budget).await?);
    }
    Ok(())
}

async fn scan_container(
    root: &Path,
    sha256: &str,
    kind: ArtifactKind,
    budget: &mut InventoryBudget,
) -> UseResult<ArtifactInventoryEntry> {
    let mut directory = fs::read_dir(root)
        .await
        .map_err(|error| io_error("read Artifact Store digest container", root, error))?;
    let mut immediate_entries = 0_usize;
    let mut content = None;
    let mut staging_entries = 0_u64;
    let mut staging_bytes = 0_u64;

    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|error| io_error("read Artifact Store digest entry", root, error))?
    {
        budget.observe_filesystem_entry()?;
        immediate_entries = immediate_entries.checked_add(1).ok_or_else(|| {
            inventory_limit("An Artifact Store digest container inventory overflowed.")
        })?;
        if immediate_entries > MAX_ARTIFACT_CONTAINER_ENTRIES {
            return Err(inventory_limit(
                "An Artifact Store digest container exceeds its bounded entry inventory.",
            ));
        }

        let path = entry.path();
        let name = entry_name(&entry, "Artifact Store digest container")?;
        let metadata = owned_metadata(&path, "Artifact Store digest entry").await?;
        match name.as_str() {
            MUTATION_LOCK => {
                validate_lock_metadata(&path, &metadata, "artifact mutation")?;
            }
            QUARANTINE_RECORD => {
                validate_quarantine_metadata(&path, &metadata, false)?;
            }
            QUARANTINE_TEMPORARY => {
                validate_quarantine_metadata(&path, &metadata, true)?;
            }
            REHYDRATION_PREPARED_RECORD | REHYDRATION_RECORD => {
                validate_rehydration_metadata(&path, &metadata, false)?;
            }
            REHYDRATION_PREPARED_TEMPORARY | REHYDRATION_TEMPORARY => {
                validate_rehydration_metadata(&path, &metadata, true)?;
            }
            CONTENT_DIRECTORY => {
                if content.is_some() {
                    return Err(ownership_error(
                        &path,
                        "An Artifact Store digest container has duplicate content.",
                    ));
                }
                content = Some(match kind {
                    ArtifactKind::Blob => {
                        validate_file_metadata(&path, &metadata, "artifact blob content")?;
                        TreeMeasurement {
                            files: 1,
                            bytes: metadata.len(),
                        }
                    }
                    ArtifactKind::ExpandedPackage => {
                        validate_directory_metadata(
                            &path,
                            &metadata,
                            "expanded-package artifact content",
                        )?;
                        measure_owned_tree(&path, budget).await?
                    }
                });
            }
            _ if name.starts_with(ARTIFACT_STAGING_PREFIX) => {
                staging_entries = staging_entries.checked_add(1).ok_or_else(|| {
                    inventory_limit("An Artifact Store staging inventory overflowed.")
                })?;
                let measurement = match kind {
                    ArtifactKind::Blob => {
                        validate_file_metadata(&path, &metadata, "artifact blob staging")?;
                        TreeMeasurement {
                            files: 1,
                            bytes: metadata.len(),
                        }
                    }
                    ArtifactKind::ExpandedPackage => {
                        validate_directory_metadata(
                            &path,
                            &metadata,
                            "expanded-package artifact staging",
                        )?;
                        measure_owned_tree(&path, budget).await?
                    }
                };
                staging_bytes = staging_bytes
                    .checked_add(measurement.bytes)
                    .ok_or_else(|| {
                        inventory_limit("An Artifact Store staging byte inventory overflowed.")
                    })?;
            }
            _ => {
                return Err(ownership_error(
                    &path,
                    "An Artifact Store digest container contains an unowned entry.",
                ));
            }
        }
    }

    let digest = format!("sha256:{sha256}");
    let quarantine = inspect_container_state(root, kind, &digest).await?;
    let rehydration = super::rehydration::inspect_rehydration_state(root).await?;
    super::rehydration::validate_container_rehydration_state(
        kind,
        &digest,
        &quarantine,
        &rehydration,
    )?;

    let (state, content_files, content_bytes) = match content {
        Some(measurement) => (
            ArtifactPhysicalState::Complete,
            measurement.files,
            measurement.bytes,
        ),
        None => (ArtifactPhysicalState::Incomplete, 0, 0),
    };
    Ok(ArtifactInventoryEntry {
        kind,
        digest: format!("sha256:{sha256}"),
        state,
        content_bytes,
        content_files,
        staging_entries,
        staging_bytes,
    })
}

#[derive(Debug, Clone, Copy)]
struct TreeMeasurement {
    files: u64,
    bytes: u64,
}

async fn measure_owned_tree(
    root: &Path,
    budget: &mut InventoryBudget,
) -> UseResult<TreeMeasurement> {
    let mut pending = vec![PathBuf::from(root)];
    let mut tree_entries = 0_usize;
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    while let Some(directory_path) = pending.pop() {
        let mut directory = fs::read_dir(&directory_path).await.map_err(|error| {
            io_error("read Artifact Store content tree", &directory_path, error)
        })?;
        while let Some(entry) = directory.next_entry().await.map_err(|error| {
            io_error(
                "read Artifact Store content tree entry",
                &directory_path,
                error,
            )
        })? {
            budget.observe_filesystem_entry()?;
            tree_entries = tree_entries.checked_add(1).ok_or_else(|| {
                inventory_limit("An Artifact Store content tree inventory overflowed.")
            })?;
            if tree_entries > MAX_ARTIFACT_TREE_ENTRIES {
                return Err(inventory_limit(
                    "An Artifact Store content tree exceeds its bounded entry inventory.",
                ));
            }

            let path = entry.path();
            let metadata = owned_metadata(&path, "Artifact Store content tree entry").await?;
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files = files.checked_add(1).ok_or_else(|| {
                    inventory_limit("An Artifact Store content file inventory overflowed.")
                })?;
                bytes = bytes.checked_add(metadata.len()).ok_or_else(|| {
                    inventory_limit("An Artifact Store content byte inventory overflowed.")
                })?;
                if files > MAX_PACKAGE_FILES as u64 || bytes > MAX_PACKAGE_BYTES {
                    return Err(inventory_limit(
                        "An Artifact Store content tree exceeds package limits.",
                    ));
                }
            } else {
                return Err(ownership_error(
                    &path,
                    "An Artifact Store content tree contains a special file.",
                ));
            }
        }
    }
    Ok(TreeMeasurement { files, bytes })
}

async fn owned_metadata(path: &Path, label: &str) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
        return Err(ownership_error(
            path,
            &format!("The {label} is a link or reparse point."),
        ));
    }
    Ok(metadata)
}

fn validate_directory_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    label: &str,
) -> UseResult<()> {
    if !metadata.is_dir() {
        return Err(ownership_error(
            path,
            &format!("The {label} must be an owned directory."),
        ));
    }
    Ok(())
}

fn validate_file_metadata(path: &Path, metadata: &std::fs::Metadata, label: &str) -> UseResult<()> {
    if !metadata.is_file() {
        return Err(ownership_error(
            path,
            &format!("The {label} must be an owned regular file."),
        ));
    }
    Ok(())
}

fn entry_name(entry: &fs::DirEntry, label: &str) -> UseResult<String> {
    entry.file_name().into_string().map_err(|_| {
        ownership_error(
            &entry.path(),
            &format!("The {label} contains a non-UTF-8 entry name."),
        )
    })
}

fn valid_shard(value: &str) -> bool {
    value.len() == 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn ownership_error(path: &Path, message: &str) -> a3s_use_core::UseError {
    artifact_store_error("use.artifact_store.ownership_invalid", message)
        .with_detail("path", path.display().to_string())
}

fn inventory_limit(message: &str) -> a3s_use_core::UseError {
    artifact_store_error("use.artifact_store.inventory_limit_exceeded", message)
}
