use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{
    InstallationId, InstallationPackageSelection, InstallationRootSelection, InstallationSnapshot,
    PluginPackageId, PluginPackageLock, UseResult, MAX_INSTALLATION_SNAPSHOT_BYTES,
};
use a3s_use_extension::{ArtifactStore, ExtensionPaths};
use tokio::fs;

#[cfg(test)]
use super::test_artifact_store;
use super::{
    acquire_lock, path_error, read_optional_bounded, store_error,
    validate_existing_directory_chain, write_new_bounded,
};
use crate::cognitive_package::package_manager_error;

const INSTALLATION_SNAPSHOT_FILE: &str = "installation-snapshot.json";
const LEGACY_INSTALLED_GRAPHS_DIRECTORY: &str = "package-graphs";

/// Durable owner of the single resolved package graph for one installation.
///
/// Root-specific locks are derived views. The only stored authority is one
/// monotonically generated `InstallationSnapshot`; a package ID cannot hold
/// conflicting selections beneath different roots.
#[derive(Debug, Clone)]
pub(crate) struct InstallationSnapshotStore {
    pub(super) artifact_store: ArtifactStore,
    installation: InstallationId,
    state_root: PathBuf,
    path: PathBuf,
    legacy_root: PathBuf,
}

impl InstallationSnapshotStore {
    #[cfg(test)]
    pub(super) fn new(
        state_root: impl Into<PathBuf>,
        installation: InstallationId,
    ) -> UseResult<Self> {
        installation.validate()?;
        let state_root = state_root.into();
        let artifact_store = test_artifact_store(&state_root);
        Ok(Self::from_parts(state_root, installation, artifact_store))
    }

    fn from_parts(
        state_root: PathBuf,
        installation: InstallationId,
        artifact_store: ArtifactStore,
    ) -> Self {
        Self {
            artifact_store,
            installation,
            path: state_root.join(INSTALLATION_SNAPSHOT_FILE),
            legacy_root: state_root.join(LEGACY_INSTALLED_GRAPHS_DIRECTORY),
            state_root,
        }
    }

    pub(crate) fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::from_parts(
            paths.installation_state_root(),
            paths.installation().clone(),
            paths.artifact_store(),
        )
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(super) async fn snapshot(&self) -> UseResult<Option<InstallationSnapshot>> {
        self.read_snapshot().await
    }

