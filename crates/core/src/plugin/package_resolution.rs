use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};

use crate::{UseError, UseResult};

use super::{
    CatalogAvailability, PluginPackageLock, PluginPackageLockHost, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_CATALOG_SCHEMA_V3,
};

pub const MAX_PLUGIN_RESOLUTION_CANDIDATES: usize = 4_096;
const MAX_PLUGIN_RESOLUTION_ATTEMPTS: usize = 65_536;

/// Bounded deterministic resolver for one exact verified root and a host-owned
/// union of verified registry candidates.
#[derive(Debug, Clone)]
pub struct PluginPackageResolver {
    host: PluginPackageLockHost,
}

#[derive(Debug, Clone)]
struct Constraint {
    required_by: String,
    requirement: VersionReq,
}

#[derive(Debug, Clone)]
enum ResolutionFailure {
    Missing(String),
    Incompatible(String),
    Conflict(String),
    Cycle,
    Limit,
}

struct ResolutionSearch<'a> {
    host: &'a PluginPackageLockHost,
    candidates: &'a BTreeMap<String, Vec<VerifiedPluginCatalogRecord>>,
    attempts: usize,
}

impl PluginPackageResolver {
    pub fn new(host: PluginPackageLockHost) -> Self {
        Self { host }
    }

    pub fn resolve(
        &self,
        root: VerifiedPluginCatalogRecord,
        candidates: Vec<VerifiedPluginCatalogRecord>,
    ) -> UseResult<PluginPackageLock> {
        self.host.validate()?;
        validate_candidate(&root)?;
        if !is_host_compatible(&root, &self.host)? {
            return Err(resolution_error(
                "use.plugin.package_dependency_incompatible",
                format!(
                    "Root cognitive package '{}' is incompatible with the selected host.",
                    root.record.package_id
                ),
            ));
        }
        if matches!(
            root.record.availability,
            CatalogAvailability::Withdrawn { .. }
        ) {
            return Err(resolution_error(
                "use.plugin.package_dependency_incompatible",
                "A withdrawn cognitive package cannot be selected as the resolution root.",
            ));
        }
        if candidates.len() > MAX_PLUGIN_RESOLUTION_CANDIDATES {
            return Err(resolution_error(
                "use.plugin.package_resolution_limit",
                "The cognitive-package candidate set exceeds its deterministic resolution bound.",
            ));
        }

        let mut by_package = BTreeMap::<String, Vec<VerifiedPluginCatalogRecord>>::new();
        let mut candidate_digests = BTreeSet::new();
        for candidate in candidates {
            validate_candidate(&candidate)?;
            let digest = candidate.descriptor_digest()?;
            if !candidate_digests.insert(digest) {
                return Err(resolution_error(
                    "use.plugin.package_resolution_invalid",
                    "The cognitive-package candidate set contains a duplicate verified record.",
                ));
            }
            by_package
                .entry(candidate.record.package_id.clone())
                .or_default()
                .push(candidate);
        }
        for records in by_package.values_mut() {
            records.sort_by(compare_candidates);
        }

        let root_package_id = root.record.package_id.clone();
        let mut selected = BTreeMap::from([(root_package_id.clone(), root)]);
        let mut search = ResolutionSearch {
            host: &self.host,
            candidates: &by_package,
            attempts: 0,
        };
        let selected = search
            .resolve(&mut selected)
            .map_err(ResolutionFailure::into_error)?;
        PluginPackageLock::from_selected(root_package_id, self.host.clone(), selected)
    }
}

