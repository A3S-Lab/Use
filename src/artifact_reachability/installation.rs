use std::io as std_io;
use std::path::{Path, PathBuf};

use a3s_use_core::{
    InstallationId, InstallationKind, InstallationSnapshot, PluginPackageId, PluginPackageLock,
    UseError, UseResult, MAX_INSTALLATION_SNAPSHOT_BYTES,
};
use a3s_use_extension::{ArtifactKind, UsePaths, ACTIVE_STATE_RESTORE_MARKER};
use tokio::fs;

use super::{reference_invalid, ArtifactReferenceSource, RawArtifactReference};
use crate::cognitive_package::{
    acquire_existing_package_graph_lock_shared, inspect_pending_artifact_references_locked,
    PendingPackageGraphArtifactReferences,
};
use crate::installation_state_layout;

mod io;
mod lifecycle;
mod receipts;

use self::io::{
    entry_name, optional_owned_directory, owned_metadata, read_bounded_bytes,
    require_owned_directory,
};

const INSTALLATIONS_DIRECTORY: &str = "installations";
const INSTALLATION_SNAPSHOT_FILE: &str = "installation-snapshot.json";
const MAX_INSTALLATIONS: usize = 10_000;
const MAX_INSTALLATION_ROOT_ENTRIES: usize = 64;
const MAX_OPERATION_FAMILIES: usize = 32;
const MAX_INSTALLATION_REFERENCE_FACTS: usize = 1_000_000;

pub(super) async fn inspect(paths: &UsePaths) -> UseResult<Vec<RawArtifactReference>> {
    let root = paths.state_root().join(INSTALLATIONS_DIRECTORY);
    if !optional_owned_directory(&root, "installation state root").await? {
        return Ok(Vec::new());
    }

    let mut budget = InventoryBudget::default();
    let mut references = Vec::new();
    let mut kinds = fs::read_dir(&root)
        .await
        .map_err(|error| inventory_io("read installation state root", &root, error))?;
    while let Some(entry) = kinds
        .next_entry()
        .await
        .map_err(|error| inventory_io("read installation kind", &root, error))?
    {
        budget.observe_entry()?;
        let name = entry_name(&entry, "installation state root")?;
        let kind = match name.as_str() {
            "user" => InstallationKind::User,
            "workspace" => InstallationKind::Workspace,
            _ => {
                return Err(inventory_invalid(
                    "The installation state root contains an unknown installation kind.",
                ))
            }
        };
        let kind_root = entry.path();
        require_owned_directory(&kind_root, "installation kind directory").await?;
        scan_kind_root(paths, &kind_root, kind, &mut budget, &mut references).await?;
    }
    Ok(references)
}

async fn scan_kind_root(
    paths: &UsePaths,
    root: &Path,
    kind: InstallationKind,
    budget: &mut InventoryBudget,
    references: &mut Vec<RawArtifactReference>,
) -> UseResult<()> {
    let mut installations = fs::read_dir(root)
        .await
        .map_err(|error| inventory_io("read installation kind directory", root, error))?;
    while let Some(entry) = installations
        .next_entry()
        .await
        .map_err(|error| inventory_io("read installation state directory", root, error))?
    {
        budget.observe_entry()?;
        budget.observe_installation()?;
        let storage_key = entry_name(&entry, "installation kind directory")?;
        if !valid_raw_sha256(&storage_key) {
            return Err(inventory_invalid(
                "An installation state directory has a non-canonical storage key.",
            ));
        }
        let state_root = entry.path();
        require_owned_directory(&state_root, "installation state directory").await?;
        let location = InstallationLocation { kind, storage_key };
        let facts = scan_installation(paths, &state_root, &location, budget).await?;
        references.extend(facts.references);
        if references.len() > MAX_INSTALLATION_REFERENCE_FACTS {
            return Err(inventory_limit(
                "The installation artifact reference inventory exceeds its fact bound.",
            ));
        }
    }
    Ok(())
}

