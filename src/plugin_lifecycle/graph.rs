use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{
    PlanPackageChangeKind, PlannedPackageTransition, PluginOperationAction,
    PluginOperationPlanEnvelope, PluginPackageId, PluginPackageLock, UseError, UseResult,
};
use a3s_use_extension::ExtensionManifest;
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::{
    PluginLifecycleAction, PluginLifecycleCoordinator, PluginLifecycleEvidence,
    PluginLifecycleIntent, PluginLifecycleOperationRecord,
};

/// One package-specific coordinator, intent, and admitted manifest belonging
/// to a single reviewed dependency-closure operation.
#[derive(Clone)]
pub struct PluginPackageLifecycleUnit {
    coordinator: PluginLifecycleCoordinator,
    intent: PluginLifecycleIntent,
    manifest: ExtensionManifest,
}

impl PluginPackageLifecycleUnit {
    pub fn new(
        coordinator: PluginLifecycleCoordinator,
        intent: PluginLifecycleIntent,
        manifest: ExtensionManifest,
    ) -> UseResult<Self> {
        intent.validate()?;
        if intent.package_id != manifest.package_id {
            return Err(graph_error(
                "A lifecycle unit manifest does not match its package intent.",
            ));
        }
        Ok(Self {
            coordinator,
            intent,
            manifest,
        })
    }

    pub fn intent(&self) -> &PluginLifecycleIntent {
        &self.intent
    }

    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    pub(crate) fn coordinator(&self) -> &PluginLifecycleCoordinator {
        &self.coordinator
    }
}

/// Exact package-keyed evidence returned by one atomic capability cutover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackagePublicationEvidence {
    package_id: String,
    evidence: PluginLifecycleEvidence,
}

/// Exact package-keyed evidence proving that an unpublished candidate was
/// discarded and, for replacements, the prior generation was restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackageRollbackEvidence {
    package_id: String,
    evidence: PluginLifecycleEvidence,
}

