use a3s_use_core::{
    PlanActor, PlanAuthority, PlanEnforcementProfile, PlanPackageChangeKind, PlanPackageRole,
    PlanPolicyDecision, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind, PlannedOperationImpact,
    PlannedPackageState, PlannedPackageTransition, PlannedPluginRelease, PlannedProviderEvidence,
    PlannedSecretChange, PlannedSecretChangeKind, PlannedStateEvidence, PlannedSurfaceChange,
    PlannedWorkspaceImpact, PluginCatalogRecord, PluginOperationAction,
    PluginOperationConfirmation, PluginOperationPlan, PluginOperationPlanEnvelope,
    PluginPlanSource, PluginSurfaceKind, PluginSurfaceRef, SurfaceChangeKind,
    VerifiedCatalogProvenance, PLUGIN_OPERATION_CONFIRMATION_SCHEMA, PLUGIN_OPERATION_PLAN_SCHEMA,
};

const CATALOG_RECORD: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-v1.json");
const INSTALL_PLAN: &[u8] = include_bytes!("../fixtures/plugins/operation-plan-install-v1.json");
const INSTALL_PLAN_DIGEST: &str =
    include_str!("../fixtures/plugins/operation-plan-install-v1.sha256").trim_ascii_end();
const OPERATION_CONFIRMATION: &[u8] =
    include_bytes!("../fixtures/plugins/operation-confirmation-v1.json");
const OPERATION_CONFIRMATION_DIGEST: &str =
    include_str!("../fixtures/plugins/operation-confirmation-v1.sha256").trim_ascii_end();
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const DIGEST_E: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const DIGEST_F: &str = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

fn qualified(kind: PluginSurfaceKind, id: &str) -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_owned(),
        surface: PluginSurfaceRef {
            kind,
            id: id.to_owned(),
        },
    }
}

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn provider(
    kind: PluginSurfaceKind,
    id: &str,
    provider_id: &str,
    enforcement: PlanEnforcementProfile,
) -> PlannedProviderEvidence {
    PlannedProviderEvidence {
        surface: qualified(kind, id),
        provider_id: provider_id.to_owned(),
        provider_build_id: "runtime:0.3.0:linux-x86_64".to_owned(),
        capability_digest: DIGEST_D.to_owned(),
        semantics_profile_digest: DIGEST_E.to_owned(),
        enforcement,
    }
}

