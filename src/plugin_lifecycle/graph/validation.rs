use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    LockedPluginPackage, PlanPackageChangeKind, PlannedPackageTransition, PluginOperationAction,
    PluginOperationPlanEnvelope, PluginPackageLock, UseError, UseResult,
};
use sha2::{Digest, Sha256};

use super::super::{PluginLifecycleAction, PluginLifecycleIntent};
use super::PluginPackageLifecycleUnit;

pub(super) fn validate_upgrade_graph<'a>(
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
    if envelope
        .prior_package_lock
        .as_ref()
        .is_some_and(|bound| bound != prior_lock)
    {
        return Err(graph_error(
            "The package-graph upgrade prior lock changed after review.",
        ));
    }
    if prior_lock.root_package_id != candidate_lock.root_package_id
        || prior_lock.host != candidate_lock.host
    {
        return Err(graph_error(
            "Prior and candidate package locks belong to different roots or hosts.",
        ));
    }

    let mut expected_candidates = BTreeSet::new();
    let mut expected_retirements = BTreeSet::new();
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
                let retained = prior_lock
                    .package(&transition.package_id)
                    .or_else(|| candidate_lock.package(&transition.package_id))
                    .ok_or_else(|| {
                        graph_error(
                            "A retained package is absent from both exact dependency locks.",
                        )
                    })?;
                validate_prior_transition(retained, transition)?;
            }
            PlanPackageChangeKind::Remove => {
                expected_retirements.insert(transition.package_id.as_str());
                let prior = prior_lock.package(&transition.package_id).ok_or_else(|| {
                    graph_error("A removed package is absent from the prior package lock.")
                })?;
                validate_prior_transition(prior, transition)?;
                if candidate_lock.package(&transition.package_id).is_some()
                    || envelope.prior_package_lock.as_ref() != Some(prior_lock)
                {
                    return Err(graph_error(
                        "A removed dependency requires exact reviewed prior/candidate package locks.",
                    ));
                }
            }
        }
    }

    validate_unit_set(candidate_units, &expected_candidates, "candidate")?;
    validate_unit_set(retirement_units, &expected_retirements, "retirement")?;
    for package_id in &expected_retirements {
        if envelope
            .plan
            .packages
            .iter()
            .find(|transition| transition.package_id == **package_id)
            .is_some_and(|transition| transition.change == PlanPackageChangeKind::Remove)
        {
            continue;
        }
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
    expected: &BTreeSet<&str>,
    label: &str,
) -> UseResult<()> {
    let provided = units
        .iter()
        .map(|unit| unit.intent.package_id.as_str())
        .collect::<BTreeSet<_>>();
    if provided.len() != units.len() || &provided != expected {
        return Err(graph_error(format!(
            "The package-graph {label} unit set does not equal the reviewed upgrade transitions.",
        )));
    }
    Ok(())
}

fn validate_prior_transition(
    prior: &LockedPluginPackage,
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

pub(super) fn validate_graph<'a>(
    envelope: &'a PluginOperationPlanEnvelope,
    units: &[PluginPackageLifecycleUnit],
    action: PluginOperationAction,
) -> UseResult<&'a PluginPackageLock> {
    envelope.validate()?;
    if envelope.plan.action != action {
        return Err(graph_error(
            "The package-graph lifecycle action does not match the reviewed plan.",
        ));
    }
    let lock = envelope.package_lock.as_ref().ok_or_else(|| {
        graph_error("A package-graph lifecycle operation requires a reviewed package lock.")
    })?;
    let mut expected = BTreeSet::new();
    for transition in &envelope.plan.packages {
        match (action, transition.change) {
            (PluginOperationAction::Install, PlanPackageChangeKind::Add)
            | (PluginOperationAction::Uninstall, PlanPackageChangeKind::Remove) => {
                expected.insert(transition.package_id.as_str());
            }
            (_, PlanPackageChangeKind::Retain) => {}
            _ => {
                return Err(graph_error(
                    "The reviewed package transitions are unsupported by this graph lifecycle action.",
                ));
            }
        }
    }
    let provided = units
        .iter()
        .map(|unit| unit.intent.package_id.as_str())
        .collect::<BTreeSet<_>>();
    if expected.len() != units.len() || expected != provided {
        return Err(graph_error(
            "The lifecycle unit set does not equal the changed package generations in the reviewed dependency closure.",
        ));
    }
    Ok(lock)
}

pub(super) fn transition_for<'a>(
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

pub(super) fn units_by_package(
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

pub(super) fn validate_unit(
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

pub(super) fn publication_key(envelope: &PluginOperationPlanEnvelope) -> UseResult<String> {
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

pub(super) fn hide_key(envelope: &PluginOperationPlanEnvelope) -> UseResult<String> {
    let lock_digest = envelope
        .plan
        .package_lock_digest
        .as_deref()
        .ok_or_else(|| graph_error("The package graph omitted its lock digest."))?;
    let identity = format!(
        "{}\n{}\npackage-graph-hide",
        envelope.plan_digest, lock_digest
    );
    Ok(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

pub(super) fn grant_rollback_key(envelope: &PluginOperationPlanEnvelope) -> UseResult<String> {
    envelope.validate()?;
    let identity = format!(
        "{}\n{}\nworkspace-grant-candidate-rollback",
        envelope.plan.operation_id, envelope.plan_digest
    );
    Ok(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

pub(super) fn rollback_key(
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

pub(super) fn attach_rollback_error(primary: UseError, rollback: UseError) -> UseError {
    primary
        .with_detail("rollbackCode", rollback.code)
        .with_detail("rollbackMessage", rollback.message)
}

pub(super) fn graph_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_graph_invalid", message)
}

pub(super) fn cutover_evidence_required() -> UseError {
    UseError::new(
        "use.plugin.package_graph_cutover_evidence_required",
        "The capability host cannot provide exact snapshot evidence required for grant composition.",
    )
}