impl PluginPackageRollbackEvidence {
    pub fn new(
        package_id: impl Into<String>,
        evidence: PluginLifecycleEvidence,
    ) -> UseResult<Self> {
        let package_id = package_id.into();
        PluginPackageId::parse(package_id.clone()).map_err(|_| {
            graph_error("Package rollback evidence has an invalid package identity.")
        })?;
        Ok(Self {
            package_id,
            evidence,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn evidence(&self) -> &PluginLifecycleEvidence {
        &self.evidence
    }
}

impl PluginPackagePublicationEvidence {
    pub fn new(
        package_id: impl Into<String>,
        evidence: PluginLifecycleEvidence,
    ) -> UseResult<Self> {
        let package_id = package_id.into();
        PluginPackageId::parse(package_id.clone()).map_err(|_| {
            graph_error("Package publication evidence has an invalid package identity.")
        })?;
        Ok(Self {
            package_id,
            evidence,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn evidence(&self) -> &PluginLifecycleEvidence {
        &self.evidence
    }
}

/// Host-owned atomic publication boundary for a prepared package closure.
#[async_trait]
pub trait PluginGraphCapabilityLifecycleHost: Send + Sync {
    async fn publish_capabilities(
        &self,
        package_lock: &PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<Vec<PluginPackagePublicationEvidence>>;

    /// Discard a bounded set of candidates while the exact prior graph is
    /// still the Registry snapshot commit point. `prior_intents` contains one
    /// exact prior generation for every replacement and none for additions.
    async fn rollback_candidates(
        &self,
        candidate_lock: &PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        prior_intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<Vec<PluginPackageRollbackEvidence>>;
}

/// Coordinates the package graph above each package's existing surface saga.
/// Dependencies are committed and prepared first, no capability is visible
/// while preparation is incomplete, and one host cutover publishes the full
/// closure. Cascade uninstall runs the exact reverse order.
#[derive(Clone)]
pub struct PluginPackageGraphLifecycleCoordinator {
    publication: Arc<dyn PluginGraphCapabilityLifecycleHost>,
}

impl PluginPackageGraphLifecycleCoordinator {
    pub fn new(publication: Arc<dyn PluginGraphCapabilityLifecycleHost>) -> Self {
        Self { publication }
    }

    pub async fn apply_install(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let lock = validate_graph(envelope, units, PluginOperationAction::Install)?;
        let units = units_by_package(units)?;
        let mut ordered = Vec::with_capacity(units.len());
        for package in lock.install_order()? {
            let transition = transition_for(envelope, package.package_id())?;
            if transition.change == PlanPackageChangeKind::Retain {
                continue;
            }
            if transition.change != PlanPackageChangeKind::Add {
                return Err(graph_error(
                    "Package-graph install supports only added or retained dependency generations.",
                ));
            }
            let unit = *units
                .get(package.package_id())
                .ok_or_else(|| graph_error("A locked dependency has no package lifecycle unit."))?;
            validate_unit(
                envelope,
                unit,
                package.package_id(),
                PluginLifecycleAction::Install,
            )?;
            unit.coordinator
                .prepare_for_graph(&unit.intent, &unit.manifest, &completed_at_ms)
                .await?;
            ordered.push(unit);
        }

        let intents = ordered
            .iter()
            .map(|unit| unit.intent.clone())
            .collect::<Vec<_>>();
        let evidence = self
            .publication
            .publish_capabilities(lock, &intents, &publication_key(envelope)?)
            .await?;
        if evidence.len() != ordered.len() {
            return Err(graph_error(
                "Package-graph publication omitted capability evidence.",
            ));
        }

        let mut records = Vec::with_capacity(ordered.len());
        for (unit, evidence) in ordered.into_iter().zip(evidence) {
            if evidence.package_id != unit.intent.package_id {
                return Err(graph_error(
                    "Package-graph publication evidence changed package order or identity.",
                ));
            }
            records.push(
                unit.coordinator
                    .complete_graph_publication(
                        &unit.intent,
                        &unit.manifest,
                        &evidence.evidence,
                        &completed_at_ms,
                    )
                    .await?,
            );
        }
        Ok(records)
    }

    pub async fn apply_uninstall(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let lock = validate_graph(envelope, units, PluginOperationAction::Uninstall)?;
        let units = units_by_package(units)?;
        let mut records = Vec::with_capacity(units.len());
        for package in lock.removal_order()? {
            let transition = transition_for(envelope, package.package_id())?;
            if transition.change == PlanPackageChangeKind::Retain {
                continue;
            }
            if transition.change != PlanPackageChangeKind::Remove {
                return Err(graph_error(
                    "Package-graph uninstall supports only removed or retained dependency generations.",
                ));
            }
            let unit = *units.get(package.package_id()).ok_or_else(|| {
                graph_error("A locked dependency has no uninstall lifecycle unit.")
            })?;
            validate_unit(
                envelope,
                unit,
                package.package_id(),
                PluginLifecycleAction::Uninstall,
            )?;
            records.push(
                unit.coordinator
                    .apply(&unit.intent, &unit.manifest, &completed_at_ms)
                    .await?,
            );
        }
        Ok(records)
    }

    /// Prepare every added or replaced package generation in dependency order,
    /// atomically publish the candidate closure, and only then retire replaced
    /// generations in the prior graph's reverse dependency order.
    ///
    /// The prior lock is required because the candidate lock cannot prove the
    /// dependency ordering or immutable state of the generations being
    /// retired. A failed candidate preparation returns before publication, so
    /// every prior generation remains the Registry snapshot commit point.
    pub async fn apply_upgrade(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        prior_lock: &PluginPackageLock,
        candidate_units: &[PluginPackageLifecycleUnit],
        retirement_units: &[PluginPackageLifecycleUnit],
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let candidate_lock =
            validate_upgrade_graph(envelope, prior_lock, candidate_units, retirement_units)?;
        let candidates = units_by_package(candidate_units)?;
        let retirements = units_by_package(retirement_units)?;
        let mut ordered_candidates = Vec::with_capacity(candidates.len());

        let mut interrupted_rollback = Vec::new();
        let mut saw_rolling_back = false;
        let mut saw_rolled_back = false;
        for package in candidate_lock.install_order()? {
            let Some(unit) = candidates.get(package.package_id()).copied() else {
                continue;
            };
            let status = unit
                .coordinator
                .graph_candidate_status(&unit.intent)
                .await?;
            match status {
                Some(super::PluginLifecycleOperationStatus::RollingBack) => {
                    saw_rolling_back = true;
                    interrupted_rollback.push(unit);
                }
                Some(super::PluginLifecycleOperationStatus::RolledBack) => {
                    saw_rolled_back = true;
                    interrupted_rollback.push(unit);
                }
                Some(super::PluginLifecycleOperationStatus::Applying) => {
                    interrupted_rollback.push(unit);
                }
                _ => {}
            }
        }
        if saw_rolling_back {
            let replay_error = UseError::new(
                "use.plugin.package_graph_upgrade_rolled_back",
                "The interrupted candidate rollback was completed; create and review a fresh upgrade plan.",
            );
            return match self
                .rollback_upgrade_candidates(
                    candidate_lock,
                    &interrupted_rollback,
                    &retirements,
                    &completed_at_ms,
                )
                .await
            {
                Ok(()) => Err(replay_error),
                Err(rollback) => Err(attach_rollback_error(replay_error, rollback)),
            };
        }
        if saw_rolled_back {
            return Err(UseError::new(
                "use.plugin.package_graph_upgrade_rolled_back",
                "This candidate graph was rolled back; create and review a fresh upgrade plan.",
            ));
        }

        for package in candidate_lock.install_order()? {
            let transition = transition_for(envelope, package.package_id())?;
            let action = match transition.change {
                PlanPackageChangeKind::Add => PluginLifecycleAction::Install,
                PlanPackageChangeKind::Replace => PluginLifecycleAction::Upgrade,
                PlanPackageChangeKind::Retain => continue,
                PlanPackageChangeKind::Remove => {
                    return Err(graph_error(
                        "A removed package cannot appear in the candidate dependency lock.",
                    ))
                }
            };
            let unit = *candidates.get(package.package_id()).ok_or_else(|| {
                graph_error("A changed candidate dependency has no package lifecycle unit.")
            })?;
            validate_unit(envelope, unit, package.package_id(), action)?;
            ordered_candidates.push(unit);
            if let Err(error) = unit
                .coordinator
                .prepare_for_graph(&unit.intent, &unit.manifest, &completed_at_ms)
                .await
            {
                return match self
                    .rollback_upgrade_candidates(
                        candidate_lock,
                        &ordered_candidates,
                        &retirements,
                        &completed_at_ms,
                    )
                    .await
                {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(attach_rollback_error(error, rollback)),
                };
            }
        }

        let intents = ordered_candidates
            .iter()
            .map(|unit| unit.intent.clone())
            .collect::<Vec<_>>();
        let evidence = match self
            .publication
            .publish_capabilities(candidate_lock, &intents, &publication_key(envelope)?)
            .await
        {
            Ok(evidence) => evidence,
            Err(error) => {
                return match self
                    .rollback_upgrade_candidates(
                        candidate_lock,
                        &ordered_candidates,
                        &retirements,
                        &completed_at_ms,
                    )
                    .await
                {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(attach_rollback_error(error, rollback)),
                };
            }
        };
        if evidence.len() != ordered_candidates.len() {
            return Err(graph_error(
                "Package-graph upgrade publication omitted candidate capability evidence.",
            ));
        }

        let mut records = Vec::with_capacity(candidate_units.len() + retirement_units.len());
        for (unit, evidence) in ordered_candidates.into_iter().zip(evidence) {
            if evidence.package_id != unit.intent.package_id {
                return Err(graph_error(
                    "Package-graph upgrade evidence changed candidate order or identity.",
                ));
            }
            records.push(
                unit.coordinator
                    .complete_graph_publication(
                        &unit.intent,
                        &unit.manifest,
                        &evidence.evidence,
                        &completed_at_ms,
                    )
                    .await?,
            );
        }

        for package in prior_lock.removal_order()? {
            let Some(transition) = envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
            else {
                continue;
            };
            if transition.change != PlanPackageChangeKind::Replace {
                continue;
            }
            let unit = *retirements.get(package.package_id()).ok_or_else(|| {
                graph_error("A replaced dependency has no prior-generation retirement unit.")
            })?;
            validate_unit(
                envelope,
                unit,
                package.package_id(),
                PluginLifecycleAction::Uninstall,
            )?;
            records.push(
                unit.coordinator
                    .apply(&unit.intent, &unit.manifest, &completed_at_ms)
                    .await?,
            );
        }
        Ok(records)
    }

    async fn rollback_upgrade_candidates(
        &self,
        candidate_lock: &PluginPackageLock,
        candidates: &[&PluginPackageLifecycleUnit],
        retirements: &BTreeMap<&str, &PluginPackageLifecycleUnit>,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<()> {
        for unit in candidates {
            let status = unit
                .coordinator
                .graph_candidate_status(&unit.intent)
                .await?;
            if status == Some(super::PluginLifecycleOperationStatus::Applying) {
                unit.coordinator
                    .start_graph_rollback(&unit.intent, &unit.manifest)
                    .await?;
            } else if !matches!(
                status,
                Some(super::PluginLifecycleOperationStatus::RollingBack)
                    | Some(super::PluginLifecycleOperationStatus::RolledBack)
            ) {
                return Err(graph_error(
                    "A candidate rollback lost its exact applying lifecycle operation.",
                ));
            }
        }

        let mut surface_evidence = BTreeMap::new();
        for unit in candidates.iter().rev() {
            let evidence = unit
                .coordinator
                .rollback_graph_candidate_surfaces(&unit.intent, &unit.manifest)
                .await?;
            surface_evidence.insert(unit.intent.package_id.as_str(), evidence);
        }

        let candidate_intents = candidates
            .iter()
            .map(|unit| unit.intent.clone())
            .collect::<Vec<_>>();
        let mut prior_intents = Vec::new();
        for unit in candidates {
            let transition = candidate_lock
                .package(&unit.intent.package_id)
                .ok_or_else(|| {
                    graph_error("A rollback candidate disappeared from its dependency lock.")
                })?;
            if let Some(prior) = retirements.get(transition.package_id()) {
                prior_intents.push(prior.intent.clone());
            }
        }
        let package_evidence = self
            .publication
            .rollback_candidates(
                candidate_lock,
                &candidate_intents,
                &prior_intents,
                &rollback_key(candidate_lock, &candidate_intents)?,
            )
            .await?;
        if package_evidence.len() != candidates.len() {
            return Err(graph_error(
                "Package-graph rollback omitted candidate package evidence.",
            ));
        }
        let package_evidence = package_evidence
            .into_iter()
            .map(|evidence| (evidence.package_id, evidence.evidence))
            .collect::<BTreeMap<_, _>>();
        if package_evidence.len() != candidates.len() {
            return Err(graph_error(
                "Package-graph rollback returned duplicate candidate evidence.",
            ));
        }
        for unit in candidates {
            if unit
                .coordinator
                .graph_candidate_status(&unit.intent)
                .await?
                == Some(super::PluginLifecycleOperationStatus::RolledBack)
            {
                continue;
            }
            let surfaces = surface_evidence
                .get(unit.intent.package_id.as_str())
                .ok_or_else(|| {
                    graph_error("A candidate rollback omitted surface cleanup evidence.")
                })?;
            let package = package_evidence
                .get(&unit.intent.package_id)
                .ok_or_else(|| {
                    graph_error("A candidate rollback changed package evidence identity.")
                })?;
            unit.coordinator
                .complete_graph_rollback(
                    &unit.intent,
                    &unit.manifest,
                    surfaces,
                    package,
                    completed_at_ms,
                )
                .await?;
        }
        Ok(())
    }
}

fn validate_upgrade_graph<'a>(
    envelope: &'a PluginOperationPlanEnvelope,
    prior_lock: &PluginPackageLock,
    candidate_units: &[PluginPackageLifecycleUnit],
    retirement_units: &[PluginPackageLifecycleUnit],
) -> UseResult<&'a PluginPackageLock> {
    envelope.validate()?;
    if envelope.plan.action != PluginOperationAction::Upgrade {
        return Err(graph_error(
            "The package-graph lifecycle action is not an upgrade.",
        ));
    }
    let candidate_lock = envelope.package_lock.as_ref().ok_or_else(|| {
        graph_error("A package-graph upgrade requires an exact candidate package lock.")
    })?;
    prior_lock.validate()?;
    if prior_lock.root_package_id != candidate_lock.root_package_id
        || prior_lock.host != candidate_lock.host
    {
        return Err(graph_error(
            "Prior and candidate package locks belong to different roots or hosts.",
        ));
    }

    let mut expected_candidates = std::collections::BTreeSet::new();
    let mut expected_retirements = std::collections::BTreeSet::new();
    for transition in &envelope.plan.packages {
        match transition.change {
            PlanPackageChangeKind::Add => {
                expected_candidates.insert(transition.package_id.as_str());
            }
            PlanPackageChangeKind::Replace => {
                expected_candidates.insert(transition.package_id.as_str());
                expected_retirements.insert(transition.package_id.as_str());
                let prior = prior_lock.package(&transition.package_id).ok_or_else(|| {
                    graph_error("A replaced package is absent from the prior package lock.")
                })?;
                validate_prior_transition(prior, transition)?;
            }
            PlanPackageChangeKind::Retain => {
                let prior = prior_lock.package(&transition.package_id).ok_or_else(|| {
                    graph_error("A retained package is absent from the prior package lock.")
                })?;
                validate_prior_transition(prior, transition)?;
            }
            PlanPackageChangeKind::Remove => {
                return Err(graph_error(
                    "Upgrade plans with removed dependency nodes require a separately reviewed garbage-collection operation.",
                ))
            }
        }
    }

    validate_unit_set(candidate_units, &expected_candidates, "candidate")?;
    validate_unit_set(retirement_units, &expected_retirements, "retirement")?;
    for package_id in &expected_retirements {
        let candidate = candidate_units
            .iter()
            .find(|unit| unit.intent.package_id == *package_id)
            .ok_or_else(|| graph_error("A replaced package lost its candidate unit."))?;
        let prior = retirement_units
            .iter()
            .find(|unit| unit.intent.package_id == *package_id)
            .ok_or_else(|| graph_error("A replaced package lost its retirement unit."))?;
        if candidate.intent.generation <= prior.intent.generation {
            return Err(graph_error(
                "A replacement candidate generation must be newer than its exact prior generation.",
            ));
        }
    }
    Ok(candidate_lock)
}

fn validate_unit_set(
    units: &[PluginPackageLifecycleUnit],
    expected: &std::collections::BTreeSet<&str>,
    label: &str,
) -> UseResult<()> {
    let provided = units
        .iter()
        .map(|unit| unit.intent.package_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if provided.len() != units.len() || &provided != expected {
        return Err(graph_error(format!(
            "The package-graph {label} unit set does not equal the reviewed upgrade transitions.",
        )));
    }
    Ok(())
}

fn validate_prior_transition(
    prior: &a3s_use_core::LockedPluginPackage,
    transition: &PlannedPackageTransition,
) -> UseResult<()> {
    let before = transition.before.as_ref().ok_or_else(|| {
        graph_error("A retained or replaced package omitted its reviewed prior state.")
    })?;
    let selected_surfaces = before
        .release
        .surfaces
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    let expected = prior.catalog.selected_state(&selected_surfaces)?;
    if expected != *before {
        return Err(graph_error(
            "A prior package generation drifted from the reviewed upgrade plan.",
        ));
    }
    Ok(())
}

fn validate_graph<'a>(
    envelope: &'a PluginOperationPlanEnvelope,
    units: &[PluginPackageLifecycleUnit],
    action: PluginOperationAction,
) -> UseResult<&'a a3s_use_core::PluginPackageLock> {
    envelope.validate()?;
    if envelope.plan.action != action {
        return Err(graph_error(
            "The package-graph lifecycle action does not match the reviewed plan.",
        ));
    }
    let lock = envelope.package_lock.as_ref().ok_or_else(|| {
        graph_error("A package-graph lifecycle operation requires a reviewed package lock.")
    })?;
    let mut expected = std::collections::BTreeSet::new();
    for transition in &envelope.plan.packages {
        match (action, transition.change) {
            (PluginOperationAction::Install, PlanPackageChangeKind::Add)
            | (PluginOperationAction::Uninstall, PlanPackageChangeKind::Remove) => {
                expected.insert(transition.package_id.as_str());
            }
            (_, PlanPackageChangeKind::Retain) => {}
            _ => return Err(graph_error(
                "The reviewed package transitions are unsupported by this graph lifecycle action.",
            )),
        }
    }
    let provided = units
        .iter()
        .map(|unit| unit.intent.package_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if expected.len() != units.len() || expected != provided {
        return Err(graph_error(
            "The lifecycle unit set does not equal the changed package generations in the reviewed dependency closure.",
        ));
    }
    Ok(lock)
}

fn transition_for<'a>(
    envelope: &'a PluginOperationPlanEnvelope,
    package_id: &str,
) -> UseResult<&'a PlannedPackageTransition> {
    envelope
        .plan
        .packages
        .iter()
        .find(|transition| transition.package_id == package_id)
        .ok_or_else(|| graph_error("A locked package is absent from the operation plan."))
}

fn units_by_package(
    units: &[PluginPackageLifecycleUnit],
) -> UseResult<BTreeMap<&str, &PluginPackageLifecycleUnit>> {
    let mut result = BTreeMap::new();
    for unit in units {
        if result
            .insert(unit.intent.package_id.as_str(), unit)
            .is_some()
        {
            return Err(graph_error(
                "A package lifecycle unit appears more than once.",
            ));
        }
    }
    Ok(result)
}

fn validate_unit(
    envelope: &PluginOperationPlanEnvelope,
    unit: &PluginPackageLifecycleUnit,
    package_id: &str,
    action: PluginLifecycleAction,
) -> UseResult<()> {
    unit.intent.validate()?;
    if unit.intent.action != action
        || unit.intent.operation_id != envelope.plan.operation_id
        || unit.intent.plan_digest != envelope.plan_digest
        || unit.intent.scope_id != envelope.plan.scope.id
        || unit.intent.package_id != package_id
        || unit.manifest.package_id != package_id
    {
        return Err(graph_error(
            "A package lifecycle unit does not bind the exact reviewed operation.",
        ));
    }
    let transition = transition_for(envelope, package_id)?;
    validate_generation_binding(unit, transition, action)
}

fn validate_generation_binding(
    unit: &PluginPackageLifecycleUnit,
    transition: &PlannedPackageTransition,
    action: PluginLifecycleAction,
) -> UseResult<()> {
    let state = match action {
        PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade => {
            transition.after.as_ref()
        }
        PluginLifecycleAction::Uninstall => transition.before.as_ref(),
        _ => None,
    }
    .ok_or_else(|| graph_error("A package lifecycle unit has no planned generation state."))?;
    if unit.intent.package_digest != state.release.package_sha256
        || unit.intent.manifest_digest != state.release.manifest_sha256
        || unit.manifest.version != state.release.version
    {
        return Err(graph_error(
            "A lifecycle package generation drifted from the reviewed plan.",
        ));
    }
    Ok(())
}

fn publication_key(envelope: &PluginOperationPlanEnvelope) -> UseResult<String> {
    let lock_digest = envelope
        .plan
        .package_lock_digest
        .as_deref()
        .ok_or_else(|| graph_error("The package graph omitted its lock digest."))?;
    let identity = format!(
        "{}\n{}\npackage-graph-publish",
        envelope.plan_digest, lock_digest
    );
    Ok(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

fn rollback_key(
    candidate_lock: &PluginPackageLock,
    candidate_intents: &[PluginLifecycleIntent],
) -> UseResult<String> {
    let mut identity = format!(
        "{}\npackage-graph-candidate-rollback",
        candidate_lock.descriptor_digest()?
    );
    for intent in candidate_intents {
        identity.push('\n');
        identity.push_str(&intent.descriptor_digest()?);
    }
    Ok(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

fn attach_rollback_error(primary: UseError, rollback: UseError) -> UseError {
    primary
        .with_detail("rollbackCode", rollback.code)
        .with_detail("rollbackMessage", rollback.message)
}

fn graph_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_graph_invalid", message)
}

#[cfg(test)]
mod tests;
