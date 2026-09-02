use a3s_use_core::{
    PlanActor, PlanAuthority, PlanPackageChangeKind, PlanPackageRole, PlanPolicyDecision,
    PlanScope, PlanScopeKind, PlannedOperationImpact, PlannedPackageTransition,
    PlannedStateEvidence, PlannedWorkspaceImpact, PluginCatalogRecord, PluginDesiredState,
    PluginHostApplyRequest, PluginHostApplyResult, PluginHostCancelRequest, PluginHostCancelResult,
    PluginHostCancellationStatus, PluginHostCapabilities, PluginHostEnablementPlanRequest,
    PluginHostEnablementPlanResult, PluginHostEnablementPlanStatus, PluginHostManager,
    PluginHostObservationRequest, PluginHostObservationResult, PluginHostObservationStatus,
    PluginHostOperationCancellability, PluginHostOperationObservationRequest,
    PluginHostOperationObservationResult, PluginHostOperationPhase, PluginHostOperationProgress,
    PluginHostOperationStatus, PluginHostOperationWatchRequest, PluginHostPackageState,
    PluginHostPlanRequest, PluginHostPlanResult, PluginHostUnavailableReason, PluginManagedScope,
    PluginObservedState, PluginOperationAction, PluginOperationConfirmation,
    PluginOperationPlanBinding, PluginOperationPlanDraft, PluginOperationPlanEnvelope,
    PluginPackageId, PluginSurfaceKind, PluginSurfaceRef, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, PLUGIN_HOST_APPLY_REQUEST_SCHEMA, PLUGIN_HOST_APPLY_RESULT_SCHEMA,
    PLUGIN_HOST_CANCEL_REQUEST_SCHEMA, PLUGIN_HOST_CANCEL_RESULT_SCHEMA,
    PLUGIN_HOST_CAPABILITIES_SCHEMA_V6, PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA, PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA, PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA, PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA,
    PLUGIN_HOST_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_PLAN_RESULT_SCHEMA, PLUGIN_HOST_PROTOCOL_LEVEL_V6,
    PLUGIN_MANAGED_SCOPE_SCHEMA_V2, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
    PLUGIN_OPERATION_PLAN_SCHEMA_V4,
};

const CATALOG: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-okf-v3.json");
const HOST_CAPABILITIES: &[u8] = include_bytes!("../fixtures/plugins/host-capabilities-v6.json");
const HOST_CAPABILITIES_DIGEST: &str =
    include_str!("../fixtures/plugins/host-capabilities-v6.sha256").trim_ascii_end();
const RETIRED_HOST_CAPABILITIES_V5: &[u8] =
    include_bytes!("../fixtures/plugins/host-capabilities-v5.json");
const RETIRED_HOST_CAPABILITIES_V4: &[u8] =
    include_bytes!("../fixtures/plugins/host-capabilities-v4.json");
const MANAGED_SCOPE: &[u8] = include_bytes!("../fixtures/plugins/managed-scope-v2.json");
const MANAGED_SCOPE_DIGEST: &str =
    include_str!("../fixtures/plugins/managed-scope-v2.sha256").trim_ascii_end();
const RETIRED_MANAGED_SCOPE: &[u8] = include_bytes!("../fixtures/plugins/managed-scope-v1.json");
const HOST_OBSERVATION: &[u8] =
    include_bytes!("../fixtures/plugins/host-observation-result-v1.json");
const HOST_OBSERVATION_DIGEST: &str =
    include_str!("../fixtures/plugins/host-observation-result-v1.sha256").trim_ascii_end();
const HOST_OPERATION_OBSERVATION_REQUEST: &[u8] =
    include_bytes!("../fixtures/plugins/host-operation-observation-request-v1.json");
const HOST_OPERATION_OBSERVATION_REQUEST_DIGEST: &str =
    include_str!("../fixtures/plugins/host-operation-observation-request-v1.sha256")
        .trim_ascii_end();
const HOST_OPERATION_OBSERVATION_RESULT: &[u8] =
    include_bytes!("../fixtures/plugins/host-operation-observation-result-v1.json");
const HOST_OPERATION_OBSERVATION_RESULT_DIGEST: &str =
    include_str!("../fixtures/plugins/host-operation-observation-result-v1.sha256")
        .trim_ascii_end();
const HOST_OPERATION_WATCH_REQUEST: &[u8] =
    include_bytes!("../fixtures/plugins/host-operation-watch-request-v1.json");