async fn scan_installation(
    paths: &UsePaths,
    state_root: &Path,
    location: &InstallationLocation,
    budget: &mut InventoryBudget,
) -> UseResult<SourceFacts> {
    let mut snapshot = None;
    let mut current_receipts = None;
    let mut retained_receipts = None;
    let mut pending_operations = None;
    let mut lifecycle_operations = None;
    let mut entries = fs::read_dir(state_root)
        .await
        .map_err(|error| inventory_io("read installation state directory", state_root, error))?;
    let mut root_entries = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| inventory_io("read installation state entry", state_root, error))?
    {
        budget.observe_entry()?;
        root_entries = checked_count(
            root_entries,
            MAX_INSTALLATION_ROOT_ENTRIES,
            "An installation state root exceeds its entry bound.",
        )?;
        let name = entry_name(&entry, "installation state directory")?;
        let path = entry.path();
        let metadata = owned_metadata(&path, "installation state entry").await?;
        if !installation_state_layout::supported_root_entry(&name, metadata.is_dir()) {
            return Err(inventory_invalid(
                "An installation state root contains an unknown or mistyped state family.",
            ));
        }
        match name.as_str() {
            ACTIVE_STATE_RESTORE_MARKER => return Err(UseError::new(
                "use.artifact_reachability.state_unstable",
                "Artifact references cannot be collected while an installation restore is active.",
            )),
            INSTALLATION_SNAPSHOT_FILE => snapshot = Some(path),
            "extensions" => current_receipts = Some(path),
            "extension-generations" => retained_receipts = Some(path),
            "operations" => {
                let operation_roots = scan_operation_roots(&path, budget).await?;
                pending_operations = operation_roots.package_graphs;
                lifecycle_operations = operation_roots.plugins;
            }
            _ => {}
        }
    }

    // Additions acquire global reference admission before the graph lock. The
    // collector already owns the inverse side of that global boundary, so one
    // shared graph lock makes the snapshot and pending-operation view coherent
    // without introducing a new authority.
    let mut facts = SourceFacts::default();
    if snapshot.is_some() || pending_operations.is_some() {
        let _package_graph_lock = acquire_existing_package_graph_lock_shared(state_root).await?;
        if let Some(path) = snapshot {
            facts.merge(scan_snapshot(&path, location).await?)?;
        }
        if let Some(root) = pending_operations {
            facts.merge(scan_pending(&root, location).await?)?;
        }
    }
    if current_receipts.is_some() || retained_receipts.is_some() {
        // Do not nest unrelated installation locks. Global admission freezes
        // additions across both sources; a retirement between source scans can
        // therefore only leave conservative extra references.
        let _registry_lock = receipts::acquire_existing_registry_lock_shared(state_root).await?;
        if let Some(root) = current_receipts {
            facts.merge(
                receipts::scan_current(&root, location, &paths.artifact_store(), budget).await?,
            )?;
        }
        if let Some(root) = retained_receipts {
            facts.merge(
                receipts::scan_retained(&root, location, &paths.artifact_store(), budget).await?,
            )?;
        }
    }
    if let Some(root) = lifecycle_operations {
        facts.merge(lifecycle::scan(&root, location, budget).await?)?;
    }
    Ok(facts)
}

async fn scan_snapshot(path: &Path, location: &InstallationLocation) -> UseResult<SourceFacts> {
    let bytes = read_bounded_bytes(
        path,
        MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        "installation snapshot",
    )
    .await?;
    let snapshot = InstallationSnapshot::from_json(&bytes)?;
    location.validate_identity(&snapshot.installation)?;
    let mut facts = SourceFacts::with_identity(snapshot.installation.clone());
    for selection in &snapshot.packages {
        let package = &selection.package.catalog.record.package;
        let digest = package.sha256.clone().ok_or_else(|| {
            inventory_invalid("An installation snapshot package omits its expanded digest.")
        })?;
        facts.references.push(RawArtifactReference {
            kind: ArtifactKind::ExpandedPackage,
            digest,
            source: ArtifactReferenceSource::InstallationSnapshot,
            installation: Some(snapshot.installation.clone()),
            expected_bytes: Some(package.expanded_bytes),
            expected_files: Some(package.file_count),
        });
    }
    Ok(facts)
}

async fn scan_pending(root: &Path, location: &InstallationLocation) -> UseResult<SourceFacts> {
    let operations: Vec<PendingPackageGraphArtifactReferences> =
        inspect_pending_artifact_references_locked(root).await?;
    let mut facts = SourceFacts::default();
    for operation in operations {
        let installation = operation.installation;
        location.validate_identity(&installation)?;
        facts.observe_identity(installation.clone())?;
        if operation.cancelled {
            continue;
        }
        let mut lock_digests = std::collections::BTreeSet::new();
        for package_lock in &operation.package_locks {
            let lock_digest = package_lock.descriptor_digest()?;
            if lock_digests.insert(lock_digest) {
                append_package_lock_references(&mut facts, &installation, package_lock)?;
            }
        }
    }
    Ok(facts)
}

fn append_package_lock_references(
    facts: &mut SourceFacts,
    installation: &InstallationId,
    package_lock: &PluginPackageLock,
) -> UseResult<()> {
    package_lock.validate()?;
    for package in &package_lock.packages {
        let record = &package.catalog.record;
        let expanded_digest = record.package.sha256.clone().ok_or_else(|| {
            inventory_invalid("A pending package lock omits its expanded package digest.")
        })?;
        facts.references.push(RawArtifactReference {
            kind: ArtifactKind::ExpandedPackage,
            digest: expanded_digest,
            source: ArtifactReferenceSource::PendingPackageGraph,
            installation: Some(installation.clone()),
            expected_bytes: Some(record.package.expanded_bytes),
            expected_files: Some(record.package.file_count),
        });
        facts.references.push(RawArtifactReference {
            kind: ArtifactKind::Blob,
            digest: record.archive.sha256.clone(),
            source: ArtifactReferenceSource::PendingPackageGraph,
            installation: Some(installation.clone()),
            expected_bytes: Some(record.archive.length),
            expected_files: None,
        });
        if let Some(planning) = &record.planning {
            facts.references.push(RawArtifactReference {
                kind: ArtifactKind::Blob,
                digest: planning.sha256.clone(),
                source: ArtifactReferenceSource::PendingPackageGraph,
                installation: Some(installation.clone()),
                expected_bytes: Some(planning.length),
                expected_files: None,
            });
        }
    }
    Ok(())
}