fn install_plan() -> PluginOperationPlan {
    let catalog = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let surfaces = catalog
        .surfaces
        .iter()
        .map(|surface| PlannedSurfaceChange {
            surface: PluginSurfaceRef {
                kind: surface.kind,
                id: surface.id.clone(),
            },
            change: SurfaceChangeKind::Add,
            before_digest: None,
            after_digest: Some(surface.descriptor_digest().unwrap()),
        })
        .collect();
    let after = PlannedPackageState {
        release: PlannedPluginRelease {
            package_id: catalog.package_id.clone(),
            version: catalog.version.clone(),
            channel: catalog.channel,
            target: catalog.target.clone(),
            package_sha256: catalog.package.sha256.clone().unwrap(),
            manifest_sha256: DIGEST_C.to_owned(),
            permission_ceiling_digest: catalog.permission_ceiling_digest.clone(),
            surfaces: catalog.surfaces.clone(),
        },
        permissions: catalog.permission_ceiling.clone(),
    };

    PluginOperationPlan {
        schema: PLUGIN_OPERATION_PLAN_SCHEMA.to_owned(),
        operation_id: "install:acme-research:0001".to_owned(),
        created_at_ms: 1_785_360_000_000,
        expires_at_ms: 1_785_360_600_000,
        action: PluginOperationAction::Install,
        package_id: catalog.package_id.clone(),
        component_id: "runtime:local".to_owned(),
        scope: PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "workspace:research".to_owned(),
        },
        package_lock_digest: None,
        prior_package_lock_digest: None,
        packages: vec![PlannedPackageTransition {
            package_id: catalog.package_id,
            role: PlanPackageRole::Root,
            change: PlanPackageChangeKind::Add,
            before: None,
            after: Some(after),
            source: Some(PluginPlanSource::Registry {
                provenance: VerifiedCatalogProvenance {
                    registry_name: "official".to_owned(),
                    registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
                    root_sha256: DIGEST_F.to_owned(),
                    root_version: 7,
                    timestamp_version: 42,
                    snapshot_version: 41,
                    targets_version: 39,
                    catalog_record_digest: DIGEST_E.to_owned(),
                },
                archive: catalog.archive,
            }),
            surfaces,
        }],
        secret_changes: vec![PlannedSecretChange {
            surface: qualified(PluginSurfaceKind::Tool, "convert"),
            secret_name: "research-api".to_owned(),
            change: PlannedSecretChangeKind::Grant,
        }],
        providers: vec![
            provider(
                PluginSurfaceKind::Mcp,
                "library",
                "runtime:mcp-http",
                PlanEnforcementProfile::Container,
            ),
            provider(
                PluginSurfaceKind::Tool,
                "convert",
                "runtime:tool-task",
                PlanEnforcementProfile::Sandbox,
            ),
            provider(
                PluginSurfaceKind::Tool,
                "index",
                "runtime:tool-service",
                PlanEnforcementProfile::Container,
            ),
        ],
        workspace_impacts: vec![PlannedWorkspaceImpact {
            scope_id: "workspace:research".to_owned(),
            grant_before_digest: None,
            grant_after_digest: Some(DIGEST_F.to_owned()),
            enabled_before: false,
            enabled_after: true,
        }],
        impact: PlannedOperationImpact {
            download_bytes: 1_048_576,
            installed_bytes_after: 4_194_304,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: DIGEST_A.to_owned(),
            confirmation_required: true,
        },
        state: PlannedStateEvidence {
            state_revision: 3,
            capability_generation: 12,
            receipt_digest: None,
        },
    }
}

#[test]
fn canonical_install_plan_fixture_binds_the_complete_resolved_delta() {
    let plan = install_plan();
    plan.validate().unwrap();
    let decoded = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    assert_eq!(decoded, plan);
    assert_eq!(
        decoded.canonical_bytes().unwrap(),
        canonical_fixture(INSTALL_PLAN)
    );
    assert_eq!(decoded.descriptor_digest().unwrap(), INSTALL_PLAN_DIGEST);
}

#[test]
fn an_empty_host_can_plan_from_capability_generation_zero() {
    let mut plan = install_plan();
    plan.state.capability_generation = 0;
    plan.validate().unwrap();
}

#[test]
fn apply_requires_the_reviewed_digest_and_valid_time_window() {
    let envelope = PluginOperationPlanEnvelope::new(install_plan()).unwrap();
    envelope
        .verify_apply(
            "install:acme-research:0001",
            &envelope.plan_digest,
            1_785_360_300_000,
        )
        .unwrap();

    let mismatch = envelope
        .verify_apply("install:acme-research:0001", DIGEST_F, 1_785_360_300_000)
        .unwrap_err();
    assert_eq!(mismatch.code, "use.plugin.plan_mismatch");
    let early = envelope
        .verify_apply(
            "install:acme-research:0001",
            &envelope.plan_digest,
            1_785_359_999_999,
        )
        .unwrap_err();
    assert_eq!(early.code, "use.plugin.plan_expired");
    let expired = envelope
        .verify_apply(
            "install:acme-research:0001",
            &envelope.plan_digest,
            1_785_360_600_000,
        )
        .unwrap_err();
    assert_eq!(expired.code, "use.plugin.plan_expired");
}