const HOST_OPERATION_WATCH_REQUEST_DIGEST: &str =
    include_str!("../fixtures/plugins/host-operation-watch-request-v1.sha256").trim_ascii_end();
const HOST_CANCEL_REQUEST: &[u8] =
    include_bytes!("../fixtures/plugins/host-cancel-request-v1.json");
const HOST_CANCEL_REQUEST_DIGEST: &str =
    include_str!("../fixtures/plugins/host-cancel-request-v1.sha256").trim_ascii_end();
const HOST_CANCEL_RESULT: &[u8] = include_bytes!("../fixtures/plugins/host-cancel-result-v1.json");
const HOST_CANCEL_RESULT_DIGEST: &str =
    include_str!("../fixtures/plugins/host-cancel-result-v1.sha256").trim_ascii_end();
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn scope() -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:node-01".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "workspace:research".to_owned(),
        authority_id: "cloud:organization-01".to_owned(),
        fence_generation: 7,
        fence_digest: DIGEST_A.to_owned(),
    }
}

fn capabilities() -> PluginHostCapabilities {
    // This fixture freezes protocol v6, not the version of the crate running
    // the test. Patch-only crate releases must not silently rewrite every
    // cross-SDK capability digest when the protocol contract is unchanged.
    PluginHostCapabilities::v6("host:node-01", "0.2.4", "use:0.2.1:linux-x86_64").unwrap()
}

fn candidate() -> VerifiedPluginCatalogRecord {
    let record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let catalog_record_digest = record.descriptor_digest().unwrap();
    VerifiedPluginCatalogRecord::new(
        record,
        VerifiedCatalogProvenance {
            registry_name: "official".to_owned(),
            registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
            root_sha256: DIGEST_D.to_owned(),
            root_version: 7,
            timestamp_version: 42,
            snapshot_version: 41,
            targets_version: 39,
            catalog_record_digest,
        },
    )
    .unwrap()
}

fn plan_request() -> PluginHostPlanRequest {
    let capabilities_digest = capabilities().descriptor_digest().unwrap();
    PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "request:plan:0001".to_owned(),
        assignment_generation: 3,
        capabilities_digest,
        scope: scope(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
        candidate: Some(candidate()),
        package_lock: None,
        selected_surfaces: Vec::new(),
    }
}

fn plan_result() -> PluginHostPlanResult {
    let request = plan_request();
    let candidate = request.candidate.as_ref().unwrap();
    let transition = candidate
        .install_transition(PlanPackageRole::Root, &request.selected_surfaces)
        .unwrap();
    let draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        request.package_id.as_str(),
        request.package_id.component_id(),
        vec![transition],
        Vec::new(),
        vec![PlannedWorkspaceImpact {
            scope_id: request.scope.scope_id.clone(),
            grant_before_digest: None,
            grant_after_digest: Some(DIGEST_B.to_owned()),
            enabled_before: false,
            enabled_after: true,
        }],
        PlannedOperationImpact {
            download_bytes: candidate.record.archive.length,
            installed_bytes_after: candidate.record.package.expanded_bytes,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 3,
            capability_generation: 12,
            receipt_digest: None,
        },
    )
    .unwrap();
    let plan = draft
        .bind(PluginOperationPlanBinding {
            operation_id: "use-operation:0001".to_owned(),
            created_at_ms: 1_785_360_000_000,
            expires_at_ms: 1_785_360_600_000,
            scope: PlanScope {
                kind: PlanScopeKind::Workspace,
                id: request.scope.scope_id.clone(),
            },
            authority: PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Ask,
                policy_digest: DIGEST_C.to_owned(),
                confirmation_required: true,
            },
        })
        .unwrap();
    PluginHostPlanResult {
        schema: PLUGIN_HOST_PLAN_RESULT_SCHEMA.to_owned(),
        request_id: request.request_id,
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest,
        scope: request.scope,
        package_id: request.package_id,
        plan: PluginOperationPlanEnvelope::new(plan).unwrap(),
        replayed: false,
    }
}

