use super::*;

#[derive(Clone, Copy)]
pub(super) struct ExpectedEffect {
    kind: ControlEffectKind,
    owner_kind: &'static str,
    surface_id: Option<&'static str>,
    installation_generation: u64,
    action: Option<PluginLifecycleAction>,
    required: bool,
}

pub(super) const fn expected_installation(
    kind: ControlEffectKind,
    installation_generation: u64,
) -> ExpectedEffect {
    ExpectedEffect {
        kind,
        owner_kind: "capability-index",
        surface_id: None,
        installation_generation,
        action: None,
        required: true,
    }
}

pub(super) const fn expected_package(
    kind: ControlEffectKind,
    owner_kind: &'static str,
    installation_generation: u64,
    action: PluginLifecycleAction,
) -> ExpectedEffect {
    ExpectedEffect {
        kind,
        owner_kind,
        surface_id: None,
        installation_generation,
        action: Some(action),
        required: true,
    }
}

pub(super) const fn expected_surface(
    kind: ControlEffectKind,
    owner_kind: &'static str,
    surface_id: &'static str,
    installation_generation: u64,
    action: PluginLifecycleAction,
    required: bool,
) -> ExpectedEffect {
    ExpectedEffect {
        kind,
        owner_kind,
        surface_id: Some(surface_id),
        installation_generation,
        action: Some(action),
        required,
    }
}

pub(super) fn assert_effects(actual: &[ControlEffectIntent], expected: &[ExpectedEffect]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(usize::try_from(actual.sequence).unwrap(), index);
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(actual.owner.kind_name(), expected.owner_kind);
        assert_eq!(
            actual.installation_generation,
            expected.installation_generation
        );
        assert_eq!(actual.required, expected.required);
        assert_eq!(
            actual.idempotency_key,
            actual.derived_idempotency_key().unwrap()
        );
        match &actual.subject {
            ControlEffectSubject::Installation { .. } => {
                assert_eq!(expected.surface_id, None);
                assert_eq!(expected.action, None);
            }
            ControlEffectSubject::Package {
                package_id, action, ..
            } => {
                assert_eq!(package_id, "acme/knowledge");
                assert_eq!(expected.surface_id, None);
                assert_eq!(Some(*action), expected.action);
            }
            ControlEffectSubject::Surface {
                package_id,
                action,
                surface,
                ..
            } => {
                assert_eq!(package_id, "acme/knowledge");
                assert_eq!(Some(surface.id.as_str()), expected.surface_id);
                assert_eq!(Some(*action), expected.action);
            }
        }
    }
}

pub(super) fn rekey(effect: &mut ControlEffectIntent) {
    effect.idempotency_key = effect.derived_idempotency_key().unwrap();
}

pub(super) fn generation(
    operation: &ReviewedControlOperation,
    projected: &ProjectedControlGeneration,
) -> ControlGeneration {
    ControlGeneration {
        operation_id: operation.operation_id().to_string(),
        snapshot: projected.snapshot.clone(),
        snapshot_digest: projected.snapshot.descriptor_digest().unwrap(),
        package_lifecycles: projected.package_lifecycles.clone(),
        grants: projected.grants.clone(),
        provider_selections: projected.provider_selections.clone(),
        capability: projected.capability.clone(),
        capability_status: ControlCapabilityStatus::Candidate,
        capability_published_at_ms: None,
        committed_at_ms: operation.reviewed_at_ms + 10,
    }
}

pub(super) fn optional_surface_operation(operation_id: &str) -> ReviewedControlOperation {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let selected = record
        .surfaces
        .iter_mut()
        .find(|surface| surface.kind == PluginSurfaceKind::Skill)
        .unwrap();
    selected.optional = true;
    let selected = selected.reference();
    record.validate().unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: digest('f'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 2,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    let lock = PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap();
    let package = lock.package(&lock.root_package_id).unwrap();
    let packages = vec![package
        .catalog
        .install_transition(PlanPackageRole::Root, &[selected])
        .unwrap()];
    let reviewed_at_ms = 10;
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        lock.root_package_id.clone(),
        "runtime:local",
        packages.clone(),
        plan_providers(PluginOperationAction::Install, &packages),
        vec![PlannedWorkspaceImpact {
            scope_id: control_installation().id,
            grant_before_digest: None,
            grant_after_digest: None,
            enabled_before: false,
            enabled_after: true,
        }],
        PlannedOperationImpact {
            download_bytes: package.catalog.record.archive.length,
            installed_bytes_after: package.catalog.record.package.expanded_bytes,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 1,
            capability_generation: 0,
            receipt_digest: None,
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
        created_at_ms: reviewed_at_ms - 2,
        expires_at_ms: reviewed_at_ms + 1_000,
        scope: control_installation(),
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('a'),
            confirmation_required: true,
        },
    })
    .unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(plan, lock).unwrap();
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: operation_id.to_string(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: reviewed_at_ms - 1,
    };
    ReviewedControlOperation::new(
        envelope,
        Some(confirmation),
        None,
        Vec::new(),
        0,
        0,
        reviewed_at_ms,
    )
    .unwrap()
}
