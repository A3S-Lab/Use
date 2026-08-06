use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginOperationPlanEnvelope, UseResult,
};
use a3s_use_extension::ExtensionManifest;

use super::coordinator::{
    coordinator_error, validate_manifest_binding, PluginLifecycleCoordinator,
};
use super::{
    PluginGrantLifecycleUnit, PluginLifecycleCheckpointKind, PluginLifecycleCheckpointOutcome,
    PluginLifecycleIntent, PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
};

impl PluginLifecycleCoordinator {
    /// Prepare, atomically publish, and authorize one already-installed
    /// permission-bearing package without replacing its immutable artifact.
    pub async fn apply_enable_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_enablement_grant_binding(
            envelope,
            intent,
            manifest,
            grants,
            PluginOperationAction::Enable,
        )?;
        if let Some(record) = self.load_exact_record(intent).await? {
            if record.status == PluginLifecycleOperationStatus::Completed {
                if grants.is_completed().await? {
                    return Ok(record);
                }
                return Err(coordinator_error(
                    "A completed enable lifecycle has incomplete Grant retirement evidence.",
                ));
            }
        }
        grants.prepare(completed_at_ms()).await?;
        let record = self
            .prepare_enablement_for_cutover(intent, manifest, &completed_at_ms)
            .await?;
        let checkpoint = intent
            .checkpoints
            .iter()
            .find(|checkpoint| {
                checkpoint.kind == PluginLifecycleCheckpointKind::CapabilityPublished
            })
            .ok_or_else(|| coordinator_error("Enablement omitted its publication checkpoint."))?;
        let publication = self
            .hosts
            .capability
            .publish_capability_with_cutover(intent, &checkpoint.idempotency_key)
            .await?;
        let record = if record.next_checkpoint().is_some() {
            self.journal
                .record_checkpoint(
                    intent,
                    &checkpoint.idempotency_key,
                    PluginLifecycleCheckpointOutcome::Applied,
                    publication.evidence().digest().to_string(),
                    None,
                    completed_at_ms(),
                )
                .await?
        } else {
            record
        };
        if record.next_checkpoint().is_some() {
            return Err(coordinator_error(
                "Enablement publication did not complete its lifecycle checkpoint sequence.",
            ));
        }
        let committed_at_ms = completed_at_ms();
        grants
            .commit_cutover(publication.cutover(), committed_at_ms, committed_at_ms)
            .await?;
        grants.retire().await?;
        self.journal.complete(intent, completed_at_ms()).await
    }

    /// Atomically hide one permission-bearing package, commit Grant cutover,
    /// drain calls admitted by the prior generation, retire the exact Grant,
    /// and only then stop package surfaces.
    pub async fn apply_disable_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_enablement_grant_binding(
            envelope,
            intent,
            manifest,
            grants,
            PluginOperationAction::Disable,
        )?;
        if let Some(record) = self.load_exact_record(intent).await? {
            if record.status == PluginLifecycleOperationStatus::Completed {
                if grants.is_completed().await? {
                    return Ok(record);
                }
                return Err(coordinator_error(
                    "A completed disable lifecycle has incomplete Grant retirement evidence.",
                ));
            }
        }
        grants.prepare(completed_at_ms()).await?;
        let mut record = self.journal.begin(intent).await?;
        let checkpoint = intent
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.kind == PluginLifecycleCheckpointKind::CapabilityHidden)
            .ok_or_else(|| coordinator_error("Disablement omitted its hide checkpoint."))?;
        let publication = self
            .hosts
            .capability
            .hide_capability_with_cutover(intent, &checkpoint.idempotency_key)
            .await?;
        if record
            .next_checkpoint()
            .is_some_and(|next| next.kind == PluginLifecycleCheckpointKind::CapabilityHidden)
        {
            record = self
                .journal
                .record_checkpoint(
                    intent,
                    &checkpoint.idempotency_key,
                    PluginLifecycleCheckpointOutcome::Applied,
                    publication.evidence().digest().to_string(),
                    None,
                    completed_at_ms(),
                )
                .await?;
        }
        if record
            .next_checkpoint()
            .is_some_and(|next| next.kind == PluginLifecycleCheckpointKind::CapabilityHidden)
        {
            return Err(coordinator_error(
                "Disablement did not record its atomic capability hide.",
            ));
        }
        let committed_at_ms = completed_at_ms();
        grants
            .commit_cutover(publication.cutover(), committed_at_ms, committed_at_ms)
            .await?;
        while let Some(next) = record.next_checkpoint().cloned() {
            if next.kind != PluginLifecycleCheckpointKind::CallsDrained {
                break;
            }
            record = self
                .execute_and_record(intent, manifest, &next, &completed_at_ms)
                .await?;
        }
        if record
            .next_checkpoint()
            .is_some_and(|next| next.kind == PluginLifecycleCheckpointKind::CallsDrained)
        {
            return Err(coordinator_error(
                "Disablement did not drain calls admitted by the prior capability generation.",
            ));
        }
        grants.retire().await?;
        self.apply(intent, manifest, completed_at_ms).await
    }

    async fn prepare_enablement_for_cutover(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        if intent.action != super::PluginLifecycleAction::Enable {
            return Err(coordinator_error(
                "Only enable operations can stage an installed package for publication.",
            ));
        }
        let mut record = self.journal.begin(intent).await?;
        loop {
            let Some(checkpoint) = record.next_checkpoint().cloned() else {
                return Ok(record);
            };
            if checkpoint.kind == PluginLifecycleCheckpointKind::CapabilityPublished {
                return Ok(record);
            }
            record = self
                .execute_and_record(intent, manifest, &checkpoint, completed_at_ms)
                .await?;
        }
    }
}

fn validate_enablement_grant_binding(
    envelope: &PluginOperationPlanEnvelope,
    intent: &PluginLifecycleIntent,
    manifest: &ExtensionManifest,
    grants: &PluginGrantLifecycleUnit,
    action: PluginOperationAction,
) -> UseResult<()> {
    envelope.validate()?;
    grants.validate_envelope(envelope)?;
    validate_manifest_binding(intent, manifest)?;
    let lifecycle_action = match action {
        PluginOperationAction::Enable => super::PluginLifecycleAction::Enable,
        PluginOperationAction::Disable => super::PluginLifecycleAction::Disable,
        _ => {
            return Err(coordinator_error(
                "Grant-bearing enablement accepts only enable or disable plans.",
            ));
        }
    };
    if envelope.plan.action != action
        || envelope.plan.operation_id != intent.operation_id
        || envelope.plan.scope.id != intent.scope_id
        || envelope.plan.package_id != intent.package_id
        || envelope.plan_digest != intent.plan_digest
        || intent.action != lifecycle_action
        || !matches!(
            envelope.plan.packages.as_slice(),
            [package]
                if package.package_id == intent.package_id
                    && package.change == PlanPackageChangeKind::Retain
                    && package.before == package.after
        )
    {
        return Err(coordinator_error(
            "The enablement lifecycle, retained artifact plan, and Grant operation disagree.",
        ));
    }
    Ok(())
}