fn installed_state(desired: PluginDesiredState) -> PluginHostPackageState {
    PluginHostPackageState {
        version: Some("1.0.0".to_owned()),
        package_generation: Some(13),
        package_digest: Some(DIGEST_A.to_owned()),
        manifest_digest: Some(DIGEST_B.to_owned()),
        receipt_digest: Some(DIGEST_C.to_owned()),
        capability_generation: 14,
        capability_revision: DIGEST_D.to_owned(),
        desired,
        observed: if desired == PluginDesiredState::Enabled {
            PluginObservedState::Ready
        } else {
            PluginObservedState::Installed
        },
        selected_surfaces: vec![
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_owned(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Skill,
                id: "research".to_owned(),
            },
        ],
    }
}

fn enablement_plan_request(enabled: bool) -> PluginHostEnablementPlanRequest {
    let capabilities = PluginHostCapabilities::v6(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();
    PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "request:enablement-plan:0001".to_owned(),
        assignment_generation: 3,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: scope(),
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
        expected_package_generation: 13,
        enabled,
    }
}

fn enablement_plan_result() -> PluginHostEnablementPlanResult {
    let request = enablement_plan_request(false);
    let candidate = candidate();
    let state = candidate.selected_state(&[]).unwrap();
    let transition = PlannedPackageTransition::resolved(
        request.package_id.as_str(),
        PlanPackageRole::Root,
        PlanPackageChangeKind::Retain,
        Some(state.clone()),
        Some(state),
        None,
    )
    .unwrap();
    let draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Disable,
        request.package_id.as_str(),
        request.package_id.component_id(),
        vec![transition],
        Vec::new(),
        vec![PlannedWorkspaceImpact {
            scope_id: request.scope.scope_id.clone(),
            grant_before_digest: Some(DIGEST_B.to_owned()),
            grant_after_digest: None,
            enabled_before: true,
            enabled_after: false,
        }],
        PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: candidate.record.package.expanded_bytes,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: true,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 3,
            capability_generation: 14,
            receipt_digest: Some(DIGEST_C.to_owned()),
        },
    )
    .unwrap();
    let plan = draft
        .bind(PluginOperationPlanBinding {
            operation_id: "use-enablement-operation:0001".to_owned(),
            created_at_ms: 1_785_360_000_000,
            expires_at_ms: 1_785_360_600_000,
            scope: request.scope.plan_scope(),
            authority: PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Ask,
                policy_digest: DIGEST_C.to_owned(),
                confirmation_required: true,
            },
        })
        .unwrap();
    assert_eq!(plan.schema, PLUGIN_OPERATION_PLAN_SCHEMA_V4);
    PluginHostEnablementPlanResult {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA.to_owned(),
        request_id: request.request_id,
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest,
        scope: request.scope,
        package_id: request.package_id,
        expected_package_generation: request.expected_package_generation,
        enabled: request.enabled,
        planned_at_ms: plan.created_at_ms,
        status: PluginHostEnablementPlanStatus::Planned,
        state: installed_state(PluginDesiredState::Enabled),
        plan: Some(PluginOperationPlanEnvelope::new(plan).unwrap()),
        replayed: false,
    }
}

fn removed_state() -> PluginHostPackageState {
    PluginHostPackageState {
        version: None,
        package_generation: None,
        package_digest: None,
        manifest_digest: None,
        receipt_digest: None,
        capability_generation: 15,
        capability_revision: DIGEST_D.to_owned(),
        desired: PluginDesiredState::Absent,
        observed: PluginObservedState::Removed,
        selected_surfaces: Vec::new(),
    }
}

#[test]
fn an_empty_host_observation_can_report_capability_generation_zero() {
    let mut state = removed_state();
    state.capability_generation = 0;
    state.validate().unwrap();
}

#[test]
fn package_identity_is_typed_and_uses_one_validation_rule() {
    let package_id = PluginPackageId::parse("acme/knowledge").unwrap();
    assert_eq!(package_id.as_str(), "acme/knowledge");
    assert_eq!(package_id.component_id(), "use/acme/knowledge");
    assert_eq!(
        serde_json::to_string(&package_id).unwrap(),
        "\"acme/knowledge\""
    );
    assert_eq!(
        serde_json::from_str::<PluginPackageId>("\"acme/knowledge\"").unwrap(),
        package_id
    );
    for invalid in [
        "Acme/knowledge",
        "acme",
        "acme/knowledge/extra",
        "acme/../knowledge",
        "acme/knowledge_2",
    ] {
        assert!(
            PluginPackageId::parse(invalid).is_err(),
            "accepted {invalid}"
        );
        assert!(serde_json::from_str::<PluginPackageId>(&format!("\"{invalid}\"")).is_err());
    }
}

