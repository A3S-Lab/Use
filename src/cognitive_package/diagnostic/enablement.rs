use super::*;
use crate::cognitive_package::enablement_store::PendingCognitivePackageEnablement;
use crate::cognitive_package::grant::PackageGraphAuthorization;
use crate::cognitive_package::host_store::{StoredPluginHostCancellation, StoredPluginHostRequest};
use crate::plugin_lifecycle::{PluginLifecycleIntent, PluginLifecycleIntentSpec};

use super::projection::{
    enablement_cutover_key, enablement_intent_cutover_key,
    expected_enablement_intent_lifecycle_unit, expected_enablement_lifecycle_units,
    project_installed_source,
};

pub(super) async fn pending_enablement(
    manager: &CognitivePackageManager,
    package_id: &PluginPackageId,
) -> UseResult<Option<PendingCognitivePackageEnablement>> {
    let state = manager
        .enablement_store()
        .get_state(manager.scope(), package_id)
        .await
        .map_err(|_| diagnostic_state_error())?;
    Ok(state.and_then(|state| state.active))
}

pub(super) async fn diagnose_reviewed_enablement_operation(
    manager: &CognitivePackageManager,
    package_id: &PluginPackageId,
    record: StoredPluginHostRequest,
    cancellation: Option<StoredPluginHostCancellation>,
) -> UseResult<Option<PluginOperationDiagnostic>> {
    record.validate().map_err(|_| diagnostic_state_error())?;
    if record.outcome.is_some() {
        return Ok(None);
    }
    let (request, result) = record
        .plan
        .enablement_parts()
        .ok_or_else(diagnostic_state_error)?;
    if result.status != a3s_use_core::PluginHostEnablementPlanStatus::Planned
        || request.scope.plan_scope() != *manager.scope()
        || request.package_id != *package_id
        || result.package_id != *package_id
    {
        return Err(diagnostic_state_error());
    }
    let envelope = result.plan.as_ref().ok_or_else(diagnostic_state_error)?;
    let expected_action = if request.enabled {
        PluginOperationAction::Enable
    } else {
        PluginOperationAction::Disable
    };
    if envelope.plan.action != expected_action
        || envelope.plan.scope != *manager.scope()
        || envelope.plan.package_id != package_id.as_str()
    {
        return Err(diagnostic_state_error());
    }

    let phase = if let Some(cancellation) = cancellation.as_ref() {
        if cancellation.operation_id != envelope.plan.operation_id
            || cancellation.plan_digest != envelope.plan_digest
            || cancellation.cancelled_at_ms < result.planned_at_ms
        {
            return Err(diagnostic_state_error());
        }
        PluginOperationDiagnosticPhase::Cancelled
    } else {
        PluginOperationDiagnosticPhase::Planned
    };

    let enablement_store = manager.enablement_store();
    if let Some(completed) = enablement_store
        .get_operation(manager.scope(), &envelope.plan.operation_id)
        .await
        .map_err(|_| diagnostic_state_error())?
    {
        completed.validate().map_err(|_| diagnostic_state_error())?;
        if completed.envelope != *envelope {
            return Err(diagnostic_state_error());
        }
        return Ok(None);
    }
    let Some(current) = enablement_store
        .get_state(manager.scope(), package_id)
        .await
        .map_err(|_| diagnostic_state_error())?
    else {
        return Ok(None);
    };
    current.validate().map_err(|_| diagnostic_state_error())?;
    if let Some(active) = current.active.clone() {
        return diagnose_enablement_operation(manager, package_id.as_str(), active)
            .await
            .map(Some);
    }
    if current.state_generation != request.expected_package_generation
        || current.enabled == request.enabled
        || current.artifact.is_none()
    {
        return Ok(None);
    }

    let (extension, _, _) = match manager.required_enablement_extension(package_id).await {
        Ok(installed) => installed,
        Err(error) if error.code == "use.extension.not_installed" => return Ok(None),
        Err(_) => return Err(diagnostic_state_error()),
    };
    let artifact = current
        .artifact
        .as_ref()
        .ok_or_else(diagnostic_state_error)?;
    let package_digest = extension
        .receipt
        .package_sha256
        .as_deref()
        .map(|digest| format!("sha256:{digest}"))
        .ok_or_else(diagnostic_state_error)?;
    let manifest_digest = format!("sha256:{}", extension.receipt.manifest_sha256);
    let lifecycle_generation = extension
        .receipt
        .lifecycle_generation
        .ok_or_else(diagnostic_state_error)?;
    let receipt_digest = extension
        .receipt
        .descriptor_digest()
        .map_err(|_| diagnostic_state_error())?;
    let selected_surfaces = extension
        .selected_surfaces()
        .map_err(|_| diagnostic_state_error())?;
    let expected_desired = if current.enabled {
        a3s_use_core::PluginDesiredState::Enabled
    } else {
        a3s_use_core::PluginDesiredState::InstalledDisabled
    };
    if extension.receipt.enabled != current.enabled
        || artifact.version != extension.receipt.version
        || artifact.generation != lifecycle_generation
        || artifact.package_digest != package_digest
        || artifact.manifest_digest != manifest_digest
        || result.state.version.as_deref() != Some(artifact.version.as_str())
        || result.state.package_generation != Some(current.state_generation)
        || result.state.package_digest.as_deref() != Some(package_digest.as_str())
        || result.state.manifest_digest.as_deref() != Some(manifest_digest.as_str())
        || result.state.receipt_digest.as_deref() != Some(receipt_digest.as_str())
        || result.state.desired != expected_desired
        || result.state.selected_surfaces != selected_surfaces
        || envelope.plan.state.receipt_digest.as_deref() != Some(receipt_digest.as_str())
        || envelope.plan.state.capability_generation != result.state.capability_generation
    {
        return Err(diagnostic_state_error());
    }
    let [transition] = envelope.plan.packages.as_slice() else {
        return Err(diagnostic_state_error());
    };
    let selected_state = transition
        .after
        .as_ref()
        .ok_or_else(diagnostic_state_error)?;
    if transition.package_id != package_id.as_str()
        || transition.change != PlanPackageChangeKind::Retain
        || transition.before.as_ref() != Some(selected_state)
        || crate::cognitive_package::plan::state_surface_refs(selected_state) != selected_surfaces
    {
        return Err(diagnostic_state_error());
    }

    let lifecycle_action = if request.enabled {
        PluginLifecycleAction::Enable
    } else {
        PluginLifecycleAction::Disable
    };
    let intent = PluginLifecycleIntent::from_manifest_selection(
        PluginLifecycleIntentSpec {
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            scope: manager.scope().clone(),
            package_id: package_id.to_string(),
            package_digest,
            manifest_digest,
            generation: lifecycle_generation,
            action: lifecycle_action,
            retained_ui_state_surfaces: Vec::new(),
        },
        &extension.manifest,
        &selected_surfaces,
    )
    .map_err(|_| diagnostic_state_error())?;
    let expected = vec![expected_enablement_intent_lifecycle_unit(&intent)?];
    let observed = observe_lifecycle(
        manager,
        &envelope.plan.operation_id,
        &envelope.plan_digest,
        phase,
        &expected,
    )
    .await?;
    let authorization = PackageGraphAuthorization::default();
    let grant = observe_grant(manager, envelope, &authorization, phase).await?;
    let snapshot = manager
        .registry
        .published_snapshot()
        .await
        .map_err(|_| diagnostic_state_error())?;
    let cutover_key = enablement_intent_cutover_key(&intent)?;
    let operation_cutover = project_registry_cutover(
        envelope,
        phase,
        &cutover_key,
        &snapshot.pending_cutovers,
        snapshot.generation,
        &observed,
        &grant,
    )?;
    let registry = PluginRegistryOperationDiagnostic {
        generation: snapshot.generation,
        snapshot_digest: snapshot
            .descriptor_digest()
            .map_err(|_| diagnostic_state_error())?,
        pending_cutover_count: bounded_count(snapshot.pending_cutovers.len(), "Registry cutover")?,
        operation_cutover,
    };
    let sources = project_installed_source(
        package_id.as_str(),
        extension
            .plan_ready_catalog()
            .map_err(|_| diagnostic_state_error())?,
    )?;
    let providers = project_providers(&envelope.plan, &observed)?;
    let recovery = if registry.operation_cutover.status
        == PluginRegistryCutoverDiagnosticStatus::GenerationDrift
    {
        PluginOperationRecoveryGuidance::OperatorReviewRequired
    } else if phase == PluginOperationDiagnosticPhase::Cancelled {
        PluginOperationRecoveryGuidance::ObserveCancellation
    } else {
        PluginOperationRecoveryGuidance::ReviewAndApplyExactPlan
    };
    let operation = pending_operation_diagnostic(
        envelope,
        phase,
        result.planned_at_ms,
        None,
        cancellation.as_ref().map(|record| record.cancelled_at_ms),
        confirmation_status(envelope, &authorization, phase),
        sources,
        providers,
        grant,
        Vec::new(),
        expected.len(),
        DownloadProjection::not_required(),
        recovery,
    )?;
    let diagnostic = PluginOperationDiagnostic {
        schema: PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA.to_owned(),
        observed_at_ms: crate::cognitive_package::plan::now_ms()
            .map_err(|_| diagnostic_state_error())?,
        scope: manager.scope().clone(),
        package_id: package_id.to_string(),
        registry,
        operation,
    };
    diagnostic.validate()?;
    Ok(Some(diagnostic))
}

