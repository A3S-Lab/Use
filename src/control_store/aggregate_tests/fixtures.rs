use super::*;

pub(super) const CATALOG: &[u8] =
    include_bytes!("../../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");

pub(super) fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

pub(super) fn effect_key(operation_id: &str, sequence: u32) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(format!("{operation_id}\n{sequence}").as_bytes())
    )
}

pub(super) fn operation(id: &str) -> ReviewedControlOperation {
    operation_at(id, PluginOperationAction::Install, 0, 0)
}

pub(super) fn operation_at(
    id: &str,
    action: PluginOperationAction,
    expected_generation: u64,
    expected_capability_generation: u64,
) -> ReviewedControlOperation {
    operation_at_with_policy(
        id,
        action,
        expected_generation,
        expected_capability_generation,
        'a',
    )
}

pub(super) fn operation_at_with_policy(
    id: &str,
    action: PluginOperationAction,
    expected_generation: u64,
    expected_capability_generation: u64,
    policy_seed: char,
) -> ReviewedControlOperation {
    let reviewed_at_ms = 10 + expected_generation * 100;
    let (envelope, confirmation) = operation_plan(
        id,
        action,
        expected_generation,
        expected_capability_generation,
        reviewed_at_ms,
        policy_seed,
    );
    ReviewedControlOperation::new(
        envelope,
        Some(confirmation),
        None,
        Vec::new(),
        expected_generation,
        expected_capability_generation,
        reviewed_at_ms,
    )
    .unwrap()
}

pub(super) fn operation_plan(
    operation_id: &str,
    action: PluginOperationAction,
    expected_generation: u64,
    expected_capability_generation: u64,
    reviewed_at_ms: u64,
    policy_seed: char,
) -> (PluginOperationPlanEnvelope, PluginOperationConfirmation) {
    let prior_lock = package_lock();
    let (candidate_lock, packages) = if action == PluginOperationAction::Upgrade {
        let candidate_lock = replacement_package_lock(&prior_lock);
        let prior = prior_lock.package(&prior_lock.root_package_id).unwrap();
        let candidate = candidate_lock
            .package(&candidate_lock.root_package_id)
            .unwrap();
        let transition = candidate
            .catalog
            .replace_transition(&prior.catalog, PlanPackageRole::Root, &[], &[])
            .unwrap();
        (candidate_lock, vec![transition])
    } else {
        let package = prior_lock.package(&prior_lock.root_package_id).unwrap();
        let transition = match action {
            PluginOperationAction::Install => package
                .catalog
                .install_transition(PlanPackageRole::Root, &[])
                .unwrap(),
            PluginOperationAction::Enable | PluginOperationAction::Disable => {
                let state = package.catalog.selected_state(&[]).unwrap();
                PlannedPackageTransition::resolved(
                    package.package_id(),
                    PlanPackageRole::Root,
                    PlanPackageChangeKind::Retain,
                    Some(state.clone()),
                    Some(state),
                    None,
                )
                .unwrap()
            }
            PluginOperationAction::Uninstall => package
                .catalog
                .remove_transition(PlanPackageRole::Root, &[])
                .unwrap(),
            PluginOperationAction::Upgrade => unreachable!(),
        };
        (prior_lock.clone(), vec![transition])
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
    let impact = match action {
        PluginOperationAction::Install => PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: 1,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Upgrade => PlannedOperationImpact {
            download_bytes: 1,
            installed_bytes_after: 1,
            reclaimed_bytes: 1,
            drain_required: private_service_before,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Enable => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 1,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Disable => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 1,
            reclaimed_bytes: 0,
            drain_required: private_service_before,
            retained_data: true,
            okf_changes: Vec::new(),
        },
        PluginOperationAction::Uninstall => PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 0,
            reclaimed_bytes: 1,
            drain_required: private_service_before,
            retained_data: true,
            okf_changes: Vec::new(),
        },
    };
    let (enabled_before, enabled_after) = match action {
        PluginOperationAction::Install | PluginOperationAction::Enable => (false, true),
        PluginOperationAction::Upgrade => (true, true),
        PluginOperationAction::Uninstall | PluginOperationAction::Disable => (true, false),
    };
    let draft = PluginOperationPlanDraft::new(
        action,
        prior_lock.root_package_id.clone(),
        "runtime:local",
        packages,
        providers,
        vec![PlannedWorkspaceImpact {
            scope_id: control_installation().id,
            grant_before_digest: None,
            grant_after_digest: None,
            enabled_before,
            enabled_after,
        }],
        impact,
        PlannedStateEvidence {
            state_revision: expected_generation + 1,
            capability_generation: expected_capability_generation,
            receipt_digest: (action != PluginOperationAction::Install).then(|| digest('9')),
        },
    )
    .unwrap();
    let created_at_ms = reviewed_at_ms - 2;
    let confirmed_at_ms = reviewed_at_ms - 1;
    let plan = draft
        .bind(PluginOperationPlanBinding {
            operation_id: operation_id.to_string(),
            created_at_ms,
            expires_at_ms: reviewed_at_ms + 1_000,
            scope: control_installation(),
            authority: PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Ask,
                policy_digest: digest(policy_seed),
                confirmation_required: true,
            },
        })
        .unwrap();
    let envelope = if action == PluginOperationAction::Upgrade {
        PluginOperationPlanEnvelope::new_with_upgrade_package_locks(
            plan,
            prior_lock,
            candidate_lock,
        )
        .unwrap()
    } else {
        PluginOperationPlanEnvelope::new_with_package_lock(plan, candidate_lock).unwrap()
    };
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: operation_id.to_string(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms,
    };
    confirmation.validate().unwrap();
    (envelope, confirmation)
}

