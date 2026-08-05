use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPackageLock, UseError, UseResult,
};
use a3s_use_extension::ExtensionManifest;
use async_trait::async_trait;

mod validation;

use validation::*;

use super::{
    PluginCapabilityCutoverEvidence, PluginGrantLifecycleUnit, PluginLifecycleAction,
    PluginLifecycleCoordinator, PluginLifecycleEvidence, PluginLifecycleIntent,
    PluginLifecycleOperationRecord,
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

/// Package receipts plus the exact capability snapshot selected by one atomic
/// graph cutover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGraphCapabilityPublication {
    packages: Vec<PluginPackagePublicationEvidence>,
    cutover: PluginCapabilityCutoverEvidence,
}

impl PluginGraphCapabilityPublication {
    pub fn new(
        packages: Vec<PluginPackagePublicationEvidence>,
        cutover: PluginCapabilityCutoverEvidence,
    ) -> Self {
        Self { packages, cutover }
    }

    pub fn packages(&self) -> &[PluginPackagePublicationEvidence] {
        &self.packages
    }

    pub fn cutover(&self) -> &PluginCapabilityCutoverEvidence {
        &self.cutover
    }
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

    /// Publish and return exact immutable capability snapshot evidence. Hosts
    /// that cannot prove this boundary must fail before mutation.
    async fn publish_capabilities_with_cutover(
        &self,
        _package_lock: &PluginPackageLock,
        _intents: &[PluginLifecycleIntent],
        _idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        Err(cutover_evidence_required())
    }

    /// Publish candidate generations and hide prior-only removed generations
    /// in one capability snapshot. Existing hosts remain source-compatible for
    /// upgrades without removals; hosts supporting graph GC must override this
    /// method with an atomic implementation.
    async fn publish_upgrade_capabilities(
        &self,
        package_lock: &PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        removed_intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<Vec<PluginPackagePublicationEvidence>> {
        if !removed_intents.is_empty() {
            return Err(graph_error(
                "The capability host does not support atomic dependency-removal publication.",
            ));
        }
        self.publish_capabilities(package_lock, candidate_intents, idempotency_key)
            .await
    }

    async fn publish_upgrade_capabilities_with_cutover(
        &self,
        _package_lock: &PluginPackageLock,
        _candidate_intents: &[PluginLifecycleIntent],
        _removed_intents: &[PluginLifecycleIntent],
        _idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        Err(cutover_evidence_required())
    }

    /// Atomically hide an uninstall closure and return one route-snapshot
    /// cutover. Package-specific hide checkpoints use the returned evidence;
    /// drain and exact removal continue through their typed hosts.
    async fn hide_capabilities_with_cutover(
        &self,
        _package_lock: &PluginPackageLock,
        _intents: &[PluginLifecycleIntent],
        _idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        Err(cutover_evidence_required())
    }

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
        self.apply_install_inner(envelope, units, None, completed_at_ms)
            .await
    }