async fn scan_operation_roots(
    root: &Path,
    budget: &mut InventoryBudget,
) -> UseResult<OperationRoots> {
    let mut roots = OperationRoots::default();
    let mut count = 0_usize;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| inventory_io("read installation operations", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| inventory_io("read installation operation family", root, error))?
    {
        budget.observe_entry()?;
        count = checked_count(
            count,
            MAX_OPERATION_FAMILIES,
            "An installation operations root exceeds its family bound.",
        )?;
        let name = entry_name(&entry, "installation operations root")?;
        if !installation_state_layout::supported_operation_directory(&name) {
            return Err(inventory_invalid(
                "An installation operations root contains an unknown state family.",
            ));
        }
        let path = entry.path();
        require_owned_directory(&path, "installation operation family").await?;
        match name.as_str() {
            "package-graphs" => roots.package_graphs = Some(path),
            "plugins" => roots.plugins = Some(path),
            _ => {}
        }
    }
    Ok(roots)
}

#[derive(Debug, Clone)]
pub(super) struct InstallationLocation {
    pub(super) kind: InstallationKind,
    pub(super) storage_key: String,
}

impl InstallationLocation {
    pub(super) fn validate_identity(&self, installation: &InstallationId) -> UseResult<()> {
        installation.validate()?;
        if installation.kind != self.kind || installation.storage_key()? != self.storage_key {
            return Err(inventory_invalid(
                "An artifact reference belongs to a different installation state path.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(super) struct SourceFacts {
    identity: Option<InstallationId>,
    pub(super) references: Vec<RawArtifactReference>,
}

impl SourceFacts {
    pub(super) fn with_identity(identity: InstallationId) -> Self {
        Self {
            identity: Some(identity),
            references: Vec::new(),
        }
    }

    pub(super) fn observe_identity(&mut self, candidate: InstallationId) -> UseResult<()> {
        if self
            .identity
            .as_ref()
            .is_some_and(|identity| identity != &candidate)
        {
            return Err(inventory_invalid(
                "One installation state directory contains conflicting installation identities.",
            ));
        }
        self.identity = Some(candidate);
        Ok(())
    }

    fn merge(&mut self, other: Self) -> UseResult<()> {
        if let Some(identity) = other.identity {
            self.observe_identity(identity)?;
        }
        self.references.extend(other.references);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct OperationRoots {
    package_graphs: Option<PathBuf>,
    plugins: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub(super) struct InventoryBudget {
    installations: usize,
    entries: usize,
}

impl InventoryBudget {
    fn observe_installation(&mut self) -> UseResult<()> {
        self.installations = checked_count(
            self.installations,
            MAX_INSTALLATIONS,
            "The installation artifact inventory exceeds its installation bound.",
        )?;
        Ok(())
    }

    pub(super) fn observe_entry(&mut self) -> UseResult<()> {
        self.entries = checked_count(
            self.entries,
            MAX_INSTALLATION_REFERENCE_FACTS,
            "The installation artifact inventory exceeds its traversal bound.",
        )?;
        Ok(())
    }
}

pub(super) fn validate_package_id(publisher: &str, package: &str) -> UseResult<String> {
    PluginPackageId::parse(format!("{publisher}/{package}"))
        .map(|package_id| package_id.as_str().to_owned())
        .map_err(|_| inventory_invalid("An artifact reference path has an invalid package ID."))
}

pub(super) fn valid_temporary_name(name: &str, prefix: &str) -> bool {
    let Some(suffix) = name
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    !suffix.is_empty()
        && suffix.len() <= 80
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
}

fn valid_raw_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn checked_count(current: usize, limit: usize, message: &str) -> UseResult<usize> {
    let next = current
        .checked_add(1)
        .ok_or_else(|| inventory_limit(message))?;
    if next > limit {
        return Err(inventory_limit(message));
    }
    Ok(next)
}

pub(super) fn inventory_invalid(message: impl Into<String>) -> UseError {
    reference_invalid(message)
}

pub(super) fn inventory_limit(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.artifact_reachability.inventory_limit_exceeded",
        message,
    )
}

pub(super) fn inventory_io(action: &str, path: &Path, error: std_io::Error) -> UseError {
    UseError::new(
        "use.artifact_reachability.inventory_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