impl ResolutionSearch<'_> {
    fn resolve(
        &mut self,
        selected: &mut BTreeMap<String, VerifiedPluginCatalogRecord>,
    ) -> Result<BTreeMap<String, VerifiedPluginCatalogRecord>, ResolutionFailure> {
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts > MAX_PLUGIN_RESOLUTION_ATTEMPTS || selected.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(ResolutionFailure::Limit);
        }
        let constraints = collect_constraints(selected)?;
        validate_selected_versions(selected, &constraints)?;
        if selected_graph_has_cycle(selected) {
            return Err(ResolutionFailure::Cycle);
        }

        let unresolved = constraints
            .keys()
            .find(|package_id| !selected.contains_key(*package_id))
            .cloned();
        let Some(package_id) = unresolved else {
            return Ok(selected.clone());
        };
        let requirements = constraints
            .get(&package_id)
            .ok_or_else(|| ResolutionFailure::Missing(package_id.clone()))?;
        let candidates = self.compatible_candidates(&package_id)?;
        let matching = candidates
            .into_iter()
            .filter(|candidate| {
                Version::parse(&candidate.record.version).is_ok_and(|version| {
                    requirements
                        .iter()
                        .all(|constraint| constraint.requirement.matches(&version))
                })
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(ResolutionFailure::Conflict(package_id));
        }

        let mut first_failure = None;
        for candidate in matching {
            selected.insert(package_id.clone(), candidate.clone());
            match self.resolve(selected) {
                Ok(result) => return Ok(result),
                Err(ResolutionFailure::Limit) => {
                    selected.remove(&package_id);
                    return Err(ResolutionFailure::Limit);
                }
                Err(failure) => {
                    first_failure.get_or_insert(failure);
                }
            }
            selected.remove(&package_id);
        }
        Err(first_failure.unwrap_or(ResolutionFailure::Conflict(package_id)))
    }

    fn compatible_candidates(
        &self,
        package_id: &str,
    ) -> Result<Vec<VerifiedPluginCatalogRecord>, ResolutionFailure> {
        let Some(records) = self.candidates.get(package_id) else {
            return Err(ResolutionFailure::Missing(package_id.to_string()));
        };
        let registries = records
            .iter()
            .map(|record| registry_identity(&record.provenance))
            .collect::<BTreeSet<_>>();
        if registries.len() > 1 {
            return Err(ResolutionFailure::Incompatible(format!(
                "registry-ambiguous:{package_id}"
            )));
        }
        let compatible = records
            .iter()
            .filter(|record| {
                !matches!(
                    record.record.availability,
                    CatalogAvailability::Withdrawn { .. }
                ) && is_host_compatible(record, self.host).unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        if compatible.is_empty() {
            return Err(ResolutionFailure::Incompatible(package_id.to_string()));
        }
        Ok(compatible)
    }
}

impl ResolutionFailure {
    fn into_error(self) -> UseError {
        match self {
            Self::Missing(package_id) => resolution_error(
                "use.plugin.package_dependency_missing",
                format!("No verified candidate exists for required cognitive package '{package_id}'."),
            ),
            Self::Incompatible(value) if value.starts_with("registry-ambiguous:") => {
                let package_id = value.trim_start_matches("registry-ambiguous:");
                resolution_error(
                    "use.plugin.package_registry_ambiguous",
                    format!(
                        "Required cognitive package '{package_id}' is present in more than one registry."
                    ),
                )
            }
            Self::Incompatible(package_id) => resolution_error(
                "use.plugin.package_dependency_incompatible",
                format!(
                    "No verified release of cognitive package '{package_id}' is compatible with the selected host."
                ),
            ),
            Self::Conflict(package_id) => resolution_error(
                "use.plugin.package_dependency_conflict",
                format!(
                    "No version of cognitive package '{package_id}' satisfies the complete dependency constraint set."
                ),
            ),
            Self::Cycle => resolution_error(
                "use.plugin.package_dependency_cycle",
                "The cognitive-package dependency graph contains a cycle.",
            ),
            Self::Limit => resolution_error(
                "use.plugin.package_resolution_limit",
                "Cognitive-package resolution exceeded its package or search-attempt bound.",
            ),
        }
    }
}

fn validate_candidate(candidate: &VerifiedPluginCatalogRecord) -> UseResult<()> {
    candidate.validate().map_err(|_| {
        resolution_error(
            "use.plugin.package_resolution_invalid",
            "A cognitive-package candidate has invalid verified catalog evidence.",
        )
    })?;
    if candidate.record.schema != PLUGIN_CATALOG_SCHEMA_V3 {
        return Err(resolution_error(
            "use.plugin.package_resolution_invalid",
            "Package dependency resolution accepts only complete catalog-v3 records.",
        ));
    }
    Ok(())
}

fn is_host_compatible(
    candidate: &VerifiedPluginCatalogRecord,
    host: &PluginPackageLockHost,
) -> UseResult<bool> {
    let requirement = VersionReq::parse(&candidate.record.requires_use).map_err(|_| {
        resolution_error(
            "use.plugin.package_resolution_invalid",
            "A package candidate has an invalid A3S Use compatibility requirement.",
        )
    })?;
    Ok(
        (candidate.record.target == "any" || candidate.record.target == host.target)
            && requirement.matches(&host.parsed_use_version()?),
    )
}

fn collect_constraints(
    selected: &BTreeMap<String, VerifiedPluginCatalogRecord>,
) -> Result<BTreeMap<String, Vec<Constraint>>, ResolutionFailure> {
    let mut constraints = BTreeMap::<String, Vec<Constraint>>::new();
    for (package_id, record) in selected {
        for dependency in &record.record.dependencies {
            let requirement = dependency
                .parsed_requirement()
                .map_err(|_| ResolutionFailure::Conflict(dependency.package_id.clone()))?;
            constraints
                .entry(dependency.package_id.clone())
                .or_default()
                .push(Constraint {
                    required_by: package_id.clone(),
                    requirement,
                });
        }
    }
    if constraints.len().saturating_add(1) > MAX_PLUGIN_PLAN_ITEMS {
        return Err(ResolutionFailure::Limit);
    }
    for values in constraints.values_mut() {
        values.sort_by(|left, right| left.required_by.cmp(&right.required_by));
    }
    Ok(constraints)
}

fn validate_selected_versions(
    selected: &BTreeMap<String, VerifiedPluginCatalogRecord>,
    constraints: &BTreeMap<String, Vec<Constraint>>,
) -> Result<(), ResolutionFailure> {
    for (package_id, requirements) in constraints {
        let Some(record) = selected.get(package_id) else {
            continue;
        };
        let version = Version::parse(&record.record.version)
            .map_err(|_| ResolutionFailure::Conflict(package_id.clone()))?;
        if requirements
            .iter()
            .any(|constraint| !constraint.requirement.matches(&version))
        {
            return Err(ResolutionFailure::Conflict(package_id.clone()));
        }
    }
    Ok(())
}

fn selected_graph_has_cycle(selected: &BTreeMap<String, VerifiedPluginCatalogRecord>) -> bool {
    fn visit(
        package_id: &str,
        selected: &BTreeMap<String, VerifiedPluginCatalogRecord>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> bool {
        if visited.contains(package_id) {
            return false;
        }
        if !visiting.insert(package_id.to_string()) {
            return true;
        }
        let cycle = selected.get(package_id).is_some_and(|record| {
            record.record.dependencies.iter().any(|dependency| {
                selected.contains_key(&dependency.package_id)
                    && visit(&dependency.package_id, selected, visiting, visited)
            })
        });
        visiting.remove(package_id);
        visited.insert(package_id.to_string());
        cycle
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    selected
        .keys()
        .any(|package_id| visit(package_id, selected, &mut visiting, &mut visited))
}

fn compare_candidates(
    left: &VerifiedPluginCatalogRecord,
    right: &VerifiedPluginCatalogRecord,
) -> Ordering {
    let left_version = Version::parse(&left.record.version).ok();
    let right_version = Version::parse(&right.record.version).ok();
    right_version
        .cmp(&left_version)
        .then_with(|| channel_rank(left).cmp(&channel_rank(right)))
        .then_with(|| target_rank(left).cmp(&target_rank(right)))
        .then_with(|| {
            left.provenance
                .catalog_record_digest
                .cmp(&right.provenance.catalog_record_digest)
        })
}

fn channel_rank(record: &VerifiedPluginCatalogRecord) -> u8 {
    match record.record.channel {
        super::PluginReleaseChannel::Stable => 0,
        super::PluginReleaseChannel::Beta => 1,
        super::PluginReleaseChannel::Nightly => 2,
    }
}

fn target_rank(record: &VerifiedPluginCatalogRecord) -> u8 {
    u8::from(record.record.target == "any")
}

fn registry_identity(provenance: &VerifiedCatalogProvenance) -> (String, String, String) {
    (
        provenance.registry_name.clone(),
        provenance.registry_url.clone(),
        provenance.root_sha256.clone(),
    )
}

fn resolution_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