pub(super) fn plan_providers(
    action: PluginOperationAction,
    packages: &[PlannedPackageTransition],
) -> Vec<PlannedProviderEvidence> {
    if matches!(
        action,
        PluginOperationAction::Disable | PluginOperationAction::Uninstall
    ) {
        return Vec::new();
    }
    let mut providers = packages
        .iter()
        .filter_map(|package| {
            package
                .after
                .as_ref()
                .map(|state| (package.package_id.as_str(), state))
        })
        .flat_map(|(package_id, state)| {
            state.release.surfaces.iter().filter_map(move |surface| {
                if !matches!(
                    surface.kind,
                    PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                ) {
                    return None;
                }
                let reference = surface.reference();
                let permission = state
                    .permissions
                    .surfaces
                    .iter()
                    .find(|permission| permission.surface == reference)?;
                Some(PlannedProviderEvidence {
                    surface: PlanQualifiedSurfaceRef {
                        package_id: package_id.to_string(),
                        surface: reference,
                    },
                    provider_id: "provider:test".to_string(),
                    provider_build_id: "provider-build:test".to_string(),
                    capability_digest: digest('6'),
                    semantics_profile_digest: digest('7'),
                    enforcement: if permission.native_execution {
                        PlanEnforcementProfile::Sandbox
                    } else {
                        PlanEnforcementProfile::Container
                    },
                })
            })
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.surface.cmp(&right.surface));
    providers
}

pub(super) fn replacement_package_lock(prior: &PluginPackageLock) -> PluginPackageLock {
    let mut candidate = prior.clone();
    let package = &mut candidate.packages[0];
    let prior_version = package.catalog.record.version.clone();
    package.catalog.record.version = "2.1.0".to_string();
    package.catalog.record.archive.target_name = package
        .catalog
        .record
        .archive
        .target_name
        .replace(&format!("/{prior_version}/"), "/2.1.0/")
        .replace(&format!("-{prior_version}-"), "-2.1.0-");
    if let Some(planning) = &mut package.catalog.record.planning {
        planning.target_name = planning
            .target_name
            .replace(&format!("/{prior_version}/"), "/2.1.0/");
    }
    package.catalog.record.archive.sha256 = digest('d');
    package.catalog.record.package.sha256 = Some(digest('d'));
    package.catalog.record.package.manifest_sha256 = Some(digest('e'));
    package.catalog.provenance.targets_version += 1;
    package.catalog.provenance.catalog_record_digest =
        package.catalog.record.descriptor_digest().unwrap();
    candidate.validate().unwrap();
    candidate
}

pub(super) fn control_installation() -> InstallationId {
    InstallationId::new(InstallationKind::Workspace, "workspace-01").unwrap()
}

pub(super) fn package_lock() -> PluginPackageLock {
    let record = PluginCatalogRecord::from_json(CATALOG).unwrap();
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

pub(super) fn snapshot(installation: InstallationId, generation: u64) -> InstallationSnapshot {
    let package_lock = package_lock();
    let selections = package_lock
        .packages
        .iter()
        .map(|package| {
            let selected_surfaces = package
                .catalog
                .record
                .resolve_surfaces(&[])
                .unwrap()
                .into_iter()
                .map(|surface| surface.reference())
                .collect();
            InstallationPackageSelection::new(package.clone(), generation, true, selected_surfaces)
                .unwrap()
        })
        .collect();
    InstallationSnapshot::from_root_locks(
        installation,
        generation,
        package_lock.host.clone(),
        vec![(
            InstallationRootSelection::new(package_lock.root_package_id.clone(), 5).unwrap(),
            package_lock.clone(),
        )],
        selections,
    )
    .unwrap()
}

pub(super) fn transition(
    installation: InstallationId,
    reviewed: &ReviewedControlOperation,
) -> ControlTransition {
    let committed_at_ms = reviewed.reviewed_at_ms + 10;
    let (snapshot, package_lifecycles, grants) =
        if reviewed.action() == PluginOperationAction::Install && reviewed.expected_generation == 0
        {
            let projected = reviewed
                .project_generation(None, &ControlProjectionHistory::default(), committed_at_ms)
                .unwrap();
            (
                projected.snapshot,
                projected.package_lifecycles,
                projected.grants,
            )
        } else {
            let snapshot = snapshot(installation.clone(), reviewed.target_generation().unwrap());
            let package_id = snapshot.packages[0].package_id().to_string();
            (
                snapshot,
                vec![ControlPackageLifecycle {
                    package_id,
                    lifecycle_generation: 41,
                }],
                Vec::new(),
            )
        };
    transition_from_projection(
        installation,
        reviewed,
        snapshot,
        package_lifecycles,
        grants,
        None,
        committed_at_ms,
    )
}

pub(super) fn projected_transition(
    reviewed: &ReviewedControlOperation,
    prior: &ControlGeneration,
    history: &ControlProjectionHistory,
) -> ControlTransition {
    let committed_at_ms = reviewed.reviewed_at_ms + 10;
    let projected = reviewed
        .project_generation(Some(prior), history, committed_at_ms)
        .unwrap();
    let installation = projected.snapshot.installation.clone();
    transition_from_projection(
        installation,
        reviewed,
        projected.snapshot,
        projected.package_lifecycles,
        projected.grants,
        Some(prior),
        committed_at_ms,
    )
}

fn transition_from_projection(
    installation: InstallationId,
    reviewed: &ReviewedControlOperation,
    snapshot: InstallationSnapshot,
    package_lifecycles: Vec<ControlPackageLifecycle>,
    grants: Vec<ControlGrantSelection>,
    prior: Option<&ControlGeneration>,
    committed_at_ms: u64,
) -> ControlTransition {
    let subject_package = snapshot
        .package_selection(reviewed.root_package_id())
        .or_else(|| {
            prior.and_then(|generation| {
                generation
                    .snapshot
                    .package_selection(reviewed.root_package_id())
            })
        })
        .unwrap();
    let package_id = subject_package.package_id().to_string();
    let lifecycle_generation = package_lifecycles
        .iter()
        .find(|lifecycle| lifecycle.package_id == package_id)
        .or_else(|| {
            prior.and_then(|generation| {
                generation
                    .package_lifecycles
                    .iter()
                    .find(|lifecycle| lifecycle.package_id == package_id)
            })
        })
        .unwrap()
        .lifecycle_generation;
    let target_package = snapshot.package_selection(&package_id);
    let mut bindings = Vec::new();
    if let Some(package) = target_package {
        let surface = package.selected_surfaces[0].clone();
        bindings.push(ControlProviderBinding {
            package_id: package_id.clone(),
            surface,
            provider_id: "provider:test".to_string(),
            binding_digest: digest('4'),
        });
    }
    let package_subject = ControlEffectSubject::Package {
        package_id: package_id.clone(),
        lifecycle_generation,
        package_digest: subject_package
            .package
            .catalog
            .record
            .package
            .sha256
            .clone()
            .unwrap(),
        manifest_digest: subject_package
            .package
            .catalog
            .record
            .package
            .manifest_sha256
            .clone()
            .unwrap(),
        action: lifecycle_action(reviewed.action()),
    };
    ControlTransition {
        operation_id: reviewed.operation_id().to_string(),
        plan_digest: reviewed.plan_digest().to_string(),
        snapshot,
        package_lifecycles,
        grants,
        bindings,
        capability: ControlCapabilitySelection {
            generation: reviewed.target_capability_generation().unwrap(),
            descriptor_digest: digest('5'),
        },
        effects: vec![
            ControlEffectIntent {
                sequence: 0,
                idempotency_key: effect_key(reviewed.operation_id(), 0),
                installation: installation.clone(),
                plan_digest: reviewed.plan_digest().to_string(),
                operation_action: reviewed.action(),
                installation_generation: reviewed.target_generation().unwrap(),
                subject: package_subject,
                provider_id: "provider:test".to_string(),
                kind: ControlEffectKind::PackageCommit,
                required: true,
            },
            ControlEffectIntent {
                sequence: 1,
                idempotency_key: effect_key(reviewed.operation_id(), 1),
                installation,
                plan_digest: reviewed.plan_digest().to_string(),
                operation_action: reviewed.action(),
                installation_generation: reviewed.target_generation().unwrap(),
                subject: capability_subject(reviewed),
                provider_id: "provider:test".to_string(),
                kind: ControlEffectKind::CapabilityPublish,
                required: false,
            },
        ],
        committed_at_ms,
    }
}

pub(super) fn lifecycle_action(action: PluginOperationAction) -> PluginLifecycleAction {
    match action {
        PluginOperationAction::Install => PluginLifecycleAction::Install,
        PluginOperationAction::Upgrade => PluginLifecycleAction::Upgrade,
        PluginOperationAction::Enable => PluginLifecycleAction::Enable,
        PluginOperationAction::Disable => PluginLifecycleAction::Disable,
        PluginOperationAction::Uninstall => PluginLifecycleAction::Uninstall,
    }
}

pub(super) fn capability_subject(reviewed: &ReviewedControlOperation) -> ControlEffectSubject {
    ControlEffectSubject::Installation {
        expected_capability_generation: reviewed.expected_capability_generation,
        capability_generation: reviewed.target_capability_generation().unwrap(),
        descriptor_digest: digest('5'),
    }
}

pub(super) async fn initialized_store() -> (tempfile::TempDir, ControlStore) {
    let temporary = tempfile::tempdir().unwrap();
    let store = ControlStore::new(temporary.path().join("state"), control_installation()).unwrap();
    store.initialize().await.unwrap();
    (temporary, store)
}

pub(super) fn claim(
    operation_id: &str,
    token: &str,
    now_ms: u64,
    lease_until_ms: u64,
    reconcile_unknown: bool,
) -> ControlEffectClaim {
    ControlEffectClaim {
        operation_id: operation_id.to_string(),
        worker_id: "worker:test".to_string(),
        claim_token: token.to_string(),
        now_ms,
        lease_until_ms,
        reconcile_unknown,
    }
}

pub(super) fn observation(
    operation_id: &str,
    idempotency_key: &str,
    claim_token: &str,
    outcome: ControlEffectOutcome,
    seed: char,
    observed_at_ms: u64,
) -> ControlEffectObservation {
    ControlEffectObservation {
        operation_id: operation_id.to_string(),
        idempotency_key: idempotency_key.to_string(),
        claim_token: claim_token.to_string(),
        outcome,
        evidence_digest: digest(seed),
        error_code: (!matches!(outcome, ControlEffectOutcome::Applied))
            .then(|| "provider.rejected".to_string()),
        observed_at_ms,
    }
}

pub(super) fn canonical_json<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).unwrap();
    bytes
}

pub(super) async fn apply_all_effects(
    store: &ControlStore,
    reviewed: &ReviewedControlOperation,
    start: u64,
) {
    let mut now = start;
    let mut sequence = 0_u32;
    loop {
        let token = format!("claim:{}:{sequence}", reviewed.operation_id());
        let Some(claimed) = store
            .claim_next_effect(claim(reviewed.operation_id(), &token, now, now + 10, false))
            .await
            .unwrap()
        else {
            break;
        };
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent.idempotency_key,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit((sequence % 10) + 1, 10).unwrap(),
                now + 5,
            ))
            .await
            .unwrap();
        now += 20;
        sequence += 1;
    }
    store
        .complete_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('f'),
            now,
        )
        .await
        .unwrap();
}
