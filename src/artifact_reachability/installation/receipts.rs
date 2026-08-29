use std::fs::File as StdFile;
use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_extension::{
    ArtifactKind, ArtifactStore, ExtensionReceipt, MAX_EXTENSION_RECEIPT_BYTES,
};
use tokio::fs;

use super::io::{
    entry_name, read_bounded_json, require_bounded_file, require_owned_directory,
    require_owned_file,
};
use super::{
    checked_count, inventory_invalid, valid_temporary_name, InstallationLocation, InventoryBudget,
    SourceFacts,
};
use crate::artifact_reachability::{ArtifactReferenceSource, RawArtifactReference};

const MAX_RECEIPT_PUBLISHERS: usize = 1_024;
const MAX_CURRENT_RECEIPTS: usize = 8_192;
const MAX_RETAINED_PACKAGE_DIRECTORIES: usize = 8_192;
const MAX_RETAINED_RECEIPTS_PER_PACKAGE: usize = 64;

pub(super) async fn acquire_existing_registry_lock_shared(state_root: &Path) -> UseResult<StdFile> {
    let path = state_root.join("extensions").join(".registry.lock");
    super::io::acquire_existing_owned_lock_shared(&path, "extension Registry lock").await
}

pub(super) async fn scan_current(
    root: &Path,
    location: &InstallationLocation,
    artifact_store: &ArtifactStore,
    budget: &mut InventoryBudget,
) -> UseResult<SourceFacts> {
    let mut facts = SourceFacts::default();
    let mut publisher_count = 0_usize;
    let mut receipt_count = 0_usize;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read current receipt root", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read current receipt entry", root, error))?
    {
        budget.observe_entry()?;
        let name = entry_name(&entry, "current receipt root")?;
        let path = entry.path();
        if name == ".registry.lock" {
            require_owned_file(&path, 4 * 1024, "extension Registry lock").await?;
            continue;
        }
        publisher_count = checked_count(
            publisher_count,
            MAX_RECEIPT_PUBLISHERS,
            "The current receipt inventory exceeds its publisher bound.",
        )?;
        require_owned_directory(&path, "current receipt publisher directory").await?;
        scan_current_publisher(
            &path,
            &name,
            location,
            artifact_store,
            budget,
            &mut receipt_count,
            &mut facts,
        )
        .await?;
    }
    Ok(facts)
}

#[allow(clippy::too_many_arguments)]
async fn scan_current_publisher(
    root: &Path,
    publisher: &str,
    location: &InstallationLocation,
    artifact_store: &ArtifactStore,
    budget: &mut InventoryBudget,
    receipt_count: &mut usize,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    super::validate_package_id(publisher, "package")?;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read current receipt publisher", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read current receipt file", root, error))?
    {
        budget.observe_entry()?;
        let name = entry_name(&entry, "current receipt publisher directory")?;
        let path = entry.path();
        if valid_temporary_name(&name, ".receipt-") {
            require_owned_file(
                &path,
                MAX_EXTENSION_RECEIPT_BYTES,
                "temporary extension receipt",
            )
            .await?;
            continue;
        }
        *receipt_count = checked_count(
            *receipt_count,
            MAX_CURRENT_RECEIPTS,
            "The current receipt inventory exceeds its package bound.",
        )?;
        let package = name.strip_suffix(".json").ok_or_else(|| {
            inventory_invalid("A current extension receipt has an unknown file name.")
        })?;
        let package_id = super::validate_package_id(publisher, package)?;
        let receipt = read_bounded_json::<ExtensionReceipt>(
            &path,
            MAX_EXTENSION_RECEIPT_BYTES,
            "current extension receipt",
        )
        .await?;
        append_receipt_reference(
            receipt,
            &package_id,
            None,
            location,
            artifact_store,
            ArtifactReferenceSource::CurrentReceipt,
            facts,
        )?;
    }
    Ok(())
}

