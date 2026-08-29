use std::collections::{BTreeMap, BTreeSet};

use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{UseError, UseResult};

use super::validation::valid_package_id;
use super::{
    InstallationId, LockedPluginPackage, PluginPackageLock, PluginPackageLockHost,
    PluginSurfaceRef, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_PACKAGE_LOCK_SCHEMA,
};

pub const INSTALLATION_SNAPSHOT_SCHEMA: &str = "a3s.use.installation-snapshot.v2";
pub const MAX_INSTALLATION_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_INSTALLATION_ROOTS: usize = MAX_PLUGIN_PLAN_ITEMS;
pub const MAX_INSTALLATION_PACKAGES: usize = MAX_PLUGIN_PLAN_ITEMS * 8;

const SNAPSHOT_ERROR: &str = "use.installation.snapshot_invalid";

/// One explicitly selected root in an installation's desired package set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationRootSelection {
    pub package_id: String,
    pub installed_at_ms: u64,
}

/// One package selected by an installation generation, including the desired
/// activation and exact capability-publication surface set.
///
/// The immutable catalog node selects package bytes and dependency edges.
/// `enabled` and `selected_surfaces` are desired intent; lifecycle receipts
/// and capability routes are observations of whether providers applied it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationPackageSelection {
    pub package: LockedPluginPackage,
    pub state_generation: u64,
    pub enabled: bool,
    pub selected_surfaces: Vec<PluginSurfaceRef>,
}

/// Canonical installed-selection authority for one User or Workspace installation.
///
/// `roots` is the desired root set. `packages` is the single resolved graph:
/// each package ID appears exactly once even when multiple roots share it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationSnapshot {
    pub schema: String,
    pub installation: InstallationId,
    pub generation: u64,
    pub host: PluginPackageLockHost,
    pub roots: Vec<InstallationRootSelection>,
    pub packages: Vec<InstallationPackageSelection>,
}

impl InstallationRootSelection {
    pub fn new(package_id: impl Into<String>, installed_at_ms: u64) -> UseResult<Self> {
        let selection = Self {
            package_id: package_id.into(),
            installed_at_ms,
        };
        selection.validate()?;
        Ok(selection)
    }

    pub fn validate(&self) -> UseResult<()> {
        if !valid_package_id(&self.package_id) || self.installed_at_ms == 0 {
            return Err(snapshot_error(
                "An installation root selection has an invalid package identity or timestamp.",
            ));
        }
        Ok(())
    }
}

impl InstallationPackageSelection {
    pub fn new(
        package: LockedPluginPackage,
        state_generation: u64,
        enabled: bool,
        selected_surfaces: Vec<PluginSurfaceRef>,
    ) -> UseResult<Self> {
        let selection = Self {
            package,
            state_generation,
            enabled,
            selected_surfaces,
        };
        selection.validate()?;
        Ok(selection)
    }

    pub fn package_id(&self) -> &str {
        self.package.package_id()
    }

    pub fn validate(&self) -> UseResult<()> {
        self.package.catalog.validate().map_err(|_| {
            snapshot_error("An installation package has invalid verified catalog evidence.")
        })?;
        if self.state_generation == 0 || self.selected_surfaces.is_empty() {
            return Err(snapshot_error(
                "An installation package has invalid state generation or publication intent.",
            ));
        }
        let resolved = self
            .package
            .catalog
            .selected_state(&self.selected_surfaces)
            .map_err(|_| snapshot_error("An installation package has invalid publication intent."))?
            .release
            .surfaces
            .into_iter()
            .map(|surface| surface.reference())
            .collect::<Vec<_>>();
        if resolved != self.selected_surfaces {
            return Err(snapshot_error(
                "An installation package publication intent is not an exact surface closure.",
            ));
        }
        Ok(())
    }
}

