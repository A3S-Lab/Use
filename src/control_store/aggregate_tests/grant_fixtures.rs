use a3s_use_core::{
    PlanActor, PlanAuthority, PlanPackageChangeKind, PlanPackageRole, PlanPolicyDecision,
    PlannedOperationImpact, PlannedPackageTransition, PlannedStateEvidence, PluginCatalogRecord,
    PluginGrantConfirmation, PluginOperationAction, PluginOperationConfirmation,
    PluginOperationPlanBinding, PluginOperationPlanDraft, PluginOperationPlanEnvelope,
    PluginPackageLock, PluginPackageResolver, PluginWorkspaceGrantSnapshot,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, WorkspaceGrantEvidence,
    PLUGIN_GRANT_CONFIRMATION_SCHEMA, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};

use super::super::model::ControlGrantAuthorizationEvidence;
use super::*;

const PERMISSIONED_CATALOG: &[u8] =
    include_bytes!("../../../crates/core/fixtures/plugins/catalog-record-v3.json");

pub(super) fn bind_action_effects(
    mut transition: ControlTransition,
    reviewed: &ReviewedControlOperation,
) -> ControlTransition {
    match reviewed.action() {
        PluginOperationAction::Disable => {
            let package_subject = transition.effects[0].subject.clone();
            transition.effects[0].kind = ControlEffectKind::CapabilityHide;
            transition.effects[0].subject = capability_subject(reviewed);
            transition.effects[1].kind = ControlEffectKind::GrantRevoke;
            transition.effects[1].subject = package_subject;
        }
        PluginOperationAction::Enable => {
            transition.effects[0].kind = ControlEffectKind::GrantApply;
        }
        PluginOperationAction::Uninstall => {
            let package_subject = transition.effects[0].subject.clone();
            transition.effects[0].kind = ControlEffectKind::CapabilityHide;
            transition.effects[0].subject = capability_subject(reviewed);
            transition.effects[1].kind = ControlEffectKind::PackageRemove;
            transition.effects[1].installation_generation = reviewed.expected_generation;
            transition.effects[1].subject = package_subject;
        }
        PluginOperationAction::Install | PluginOperationAction::Upgrade => {}
    }
    transition
}

pub(super) fn reviewed_grant_operation(
    operation_id: &str,
    action: PluginOperationAction,
    prior: Option<&ControlGeneration>,
    snapshot_override: Option<PluginWorkspaceGrantSnapshot>,
) -> ReviewedControlOperation {
    reviewed_grant_operation_for(
        &control_installation(),
        operation_id,
        action,
        prior,
        snapshot_override,
        None,
    )
}

pub(super) fn reviewed_grant_operation_for(
    installation: &InstallationId,
    operation_id: &str,
    action: PluginOperationAction,
    prior: Option<&ControlGeneration>,
    snapshot_override: Option<PluginWorkspaceGrantSnapshot>,
    install_lock: Option<PluginPackageLock>,
) -> ReviewedControlOperation {
    let expected_generation = prior.map_or(0, |generation| generation.snapshot.generation);
    let expected_capability_generation =
        prior.map_or(0, |generation| generation.capability.generation);
    let current_lock = prior.map(|generation| {
        generation
            .snapshot
            .package_locks()
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    });
    let (candidate_lock, packages) = match action {
        PluginOperationAction::Install => {
            let lock = install_lock.unwrap_or_else(permissioned_package_lock);
            let package = lock.package(&lock.root_package_id).unwrap();
            let transition = package
                .catalog
                .install_transition(PlanPackageRole::Root, &[])
                .unwrap();
            (lock, vec![transition])
        }
        PluginOperationAction::Upgrade => {
            let current = current_lock.as_ref().unwrap();
            let candidate = replacement_package_lock(current);
            let before = current.package(&current.root_package_id).unwrap();
            let after = candidate.package(&candidate.root_package_id).unwrap();
            let transition = after
                .catalog
                .replace_transition(&before.catalog, PlanPackageRole::Root, &[], &[])
                .unwrap();
            (candidate, vec![transition])
        }
        PluginOperationAction::Enable | PluginOperationAction::Disable => {
            let current = current_lock.as_ref().unwrap();
            let package = current.package(&current.root_package_id).unwrap();
            let selected = prior
                .unwrap()
                .snapshot
                .package_selection(package.package_id())
                .unwrap();
            let state = package
                .catalog
                .selected_state(&selected.selected_surfaces)
                .unwrap();
            let transition = PlannedPackageTransition::resolved(
                package.package_id(),
                PlanPackageRole::Root,
                PlanPackageChangeKind::Retain,
                Some(state.clone()),
                Some(state),
                None,
            )
            .unwrap();
            (current.clone(), vec![transition])
        }
        PluginOperationAction::Uninstall => {
            let current = current_lock.as_ref().unwrap();
            let package = current.package(&current.root_package_id).unwrap();
            let selected = prior
                .unwrap()
                .snapshot
                .package_selection(package.package_id())
                .unwrap();
            let transition = package
                .catalog
                .remove_transition(PlanPackageRole::Root, &selected.selected_surfaces)
                .unwrap();
            (current.clone(), vec![transition])
        }
    };
    let providers = plan_providers(action, &packages);
    let private_service_before = packages.iter().any(|package| {
        package.before.as_ref().is_some_and(|state| {
            state
                .permissions
                .surfaces
                .iter()
                .any(|permission| permission.private_service)
        })
    });
    let package_bytes = candidate_lock
        .packages
        .iter()
        .map(|package| package.catalog.record.package.expanded_bytes)
        .sum();
    let impact = match action {
        PluginOperationAction::Install => PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: package_bytes,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Upgrade => PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: package_bytes,
            reclaimed_bytes: 1,
            drain_required: private_service_before,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Enable => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: package_bytes,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Disable => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: package_bytes,
            reclaimed_bytes: 0,
            drain_required: private_service_before,
            retained_data: true,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Uninstall => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 0,
            reclaimed_bytes: package_bytes,
            drain_required: private_service_before,
            retained_data: true,
            okf_changes: Vec::new(),
        },
    };
    let reviewed_at_ms = 1_000 + expected_generation * 1_000;
    let binding = PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
        created_at_ms: reviewed_at_ms - 2,
        expires_at_ms: reviewed_at_ms + 500,
        scope: installation.clone(),
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('a'),
            confirmation_required: true,
        },
    };
    let snapshot = snapshot_override
        .unwrap_or_else(|| grant_snapshot(installation, prior, expected_generation + 1));
    let mut draft = PluginOperationPlanDraft::new(
        action,
        candidate_lock.root_package_id.clone(),
        "runtime:local",
        packages,
        providers,
        Vec::new(),
        impact,
        PlannedStateEvidence {
            state_revision: expected_generation + 1,
            capability_generation: expected_capability_generation,
            receipt_digest: (action != PluginOperationAction::Install).then(|| digest('9')),
        },
    )
    .unwrap();
    crate::cognitive_package::bind_cognitive_package_grant_impacts(&mut draft, &binding, &snapshot)
        .unwrap();
    let plan = draft.bind(binding).unwrap();
    let envelope = match action {
        PluginOperationAction::Upgrade => {
            PluginOperationPlanEnvelope::new_with_upgrade_package_locks(
                plan,
                current_lock.unwrap(),
                candidate_lock,
            )
            .unwrap()
        }
        _ => PluginOperationPlanEnvelope::new_with_package_lock(plan, candidate_lock).unwrap(),
    };
    let planned =
        crate::cognitive_package::reconstruct_planned_workspace_grants(&envelope.plan, &snapshot)
            .unwrap()
            .unwrap();
    let confirmed_at_ms = reviewed_at_ms - 1;
    let operation_confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: operation_id.to_string(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms,
    };
    let grant_confirmations = planned
        .change_set
        .changes
        .iter()
        .filter_map(|change| change.after.as_ref())
        .map(|proposal| PluginGrantConfirmation {
            schema: PLUGIN_GRANT_CONFIRMATION_SCHEMA.to_string(),
            operation_id: operation_id.to_string(),
            plan_digest: envelope.plan_digest.clone(),
            proposal_digest: proposal.descriptor_digest().unwrap(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms,
        })
        .collect();
    ReviewedControlOperation::new(
        envelope,
        Some(operation_confirmation),
        Some(ControlGrantAuthorizationEvidence {
            snapshot: planned.snapshot,
            change_set: planned.change_set,
        }),
        grant_confirmations,
        expected_generation,
        expected_capability_generation,
        reviewed_at_ms,
    )
    .unwrap()
}

