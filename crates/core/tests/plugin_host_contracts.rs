use a3s_use_core::{
    CatalogSurface, PlanActor, PlanAuthority, PlanPackageRole, PlanPolicyDecision, PlanScope,
    PlanScopeKind, PlannedOperationImpact, PlannedStateEvidence, PlannedWorkspaceImpact,
    PluginCatalogRecord, PluginDesiredState, PluginHostApplyRequest, PluginHostApplyResult,
    PluginHostCapabilities, PluginHostEnablementRequest, PluginHostEnablementResult,
    PluginHostManager, PluginHostObservationRequest, PluginHostObservationResult,
    PluginHostObservationStatus, PluginHostPackageState, PluginHostPlanRequest,
    PluginHostPlanResult, PluginHostUnavailableReason, PluginManagedScope, PluginObservedState,
    PluginOperationAction, PluginOperationConfirmation, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginPackageId, PluginSurfaceKind,
    PluginSurfaceRef, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
    PLUGIN_HOST_APPLY_REQUEST_SCHEMA, PLUGIN_HOST_APPLY_RESULT_SCHEMA,
    PLUGIN_HOST_CAPABILITIES_SCHEMA_V2, PLUGIN_HOST_CAPABILITIES_SCHEMA_V3,
    PLUGIN_HOST_ENABLEMENT_REQUEST_SCHEMA, PLUGIN_HOST_ENABLEMENT_RESULT_SCHEMA,
    PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA, PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA,
    PLUGIN_HOST_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_PLAN_RESULT_SCHEMA, PLUGIN_HOST_PROTOCOL_LEVEL_V2,
    PLUGIN_HOST_PROTOCOL_LEVEL_V3, PLUGIN_MANAGED_SCOPE_SCHEMA,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA, PLUGIN_OPERATION_PLAN_SCHEMA_V3,
};

const CATALOG: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-okf-v3.json");
const HOST_CAPABILITIES: &[u8] = include_bytes!("../fixtures/plugins/host-capabilities-v1.json");
const HOST_CAPABILITIES_DIGEST: &str =
    include_str!("../fixtures/plugins/host-capabilities-v1.sha256").trim_ascii_end();
const HOST_CAPABILITIES_V2: &[u8] = include_bytes!("../fixtures/plugins/host-capabilities-v2.json");
const HOST_CAPABILITIES_V2_DIGEST: &str =
    include_str!("../fixtures/plugins/host-capabilities-v2.sha256").trim_ascii_end();
const HOST_CAPABILITIES_V3: &[u8] = include_bytes!("../fixtures/plugins/host-capabilities-v3.json");
const HOST_CAPABILITIES_V3_DIGEST: &str =
    include_str!("../fixtures/plugins/host-capabilities-v3.sha256").trim_ascii_end();
const MANAGED_SCOPE: &[u8] = include_bytes!("../fixtures/plugins/managed-scope-v1.json");
const MANAGED_SCOPE_DIGEST: &str =
    include_str!("../fixtures/plugins/managed-scope-v1.sha256").trim_ascii_end();
const HOST_OBSERVATION: &[u8] =
    include_bytes!("../fixtures/plugins/host-observation-result-v1.json");
const HOST_OBSERVATION_DIGEST: &str =
    include_str!("../fixtures/plugins/host-observation-result-v1.sha256").trim_ascii_end();
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn scope() -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA.to_owned(),
        host_id: "host:node-01".to_owned(),
        scope_id: "workspace:research".to_owned(),
        authority_id: "cloud:organization-01".to_owned(),
        fence_generation: 7,
        fence_digest: DIGEST_A.to_owned(),
    }
}