#[test]
fn capabilities_freeze_the_single_current_host_contract() {
    let capabilities = capabilities();
    capabilities.validate().unwrap();
    assert_eq!(capabilities.schema, PLUGIN_HOST_CAPABILITIES_SCHEMA_V6);
    assert_eq!(capabilities.protocol_level, PLUGIN_HOST_PROTOCOL_LEVEL_V6);
    assert_eq!(capabilities.catalog_schemas, ["a3s.use.plugin-catalog.v3"]);
    assert_eq!(capabilities.plan_schemas, [PLUGIN_OPERATION_PLAN_SCHEMA_V4]);
    assert!(capabilities.exclusive_managed_scope_mutation);
    assert_eq!(
        capabilities.surface_kinds,
        vec![
            PluginSurfaceKind::Flow,
            PluginSurfaceKind::Mcp,
            PluginSurfaceKind::Okf,
            PluginSurfaceKind::Skill,
            PluginSurfaceKind::Tool,
            PluginSurfaceKind::Ui,
        ]
    );
    assert!(capabilities
        .contract_schemas
        .contains(&PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned()));
    assert!(capabilities
        .contract_schemas
        .contains(&PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned()));
    assert!(capabilities
        .contract_schemas
        .contains(&PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA.to_owned()));
    assert!(capabilities
        .contract_schemas
        .contains(&PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned()));
    assert!(capabilities
        .contract_schemas
        .contains(&PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned()));
    assert!(!capabilities
        .contract_schemas
        .iter()
        .any(|schema| schema.contains("host-enablement-request")));

    let mut retired = serde_json::to_value(&capabilities).unwrap();
    retired["schema"] = serde_json::json!("a3s.use.plugin-host-capabilities.v3");
    retired["protocolLevel"] = serde_json::json!(3);
    assert!(PluginHostCapabilities::from_json(&serde_json::to_vec(&retired).unwrap()).is_err());
    assert!(PluginHostCapabilities::from_json(RETIRED_HOST_CAPABILITIES_V5).is_err());
    assert!(PluginHostCapabilities::from_json(RETIRED_HOST_CAPABILITIES_V4).is_err());

    let mut expanded = capabilities;
    expanded
        .contract_schemas
        .push("a3s.use.plugin-host-universal-action.v1".to_owned());
    assert!(expanded.validate().is_err());
}

#[test]
fn host_enablement_plan_is_explicit_and_reuses_digest_only_apply() {
    let capabilities = PluginHostCapabilities::v6(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();
    let request = enablement_plan_request(false);
    request.validate_for_capabilities(&capabilities).unwrap();
    let result = enablement_plan_result();
    result.validate_for(&request, &capabilities).unwrap();
    assert_eq!(
        PluginHostEnablementPlanResult::from_json(&result.canonical_bytes().unwrap()).unwrap(),
        result
    );

    let envelope = result.plan.as_ref().unwrap();
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: envelope.plan.created_at_ms + 1,
    };
    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "request:enablement-apply:0001".to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        confirmation: Some(confirmation),
    };
    apply
        .verify_apply_for_enablement_plan(&result, &capabilities, envelope.plan.created_at_ms + 1)
        .unwrap();

    let mut no_change = result;
    no_change.status = PluginHostEnablementPlanStatus::NoChange;
    no_change.state = installed_state(PluginDesiredState::InstalledDisabled);
    no_change.plan = None;
    no_change.validate_for(&request, &capabilities).unwrap();
    assert!(apply
        .validate_for_enablement_plan(&no_change, &capabilities)
        .is_err());
}