    pub async fn apply_install_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_install_inner(envelope, units, Some(grants), completed_at_ms)
            .await
    }

    async fn apply_install_inner(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let lock = validate_graph(envelope, units, PluginOperationAction::Install)?;
        if let Some(grants) = grants {
            grants.validate_envelope(envelope)?;
            grants.prepare(completed_at_ms()).await?;
        }
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
        let (evidence, cutover) = if grants.is_some() {
            let publication = self
                .publication
                .publish_capabilities_with_cutover(lock, &intents, &publication_key(envelope)?)
                .await?;
            (publication.packages, Some(publication.cutover))
        } else {
            (
                self.publication
                    .publish_capabilities(lock, &intents, &publication_key(envelope)?)
                    .await?,
                None,
            )
        };
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
        if let (Some(grants), Some(cutover)) = (grants, cutover.as_ref()) {
            let committed_at_ms = completed_at_ms();
            grants
                .commit_cutover(cutover, committed_at_ms, committed_at_ms)
                .await?;
            grants.retire().await?;
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

    pub async fn apply_uninstall_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let lock = validate_graph(envelope, units, PluginOperationAction::Uninstall)?;
        grants.validate_envelope(envelope)?;
        grants.prepare(completed_at_ms()).await?;
        let units_by_id = units_by_package(units)?;
        let mut ordered = Vec::with_capacity(units.len());
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
            let unit = *units_by_id.get(package.package_id()).ok_or_else(|| {
                graph_error("A locked dependency has no uninstall lifecycle unit.")
            })?;
            validate_unit(
                envelope,
                unit,
                package.package_id(),
                PluginLifecycleAction::Uninstall,
            )?;
            ordered.push(unit);
        }

        let intents = ordered
            .iter()
            .map(|unit| unit.intent.clone())
            .collect::<Vec<_>>();
        let publication = self
            .publication
            .hide_capabilities_with_cutover(lock, &intents, &hide_key(envelope)?)
            .await?;
        if publication.packages.len() != ordered.len() {
            return Err(graph_error(
                "Package-graph hiding omitted capability evidence.",
            ));
        }
        for (unit, evidence) in ordered.iter().zip(&publication.packages) {
            if evidence.package_id != unit.intent.package_id {
                return Err(graph_error(
                    "Package-graph hide evidence changed package order or identity.",
                ));
            }
            unit.coordinator
                .record_graph_capability_hidden(
                    &unit.intent,
                    &unit.manifest,
                    &evidence.evidence,
                    &completed_at_ms,
                )
                .await?;
        }

        let committed_at_ms = completed_at_ms();
        grants
            .commit_cutover(&publication.cutover, committed_at_ms, committed_at_ms)
            .await?;

        for unit in &ordered {
            unit.coordinator
                .drain_graph_retirement(&unit.intent, &unit.manifest, &completed_at_ms)
                .await?;
        }
        grants.retire().await?;

        let mut records = Vec::with_capacity(ordered.len());
        for unit in ordered {
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
        self.apply_upgrade_inner(
            envelope,
            prior_lock,
            candidate_units,
            retirement_units,
            None,
            completed_at_ms,
        )
        .await
    }

    pub async fn apply_upgrade_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        prior_lock: &PluginPackageLock,
        candidate_units: &[PluginPackageLifecycleUnit],
        retirement_units: &[PluginPackageLifecycleUnit],
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_upgrade_inner(
            envelope,
            prior_lock,
            candidate_units,
            retirement_units,
            Some(grants),
            completed_at_ms,
        )
        .await
    }

    async fn apply_upgrade_inner(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        prior_lock: &PluginPackageLock,
        candidate_units: &[PluginPackageLifecycleUnit],
        retirement_units: &[PluginPackageLifecycleUnit],
        grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let candidate_lock =
            validate_upgrade_graph(envelope, prior_lock, candidate_units, retirement_units)?;
        if let Some(grants) = grants {
            grants.validate_envelope(envelope)?;
        }
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
                .rollback_upgrade_operation(
                    envelope,
                    candidate_lock,
                    &interrupted_rollback,
                    &retirements,
                    grants,
                    &completed_at_ms,
                )
                .await
            {
                Ok(()) => Err(replay_error),
                Err(rollback) => Err(attach_rollback_error(replay_error, rollback)),
            };
        }
        if saw_rolled_back {
            let replay_error = UseError::new(
                "use.plugin.package_graph_upgrade_rolled_back",
                "This candidate graph was rolled back; create and review a fresh upgrade plan.",
            );
            if let Some(grants) = grants {
                let rolled_back_at_ms = completed_at_ms();
                if let Err(rollback) = grants
                    .rollback(
                        grant_rollback_key(envelope)?,
                        rolled_back_at_ms,
                        rolled_back_at_ms,
                    )
                    .await
                {
                    return Err(attach_rollback_error(replay_error, rollback));
                }
            }
            return Err(replay_error);
        }
        if let Some(grants) = grants {
            if grants.is_rolled_back().await? {
                return Err(UseError::new(
                    "use.plugin.package_graph_upgrade_rolled_back",
                    "This candidate graph grant operation was rolled back; create and review a fresh upgrade plan.",
                ));
            }
            grants.prepare(completed_at_ms()).await?;
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
                    .rollback_upgrade_operation(
                        envelope,
                        candidate_lock,
                        &ordered_candidates,
                        &retirements,
                        grants,
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
        let removed_intents = prior_lock
            .removal_order()?
            .into_iter()
            .filter_map(|package| {
                envelope
                    .plan
                    .packages
                    .iter()
                    .find(|transition| transition.package_id == package.package_id())
                    .filter(|transition| transition.change == PlanPackageChangeKind::Remove)
                    .and_then(|_| retirements.get(package.package_id()))
                    .map(|unit| unit.intent.clone())
            })
            .collect::<Vec<_>>();
        let grant_has_cutover = match grants {
            Some(grants) => grants.has_cutover().await?,
            None => false,
        };
        let publication = if grants.is_some() {
            self.publication
                .publish_upgrade_capabilities_with_cutover(
                    candidate_lock,
                    &intents,
                    &removed_intents,
                    &publication_key(envelope)?,
                )
                .await
                .map(|publication| (publication.packages, Some(publication.cutover)))
        } else {
            self.publication
                .publish_upgrade_capabilities(
                    candidate_lock,
                    &intents,
                    &removed_intents,
                    &publication_key(envelope)?,
                )
                .await
                .map(|evidence| (evidence, None))
        };
        let (evidence, cutover) = match publication {
            Ok(publication) => publication,
            Err(error) => {
                if grant_has_cutover {
                    return Err(error);
                }
                return match self
                    .rollback_upgrade_operation(
                        envelope,
                        candidate_lock,
                        &ordered_candidates,
                        &retirements,
                        grants,
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

        if let (Some(grants), Some(cutover)) = (grants, cutover.as_ref()) {
            let committed_at_ms = completed_at_ms();
            grants
                .commit_cutover(cutover, committed_at_ms, committed_at_ms)
                .await?;
        }

        if let Some(grants) = grants {
            for package in prior_lock.removal_order()? {
                let Some(transition) = envelope
                    .plan
                    .packages
                    .iter()
                    .find(|transition| transition.package_id == package.package_id())
                else {
                    continue;
                };
                if !matches!(
                    transition.change,
                    PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
                ) {
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
                unit.coordinator
                    .drain_graph_retirement(&unit.intent, &unit.manifest, &completed_at_ms)
                    .await?;
            }
            grants.retire().await?;
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
            if !matches!(
                transition.change,
                PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
            ) {
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

    async fn rollback_upgrade_operation(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        candidate_lock: &PluginPackageLock,
        candidates: &[&PluginPackageLifecycleUnit],
        retirements: &BTreeMap<&str, &PluginPackageLifecycleUnit>,
        grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<()> {
        let package_rollback = self
            .rollback_upgrade_candidates(candidate_lock, candidates, retirements, completed_at_ms)
            .await;
        let grant_rollback = match grants {
            Some(grants) => {
                let rolled_back_at_ms = completed_at_ms();
                grants
                    .rollback(
                        grant_rollback_key(envelope)?,
                        rolled_back_at_ms,
                        rolled_back_at_ms,
                    )
                    .await
                    .map(drop)
            }
            None => Ok(()),
        };
        match (package_rollback, grant_rollback) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(package), Ok(())) => Err(package),
            (Ok(()), Err(grant)) => Err(grant),
            (Err(package), Err(grant)) => Err(attach_rollback_error(package, grant)),
        }
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

#[cfg(test)]
mod tests;
