use super::*;
use crate::capability_catalog_store::CapabilityGatewayCatalogPublication;

pub(super) const CATALOG: &[u8] =
    include_bytes!("../../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");

pub(in crate::control_store) fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

pub(in crate::control_store) fn catalog_binding(
    installation: &InstallationId,
    generation: u64,
) -> ControlCapabilityCatalogBinding {
    ControlCapabilityCatalogBinding::from_publication(&CapabilityGatewayCatalogPublication {
        digest: digest('8'),
        installation: installation.clone(),
        generation,
        revision: digest('9'),
    })
    .unwrap()
}

pub(in crate::control_store) fn operation(id: &str) -> ReviewedControlOperation {
    operation_at(id, PluginOperationAction::Install, 0, 0)
}

pub(in crate::control_store) fn operation_at(
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

pub(in crate::control_store) fn control_installation() -> InstallationId {
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

pub(in crate::control_store) fn snapshot(
    installation: InstallationId,
    generation: u64,
) -> InstallationSnapshot {
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

pub(in crate::control_store) fn transition(
    installation: InstallationId,
    reviewed: &ReviewedControlOperation,
) -> ControlTransition {
    let committed_at_ms = reviewed.reviewed_at_ms + 10;
    let projection = if reviewed.action() == PluginOperationAction::Install
        && reviewed.expected_generation == 0
    {
        let projected = reviewed
            .project_generation(None, &ControlProjectionHistory::default(), committed_at_ms)
            .unwrap();
        TransitionProjectionFixture::from(projected)
    } else {
        let snapshot = snapshot(installation.clone(), reviewed.target_generation().unwrap());
        let package_id = snapshot.packages[0].package_id().to_string();
        let capability = ControlCapabilitySelection {
            generation: reviewed.target_capability_generation().unwrap(),
            descriptor_digest: digest('5'),
        };
        let effects = vec![ControlEffectIntent::new(
            0,
            installation.clone(),
            reviewed.plan_digest().to_string(),
            reviewed.action(),
            snapshot.generation,
            capability_subject(reviewed, &capability),
            ControlEffectOwner::CapabilityIndex,
            ControlEffectKind::CapabilityCutover,
            true,
        )
        .unwrap()];
        TransitionProjectionFixture {
            snapshot,
            package_lifecycles: vec![ControlPackageLifecycle {
                package_id,
                lifecycle_generation: 41,
            }],
            grants: Vec::new(),
            provider_selections: Vec::new(),
            capability,
            effects,
        }
    };
    transition_from_projection(installation, reviewed, projection, None, committed_at_ms)
}

pub(in crate::control_store) fn projected_transition(
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
        TransitionProjectionFixture::from(projected),
        Some(prior),
        committed_at_ms,
    )
}

struct TransitionProjectionFixture {
    snapshot: InstallationSnapshot,
    package_lifecycles: Vec<ControlPackageLifecycle>,
    grants: Vec<ControlGrantSelection>,
    provider_selections: Vec<ControlProviderSelection>,
    capability: ControlCapabilitySelection,
    effects: Vec<ControlEffectIntent>,
}

impl From<ProjectedControlGeneration> for TransitionProjectionFixture {
    fn from(projected: ProjectedControlGeneration) -> Self {
        Self {
            snapshot: projected.snapshot,
            package_lifecycles: projected.package_lifecycles,
            grants: projected.grants,
            provider_selections: projected.provider_selections,
            capability: projected.capability,
            effects: projected.effects,
        }
    }
}

fn transition_from_projection(
    _installation: InstallationId,
    reviewed: &ReviewedControlOperation,
    projection: TransitionProjectionFixture,
    prior: Option<&ControlGeneration>,
    committed_at_ms: u64,
) -> ControlTransition {
    let TransitionProjectionFixture {
        snapshot,
        package_lifecycles,
        grants,
        provider_selections,
        capability,
        effects,
    } = projection;
    let _ = prior;
    ControlTransition {
        operation_id: reviewed.operation_id().to_string(),
        plan_digest: reviewed.plan_digest().to_string(),
        snapshot,
        package_lifecycles,
        grants,
        provider_selections,
        capability,
        effects,
        committed_at_ms,
    }
}

pub(super) fn capability_subject(
    reviewed: &ReviewedControlOperation,
    capability: &ControlCapabilitySelection,
) -> ControlEffectSubject {
    ControlEffectSubject::Installation {
        expected_capability_generation: reviewed.expected_capability_generation,
        capability_generation: reviewed.target_capability_generation().unwrap(),
        descriptor_digest: capability.descriptor_digest.clone(),
    }
}

pub(in crate::control_store) async fn initialized_store() -> (tempfile::TempDir, ControlStore) {
    let temporary = tempfile::tempdir().unwrap();
    let store = ControlStore::new(temporary.path().join("state"), control_installation()).unwrap();
    store.initialize().await.unwrap();
    (temporary, store)
}

pub(in crate::control_store) fn claim(
    operation_id: &str,
    token: &str,
    now_ms: u64,
    lease_until_ms: u64,
    explicit_reconciliation: bool,
) -> ControlEffectClaim {
    ControlEffectClaim {
        operation_id: operation_id.to_string(),
        worker_id: "worker:test".to_string(),
        claim_token: token.to_string(),
        now_ms,
        lease_until_ms,
        explicit_reconciliation,
    }
}

pub(in crate::control_store) fn observation(
    operation_id: &str,
    intent: &ControlEffectIntent,
    claim_token: &str,
    outcome: ControlEffectOutcome,
    seed: char,
    observed_at_ms: u64,
) -> ControlEffectObservation {
    ControlEffectObservation {
        operation_id: operation_id.to_string(),
        idempotency_key: intent.idempotency_key.clone(),
        claim_token: claim_token.to_string(),
        outcome,
        application: matches!(outcome, ControlEffectOutcome::Applied)
            .then(|| application(intent, seed)),
        failure_evidence_digest: (!matches!(outcome, ControlEffectOutcome::Applied))
            .then(|| digest(seed)),
        error_code: match outcome {
            ControlEffectOutcome::Applied => None,
            ControlEffectOutcome::Deferred => Some("provider.temporarily_unavailable".to_string()),
            ControlEffectOutcome::Rejected => Some("provider.rejected".to_string()),
            ControlEffectOutcome::Unknown => Some("provider.acceptance_unknown".to_string()),
        },
        observed_at_ms,
        retry_not_before_ms: matches!(outcome, ControlEffectOutcome::Deferred)
            .then(|| observed_at_ms + 1),
    }
}

pub(super) fn application(intent: &ControlEffectIntent, seed: char) -> ControlAppliedEffect {
    let receipt_digest = digest(seed);
    let state = match intent.kind {
        ControlEffectKind::SurfacePrepare => ControlSurfaceObservationState::Prepared,
        ControlEffectKind::SurfaceStop => ControlSurfaceObservationState::Stopped,
        ControlEffectKind::SurfaceRemove => ControlSurfaceObservationState::Removed,
        ControlEffectKind::CapabilityCutover | ControlEffectKind::CallsDrain => {
            ControlSurfaceObservationState::Prepared
        }
    };
    let evidence = match (&intent.owner, &intent.subject) {
        (
            ControlEffectOwner::CapabilityIndex,
            ControlEffectSubject::Installation {
                capability_generation,
                descriptor_digest,
                ..
            },
        ) => ControlAppliedEffectEvidence::CapabilityIndex {
            capability_generation: *capability_generation,
            descriptor_digest: descriptor_digest.clone(),
            catalog: catalog_binding(&intent.installation, *capability_generation),
            receipt_digest,
        },
        (
            ControlEffectOwner::InvocationLeases,
            ControlEffectSubject::Package {
                package_id,
                lifecycle_generation,
                ..
            },
        ) => ControlAppliedEffectEvidence::InvocationLeases {
            package_id: package_id.clone(),
            lifecycle_generation: *lifecycle_generation,
            receipt_digest,
        },
        (
            ControlEffectOwner::RuntimeProvider {
                provider_id,
                selection_digest,
            },
            ControlEffectSubject::Surface { .. },
        ) => ControlAppliedEffectEvidence::RuntimeProvider {
            state,
            provider_id: provider_id.clone(),
            selection_digest: selection_digest.clone(),
            receipt_digest,
            binding: (intent.kind == ControlEffectKind::SurfacePrepare)
                .then_some(ControlRuntimeBindingObservation::Task),
        },
        (ControlEffectOwner::FlowHost, ControlEffectSubject::Surface { .. }) => {
            ControlAppliedEffectEvidence::FlowHost {
                state,
                receipt_digest,
                artifact_digest: (intent.kind == ControlEffectKind::SurfacePrepare)
                    .then(|| digest('a')),
            }
        }
        (ControlEffectOwner::KnowledgeHost, ControlEffectSubject::Surface { .. }) => {
            ControlAppliedEffectEvidence::KnowledgeHost {
                state,
                receipt_digest,
                projection_digest: (intent.kind == ControlEffectKind::SurfacePrepare)
                    .then(|| digest('b')),
            }
        }
        (ControlEffectOwner::SkillHost, ControlEffectSubject::Surface { .. }) => {
            ControlAppliedEffectEvidence::SkillHost {
                state,
                receipt_digest,
                content_digest: (intent.kind == ControlEffectKind::SurfacePrepare)
                    .then(|| digest('c')),
            }
        }
        (ControlEffectOwner::UiHost, ControlEffectSubject::Surface { .. }) => {
            ControlAppliedEffectEvidence::UiHost {
                state,
                receipt_digest,
                content_digest: (intent.kind == ControlEffectKind::SurfacePrepare)
                    .then(|| digest('d')),
            }
        }
        _ => panic!("the test effect owner and subject must agree"),
    };
    ControlAppliedEffect::new(intent, evidence).unwrap()
}

pub(super) fn canonical_json<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).unwrap();
    bytes
}

pub(in crate::control_store) async fn apply_all_effects(
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
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence % 16, 16).unwrap(),
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
