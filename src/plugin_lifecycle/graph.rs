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
}

/// Exact package-keyed evidence returned by one atomic capability cutover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackagePublicationEvidence {
    package_id: String,
    evidence: PluginLifecycleEvidence,
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
        PluginLifecycleAction::Install => transition.after.as_ref(),
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

fn graph_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_graph_invalid", message)
}

#[cfg(test)]
mod tests;