    pub async fn put(
        &self,
        lock: &PluginPackageLock,
        installed_at_ms: u64,
        package_selections: Vec<InstallationPackageSelection>,
    ) -> UseResult<bool> {
        lock.validate()?;
        validate_candidate_selections(lock, &package_selections)?;
        let selection = InstallationRootSelection::new(&lock.root_package_id, installed_at_ms)?;
        let _artifact_admission = self.artifact_store.acquire_reference_admission().await?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let current = self.read_snapshot_file().await?;
        let (generation, host, mut roots) = match current.as_ref() {
            Some(snapshot) => {
                if let Some(current_lock) = snapshot.package_lock(&lock.root_package_id)? {
                    if current_lock == *lock {
                        ensure_current_selections(snapshot, &package_selections)?;
                        return Ok(false);
                    }
                    return Err(package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Cognitive package '{}' already owns a different installed dependency lock.",
                            lock.root_package_id
                        ),
                    ));
                }
                let host = if snapshot.roots.is_empty() {
                    lock.host.clone()
                } else {
                    snapshot.host.clone()
                };
                (
                    next_snapshot_generation(snapshot.generation)?,
                    host,
                    snapshot_root_locks(snapshot)?,
                )
            }
            None => (1, lock.host.clone(), Vec::new()),
        };
        if host != lock.host {
            return Err(store_error(
                "An installed package lock targets a different installation host.",
            ));
        }
        roots.push((selection, lock.clone()));
        let package_selections =
            merge_package_selections(&roots, current.as_ref(), package_selections)?;
        let replacement = InstallationSnapshot::from_root_locks(
            self.installation.clone(),
            generation,
            host,
            roots,
            package_selections,
        )?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok(true)
    }

    pub async fn get(&self, root_package_id: &str) -> UseResult<Option<PluginPackageLock>> {
        validate_root_package_id(root_package_id)?;
        let Some(snapshot) = self.read_snapshot().await? else {
            return Ok(None);
        };
        snapshot.package_lock(root_package_id)
    }

    pub async fn replace(
        &self,
        root_package_id: &str,
        expected_digest: &str,
        replacement: &PluginPackageLock,
        installed_at_ms: u64,
        package_selections: Vec<InstallationPackageSelection>,
    ) -> UseResult<bool> {
        validate_root_package_id(root_package_id)?;
        replacement.validate()?;
        validate_candidate_selections(replacement, &package_selections)?;
        if replacement.root_package_id != root_package_id {
            return Err(store_error(
                "A replacement graph does not own the requested root package.",
            ));
        }
        let selection = InstallationRootSelection::new(root_package_id, installed_at_ms)?;
        let _artifact_admission = self.artifact_store.acquire_reference_admission().await?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let snapshot = self.read_snapshot_file().await?.ok_or_else(|| {
            store_error("The installation snapshot disappeared before replacement.")
        })?;
        let current = snapshot.package_lock(root_package_id)?.ok_or_else(|| {
            store_error("The installed package graph disappeared before replacement.")
        })?;
        if current == *replacement {
            ensure_current_selections(&snapshot, &package_selections)?;
            return Ok(false);
        }
        if current.descriptor_digest()? != expected_digest {
            return Err(store_error(
                "The installed package graph changed before replacement.",
            ));
        }
        let mut roots = snapshot_root_locks(&snapshot)?;
        roots.retain(|(root, _)| root.package_id != root_package_id);
        roots.push((selection, replacement.clone()));
        let package_selections =
            merge_package_selections(&roots, Some(&snapshot), package_selections)?;
        let replacement = InstallationSnapshot::from_root_locks(
            self.installation.clone(),
            next_snapshot_generation(snapshot.generation)?,
            snapshot.host.clone(),
            roots,
            package_selections,
        )?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok(true)
    }

    pub async fn list(&self) -> UseResult<Vec<PluginPackageLock>> {
        let Some(snapshot) = self.read_snapshot().await? else {
            return Ok(Vec::new());
        };
        snapshot.package_locks()
    }

    pub(crate) async fn current(&self) -> UseResult<Option<InstallationSnapshot>> {
        self.read_snapshot().await
    }

    /// Complete or replay the cutover owned by one already-admitted
    /// enablement operation. A replay is accepted only at exactly the next
    /// package state generation with the requested desired state.
    pub async fn complete_package_enablement(
        &self,
        package_id: &str,
        expected_state_generation_before: u64,
        enabled: bool,
    ) -> UseResult<(InstallationSnapshot, bool)> {
        PluginPackageId::parse(package_id.to_owned())?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let snapshot = self.read_snapshot_file().await?.ok_or_else(|| {
            store_error("The installation snapshot is absent during enablement completion.")
        })?;
        if snapshot
            .package_selection(package_id)
            .is_some_and(|selection| {
                selection.enabled == enabled
                    && expected_state_generation_before.checked_add(1)
                        == Some(selection.state_generation)
            })
        {
            return Ok((snapshot, false));
        }
        let replacement = snapshot
            .transition_package_enablement(package_id, expected_state_generation_before, enabled)?
            .ok_or_else(|| {
                store_error(
                    "An admitted enablement operation no longer describes a state transition.",
                )
            })?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok((replacement, true))
    }

    pub async fn remove(&self, root_package_id: &str, expected_digest: &str) -> UseResult<bool> {
        validate_root_package_id(root_package_id)?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let Some(snapshot) = self.read_snapshot_file().await? else {
            return Ok(false);
        };
        let Some(current) = snapshot.package_lock(root_package_id)? else {
            return Ok(false);
        };
        if current.descriptor_digest()? != expected_digest {
            return Err(store_error(
                "The installed package graph changed before removal.",
            ));
        }
        let mut roots = snapshot_root_locks(&snapshot)?;
        roots.retain(|(root, _)| root.package_id != root_package_id);
        let reachable = roots
            .iter()
            .flat_map(|(_, lock)| lock.packages.iter())
            .map(|package| package.package_id().to_owned())
            .collect::<BTreeSet<_>>();
        let package_selections = snapshot
            .packages
            .iter()
            .filter(|selection| reachable.contains(selection.package_id()))
            .cloned()
            .collect();
        let replacement = InstallationSnapshot::from_root_locks(
            self.installation.clone(),
            next_snapshot_generation(snapshot.generation)?,
            snapshot.host.clone(),
            roots,
            package_selections,
        )?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok(true)
    }

    async fn read_snapshot(&self) -> UseResult<Option<InstallationSnapshot>> {
        if !validate_existing_directory_chain(&self.state_root, &self.state_root).await? {
            return Ok(None);
        }
        reject_legacy_graph_layout(&self.legacy_root).await?;
        self.read_snapshot_file().await
    }

    async fn read_snapshot_file(&self) -> UseResult<Option<InstallationSnapshot>> {
        let snapshot = read_optional_bounded::<InstallationSnapshot>(
            &self.path,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        if let Some(snapshot) = &snapshot {
            snapshot.validate()?;
            self.installation.ensure_same(&snapshot.installation)?;
        }
        Ok(snapshot)
    }
}