#[test]
fn ask_apply_requires_user_confirmation_of_the_exact_operation_plan() {
    let envelope = PluginOperationPlanEnvelope::new(install_plan()).unwrap();
    assert_eq!(
        envelope
            .verify_confirmed_apply(
                "install:acme-research:0001",
                &envelope.plan_digest,
                None,
                1_785_360_300_000,
            )
            .unwrap_err()
            .code,
        "use.plugin.plan_confirmation_required"
    );
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: 1_785_360_200_000,
    };
    assert_eq!(
        confirmation.canonical_bytes().unwrap(),
        canonical_fixture(OPERATION_CONFIRMATION)
    );
    assert_eq!(
        PluginOperationConfirmation::from_json(OPERATION_CONFIRMATION).unwrap(),
        confirmation
    );
    assert_eq!(
        confirmation.descriptor_digest().unwrap(),
        OPERATION_CONFIRMATION_DIGEST
    );
    envelope
        .verify_confirmed_apply(
            "install:acme-research:0001",
            &envelope.plan_digest,
            Some(&confirmation),
            1_785_360_300_000,
        )
        .unwrap();

    let mut substituted = confirmation;
    substituted.plan_digest = DIGEST_F.to_string();
    assert_eq!(
        envelope
            .verify_confirmed_apply(
                "install:acme-research:0001",
                &envelope.plan_digest,
                Some(&substituted),
                1_785_360_300_000,
            )
            .unwrap_err()
            .code,
        "use.plugin.plan_confirmation_mismatch"
    );

    let mut future = PluginOperationConfirmation::from_json(OPERATION_CONFIRMATION).unwrap();
    future.confirmed_at_ms = 1_785_360_300_001;
    assert_eq!(
        envelope
            .verify_confirmed_apply(
                "install:acme-research:0001",
                &envelope.plan_digest,
                Some(&future),
                1_785_360_300_000,
            )
            .unwrap_err()
            .code,
        "use.plugin.plan_confirmation_mismatch"
    );

    let mut unknown: serde_json::Value = serde_json::from_slice(OPERATION_CONFIRMATION).unwrap();
    unknown["userToken"] = serde_json::json!("do-not-echo");
    let error =
        PluginOperationConfirmation::from_json(&serde_json::to_vec(&unknown).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.plan_confirmation_invalid");
    assert!(!error.message.contains("do-not-echo"));
}

#[test]
fn plan_rejects_permission_provider_and_source_drift() {
    let mut secret_drift = install_plan();
    secret_drift.secret_changes.clear();
    assert!(secret_drift.validate().is_err());

    let mut provider_drift = install_plan();
    provider_drift.providers[1].enforcement = PlanEnforcementProfile::Container;
    assert!(provider_drift.validate().is_err());

    let mut source_drift = install_plan();
    source_drift.packages[0].source = Some(PluginPlanSource::ReleaseBundle {
        bundle_digest: DIGEST_D.to_owned(),
        package_digest: DIGEST_C.to_owned(),
    });
    assert!(source_drift.validate().is_err());
}

#[test]
fn unattended_agent_cannot_accept_unconfined_or_unsigned_execution() {
    let mut unconfined = install_plan();
    unconfined.authority = PlanAuthority {
        actor: PlanActor::Agent,
        decision: PlanPolicyDecision::Allow,
        policy_digest: DIGEST_A.to_owned(),
        confirmation_required: false,
    };
    unconfined.providers[1].enforcement = PlanEnforcementProfile::NativeUnconfined;
    assert!(unconfined.validate().is_err());

    let mut local = install_plan();
    local.authority = PlanAuthority {
        actor: PlanActor::Agent,
        decision: PlanPolicyDecision::Allow,
        policy_digest: DIGEST_A.to_owned(),
        confirmation_required: false,
    };
    local.packages[0].source = Some(PluginPlanSource::LocalReviewed {
        source_digest: DIGEST_D.to_owned(),
        package_digest: DIGEST_A.to_owned(),
        unsigned: true,
    });
    let Some(after) = local.packages[0].after.as_mut() else {
        panic!("fixture has an after state");
    };
    after.release.package_sha256 = DIGEST_A.to_owned();
    assert!(local.validate().is_err());
}

#[test]
fn unknown_plan_fields_fail_closed_without_echoing_values() {
    let mut value: serde_json::Value = serde_json::from_slice(INSTALL_PLAN).unwrap();
    value["authority"]["secretValue"] = serde_json::json!("do-not-echo");
    let error = PluginOperationPlan::from_json(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.plan_invalid");
    assert!(!error.message.contains("do-not-echo"));
}