#[test]
fn current_host_protocol_fixtures_are_canonical() {
    let parsed_capabilities = PluginHostCapabilities::from_json(HOST_CAPABILITIES).unwrap();
    assert_eq!(parsed_capabilities, capabilities());
    assert_eq!(
        parsed_capabilities.canonical_bytes().unwrap(),
        canonical_fixture(HOST_CAPABILITIES)
    );
    assert_eq!(
        parsed_capabilities.descriptor_digest().unwrap(),
        HOST_CAPABILITIES_DIGEST
    );

    let scope = PluginManagedScope::from_json(MANAGED_SCOPE).unwrap();
    assert_eq!(scope, self::scope());
    assert_eq!(
        scope.canonical_bytes().unwrap(),
        canonical_fixture(MANAGED_SCOPE)
    );
    assert_eq!(scope.descriptor_digest().unwrap(), MANAGED_SCOPE_DIGEST);
    assert!(PluginManagedScope::from_json(RETIRED_MANAGED_SCOPE).is_err());

    let observation = PluginHostObservationResult::from_json(HOST_OBSERVATION).unwrap();
    assert_eq!(
        observation.canonical_bytes().unwrap(),
        canonical_fixture(HOST_OBSERVATION)
    );
    assert_eq!(
        observation.descriptor_digest().unwrap(),
        HOST_OBSERVATION_DIGEST
    );
    assert_eq!(
        observation.capabilities_digest,
        parsed_capabilities.descriptor_digest().unwrap()
    );

    let operation_request =
        PluginHostOperationObservationRequest::from_json(HOST_OPERATION_OBSERVATION_REQUEST)
            .unwrap();
    operation_request
        .validate_for_capabilities(&parsed_capabilities)
        .unwrap();
    assert_eq!(
        operation_request.canonical_bytes().unwrap(),
        canonical_fixture(HOST_OPERATION_OBSERVATION_REQUEST)
    );
    assert_eq!(
        operation_request.descriptor_digest().unwrap(),
        HOST_OPERATION_OBSERVATION_REQUEST_DIGEST
    );

    let operation_result =
        PluginHostOperationObservationResult::from_json(HOST_OPERATION_OBSERVATION_RESULT).unwrap();
    operation_result
        .validate_for(&operation_request, &parsed_capabilities)
        .unwrap();
    assert_eq!(
        operation_result.canonical_bytes().unwrap(),
        canonical_fixture(HOST_OPERATION_OBSERVATION_RESULT)
    );
    assert_eq!(
        operation_result.descriptor_digest().unwrap(),
        HOST_OPERATION_OBSERVATION_RESULT_DIGEST
    );

    let watch = PluginHostOperationWatchRequest::from_json(HOST_OPERATION_WATCH_REQUEST).unwrap();
    watch
        .validate_for_capabilities(&parsed_capabilities)
        .unwrap();
    assert_eq!(
        watch.canonical_bytes().unwrap(),
        canonical_fixture(HOST_OPERATION_WATCH_REQUEST)
    );
    assert_eq!(
        watch.descriptor_digest().unwrap(),
        HOST_OPERATION_WATCH_REQUEST_DIGEST
    );

    let cancellation = PluginHostCancelRequest::from_json(HOST_CANCEL_REQUEST).unwrap();
    cancellation
        .validate_for_capabilities(&parsed_capabilities)
        .unwrap();
    assert_eq!(
        cancellation.canonical_bytes().unwrap(),
        canonical_fixture(HOST_CANCEL_REQUEST)
    );
    assert_eq!(
        cancellation.descriptor_digest().unwrap(),
        HOST_CANCEL_REQUEST_DIGEST
    );

    let cancellation_result = PluginHostCancelResult::from_json(HOST_CANCEL_RESULT).unwrap();
    cancellation_result
        .validate_for(&cancellation, &parsed_capabilities)
        .unwrap();
    assert_eq!(
        cancellation_result.canonical_bytes().unwrap(),
        canonical_fixture(HOST_CANCEL_RESULT)
    );
    assert_eq!(
        cancellation_result.descriptor_digest().unwrap(),
        HOST_CANCEL_RESULT_DIGEST
    );
}

#[test]
fn managed_scope_is_opaque_and_requires_an_exact_fence() {
    let scope = scope();
    scope.validate().unwrap();
    assert_eq!(
        scope.plan_scope(),
        PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "workspace:research".to_owned(),
        }
    );
    scope.verify_current_fence(&scope.clone()).unwrap();

    let mut same_id_user = scope.clone();
    same_id_user.scope_kind = PlanScopeKind::User;
    same_id_user.validate().unwrap();
    assert_eq!(
        same_id_user.plan_scope(),
        PlanScope {
            kind: PlanScopeKind::User,
            id: "workspace:research".to_owned(),
        }
    );
    assert_ne!(
        same_id_user.descriptor_digest().unwrap(),
        scope.descriptor_digest().unwrap()
    );
    assert_eq!(
        same_id_user.verify_current_fence(&scope).unwrap_err().code,
        "use.plugin.managed_scope_fence_mismatch"
    );
    assert_eq!(
        scope.verify_current_fence(&same_id_user).unwrap_err().code,
        "use.plugin.managed_scope_fence_mismatch"
    );

    let mut stale = scope.clone();
    stale.fence_generation -= 1;
    assert!(stale.verify_current_fence(&scope).is_err());
    let mut conflicting = scope.clone();
    conflicting.fence_digest = DIGEST_B.to_owned();
    assert!(conflicting.verify_current_fence(&scope).is_err());
    let mut path = scope;
    path.scope_id = "../../workspace".to_owned();
    assert!(path.validate().is_err());
}

