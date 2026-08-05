use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::{valid_package_id, valid_target};
use super::{
    canonical_digest, canonical_json, parse_contract, CatalogAvailability, PlanPackageChangeKind,
    PlanPackageRole, PlannedPackageState, PlannedPackageTransition, PluginOperationPlan,
    PluginPlanSource, VerifiedPluginCatalogRecord, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_CATALOG_SCHEMA_V3,
};

const PACKAGE_LOCK_ERROR: &str = "use.plugin.package_lock_invalid";
pub const PLUGIN_PACKAGE_LOCK_SCHEMA: &str = "a3s.use.plugin-package-lock.v1";

/// Concrete host compatibility boundary used while resolving a lock graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPackageLockHost {
    pub target: String,
    pub use_version: String,
}

/// Exact selected version for one signed dependency edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedPluginPackageDependency {
    pub package_id: String,
    pub version_requirement: String,
    pub version: String,
}

/// One immutable package node and the verified catalog evidence that selected
/// its archive, content, manifest, registry, TUF root, channel, and target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedPluginPackage {
    pub catalog: VerifiedPluginCatalogRecord,
    pub dependencies: Vec<LockedPluginPackageDependency>,
}

/// Canonical, content-addressed closure for one cognitive-package operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPackageLock {
    pub schema: String,
    pub root_package_id: String,
    pub host: PluginPackageLockHost,
    pub packages: Vec<LockedPluginPackage>,
}

impl PluginPackageLockHost {
    pub fn new(target: impl Into<String>, use_version: impl Into<String>) -> UseResult<Self> {
        let host = Self {
            target: target.into(),
            use_version: use_version.into(),
        };
        host.validate()?;
        Ok(host)
    }

    pub fn validate(&self) -> UseResult<()> {
        let version = Version::parse(&self.use_version)
            .map_err(|_| lock_error("The package-lock host version is invalid."))?;
        if self.target == "any"
            || !valid_target(&self.target)
            || version.to_string() != self.use_version
        {
            return Err(lock_error(
                "The package-lock host target or A3S Use version is invalid.",
            ));
        }
        Ok(())
    }

    pub(crate) fn parsed_use_version(&self) -> UseResult<Version> {
        self.validate()?;
        Version::parse(&self.use_version)
            .map_err(|_| lock_error("The package-lock host version is invalid."))
    }
}

impl LockedPluginPackage {
    pub fn package_id(&self) -> &str {
        &self.catalog.record.package_id
    }

    pub fn version(&self) -> &str {
        &self.catalog.record.version
    }
}

