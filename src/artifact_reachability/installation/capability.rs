//! Fail-closed inspection of Capability Gateway payload-owner state.
//!
//! Capability catalogs and descriptor snapshots do not currently contribute
//! Artifact Store digests: their artifact references are opaque projections
//! and lifecycle receipts remain the authority for garbage-collection facts.
//! They still belong to the installation state boundary, however.  Ignoring
//! malformed nested entries would let a substituted payload survive an
//! otherwise successful reachability audit, so this scanner validates the
//! complete owner layout and every immutable record.

use std::path::Path;

use a3s_use_core::{CapabilityGatewayCatalog, UseError, UseResult};
use tokio::fs;

use super::io::{
    entry_name, owned_metadata, read_bounded_bytes, require_owned_directory, require_owned_file,
};
use super::{checked_count, inventory_invalid, InstallationLocation, InventoryBudget, SourceFacts};
use crate::state_backup::capability_payload;

const ROOT: &str = "capability-gateway";
const CATALOGS: &str = "catalogs";
const DESCRIPTOR_SNAPSHOTS: &str = "descriptor-snapshots";
const SHA256: &str = "sha256";
const STAGING: &str = ".staging";
const MUTATION_LOCK: &str = ".mutation.lock";
const RETENTION_JOURNAL: &str = ".retention.journal";
const MAX_ROOT_ENTRIES: usize = 8;
const MAX_SHARDS: usize = 256;
const MAX_RECORDS: usize = 4_096;
const MAX_LOCK_BYTES: u64 = 4 * 1024;

pub(super) async fn scan(
    root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
) -> UseResult<SourceFacts> {
    require_owned_directory(root, "Capability Gateway state root").await?;
    validate_layout(ROOT, true)?;

    let mut facts = SourceFacts::default();
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read Capability Gateway state root", root, error))?;
    let mut count = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read Capability Gateway state entry", root, error))?
    {
        budget.observe_entry()?;
        count = checked_count(
            count,
            MAX_ROOT_ENTRIES,
            "The Capability Gateway state root exceeds its entry bound.",
        )?;
        let name = entry_name(&entry, "Capability Gateway state root")?;
        let path = entry.path();
        let metadata = owned_metadata(&path, "Capability Gateway state entry").await?;
        let portable = format!("{ROOT}/{name}");
        validate_layout(&portable, metadata.is_dir())?;
        match name.as_str() {
            CATALOGS if metadata.is_dir() => {
                scan_catalogs(&path, location, budget, &mut facts).await?;
            }
            DESCRIPTOR_SNAPSHOTS if metadata.is_dir() => {
                scan_descriptor_snapshots(&path, location, budget, &mut facts).await?;
            }
            _ => {
                return Err(inventory_invalid(
                    "The Capability Gateway state root contains an unknown entry.",
                ));
            }
        }
    }
    Ok(facts)
}

async fn scan_catalogs(
    root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    require_owned_directory(root, "Capability Gateway catalog root").await?;
    let mut entries = fs::read_dir(root).await.map_err(|error| {
        super::inventory_io("read Capability Gateway catalog root", root, error)
    })?;
    let mut count = 0_usize;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        super::inventory_io("read Capability Gateway catalog entry", root, error)
    })? {
        budget.observe_entry()?;
        count = checked_count(
            count,
            MAX_ROOT_ENTRIES,
            "The Capability Gateway catalog root exceeds its entry bound.",
        )?;
        let name = entry_name(&entry, "Capability Gateway catalog root")?;
        let path = entry.path();
        let metadata = owned_metadata(&path, "Capability Gateway catalog entry").await?;
        let portable = format!("{ROOT}/{CATALOGS}/{name}");
        validate_layout(&portable, metadata.is_dir())?;
        match name.as_str() {
            SHA256 if metadata.is_dir() => {
                scan_catalog_shards(&path, location, budget, facts).await?;
            }
            STAGING if metadata.is_dir() => {
                reject_staging(&path, &portable, budget, "catalog").await?;
            }
            MUTATION_LOCK if metadata.is_file() => {
                require_owned_file(&path, MAX_LOCK_BYTES, "Capability Gateway catalog lock")
                    .await?;
            }
            RETENTION_JOURNAL if metadata.is_file() => {
                return Err(inventory_invalid(
                    "Capability Gateway catalog retention is not quiescent.",
                ));
            }
            _ => {
                return Err(inventory_invalid(
                    "The Capability Gateway catalog root contains an unknown entry.",
                ));
            }
        }
    }
    Ok(())
}