pub(in crate::cognitive_package) async fn diagnose_enablement_operation(
    manager: &CognitivePackageManager,
    package_id: &str,
    active: PendingCognitivePackageEnablement,
) -> UseResult<PluginOperationDiagnostic> {
    let phase = PluginOperationDiagnosticPhase::Admitted;
    if active.envelope.plan.scope != *manager.scope()
        || active.request.package_id.as_str() != package_id
        || !matches!(
            active.envelope.plan.action,
            PluginOperationAction::Enable | PluginOperationAction::Disable
        )
    {
        return Err(diagnostic_state_error());
    }
    active
        .authorization
        .validate_against(&active.envelope, active.started_at_ms)
        .map_err(|_| diagnostic_state_error())?;

    let snapshot = manager
        .registry
        .published_snapshot()
        .await
        .map_err(|_| diagnostic_state_error())?;
    let expected = expected_enablement_lifecycle_units(&active)?;
    let observed = observe_lifecycle(
        manager,
        &active.envelope.plan.operation_id,
        &active.envelope.plan_digest,
        phase,
        &expected,
    )
    .await?;
    let grant = observe_grant(manager, &active.envelope, &active.authorization, phase).await?;
    let cutover_key = enablement_cutover_key(&active)?;
    let operation_cutover = project_registry_cutover(
        &active.envelope,
        phase,
        &cutover_key,
        &snapshot.pending_cutovers,
        snapshot.generation,
        &observed,
        &grant,
    )?;
    let registry = PluginRegistryOperationDiagnostic {
        generation: snapshot.generation,
        snapshot_digest: snapshot
            .descriptor_digest()
            .map_err(|_| diagnostic_state_error())?,
        pending_cutover_count: bounded_count(snapshot.pending_cutovers.len(), "Registry cutover")?,
        operation_cutover,
    };

    let extension = manager
        .registry
        .get(package_id)
        .await
        .map_err(|_| diagnostic_state_error())?
        .ok_or_else(diagnostic_state_error)?;
    let package_digest = extension
        .receipt
        .package_sha256
        .as_deref()
        .map(|digest| format!("sha256:{digest}"));
    let manifest_digest = format!("sha256:{}", extension.receipt.manifest_sha256);
    if extension.receipt.lifecycle_generation != Some(active.intent.generation)
        || package_digest.as_deref() != Some(active.intent.package_digest.as_str())
        || manifest_digest != active.intent.manifest_digest
    {
        return Err(diagnostic_state_error());
    }
    let catalog = extension
        .plan_ready_catalog()
        .map_err(|_| diagnostic_state_error())?;
    let sources = project_installed_source(package_id, catalog)?;
    let providers = project_providers(&active.envelope.plan, &observed)?;
    let lifecycle = observed
        .iter()
        .map(|unit| unit.summary.clone())
        .collect::<Vec<_>>();
    let recovery = if registry.operation_cutover.status
        == PluginRegistryCutoverDiagnosticStatus::GenerationDrift
    {
        PluginOperationRecoveryGuidance::OperatorReviewRequired
    } else {
        PluginOperationRecoveryGuidance::ResumeExactPlan
    };
    let operation = pending_operation_diagnostic(
        &active.envelope,
        phase,
        active.envelope.plan.created_at_ms,
        Some(active.started_at_ms),
        None,
        confirmation_status(&active.envelope, &active.authorization, phase),
        sources,
        providers,
        grant,
        lifecycle,
        expected.len(),
        DownloadProjection::not_required(),
        recovery,
    )?;
    let diagnostic = PluginOperationDiagnostic {
        schema: PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA.to_owned(),
        observed_at_ms: crate::cognitive_package::plan::now_ms()
            .map_err(|_| diagnostic_state_error())?,
        scope: manager.scope().clone(),
        package_id: package_id.to_owned(),
        registry,
        operation,
    };
    diagnostic.validate()?;
    Ok(diagnostic)
}