#[test]
fn plan_contract_reuses_catalog_plan_and_host_policy_authority() {
    let request = plan_request();
    request.validate().unwrap();
    request.validate_for_capabilities(&capabilities()).unwrap();
    let encoded = serde_json::to_value(&request).unwrap();
    for forbidden in [
        "authority",
        "provider",
        "executable",
        "endpoint",
        "secret",
        "path",
    ] {
        assert!(
            !encoded.as_object().unwrap().contains_key(forbidden),
            "plan request exposes {forbidden}"
        );
    }

    let result = plan_result();
    result.validate().unwrap();
    result.validate_for(&request, &capabilities()).unwrap();

    let mut substituted = result;
    substituted.package_id = PluginPackageId::parse("acme/other").unwrap();
    assert!(substituted.validate().is_err());

    let mut uninstall = request;
    uninstall.action = PluginOperationAction::Uninstall;
    assert!(uninstall.validate().is_err());
    uninstall.candidate = None;
    uninstall.selected_surfaces.clear();
    uninstall.validate().unwrap();

    let mut enable = plan_request();
    enable.action = PluginOperationAction::Enable;
    enable.candidate = None;
    enable.selected_surfaces.clear();
    assert!(enable.validate().is_err());

    let mut disable = plan_request();
    disable.action = PluginOperationAction::Disable;
    disable.candidate = None;
    disable.selected_surfaces.clear();
    assert!(disable.validate().is_err());
}

#[test]
fn apply_binds_only_the_stored_plan_and_exact_confirmation() {
    let plan = plan_result();
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
        operation_id: plan.plan.plan.operation_id.clone(),
        plan_digest: plan.plan.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: plan.plan.plan.created_at_ms + 1,
    };
    let request = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "request:apply:0001".to_owned(),
        assignment_generation: plan.assignment_generation,
        capabilities_digest: plan.capabilities_digest.clone(),
        scope: plan.scope.clone(),
        package_id: plan.package_id.clone(),
        operation_id: plan.plan.plan.operation_id.clone(),
        plan_digest: plan.plan.plan_digest.clone(),
        confirmation: Some(confirmation),
    };
    request.validate().unwrap();
    let mut request_with_unknown = serde_json::to_value(&request).unwrap();
    request_with_unknown["unexpected"] = serde_json::json!(true);
    assert!(
        PluginHostApplyRequest::from_json(&serde_json::to_vec(&request_with_unknown).unwrap())
            .is_err()
    );
    request.validate_for_capabilities(&capabilities()).unwrap();
    request.validate_for_plan(&plan, &capabilities()).unwrap();
    assert_eq!(
        request
            .verify_apply_for_plan(&plan, &capabilities(), plan.plan.plan.expires_at_ms,)
            .unwrap_err()
            .code,
        "use.plugin.plan_expired"
    );
    request
        .verify_admitted_replay_for_plan(&plan, &capabilities())
        .unwrap();

    let mut late_confirmation = request.clone();
    late_confirmation
        .confirmation
        .as_mut()
        .unwrap()
        .confirmed_at_ms = plan.plan.plan.expires_at_ms;
    assert!(late_confirmation
        .verify_admitted_replay_for_plan(&plan, &capabilities())
        .is_err());

    let result = PluginHostApplyResult {
        schema: PLUGIN_HOST_APPLY_RESULT_SCHEMA.to_owned(),
        request_id: request.request_id.clone(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: request.operation_id.clone(),
        plan_digest: request.plan_digest.clone(),
        completed_at_ms: 1_785_360_100_000,
        operation_result_digest: DIGEST_A.to_owned(),
        state: installed_state(PluginDesiredState::Enabled),
        replayed: false,
    };
    result.validate_for(&request, &capabilities()).unwrap();
    let mut result_with_unknown = serde_json::to_value(&result).unwrap();
    result_with_unknown["unexpected"] = serde_json::json!(true);
    assert!(
        PluginHostApplyResult::from_json(&serde_json::to_vec(&result_with_unknown).unwrap())
            .is_err()
    );

    let mut mismatch = request;
    mismatch.confirmation.as_mut().unwrap().plan_digest = DIGEST_B.to_owned();
    assert!(mismatch.validate().is_err());
}