async fn scan_catalog_shards(
    root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    require_owned_directory(root, "Capability Gateway catalog shard root").await?;
    let mut entries = fs::read_dir(root).await.map_err(|error| {
        super::inventory_io("read Capability Gateway catalog shards", root, error)
    })?;
    let mut shard_count = 0_usize;
    let mut record_count = 0_usize;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        super::inventory_io("read Capability Gateway catalog shard", root, error)
    })? {
        budget.observe_entry()?;
        shard_count = checked_count(
            shard_count,
            MAX_SHARDS,
            "The Capability Gateway catalog exceeds its shard bound.",
        )?;
        let name = entry_name(&entry, "Capability Gateway catalog shard root")?;
        let path = entry.path();
        let metadata = owned_metadata(&path, "Capability Gateway catalog shard").await?;
        let portable = format!("{ROOT}/{CATALOGS}/{SHA256}/{name}");
        validate_layout(&portable, metadata.is_dir())?;
        if !metadata.is_dir() || !valid_hex(&name, 2) {
            return Err(inventory_invalid(
                "A Capability Gateway catalog shard is not a canonical directory.",
            ));
        }
        scan_catalog_shard(&path, &name, location, budget, &mut record_count, facts).await?;
    }
    Ok(())
}

async fn scan_catalog_shard(
    root: &Path,
    shard: &str,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
    record_count: &mut usize,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    let mut entries = fs::read_dir(root).await.map_err(|error| {
        super::inventory_io("read Capability Gateway catalog records", root, error)
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        super::inventory_io("read Capability Gateway catalog record", root, error)
    })? {
        budget.observe_entry()?;
        *record_count = checked_count(
            *record_count,
            MAX_RECORDS,
            "The Capability Gateway catalog exceeds its record bound.",
        )?;
        let name = entry_name(&entry, "Capability Gateway catalog shard")?;
        let path = entry.path();
        let metadata = owned_metadata(&path, "Capability Gateway catalog record").await?;
        let portable = format!("{ROOT}/{CATALOGS}/{SHA256}/{shard}/{name}");
        validate_layout(&portable, metadata.is_dir())?;
        if metadata.is_dir() || !valid_record_name(&name) || !name.starts_with(shard) {
            return Err(inventory_invalid(
                "A Capability Gateway catalog record has a non-canonical filename.",
            ));
        }
        let bytes = read_bounded_bytes(
            &path,
            capability_payload::MAX_RECORD_BYTES as u64,
            "Capability Gateway catalog record",
        )
        .await?;
        let catalog = CapabilityGatewayCatalog::from_json(&bytes).map_err(|_| {
            inventory_invalid("A Capability Gateway catalog record contains invalid JSON.")
        })?;
        let installation = catalog.installation().clone();
        location.validate_identity(&installation)?;
        validate_payload_bytes(&portable, &bytes, &installation)?;
        facts.observe_identity(installation)?;
    }
    Ok(())
}