impl PluginPackageLock {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin package lock",
            PACKAGE_LOCK_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_PACKAGE_LOCK_SCHEMA
            || !valid_package_id(&self.root_package_id)
            || self.packages.is_empty()
            || self.packages.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(lock_error(
                "The cognitive-package lock identity or package bound is invalid.",
            ));
        }
        self.host.validate()?;
        let host_version = self.host.parsed_use_version()?;
        let mut by_id = BTreeMap::new();
        let mut previous = None;
        for package in &self.packages {
            package.catalog.validate().map_err(|_| {
                lock_error("A package-lock node has invalid verified catalog evidence.")
            })?;
            let record = &package.catalog.record;
            if record.schema != PLUGIN_CATALOG_SCHEMA_V3
                || matches!(record.availability, CatalogAvailability::Withdrawn { .. })
                || (record.target != "any" && record.target != self.host.target)
                || !VersionReq::parse(&record.requires_use)
                    .map(|requirement| requirement.matches(&host_version))
                    .unwrap_or(false)
                || previous
                    .as_ref()
                    .is_some_and(|package_id| package_id >= &record.package_id)
            {
                return Err(lock_error(
                    "Package-lock nodes must be compatible catalog-v3 releases sorted uniquely by package ID.",
                ));
            }
            if by_id.insert(record.package_id.clone(), package).is_some() {
                return Err(lock_error("A package-lock node appears more than once."));
            }
            previous = Some(record.package_id.clone());
        }
        if !by_id.contains_key(&self.root_package_id) {
            return Err(lock_error(
                "The package-lock root is absent from the selected closure.",
            ));
        }

        for package in &self.packages {
            let signed = &package.catalog.record.dependencies;
            if package.dependencies.len() != signed.len()
                || package
                    .dependencies
                    .windows(2)
                    .any(|pair| pair[0].package_id >= pair[1].package_id)
            {
                return Err(lock_error(
                    "Locked dependency edges do not match the signed dependency inventory.",
                ));
            }
            for (edge, requirement) in package.dependencies.iter().zip(signed) {
                let Some(selected) = by_id.get(&edge.package_id) else {
                    return Err(lock_error(
                        "A locked dependency is absent from the selected closure.",
                    ));
                };
                let selected_version = Version::parse(selected.version()).map_err(|_| {
                    lock_error("A selected package version is invalid semantic versioning.")
                })?;
                let parsed_requirement =
                    VersionReq::parse(&edge.version_requirement).map_err(|_| {
                        lock_error(
                            "A locked dependency requirement is invalid semantic versioning.",
                        )
                    })?;
                if edge.package_id != requirement.package_id
                    || edge.version_requirement != requirement.version_requirement
                    || edge.version != selected.version()
                    || selected_version.to_string() != edge.version
                    || parsed_requirement.to_string() != edge.version_requirement
                    || !parsed_requirement.matches(&selected_version)
                {
                    return Err(lock_error(
                        "A locked dependency version does not satisfy its signed requirement.",
                    ));
                }
            }
        }

        let reachable = self.reachable_packages(&by_id)?;
        if reachable.len() != self.packages.len() {
            return Err(lock_error(
                "The package lock contains nodes outside the root dependency closure.",
            ));
        }
        self.topological_indices(&by_id).map(drop)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin package lock", PACKAGE_LOCK_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn package(&self, package_id: &str) -> Option<&LockedPluginPackage> {
        self.packages
            .iter()
            .find(|package| package.package_id() == package_id)
    }

    /// Verify that a reviewed operation plan describes this exact resolved
    /// closure. Surface selection may narrow activation, but package version,
    /// content, manifest, archive, and Registry/TUF provenance cannot drift.
    pub fn validate_for_plan(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        self.validate()?;
        plan.validate()?;
        if plan.package_id != self.root_package_id || plan.packages.len() != self.packages.len() {
            return Err(lock_error(
                "The operation plan does not cover the exact package-lock closure.",
            ));
        }
        let transitions = plan
            .packages
            .iter()
            .map(|transition| (transition.package_id.as_str(), transition))
            .collect::<BTreeMap<_, _>>();
        for locked in &self.packages {
            let transition = transitions
                .get(locked.package_id())
                .ok_or_else(|| lock_error("A locked package is absent from the operation plan."))?;
            let expected_role = if locked.package_id() == self.root_package_id {
                PlanPackageRole::Root
            } else {
                PlanPackageRole::Dependency
            };
            if transition.role != expected_role {
                return Err(lock_error(
                    "A planned package role does not match the resolved dependency graph.",
                ));
            }
            validate_locked_transition(locked, transition)?;
        }
        Ok(())
    }

    /// Verify one immutable upgrade plan against the union of its exact prior
    /// and candidate dependency locks. Candidate-only nodes are additions,
    /// prior-only nodes are removals, and shared nodes are either exact
    /// retentions or replacements. This keeps the existing single-lock plan
    /// validation strict while making graph garbage collection reviewable.
    pub fn validate_upgrade_plan(
        prior: &Self,
        candidate: &Self,
        plan: &PluginOperationPlan,
    ) -> UseResult<()> {
        prior.validate()?;
        candidate.validate()?;
        plan.validate()?;
        if plan.action != super::PluginOperationAction::Upgrade
            || plan.package_id != candidate.root_package_id
            || prior.root_package_id != candidate.root_package_id
            || prior.host != candidate.host
        {
            return Err(lock_error(
                "The upgrade plan and package locks belong to different actions, roots, or hosts.",
            ));
        }

        let package_ids = prior
            .packages
            .iter()
            .chain(&candidate.packages)
            .map(|package| package.package_id())
            .collect::<BTreeSet<_>>();
        if package_ids.len() != plan.packages.len() {
            return Err(lock_error(
                "The upgrade plan does not cover the exact prior/candidate package-lock union.",
            ));
        }
        let transitions = plan
            .packages
            .iter()
            .map(|transition| (transition.package_id.as_str(), transition))
            .collect::<BTreeMap<_, _>>();
        if transitions.len() != plan.packages.len() {
            return Err(lock_error(
                "An upgrade package transition appears more than once.",
            ));
        }

        for package_id in package_ids {
            let transition = transitions.get(package_id).ok_or_else(|| {
                lock_error("A prior or candidate package is absent from the upgrade plan.")
            })?;
            let expected_role = if package_id == candidate.root_package_id {
                PlanPackageRole::Root
            } else {
                PlanPackageRole::Dependency
            };
            if transition.role != expected_role {
                return Err(lock_error(
                    "An upgrade package role does not match its resolved dependency graph.",
                ));
            }

            match (prior.package(package_id), candidate.package(package_id)) {
                (None, Some(after)) if transition.change == PlanPackageChangeKind::Add => {
                    validate_candidate_transition(after, transition)?;
                }
                (None, Some(after)) if transition.change == PlanPackageChangeKind::Retain => {
                    validate_prior_state(after, transition)?;
                    validate_candidate_state(after, transition)?;
                }
                (Some(before), None) if transition.change == PlanPackageChangeKind::Remove => {
                    validate_prior_state(before, transition)?;
                }
                (Some(before), None) if transition.change == PlanPackageChangeKind::Retain => {
                    validate_prior_state(before, transition)?;
                    validate_candidate_state(before, transition)?;
                }
                (Some(before), Some(after))
                    if before.catalog == after.catalog
                        && transition.change == PlanPackageChangeKind::Retain =>
                {
                    validate_prior_state(before, transition)?;
                    validate_candidate_state(after, transition)?;
                }
                (Some(before), Some(after))
                    if before.catalog != after.catalog
                        && transition.change == PlanPackageChangeKind::Replace =>
                {
                    validate_prior_state(before, transition)?;
                    validate_candidate_transition(after, transition)?;
                }
                _ => return Err(lock_error(
                    "An upgrade transition does not match the exact prior/candidate lock delta.",
                )),
            }
        }
        Ok(())
    }

    pub fn install_order(&self) -> UseResult<Vec<&LockedPluginPackage>> {
        self.validate()?;
        let by_id = self
            .packages
            .iter()
            .map(|package| (package.package_id().to_string(), package))
            .collect::<BTreeMap<_, _>>();
        self.topological_indices(&by_id).map(|indices| {
            indices
                .into_iter()
                .map(|index| &self.packages[index])
                .collect()
        })
    }

    pub fn removal_order(&self) -> UseResult<Vec<&LockedPluginPackage>> {
        let mut order = self.install_order()?;
        order.reverse();
        Ok(order)
    }

    pub(super) fn from_selected(
        root_package_id: String,
        host: PluginPackageLockHost,
        selected: BTreeMap<String, VerifiedPluginCatalogRecord>,
    ) -> UseResult<Self> {
        let packages = selected
            .values()
            .map(|catalog| {
                let dependencies = catalog
                    .record
                    .dependencies
                    .iter()
                    .map(|dependency| {
                        let version = selected
                            .get(&dependency.package_id)
                            .map(|record| record.record.version.clone())
                            .ok_or_else(|| {
                                lock_error(
                                    "A resolved dependency is absent while constructing the lock.",
                                )
                            })?;
                        Ok(LockedPluginPackageDependency {
                            package_id: dependency.package_id.clone(),
                            version_requirement: dependency.version_requirement.clone(),
                            version,
                        })
                    })
                    .collect::<UseResult<Vec<_>>>()?;
                Ok(LockedPluginPackage {
                    catalog: catalog.clone(),
                    dependencies,
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        let lock = Self {
            schema: PLUGIN_PACKAGE_LOCK_SCHEMA.to_string(),
            root_package_id,
            host,
            packages,
        };
        lock.validate()?;
        Ok(lock)
    }

    fn reachable_packages(
        &self,
        by_id: &BTreeMap<String, &LockedPluginPackage>,
    ) -> UseResult<BTreeSet<String>> {
        let mut reachable = BTreeSet::from([self.root_package_id.clone()]);
        let mut pending = vec![self.root_package_id.clone()];
        while let Some(package_id) = pending.pop() {
            let package = by_id.get(&package_id).ok_or_else(|| {
                lock_error("A reachable package disappeared from the lock graph.")
            })?;
            for dependency in &package.dependencies {
                if reachable.insert(dependency.package_id.clone()) {
                    pending.push(dependency.package_id.clone());
                }
            }
        }
        Ok(reachable)
    }

    fn topological_indices(
        &self,
        by_id: &BTreeMap<String, &LockedPluginPackage>,
    ) -> UseResult<Vec<usize>> {
        let indices = self
            .packages
            .iter()
            .enumerate()
            .map(|(index, package)| (package.package_id(), index))
            .collect::<BTreeMap<_, _>>();
        let mut completed = BTreeSet::new();
        let mut order = Vec::with_capacity(self.packages.len());
        while order.len() < self.packages.len() {
            let ready = by_id
                .iter()
                .filter(|(package_id, package)| {
                    !completed.contains(*package_id)
                        && package
                            .dependencies
                            .iter()
                            .all(|dependency| completed.contains(&dependency.package_id))
                })
                .map(|(package_id, _)| package_id.clone())
                .collect::<Vec<_>>();
            if ready.is_empty() {
                return Err(lock_error(
                    "The cognitive-package lock graph contains a dependency cycle.",
                ));
            }
            for package_id in ready {
                completed.insert(package_id.clone());
                order.push(
                    *indices.get(package_id.as_str()).ok_or_else(|| {
                        lock_error("A package-lock topological index is missing.")
                    })?,
                );
            }
        }
        Ok(order)
    }
}

fn validate_locked_transition(
    locked: &LockedPluginPackage,
    transition: &PlannedPackageTransition,
) -> UseResult<()> {
    let state = match transition.change {
        PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace => transition.after.as_ref(),
        PlanPackageChangeKind::Remove | PlanPackageChangeKind::Retain => transition.before.as_ref(),
    }
    .ok_or_else(|| lock_error("A locked package transition omitted its selected package state."))?;
    validate_locked_state(locked, state)?;

    match transition.change {
        PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace => {
            let expected = PluginPlanSource::Registry {
                provenance: locked.catalog.provenance.clone(),
                archive: locked.catalog.record.archive.clone(),
            };
            if transition.source.as_ref() != Some(&expected) {
                return Err(lock_error(
                    "A planned package source does not match its locked Registry and TUF evidence.",
                ));
            }
        }
        PlanPackageChangeKind::Remove | PlanPackageChangeKind::Retain => {
            if transition.source.is_some() {
                return Err(lock_error(
                    "A remove or retained locked package must not select a new source.",
                ));
            }
        }
    }
    Ok(())
}

fn validate_prior_state(
    locked: &LockedPluginPackage,
    transition: &PlannedPackageTransition,
) -> UseResult<()> {
    let state = transition.before.as_ref().ok_or_else(|| {
        lock_error("A prior locked package transition omitted its selected package state.")
    })?;
    validate_locked_state(locked, state)
}

fn validate_candidate_state(
    locked: &LockedPluginPackage,
    transition: &PlannedPackageTransition,
) -> UseResult<()> {
    let state = transition.after.as_ref().ok_or_else(|| {
        lock_error("A candidate locked package transition omitted its selected package state.")
    })?;
    validate_locked_state(locked, state)
}

fn validate_candidate_transition(
    locked: &LockedPluginPackage,
    transition: &PlannedPackageTransition,
) -> UseResult<()> {
    validate_candidate_state(locked, transition)?;
    let expected = PluginPlanSource::Registry {
        provenance: locked.catalog.provenance.clone(),
        archive: locked.catalog.record.archive.clone(),
    };
    if transition.source.as_ref() != Some(&expected) {
        return Err(lock_error(
            "A candidate package source does not match its locked Registry and TUF evidence.",
        ));
    }
    Ok(())
}

fn validate_locked_state(
    locked: &LockedPluginPackage,
    state: &PlannedPackageState,
) -> UseResult<()> {
    let selected_surfaces = state
        .release
        .surfaces
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    let expected = locked
        .catalog
        .selected_state(&selected_surfaces)
        .map_err(|_| {
            lock_error("A locked catalog record cannot reconstruct the planned package state.")
        })?;
    if expected != *state {
        return Err(lock_error(
            "A planned package version, digest, target, surface, or permission set drifted from its lock.",
        ));
    }
    Ok(())
}

fn lock_error(message: impl Into<String>) -> UseError {
    UseError::new(PACKAGE_LOCK_ERROR, message)
}