#[test]
fn observation_uses_the_use_owned_state_projection() {
    let capabilities_digest = capabilities().descriptor_digest().unwrap();
    let observe = PluginHostObservationRequest {
        schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "request:observe:0001".to_owned(),
        assignment_generation: 4,
        capabilities_digest,
        scope: scope(),
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
    };
    let available = PluginHostObservationResult {
        schema: PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA.to_owned(),
        request_id: observe.request_id.clone(),
        assignment_generation: observe.assignment_generation,
        capabilities_digest: observe.capabilities_digest.clone(),
        scope: observe.scope.clone(),
        package_id: observe.package_id.clone(),
        observed_at_ms: 1_785_360_300_000,
        status: PluginHostObservationStatus::Available {
            state: installed_state(PluginDesiredState::Enabled),
        },
    };
    available.validate_for(&observe, &capabilities()).unwrap();

    let unavailable = PluginHostObservationResult {
        status: PluginHostObservationStatus::Unavailable {
            reason: PluginHostUnavailableReason::ManagerRecovering,
        },
        ..available
    };
    unavailable.validate_for(&observe, &capabilities()).unwrap();
}

#[test]
fn state_projection_never_infers_absence_or_success() {
    removed_state().validate().unwrap();
    let mut false_success = removed_state();
    false_success.observed = PluginObservedState::Ready;
    assert!(false_success.validate().is_err());

    let mut missing_receipt = installed_state(PluginDesiredState::Enabled);
    missing_receipt.receipt_digest = None;
    assert!(missing_receipt.validate().is_err());

    let mut zero_generation = installed_state(PluginDesiredState::InstalledDisabled);
    zero_generation.package_generation = Some(0);
    assert!(zero_generation.validate().is_err());
}

#[test]
fn host_capability_scope_plan_and_observation_decoders_reject_unknown_fields() {
    fn with_unknown<T: serde::Serialize>(value: T) -> Vec<u8> {
        let mut value = serde_json::to_value(value).unwrap();
        value["unexpected"] = serde_json::json!(true);
        serde_json::to_vec(&value).unwrap()
    }

    assert!(PluginHostCapabilities::from_json(&with_unknown(capabilities())).is_err());
    assert!(PluginManagedScope::from_json(&with_unknown(scope())).is_err());
    let mut missing_scope_kind = serde_json::to_value(scope()).unwrap();
    missing_scope_kind
        .as_object_mut()
        .unwrap()
        .remove("scopeKind");
    assert!(
        PluginManagedScope::from_json(&serde_json::to_vec(&missing_scope_kind).unwrap()).is_err()
    );
    assert!(PluginHostPlanRequest::from_json(&with_unknown(plan_request())).is_err());
    assert!(PluginHostPlanResult::from_json(&with_unknown(plan_result())).is_err());
    let observation_request = PluginHostObservationRequest {
        schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "request:observe:0001".to_owned(),
        assignment_generation: 3,
        capabilities_digest: capabilities().descriptor_digest().unwrap(),
        scope: scope(),
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
    };
    assert!(PluginHostObservationRequest::from_json(&with_unknown(observation_request)).is_err());
    let observation_result = PluginHostObservationResult::from_json(HOST_OBSERVATION).unwrap();
    assert!(PluginHostObservationResult::from_json(&with_unknown(observation_result)).is_err());
    let operation_request =
        PluginHostOperationObservationRequest::from_json(HOST_OPERATION_OBSERVATION_REQUEST)
            .unwrap();
    assert!(
        PluginHostOperationObservationRequest::from_json(&with_unknown(operation_request)).is_err()
    );
    let operation_result =
        PluginHostOperationObservationResult::from_json(HOST_OPERATION_OBSERVATION_RESULT).unwrap();
    assert!(
        PluginHostOperationObservationResult::from_json(&with_unknown(operation_result)).is_err()
    );
    let watch = PluginHostOperationWatchRequest::from_json(HOST_OPERATION_WATCH_REQUEST).unwrap();
    assert!(PluginHostOperationWatchRequest::from_json(&with_unknown(watch)).is_err());
    let cancellation = PluginHostCancelRequest::from_json(HOST_CANCEL_REQUEST).unwrap();
    assert!(PluginHostCancelRequest::from_json(&with_unknown(cancellation)).is_err());
    let cancellation_result = PluginHostCancelResult::from_json(HOST_CANCEL_RESULT).unwrap();
    assert!(PluginHostCancelResult::from_json(&with_unknown(cancellation_result)).is_err());
}