async fn scan_descriptor_snapshots(
    root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    require_owned_directory(root, "Capability Gateway descriptor snapshot root").await?;
    let mut entries = fs::read_dir(root).await.map_err(|error| {
        super::inventory_io(
            "read Capability Gateway descriptor snapshot root",
            root,
            error,
        )
    })?;
    let mut count = 0_usize;
    let mut record_count = 0_usize;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        super::inventory_io(
            "read Capability Gateway descriptor snapshot entry",
            root,
            error,
        )
    })? {
        budget.observe_entry()?;
        count = checked_count(
            count,
            MAX_ROOT_ENTRIES,
            "The Capability Gateway descriptor snapshot root exceeds its entry bound.",
        )?;
        let name = entry_name(&entry, "Capability Gateway descriptor snapshot root")?;
        let path = entry.path();
        let metadata =
            owned_metadata(&path, "Capability Gateway descriptor snapshot entry").await?;
        let portable = format!("{ROOT}/{DESCRIPTOR_SNAPSHOTS}/{name}");
        validate_layout(&portable, metadata.is_dir())?;
        match name.as_str() {
            STAGING if metadata.is_dir() => {
                reject_staging(&path, &portable, budget, "descriptor snapshot").await?;
            }
            MUTATION_LOCK if metadata.is_file() => {
                require_owned_file(
                    &path,
                    MAX_LOCK_BYTES,
                    "Capability Gateway descriptor snapshot lock",
                )
                .await?;
            }
            _ if !metadata.is_dir() => {
                record_count = checked_count(
                    record_count,
                    MAX_RECORDS,
                    "The Capability Gateway descriptor snapshot root exceeds its record bound.",
                )?;
                if !valid_record_name(&name) {
                    return Err(inventory_invalid(
                        "A Capability Gateway descriptor snapshot has a non-canonical filename.",
                    ));
                }
                let bytes = read_bounded_bytes(
                    &path,
                    capability_payload::MAX_RECORD_BYTES as u64,
                    "Capability Gateway descriptor snapshot",
                )
                .await?;
                let installation =
                    crate::control_store::capability_descriptor_snapshot_backup_installation(
                        &bytes,
                    )
                    .map_err(map_payload_error)?;
                location.validate_identity(&installation)?;
                validate_payload_bytes(&portable, &bytes, &installation)?;
                facts.observe_identity(installation)?;
            }
            _ => {
                return Err(inventory_invalid(
                    "The Capability Gateway descriptor snapshot root contains an unknown entry.",
                ));
            }
        }
    }
    Ok(())
}

async fn reject_staging(
    root: &Path,
    portable: &str,
    budget: &mut InventoryBudget,
    owner: &str,
) -> UseResult<()> {
    require_owned_directory(root, "Capability Gateway staging directory").await?;
    let mut entries = fs::read_dir(root).await.map_err(|error| {
        super::inventory_io("read Capability Gateway staging directory", root, error)
    })?;
    if let Some(entry) = entries.next_entry().await.map_err(|error| {
        super::inventory_io("read Capability Gateway staging entry", root, error)
    })? {
        budget.observe_entry()?;
        let path = entry.path();
        // Inspect the entry before reporting it so links and special files are
        // never silently classified as ordinary in-flight evidence.
        owned_metadata(&path, "Capability Gateway staging entry").await?;
        let name = entry_name(&entry, "Capability Gateway staging directory")?;
        let child = format!("{portable}/{name}");
        validate_layout(&child, false)?;
        return Err(inventory_invalid(format!(
            "A Capability Gateway {owner} publication is still staged."
        )));
    }
    Ok(())
}

fn validate_layout(path: &str, directory: bool) -> UseResult<()> {
    capability_payload::validate_layout(path, directory).map_err(map_payload_error)
}

fn validate_payload_bytes(
    path: &str,
    bytes: &[u8],
    installation: &a3s_use_core::InstallationId,
) -> UseResult<()> {
    capability_payload::validate_bytes(path, bytes, installation).map_err(map_payload_error)
}

fn map_payload_error(error: UseError) -> UseError {
    inventory_invalid(format!(
        "Capability Gateway payload validation failed: {}",
        error.message
    ))
}

fn valid_record_name(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(|hex| valid_hex(hex, 64))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