fn capabilities() -> PluginHostCapabilities {
    PluginHostCapabilities::v1(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.2.1:linux-x86_64",
    )
    .unwrap()
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

fn flow_candidate() -> VerifiedPluginCatalogRecord {
    let candidate = candidate();
    let mut record = candidate.record;
    record.surfaces.insert(
        0,
        CatalogSurface {
            kind: PluginSurfaceKind::Flow,
            id: "reason".to_owned(),
            optional: false,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: vec![PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_owned(),
            }],
        },
    );
    record.surfaces[2].requires.insert(
        0,
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "reason".to_owned(),
        },
    );
    let mut provenance = candidate.provenance;
    provenance.catalog_record_digest = record.descriptor_digest().unwrap();
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
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
fn capabilities_freeze_one_versioned_host_contract() {
    let capabilities = capabilities();
    capabilities.validate().unwrap();
    assert_eq!(capabilities.protocol_level, 1);
    assert!(capabilities.exclusive_managed_scope_mutation);
    assert_eq!(
        capabilities.surface_kinds,
        vec![
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

    let mut mixed = capabilities.clone();
    mixed.protocol_level = 2;
    assert!(mixed.validate().is_err());
    let mut expanded = capabilities;
    expanded
        .contract_schemas
        .push("a3s.use.plugin-host-universal-action.v1".to_owned());
    assert!(expanded.validate().is_err());
}

#[test]
fn capabilities_v2_advertises_flow_without_rewriting_v1() {
    let v1 = capabilities();
    let v2 = PluginHostCapabilities::v2(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();
    assert_eq!(v1.protocol_level, 1);
    assert!(!v1.surface_kinds.contains(&PluginSurfaceKind::Flow));
    assert_eq!(v2.schema, PLUGIN_HOST_CAPABILITIES_SCHEMA_V2);
    assert_eq!(v2.protocol_level, PLUGIN_HOST_PROTOCOL_LEVEL_V2);
    assert_eq!(v2.surface_kinds[0], PluginSurfaceKind::Flow);
    assert!(v2
        .contract_schemas
        .contains(&PLUGIN_HOST_CAPABILITIES_SCHEMA_V2.to_owned()));
    assert_eq!(
        PluginHostCapabilities::from_json(&v2.canonical_bytes().unwrap()).unwrap(),
        v2
    );

    let fixture = PluginHostCapabilities::from_json(HOST_CAPABILITIES_V2).unwrap();
    assert_eq!(
        fixture,
        PluginHostCapabilities::v2(
            "host:node-01",
            env!("CARGO_PKG_VERSION"),
            "use:0.2.1:linux-x86_64",
        )
        .unwrap()
    );
    assert_eq!(
        fixture.canonical_bytes().unwrap(),
        canonical_fixture(HOST_CAPABILITIES_V2)
    );
    assert_eq!(
        fixture.descriptor_digest().unwrap(),
        HOST_CAPABILITIES_V2_DIGEST
    );
}

#[test]
fn capabilities_v3_advertises_graph_upgrade_plans_without_rewriting_prior_versions() {
    let v1 = capabilities();
    let v2 = PluginHostCapabilities::v2(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();
    let v3 = PluginHostCapabilities::v3(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();

    assert_eq!(v3.schema, PLUGIN_HOST_CAPABILITIES_SCHEMA_V3);
    assert_eq!(v3.protocol_level, PLUGIN_HOST_PROTOCOL_LEVEL_V3);
    assert_eq!(v3.surface_kinds, v2.surface_kinds);
    assert!(!v1.supports_plan_schema(PLUGIN_OPERATION_PLAN_SCHEMA_V3));
    assert!(!v2.supports_plan_schema(PLUGIN_OPERATION_PLAN_SCHEMA_V3));
    assert!(v3.supports_plan_schema(PLUGIN_OPERATION_PLAN_SCHEMA_V3));
    assert!(v3
        .contract_schemas
        .contains(&PLUGIN_HOST_CAPABILITIES_SCHEMA_V3.to_owned()));

    let mut expanded_v2 = v2.clone();
    expanded_v2
        .plan_schemas
        .push(PLUGIN_OPERATION_PLAN_SCHEMA_V3.to_owned());
    assert!(expanded_v2.validate().is_err());
    let mut narrowed_v3 = v3;
    narrowed_v3.plan_schemas.pop();
    assert!(narrowed_v3.validate().is_err());

    let fixture = PluginHostCapabilities::from_json(HOST_CAPABILITIES_V3).unwrap();
    assert_eq!(
        fixture,
        PluginHostCapabilities::v3(
            "host:node-01",
            env!("CARGO_PKG_VERSION"),
            "use:0.2.1:linux-x86_64",
        )
        .unwrap()
    );
    assert_eq!(
        fixture.canonical_bytes().unwrap(),
        canonical_fixture(HOST_CAPABILITIES_V3)
    );
    assert_eq!(
        fixture.descriptor_digest().unwrap(),
        HOST_CAPABILITIES_V3_DIGEST
    );
}

#[test]
fn host_protocol_v1_rejects_flow_plans_while_v2_accepts_them() {
    let v1 = capabilities();
    let mut request = plan_request();
    request.candidate = Some(flow_candidate());
    request.capabilities_digest = v1.descriptor_digest().unwrap();
    request.validate().unwrap();
    let error = request.validate_for_capabilities(&v1).unwrap_err();
    assert_eq!(error.code, "use.plugin.host_surface_unsupported");

    let v2 = PluginHostCapabilities::v2(
        "host:node-01",
        env!("CARGO_PKG_VERSION"),
        "use:0.3.0:linux-x86_64",
    )
    .unwrap();
    request.capabilities_digest = v2.descriptor_digest().unwrap();
    request.validate_for_capabilities(&v2).unwrap();
}

#[test]
fn host_capability_scope_and_observation_fixtures_are_canonical() {
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

    let observation = PluginHostObservationResult::from_json(HOST_OBSERVATION).unwrap();
    assert_eq!(
        observation.canonical_bytes().unwrap(),
        canonical_fixture(HOST_OBSERVATION)
    );
    assert_eq!(
        observation.descriptor_digest().unwrap(),
        HOST_OBSERVATION_DIGEST
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
fn enablement_and_observation_share_one_use_owned_state_projection() {
    let capabilities_digest = capabilities().descriptor_digest().unwrap();
    let request = PluginHostEnablementRequest {
        schema: PLUGIN_HOST_ENABLEMENT_REQUEST_SCHEMA.to_owned(),
        request_id: "request:enable:0001".to_owned(),
        operation_id: "use-toggle:0001".to_owned(),
        assignment_generation: 4,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope(),
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
        expected_package_generation: 13,
        enabled: true,
    };
    request.validate().unwrap();
    let mut request_with_unknown = serde_json::to_value(&request).unwrap();
    request_with_unknown["unexpected"] = serde_json::json!(true);
    assert!(PluginHostEnablementRequest::from_json(
        &serde_json::to_vec(&request_with_unknown).unwrap()
    )
    .is_err());
    let mut enabled_state = installed_state(PluginDesiredState::Enabled);
    enabled_state.package_generation = Some(14);
    let result = PluginHostEnablementResult {
        schema: PLUGIN_HOST_ENABLEMENT_RESULT_SCHEMA.to_owned(),
        request_id: request.request_id.clone(),
        operation_id: request.operation_id.clone(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        completed_at_ms: 1_785_360_200_000,
        operation_result_digest: DIGEST_A.to_owned(),
        changed: true,
        state: enabled_state,
        replayed: false,
    };
    result.validate_for(&request, &capabilities()).unwrap();
    let mut result_with_unknown = serde_json::to_value(&result).unwrap();
    result_with_unknown["unexpected"] = serde_json::json!(true);
    assert!(PluginHostEnablementResult::from_json(
        &serde_json::to_vec(&result_with_unknown).unwrap()
    )
    .is_err());

    let observe = PluginHostObservationRequest {
        schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "request:observe:0001".to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest,
        scope: request.scope,
        package_id: request.package_id,
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
    assert_send_sync::<PluginHostEnablementRequest>();
    assert_send_sync::<PluginHostEnablementResult>();
    assert_send_sync::<PluginHostObservationRequest>();
    assert_send_sync::<PluginHostObservationResult>();
    assert_manager_port::<dyn PluginHostManager>();
}