fn permissioned_package_lock() -> PluginPackageLock {
    let record = PluginCatalogRecord::from_json(PERMISSIONED_CATALOG).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: digest('f'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap()
}

pub(super) fn permissioned_package_lock_named(package_id: &str, seed: char) -> PluginPackageLock {
    let mut record = PluginCatalogRecord::from_json(PERMISSIONED_CATALOG).unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    record.package_id = package_id.to_string();
    record.publisher = publisher.to_string();
    record.display_name = format!("{publisher} {name}");
    record.description = format!("Permission-bearing fixture for {package_id}.");
    record.repository = format!("https://github.com/{publisher}/{name}");
    record.archive.target_name = format!(
        "extensions/{package_id}/2.0.0/stable/linux-x86_64/{publisher}-{name}-2.0.0-linux-x86_64.tar.gz"
    );
    record.archive.sha256 = digest(seed);
    record.package.sha256 = Some(digest(seed));
    record.package.manifest_sha256 = Some(digest('e'));
    if let Some(planning) = &mut record.planning {
        planning.target_name =
            format!("extensions/{package_id}/2.0.0/stable/linux-x86_64/planning-v1.json");
    }
    record.validate().unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: digest('f'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap()
}

pub(super) fn grant_snapshot(
    installation: &InstallationId,
    prior: Option<&ControlGeneration>,
    state_revision: u64,
) -> PluginWorkspaceGrantSnapshot {
    let snapshot = PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: installation.id.clone(),
        state_revision,
        grants: prior
            .into_iter()
            .flat_map(|generation| &generation.grants)
            .map(|selection| WorkspaceGrantEvidence {
                package_id: selection.package_id().to_string(),
                package_digest: selection.grant.package_digest.clone(),
                receipt_revision: selection.receipt_revision,
                grant_digest: selection.grant_digest.clone(),
            })
            .collect(),
    };
    snapshot.validate().unwrap();
    snapshot
}

pub(super) fn generation_from_projection(
    operation: &ReviewedControlOperation,
    projected: &ProjectedControlGeneration,
) -> ControlGeneration {
    ControlGeneration {
        operation_id: operation.operation_id().to_string(),
        snapshot: projected.snapshot.clone(),
        snapshot_digest: projected.snapshot.descriptor_digest().unwrap(),
        package_lifecycles: projected.package_lifecycles.clone(),
        grants: projected.grants.clone(),
        bindings: Vec::new(),
        capability: ControlCapabilitySelection {
            generation: operation.target_capability_generation().unwrap(),
            descriptor_digest: digest('5'),
        },
        capability_status: ControlCapabilityStatus::Candidate,
        capability_published_at_ms: None,
        committed_at_ms: operation.reviewed_at_ms + 10,
    }
}
