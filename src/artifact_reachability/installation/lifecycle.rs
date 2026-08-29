use std::fs::File as StdFile;
use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_extension::ArtifactKind;
use tokio::fs;

use super::io::{entry_name, read_bounded_json, require_owned_directory, require_owned_file};
use super::{
    checked_count, inventory_invalid, valid_temporary_name, InstallationLocation, InventoryBudget,
    SourceFacts,
};
use crate::artifact_reachability::{ArtifactReferenceSource, RawArtifactReference};
use crate::plugin_lifecycle::{PluginLifecycleOperationRecord, PluginLifecycleOperationStatus};

const MAX_LIFECYCLE_OPERATION_BYTES: u64 = 1024 * 1024;
const MAX_LIFECYCLE_PUBLISHERS: usize = 1_024;
const MAX_LIFECYCLE_PACKAGES: usize = 8_192;

pub(super) async fn scan(
    root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
) -> UseResult<SourceFacts> {
    let mut kind_entries = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read lifecycle operation root", root, error))?;
    let kind_entry = kind_entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read lifecycle scope kind", root, error))?
        .ok_or_else(|| {
            inventory_invalid("A lifecycle operation root omits its installation kind.")
        })?;
    budget.observe_entry()?;
    if entry_name(&kind_entry, "lifecycle operation root")? != location.kind.as_str() {
        return Err(inventory_invalid(
            "A lifecycle operation root belongs to another installation kind.",
        ));
    }
    let kind_root = kind_entry.path();
    require_owned_directory(&kind_root, "lifecycle scope-kind directory").await?;
    if kind_entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read lifecycle scope kind", root, error))?
        .is_some()
    {
        return Err(inventory_invalid(
            "A lifecycle operation root contains multiple installation kinds.",
        ));
    }

    let mut scope_entries = fs::read_dir(&kind_root).await.map_err(|error| {
        super::inventory_io("read lifecycle installation key", &kind_root, error)
    })?;
    let scope_entry = scope_entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read lifecycle installation key", &kind_root, error))?
        .ok_or_else(|| {
            inventory_invalid("A lifecycle operation root omits its installation key.")
        })?;
    budget.observe_entry()?;
    if entry_name(&scope_entry, "lifecycle scope-kind directory")? != location.storage_key {
        return Err(inventory_invalid(
            "A lifecycle operation root belongs to another installation key.",
        ));
    }
    let scope_root = scope_entry.path();
    require_owned_directory(&scope_root, "lifecycle installation directory").await?;
    if scope_entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read lifecycle installation key", &kind_root, error))?
        .is_some()
    {
        return Err(inventory_invalid(
            "A lifecycle operation root contains multiple installation keys.",
        ));
    }
    scan_publishers(&scope_root, location, budget).await
}

async fn scan_publishers(
    root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
) -> UseResult<SourceFacts> {
    let mut facts = SourceFacts::default();
    let mut publisher_count = 0_usize;
    let mut package_count = 0_usize;
    let mut publishers = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read lifecycle publishers", root, error))?;
    while let Some(publisher) = publishers
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read lifecycle publisher", root, error))?
    {
        budget.observe_entry()?;
        publisher_count = checked_count(
            publisher_count,
            MAX_LIFECYCLE_PUBLISHERS,
            "The lifecycle operation inventory exceeds its publisher bound.",
        )?;
        let publisher_name = entry_name(&publisher, "lifecycle installation directory")?;
        super::validate_package_id(&publisher_name, "package")?;
        let publisher_root = publisher.path();
        require_owned_directory(&publisher_root, "lifecycle publisher directory").await?;
        let mut packages = fs::read_dir(&publisher_root).await.map_err(|error| {
            super::inventory_io("read lifecycle package root", &publisher_root, error)
        })?;
        while let Some(package) = packages.next_entry().await.map_err(|error| {
            super::inventory_io("read lifecycle package", &publisher_root, error)
        })? {
            budget.observe_entry()?;
            package_count = checked_count(
                package_count,
                MAX_LIFECYCLE_PACKAGES,
                "The lifecycle operation inventory exceeds its package bound.",
            )?;
            let package_name = entry_name(&package, "lifecycle publisher directory")?;
            let package_id = super::validate_package_id(&publisher_name, &package_name)?;
            let package_root = package.path();
            require_owned_directory(&package_root, "lifecycle package directory").await?;
            scan_package(&package_root, &package_id, location, budget, &mut facts).await?;
        }
    }
    Ok(facts)
}

async fn scan_package(
    root: &Path,
    package_id: &str,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    let _lock = acquire_existing_operation_lock(root).await?;
    let mut active = None;
    let mut last = None;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| super::inventory_io("read lifecycle package directory", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| super::inventory_io("read lifecycle package entry", root, error))?
    {
        budget.observe_entry()?;
        let name = entry_name(&entry, "lifecycle package directory")?;
        let path = entry.path();
        match name.as_str() {
            ".operation.lock" => {
                require_owned_file(&path, 4 * 1024, "lifecycle operation lock").await?;
            }
            "active.json" => {
                active = Some(
                    read_bounded_json::<PluginLifecycleOperationRecord>(
                        &path,
                        MAX_LIFECYCLE_OPERATION_BYTES,
                        "active lifecycle operation",
                    )
                    .await?,
                );
            }
            "last.json" => {
                last = Some(
                    read_bounded_json::<PluginLifecycleOperationRecord>(
                        &path,
                        MAX_LIFECYCLE_OPERATION_BYTES,
                        "previous lifecycle operation",
                    )
                    .await?,
                );
            }
            _ if valid_temporary_name(&name, ".operation-") => {
                require_owned_file(
                    &path,
                    MAX_LIFECYCLE_OPERATION_BYTES,
                    "temporary lifecycle operation",
                )
                .await?;
            }
            _ => {
                return Err(inventory_invalid(
                    "A lifecycle package directory contains an unknown entry.",
                ))
            }
        }
    }
    let active = active.ok_or_else(|| {
        inventory_invalid("A lifecycle package directory omits its active operation record.")
    })?;
    validate_record(&active, package_id, location, facts)?;
    if let Some(last) = &last {
        validate_record(last, package_id, location, facts)?;
        if matches!(
            last.status,
            PluginLifecycleOperationStatus::Applying | PluginLifecycleOperationStatus::RollingBack
        ) || last == &active
        {
            return Err(inventory_invalid(
                "A previous lifecycle record is nonterminal or duplicates the active record.",
            ));
        }
    }
    if matches!(
        active.status,
        PluginLifecycleOperationStatus::Applying | PluginLifecycleOperationStatus::RollingBack
    ) {
        facts.references.push(RawArtifactReference {
            kind: ArtifactKind::ExpandedPackage,
            digest: active.intent.package_digest.clone(),
            source: ArtifactReferenceSource::PluginLifecycleOperation,
            installation: Some(active.intent.scope.clone()),
            expected_bytes: None,
            expected_files: None,
        });
    }
    Ok(())
}

fn validate_record(
    record: &PluginLifecycleOperationRecord,
    package_id: &str,
    location: &InstallationLocation,
    facts: &mut SourceFacts,
) -> UseResult<()> {
    record.validate()?;
    location.validate_identity(&record.intent.scope)?;
    if record.intent.package_id != package_id {
        return Err(inventory_invalid(
            "A lifecycle operation does not match its package path.",
        ));
    }
    facts.observe_identity(record.intent.scope.clone())
}

async fn acquire_existing_operation_lock(root: &Path) -> UseResult<StdFile> {
    let path = root.join(".operation.lock");
    super::io::acquire_existing_owned_lock_shared(&path, "lifecycle operation lock").await
}