impl InstallationSnapshot {
    /// Build one canonical graph from independently resolved root closures.
    ///
    /// A package selected by more than one root must have byte-for-byte equal
    /// catalog and dependency evidence. Conflicting selections are rejected.
    pub fn from_root_locks(
        installation: InstallationId,
        generation: u64,
        host: PluginPackageLockHost,
        roots: Vec<(InstallationRootSelection, PluginPackageLock)>,
        package_selections: Vec<InstallationPackageSelection>,
    ) -> UseResult<Self> {
        if roots.len() > MAX_INSTALLATION_ROOTS
            || package_selections.len() > MAX_INSTALLATION_PACKAGES
        {
            return Err(snapshot_error(
                "The installation snapshot exceeds its root or package bound.",
            ));
        }
        let mut root_selections = BTreeMap::new();
        let mut packages = BTreeMap::<String, LockedPluginPackage>::new();
        for (selection, package_lock) in roots {
            selection.validate()?;
            package_lock.validate().map_err(|_| {
                snapshot_error("An installation root has an invalid resolved package lock.")
            })?;
            if package_lock.root_package_id != selection.package_id || package_lock.host != host {
                return Err(snapshot_error(
                    "An installation root lock belongs to a different root or host.",
                ));
            }
            if root_selections
                .insert(selection.package_id.clone(), selection)
                .is_some()
            {
                return Err(snapshot_error(
                    "An installation root appears more than once.",
                ));
            }
            for package in package_lock.packages {
                let package_id = package.package_id().to_owned();
                if let Some(existing) = packages.get(&package_id) {
                    if existing != &package {
                        return Err(snapshot_error(format!(
                            "Package '{package_id}' has conflicting selections across installation roots."
                        )));
                    }
                    continue;
                }
                packages.insert(package_id, package);
                if packages.len() > MAX_INSTALLATION_PACKAGES {
                    return Err(snapshot_error(
                        "The installation snapshot exceeds its package bound.",
                    ));
                }
            }
        }

        let mut selections = BTreeMap::new();
        for selection in package_selections {
            selection.validate()?;
            let package_id = selection.package_id().to_owned();
            if selections.insert(package_id, selection).is_some() {
                return Err(snapshot_error(
                    "An installation package selection appears more than once.",
                ));
            }
        }
        if selections.len() != packages.len()
            || packages.iter().any(|(package_id, package)| {
                selections
                    .get(package_id)
                    .is_none_or(|selection| &selection.package != package)
            })
        {
            return Err(snapshot_error(
                "Installation package selections must exactly match the resolved graph.",
            ));
        }

        let snapshot = Self {
            schema: INSTALLATION_SNAPSHOT_SCHEMA.to_owned(),
            installation,
            generation,
            host,
            roots: root_selections.into_values().collect(),
            packages: selections.into_values().collect(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > MAX_INSTALLATION_SNAPSHOT_BYTES {
            return Err(snapshot_error(
                "The installation snapshot exceeds its input bound.",
            ));
        }
        let snapshot: Self = serde_json::from_slice(input).map_err(|error| {
            snapshot_error(format!(
                "Failed to decode the installation snapshot at line {}, column {}.",
                error.line(),
                error.column()
            ))
        })?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != INSTALLATION_SNAPSHOT_SCHEMA
            || self.installation.validate().is_err()
            || self.generation == 0
            || self.host.validate().is_err()
            || self.roots.len() > MAX_INSTALLATION_ROOTS
            || self.packages.len() > MAX_INSTALLATION_PACKAGES
            || self.roots.is_empty() != self.packages.is_empty()
        {
            return Err(snapshot_error(
                "The installation snapshot identity, generation, host, or item bounds are invalid.",
            ));
        }
        if self
            .roots
            .windows(2)
            .any(|pair| pair[0].package_id >= pair[1].package_id)
            || self
                .packages
                .windows(2)
                .any(|pair| pair[0].package_id() >= pair[1].package_id())
        {
            return Err(snapshot_error(
                "Installation roots and packages must be sorted uniquely by package ID.",
            ));
        }
        for root in &self.roots {
            root.validate()?;
        }
        for package in &self.packages {
            package.validate()?;
        }

        let packages = self
            .packages
            .iter()
            .map(|selection| (selection.package_id(), selection))
            .collect::<BTreeMap<_, _>>();
        let mut reachable = BTreeSet::new();
        for root in &self.roots {
            let lock = self
                .package_lock_unchecked(&root.package_id, &packages)?
                .ok_or_else(|| {
                    snapshot_error("An installation root is absent from the resolved graph.")
                })?;
            lock.validate().map_err(|_| {
                snapshot_error("An installation root does not own a valid resolved closure.")
            })?;
            reachable.extend(
                lock.packages
                    .iter()
                    .map(|package| package.package_id().to_owned()),
            );
        }
        if reachable.len() != self.packages.len() {
            return Err(snapshot_error(
                "The installation graph contains a package unreachable from every desired root.",
            ));
        }
        for selection in &self.packages {
            if !selection.enabled {
                continue;
            }
            for dependency in &selection.package.dependencies {
                if packages
                    .get(dependency.package_id.as_str())
                    .is_none_or(|dependency| !dependency.enabled)
                {
                    return Err(snapshot_dependency_disabled(format!(
                        "Enabled package '{}' requires disabled dependency '{}'.",
                        selection.package_id(),
                        dependency.package_id
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            snapshot_error(format!(
                "Failed to encode canonical installation snapshot JSON: {error}"
            ))
        })?;
        if bytes.len() > MAX_INSTALLATION_SNAPSHOT_BYTES {
            return Err(snapshot_error(
                "The canonical installation snapshot exceeds its size bound.",
            ));
        }
        Ok(bytes)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }

    /// Reconstruct the exact closure for one desired root from the unified graph.
    pub fn package_lock(&self, root_package_id: &str) -> UseResult<Option<PluginPackageLock>> {
        self.validate()?;
        let packages = self
            .packages
            .iter()
            .map(|selection| (selection.package_id(), selection))
            .collect::<BTreeMap<_, _>>();
        self.package_lock_unchecked(root_package_id, &packages)
    }

    /// Return one exact selected package and its desired activation intent.
    pub fn package_selection(&self, package_id: &str) -> Option<&InstallationPackageSelection> {
        self.packages
            .binary_search_by(|selection| selection.package_id().cmp(package_id))
            .ok()
            .map(|index| &self.packages[index])
    }

    /// Build the next installation generation for one package enablement
    /// transition using the package-local state generation as its CAS token.
    ///
    /// `Ok(None)` means the exact requested desired state already holds.
    pub fn transition_package_enablement(
        &self,
        package_id: &str,
        expected_state_generation: u64,
        enabled: bool,
    ) -> UseResult<Option<Self>> {
        self.validate()?;
        let index = self
            .packages
            .binary_search_by(|selection| selection.package_id().cmp(package_id))
            .map_err(|_| snapshot_error("The selected package is absent from the installation."))?;
        let current = &self.packages[index];
        if current.state_generation != expected_state_generation {
            return Err(snapshot_generation_changed());
        }
        if current.enabled == enabled {
            return Ok(None);
        }
        let mut replacement = self.clone();
        replacement.generation = replacement
            .generation
            .checked_add(1)
            .ok_or_else(snapshot_generation_exhausted)?;
        replacement.packages[index].state_generation = replacement.packages[index]
            .state_generation
            .checked_add(1)
            .ok_or_else(snapshot_generation_exhausted)?;
        replacement.packages[index].enabled = enabled;
        replacement.validate()?;
        Ok(Some(replacement))
    }

    /// Reconstruct every root closure in canonical root order.
    pub fn package_locks(&self) -> UseResult<Vec<PluginPackageLock>> {
        self.validate()?;
        let packages = self
            .packages
            .iter()
            .map(|selection| (selection.package_id(), selection))
            .collect::<BTreeMap<_, _>>();
        self.roots
            .iter()
            .map(|root| {
                self.package_lock_unchecked(&root.package_id, &packages)?
                    .ok_or_else(|| {
                        snapshot_error("An installation root is absent from the resolved graph.")
                    })
            })
            .collect()
    }

    fn package_lock_unchecked(
        &self,
        root_package_id: &str,
        packages: &BTreeMap<&str, &InstallationPackageSelection>,
    ) -> UseResult<Option<PluginPackageLock>> {
        if !self
            .roots
            .iter()
            .any(|root| root.package_id == root_package_id)
        {
            return Ok(None);
        }
        let mut reachable = BTreeSet::new();
        let mut pending = vec![root_package_id.to_owned()];
        while let Some(package_id) = pending.pop() {
            if !reachable.insert(package_id.clone()) {
                continue;
            }
            let selection = packages.get(package_id.as_str()).ok_or_else(|| {
                snapshot_error(format!(
                    "Package '{package_id}' is absent from the installation graph."
                ))
            })?;
            pending.extend(
                selection
                    .package
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.package_id.clone()),
            );
        }
        Ok(Some(PluginPackageLock {
            schema: PLUGIN_PACKAGE_LOCK_SCHEMA.to_owned(),
            root_package_id: root_package_id.to_owned(),
            host: self.host.clone(),
            packages: self
                .packages
                .iter()
                .filter(|selection| reachable.contains(selection.package_id()))
                .map(|selection| selection.package.clone())
                .collect(),
        }))
    }
}

fn snapshot_error(message: impl Into<String>) -> UseError {
    UseError::new(SNAPSHOT_ERROR, message)
}

fn snapshot_generation_changed() -> UseError {
    UseError::new(
        "use.installation.snapshot_generation_changed",
        "The installation package state generation changed before cutover.",
    )
}

fn snapshot_generation_exhausted() -> UseError {
    UseError::new(
        "use.installation.snapshot_generation_exhausted",
        "The installation snapshot or package state generation is exhausted.",
    )
}

fn snapshot_dependency_disabled(message: impl Into<String>) -> UseError {
    UseError::new("use.installation.snapshot_dependency_disabled", message)
}