fn snapshot_root_locks(
    snapshot: &InstallationSnapshot,
) -> UseResult<Vec<(InstallationRootSelection, PluginPackageLock)>> {
    let locks = snapshot.package_locks()?;
    if locks.len() != snapshot.roots.len() {
        return Err(store_error(
            "The installation snapshot root and lock counts differ.",
        ));
    }
    Ok(snapshot.roots.iter().cloned().zip(locks).collect())
}

fn validate_candidate_selections(
    lock: &PluginPackageLock,
    selections: &[InstallationPackageSelection],
) -> UseResult<()> {
    if selections.len() != lock.packages.len() {
        return Err(store_error(
            "A graph cutover must provide one activation selection for every candidate package.",
        ));
    }
    for (package, selection) in lock.packages.iter().zip(selections) {
        selection.validate()?;
        if selection.package != *package {
            return Err(store_error(
                "A graph activation selection differs from its exact candidate package.",
            ));
        }
    }
    Ok(())
}

fn ensure_current_selections(
    snapshot: &InstallationSnapshot,
    selections: &[InstallationPackageSelection],
) -> UseResult<()> {
    if selections
        .iter()
        .all(|selection| snapshot.package_selection(selection.package_id()) == Some(selection))
    {
        return Ok(());
    }
    Err(package_manager_error(
        "use.plugin.package_graph_reconcile_required",
        "The installed dependency lock matches, but its activation intent differs from the installation snapshot.",
    ))
}

fn merge_package_selections(
    roots: &[(InstallationRootSelection, PluginPackageLock)],
    current: Option<&InstallationSnapshot>,
    candidate: Vec<InstallationPackageSelection>,
) -> UseResult<Vec<InstallationPackageSelection>> {
    let packages = roots
        .iter()
        .flat_map(|(_, lock)| lock.packages.iter())
        .map(|package| (package.package_id().to_owned(), package))
        .collect::<BTreeMap<_, _>>();
    let mut merged = current
        .into_iter()
        .flat_map(|snapshot| snapshot.packages.iter().cloned())
        .map(|selection| (selection.package_id().to_owned(), selection))
        .collect::<BTreeMap<_, _>>();
    for selection in candidate {
        if let Some(existing) = merged.get(selection.package_id()) {
            if selection.package == existing.package && &selection != existing {
                return Err(store_error(
                    "A graph cutover attempted to change activation intent for a retained package.",
                ));
            }
            if selection.package != existing.package
                && (selection.state_generation <= existing.state_generation
                    || selection.enabled != existing.enabled)
            {
                return Err(store_error(
                    "A graph replacement must advance package state without changing enablement intent.",
                ));
            }
        }
        merged.insert(selection.package_id().to_owned(), selection);
    }
    merged.retain(|package_id, _| packages.contains_key(package_id));
    if merged.len() != packages.len()
        || packages.iter().any(|(package_id, package)| {
            merged
                .get(package_id)
                .is_none_or(|selection| &selection.package != *package)
        })
    {
        return Err(store_error(
            "The graph cutover does not own complete activation intent for its resolved packages.",
        ));
    }
    Ok(merged.into_values().collect())
}

fn next_snapshot_generation(generation: u64) -> UseResult<u64> {
    generation.checked_add(1).ok_or_else(|| {
        package_manager_error(
            "use.installation.snapshot_generation_exhausted",
            "The installation snapshot generation is exhausted.",
        )
    })
}

fn validate_root_package_id(root_package_id: &str) -> UseResult<()> {
    PluginPackageId::parse(root_package_id.to_owned())
        .map(drop)
        .map_err(|_| store_error("An installation root package identity is invalid."))
}

async fn reject_legacy_graph_layout(path: &Path) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Err(package_manager_error(
            "use.installation.snapshot_legacy_state_unsupported",
            "Per-root installed package graph files are unsupported; preserve the old state for review and reinstall into a clean installation root.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error("inspect legacy installed graph state", path, error)),
    }
}