#[test]
fn public_host_types_and_service_port_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_manager_port<T: ?Sized + PluginHostManager + Send + Sync>() {}

    assert_send_sync::<PluginPackageId>();
    assert_send_sync::<PluginManagedScope>();
    assert_send_sync::<PluginHostCapabilities>();
    assert_send_sync::<PluginHostPlanRequest>();
    assert_send_sync::<PluginHostPlanResult>();
    assert_send_sync::<PluginHostApplyRequest>();
    assert_send_sync::<PluginHostApplyResult>();
    assert_send_sync::<PluginHostEnablementPlanRequest>();
    assert_send_sync::<PluginHostEnablementPlanResult>();
    assert_send_sync::<PluginHostObservationRequest>();
    assert_send_sync::<PluginHostObservationResult>();
    assert_send_sync::<PluginHostOperationObservationRequest>();
    assert_send_sync::<PluginHostOperationObservationResult>();
    assert_send_sync::<PluginHostOperationWatchRequest>();
    assert_send_sync::<PluginHostCancelRequest>();
    assert_send_sync::<PluginHostCancelResult>();
    assert_manager_port::<dyn PluginHostManager>();
}

#[test]
fn operation_observation_revision_binds_exact_progress_without_percentages() {
    let capabilities = PluginHostCapabilities::v6(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();
    let request = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:operation:0001".to_owned(),
        assignment_generation: 4,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: scope(),
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
        operation_id: "use-operation:0001".to_owned(),
        plan_digest: DIGEST_A.to_owned(),
    };
    request.validate_for_capabilities(&capabilities).unwrap();
    let status = PluginHostOperationStatus {
        phase: PluginHostOperationPhase::Preparing,
        cancellability: PluginHostOperationCancellability::TooLate,
        progress: Some(PluginHostOperationProgress {
            completed_steps: 2,
            total_steps: 5,
            current_surface: Some(PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_owned(),
            }),
        }),
        error_code: None,
        completed_at_ms: None,
        operation_result_digest: None,
        state: None,
    };
    let result = PluginHostOperationObservationResult {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA.to_owned(),
        request_id: request.request_id.clone(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: request.operation_id.clone(),
        plan_digest: request.plan_digest.clone(),
        observed_at_ms: 1_785_360_300_000,
        revision: status.descriptor_digest().unwrap(),
        changed: true,
        timed_out: false,
        status,
    };
    result.validate_for(&request, &capabilities).unwrap();
    let encoded = serde_json::to_value(&result).unwrap();
    assert!(encoded.get("percentage").is_none());

    let watch = PluginHostOperationWatchRequest {
        schema: PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA.to_owned(),
        observation: request.clone(),
        after_revision: Some(result.revision.clone()),
        timeout_ms: 30_000,
    };
    watch.validate_for_capabilities(&capabilities).unwrap();

    let cancellation = PluginHostCancelRequest {
        schema: PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned(),
        request_id: "cancel:operation:0001".to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: request.operation_id.clone(),
        plan_digest: request.plan_digest.clone(),
        requested_by: PlanActor::User,
    };
    let cancellation_result = PluginHostCancelResult {
        schema: PLUGIN_HOST_CANCEL_RESULT_SCHEMA.to_owned(),
        request_id: cancellation.request_id.clone(),
        assignment_generation: cancellation.assignment_generation,
        capabilities_digest: cancellation.capabilities_digest.clone(),
        scope: cancellation.scope.clone(),
        package_id: cancellation.package_id.clone(),
        operation_id: cancellation.operation_id.clone(),
        plan_digest: cancellation.plan_digest.clone(),
        observed_at_ms: 1_785_360_300_001,
        status: PluginHostCancellationStatus::TooLate,
    };
    cancellation_result
        .validate_for(&cancellation, &capabilities)
        .unwrap();
    let mut agent_cancel = cancellation;
    agent_cancel.requested_by = PlanActor::Agent;
    assert!(agent_cancel.validate().is_err());
}