pub(super) async fn scan_retained(
    root: &Path,
    location: &InstallationLocation,
    artifact_store: &ArtifactStore,
    budget: &mut InventoryBudget,
) -> UseResult<SourceFacts> {
    let mut facts = SourceFacts::default();
    let mut publisher_count = 0_usize;
    let mut package_count = 0_usize;
    let mut publishers = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read retained receipt root", root, error))?;
    while let Some(publisher) = publishers
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read retained receipt publisher", root, error))?
    {
        budget.observe_entry()?;
        publisher_count = checked_count(
            publisher_count,
            MAX_RECEIPT_PUBLISHERS,
            "The retained receipt inventory exceeds its publisher bound.",
        )?;
        let publisher_name = entry_name(&publisher, "retained receipt root")?;
        super::validate_package_id(&publisher_name, "package")?;
        let publisher_root = publisher.path();
        require_owned_directory(&publisher_root, "retained receipt publisher directory").await?;
        let mut packages = fs::read_dir(&publisher_root).await.map_err(|error| {
            super::inventory_io("read retained receipt package root", &publisher_root, error)
        })?;
        while let Some(package) = packages.next_entry().await.map_err(|error| {
            super::inventory_io("read retained receipt package", &publisher_root, error)
        })? {
            budget.observe_entry()?;
            package_count = checked_count(
                package_count,
                MAX_RETAINED_PACKAGE_DIRECTORIES,
                "The retained receipt inventory exceeds its package bound.",
            )?;
            let package_name = entry_name(&package, "retained receipt publisher directory")?;
            let package_id = super::validate_package_id(&publisher_name, &package_name)?;
            let package_root = package.path();
            require_owned_directory(&package_root, "retained receipt package directory").await?;
            scan_retained_package(
                &package_root,
                &package_id,
                location,
                artifact_store,
                budget,
                &mut facts,
            )
            .await?;
        }
    }
    Ok(facts)
}

async fn scan_retained_package(
    root: &Path,
    package_id: &str,
    location: &InstallationLocation,
    artifact_store: &ArtifactStore,
    budget: &mut InventoryBudget,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    let mut receipt_count = 0_usize;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read retained receipt package", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read retained receipt file", root, error))?
    {
        budget.observe_entry()?;
        let name = entry_name(&entry, "retained receipt package directory")?;
        let path = entry.path();
        if valid_temporary_name(&name, ".receipt-") {
            require_owned_file(
                &path,
                MAX_EXTENSION_RECEIPT_BYTES,
                "temporary retained receipt",
            )
            .await?;
            continue;
        }
        receipt_count = checked_count(
            receipt_count,
            MAX_RETAINED_RECEIPTS_PER_PACKAGE,
            "A package exceeds its retained receipt bound.",
        )?;
        let (generation, digest) = retained_file_identity(&name)?;
        require_bounded_file(
            &path,
            MAX_EXTENSION_RECEIPT_BYTES,
            "retained extension receipt",
        )
        .await?;
        let receipt = read_bounded_json::<ExtensionReceipt>(
            &path,
            MAX_EXTENSION_RECEIPT_BYTES,
            "retained extension receipt",
        )
        .await?;
        append_receipt_reference(
            receipt,
            package_id,
            Some((generation, digest.as_str())),
            location,
            artifact_store,
            ArtifactReferenceSource::RetainedReceipt,
            facts,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_receipt_reference(
    receipt: ExtensionReceipt,
    expected_package_id: &str,
    retained_identity: Option<(u64, &str)>,
    location: &InstallationLocation,
    artifact_store: &ArtifactStore,
    source: ArtifactReferenceSource,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    location.validate_identity(&receipt.installation)?;
    if receipt.package_id != expected_package_id {
        return Err(inventory_invalid(
            "An extension receipt does not match its package path.",
        ));
    }
    let reference = receipt.artifact_reference(artifact_store)?;
    if retained_identity.is_some_and(|(generation, digest)| {
        receipt.lifecycle_generation != Some(generation)
            || receipt.package_sha256.as_deref() != Some(digest)
    }) {
        return Err(inventory_invalid(
            "A retained extension receipt does not match its generation and digest path.",
        ));
    }
    facts.observe_identity(receipt.installation.clone())?;
    facts.references.push(RawArtifactReference {
        kind: ArtifactKind::ExpandedPackage,
        digest: reference.digest,
        source,
        installation: Some(receipt.installation),
        expected_bytes: reference.expected_bytes,
        expected_files: reference.expected_files,
    });
    Ok(())
}

fn retained_file_identity(name: &str) -> UseResult<(u64, String)> {
    let stem = name.strip_suffix(".json").ok_or_else(|| {
        inventory_invalid("A retained extension receipt has an unknown file name.")
    })?;
    let (generation, digest) = stem.split_once('-').ok_or_else(|| {
        inventory_invalid("A retained extension receipt has an invalid identity.")
    })?;
    let generation_value = generation.parse::<u64>().map_err(|_| {
        inventory_invalid("A retained extension receipt has an invalid generation.")
    })?;
    if generation.len() != 20
        || generation_value == 0
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(inventory_invalid(
            "A retained extension receipt has a non-canonical generation or digest.",
        ));
    }
    Ok((generation_value, digest.to_owned()))
}
