use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPackageLock, UseError, UseResult,
};
use a3s_use_extension::ExtensionManifest;
use async_trait::async_trait;

mod recovery;
mod validation;

use recovery::*;
use validation::*;

use super::{
    PluginCapabilityCutoverEvidence, PluginLifecycleAction, PluginLifecycleCoordinator,
    PluginLifecycleEvidence, PluginLifecycleIntent, PluginLifecycleOperationRecord,
};
#[cfg(test)]
use super::PluginGrantLifecycleUnit;

pub(crate) fn operation_cutover_key(envelope: &PluginOperationPlanEnvelope) -> UseResult<String> {
    match envelope.plan.action {
        PluginOperationAction::Install | PluginOperationAction::Upgrade => {
            publication_key(envelope)
        }
        PluginOperationAction::Uninstall => hide_key(envelope),
        PluginOperationAction::Enable | PluginOperationAction::Disable => Err(graph_error(
            "An enablement operation has no package-graph Registry cutover key.",
        )),
    }
}

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

/// Host-owned activation boundary for a durable capability cutover.
///
/// A graph publication is not enough to make a live endpoint safe: new
/// sessions must route to the newly published immutable catalog before any
/// prior-generation calls are drained. Implementations must be idempotent
/// and recover from the durable publication identified by `idempotency_key`;
/// the callback may be invoked again after a process stop between publication
/// and lifecycle checkpoint persistence.
#[async_trait]
pub trait PluginGraphCapabilityCutoverActivation: Send + Sync {
    async fn activate_capability_cutover(&self, idempotency_key: &str) -> UseResult<()>;
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
    /// Publish and return exact immutable capability snapshot evidence. Hosts
    /// that cannot prove this boundary must fail before mutation.
    async fn publish_capabilities_with_cutover(
        &self,
        package_lock: &PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        expected_capability_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication>;

    /// Publish candidate generations and hide prior-only removed generations
    /// in one capability snapshot.
    async fn publish_upgrade_capabilities_with_cutover(
        &self,
        package_lock: &PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        removed_intents: &[PluginLifecycleIntent],
        expected_capability_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication>;

    /// Atomically hide an uninstall closure and return one package-snapshot
    /// cutover. Package-specific hide checkpoints use the returned evidence;
    /// drain and exact removal continue through their typed hosts.
    async fn hide_capabilities_with_cutover(
        &self,
        package_lock: &PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        expected_capability_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication>;

    /// Release host-owned replay evidence after durable package and Grant
    /// journals can resume without invoking the cutover again.
    async fn complete_capability_cutover(&self, _idempotency_key: &str) -> UseResult<()> {
        Ok(())
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
    activation: Option<Arc<dyn PluginGraphCapabilityCutoverActivation>>,
}

impl PluginPackageGraphLifecycleCoordinator {
    pub fn new(publication: Arc<dyn PluginGraphCapabilityLifecycleHost>) -> Self {
        Self {
            publication,
            activation: None,
        }
    }

    /// Attach the host-owned live-endpoint activation boundary.
    ///
    /// The default remains a no-op for hosts that do not expose a live
    /// Gateway. When present, activation is called after the durable
    /// publication (including an exact replay) and before any old-generation
    /// drain or retirement starts.
    pub fn with_capability_cutover_activation(
        mut self,
        activation: Arc<dyn PluginGraphCapabilityCutoverActivation>,
    ) -> Self {
        self.activation = Some(activation);
        self
    }

    async fn activate_capability_cutover(&self, idempotency_key: &str) -> UseResult<()> {
        if let Some(activation) = &self.activation {
            activation
                .activate_capability_cutover(idempotency_key)
                .await?;
        }
        Ok(())
    }

    pub async fn apply_install(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_install_inner(
            envelope,
            units,
            #[cfg(test)]
            None,
            completed_at_ms,
        )
        .await
    }

    #[cfg(test)]
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
        #[cfg(test)] grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let lock = validate_graph(envelope, units, PluginOperationAction::Install)?;
        #[cfg(test)]
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
        let cutover_key = publication_key(envelope)?;
        #[cfg(test)]
        if let Some(grants) = grants {
            if grants.has_cutover().await? {
                self.activate_capability_cutover(&cutover_key).await?;
                let records = completed_publication_records(&ordered).await?;
                grants.retire().await?;
                self.publication
                    .complete_capability_cutover(&cutover_key)
                    .await?;
                return Ok(records);
            }
        }
        let publication = self
            .publication
            .publish_capabilities_with_cutover(
                lock,
                &intents,
                envelope.plan.state.capability_generation,
                &cutover_key,
            )
            .await?;
        let evidence = publication.packages;
        let cutover = publication.cutover;
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
        #[cfg(test)]
        if let Some(grants) = grants {
            let committed_at_ms = completed_at_ms();
            grants
                .commit_cutover(&cutover, committed_at_ms, committed_at_ms)
                .await?;
            self.activate_capability_cutover(&cutover_key).await?;
            grants.retire().await?;
            self.publication
                .complete_capability_cutover(&cutover_key)
                .await?;
            return Ok(records);
        }
        let _ = cutover;
        self.activate_capability_cutover(&cutover_key).await?;
        self.publication
            .complete_capability_cutover(&cutover_key)
            .await?;
        Ok(records)
    }

    pub async fn apply_uninstall(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_uninstall_inner(
            envelope,
            units,
            #[cfg(test)]
            None,
            completed_at_ms,
        )
        .await
    }

    #[cfg(test)]
    pub async fn apply_uninstall_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_uninstall_inner(envelope, units, Some(grants), completed_at_ms)
            .await
    }

    async fn apply_uninstall_inner(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        units: &[PluginPackageLifecycleUnit],
        #[cfg(test)] grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let lock = validate_graph(envelope, units, PluginOperationAction::Uninstall)?;
        #[cfg(test)]
        if let Some(grants) = grants {
            grants.validate_envelope(envelope)?;
            grants.prepare(completed_at_ms()).await?;
        }
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
        let cutover_key = hide_key(envelope)?;
        #[cfg(test)]
        let grant_has_cutover = match grants {
            Some(grants) => grants.has_cutover().await?,
            None => false,
        };
        #[cfg(not(test))]
        let grant_has_cutover = false;
        if grant_has_cutover {
            validate_hidden_records(&ordered).await?;
        } else {
            let publication = self
                .publication
                .hide_capabilities_with_cutover(
                    lock,
                    &intents,
                    envelope.plan.state.capability_generation,
                    &cutover_key,
                )
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

            #[cfg(test)]
            if let Some(grants) = grants {
                let committed_at_ms = completed_at_ms();
                grants
                    .commit_cutover(&publication.cutover, committed_at_ms, committed_at_ms)
                    .await?;
            }
        }

        self.activate_capability_cutover(&cutover_key).await?;

        for unit in &ordered {
            unit.coordinator
                .drain_graph_retirement(&unit.intent, &unit.manifest, &completed_at_ms)
                .await?;
        }
        #[cfg(test)]
        if let Some(grants) = grants {
            grants.retire().await?;
        }

        let mut records = Vec::with_capacity(ordered.len());
        for unit in ordered {
            records.push(
                unit.coordinator
                    .apply(&unit.intent, &unit.manifest, &completed_at_ms)
                    .await?,
            );
        }
        self.publication
            .complete_capability_cutover(&cutover_key)
            .await?;
        Ok(records)
    }
}

include!("graph_upgrade.rs");

#[cfg(test)]
mod tests;
