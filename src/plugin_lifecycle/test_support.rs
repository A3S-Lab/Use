use a3s_use_core::{
    CatalogMcpTransport, PlanPolicyDecision, PlanScopeKind, PlannedWorkspaceImpact,
    PluginOperationPlan, PluginOperationPlanEnvelope, PluginSurfaceKind, PluginWorkspaceGrant,
    ResolvedWorkspaceGrant, ResolvedWorkspaceGrantChangeSet, ToolWorkloadClass,
    WorkspaceGrantAuthority, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};
use a3s_use_extension::WorkspaceGrantCandidateCeiling;

use crate::plugin_runtime::{
    RuntimeBindingCandidateKind, RuntimeBindingCandidatePlan, RuntimeBindingOperationIntent,
    RuntimeBindingReceipt, RuntimeEndpointRef, RuntimeMcpInitializeEvidence,
    RuntimePreparedTaskBinding, RuntimeServiceBindingReceipt, RuntimeServiceReadinessEvidence,
    RuntimeSurfaceContract, RUNTIME_SERVICE_BINDING_SCHEMA, RUNTIME_TASK_BINDING_SCHEMA,
};

pub(super) const TRANSITIONED_AT_MS: u64 = 1_785_360_000_100;
pub(super) const COMMITTED_AT_MS: u64 = TRANSITIONED_AT_MS + 100;
pub(super) const SNAPSHOT_DIGEST: &str =
    "sha256:3333333333333333333333333333333333333333333333333333333333333333";

const DESCRIPTOR_DIGEST: &str =
    "sha256:4444444444444444444444444444444444444444444444444444444444444444";
const ARTIFACT_DIGEST: &str =
    "sha256:5555555555555555555555555555555555555555555555555555555555555555";
const SPEC_DIGEST: &str = "sha256:6666666666666666666666666666666666666666666666666666666666666666";
const PROPOSAL_DIGEST: &str =
    "sha256:7777777777777777777777777777777777777777777777777777777777777777";
const CONFIRMATION_DIGEST: &str =
    "sha256:8888888888888888888888888888888888888888888888888888888888888888";

pub(super) fn canonical_envelope() -> PluginOperationPlanEnvelope {
    let plan = PluginOperationPlan::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/operation-plan-install-v1.json"
    ))
    .unwrap();
    PluginOperationPlanEnvelope::new(plan).unwrap()
}

pub(super) fn runtime_only_envelope() -> PluginOperationPlanEnvelope {
    let mut envelope = canonical_envelope();
    envelope.plan.workspace_impacts[0].grant_after_digest = None;
    PluginOperationPlanEnvelope::new(envelope.plan).unwrap()
}

pub(super) fn multi_scope_runtime_envelope() -> PluginOperationPlanEnvelope {
    let mut envelope = runtime_only_envelope();
    envelope.plan.scope.kind = PlanScopeKind::User;
    envelope.plan.scope.id = "user:alice".to_string();
    envelope.plan.workspace_impacts = ["workspace:alpha", "workspace:beta"]
        .into_iter()
        .map(|scope_id| PlannedWorkspaceImpact {
            scope_id: scope_id.to_string(),
            grant_before_digest: None,
            grant_after_digest: None,
            enabled_before: false,
            enabled_after: true,
        })
        .collect();
    PluginOperationPlanEnvelope::new(envelope.plan).unwrap()
}

pub(super) fn runtime_intent(
    envelope: &PluginOperationPlanEnvelope,
    scope_id: &str,
    grant_change_set_digest: Option<String>,
) -> RuntimeBindingOperationIntent {
    let package = envelope.plan.packages[0].after.as_ref().unwrap();
    let candidates = envelope
        .plan
        .providers
        .iter()
        .filter_map(|provider| {
            let surface = package
                .release
                .surfaces
                .iter()
                .find(|surface| surface.reference() == provider.surface.surface)
                .unwrap();
            let kind = match (surface.kind, surface.workload, surface.mcp_transport) {
                (PluginSurfaceKind::Tool, Some(ToolWorkloadClass::Task), _) => {
                    RuntimeBindingCandidateKind::Task {
                        artifact_digest: ARTIFACT_DIGEST.to_string(),
                        artifact_media_type: "application/vnd.oci.image.manifest.v1+json"
                            .to_string(),
                    }
                }
                (PluginSurfaceKind::Tool, Some(ToolWorkloadClass::Service), _) => {
                    RuntimeBindingCandidateKind::Service {
                        unit_id: format!("use:service:{}", surface.id),
                        spec_digest: SPEC_DIGEST.to_string(),
                        contract: RuntimeSurfaceContract::ToolService {
                            port_name: "http".to_string(),
                            base_path: "/api".to_string(),
                            shutdown_grace_ms: 30_000,
                            api_contract_digest: None,
                        },
                    }
                }
                (PluginSurfaceKind::Mcp, _, Some(CatalogMcpTransport::StreamableHttp)) => {
                    RuntimeBindingCandidateKind::Service {
                        unit_id: format!("use:mcp:{}", surface.id),
                        spec_digest: SPEC_DIGEST.to_string(),
                        contract: RuntimeSurfaceContract::McpService {
                            port_name: "http".to_string(),
                            endpoint_path: "/mcp".to_string(),
                            protocol_version: "2025-03-26".to_string(),
                            shutdown_grace_ms: 30_000,
                        },
                    }
                }
                _ => return None,
            };
            Some(RuntimeBindingCandidatePlan {
                surface: provider.surface.clone(),
                package_digest: package.release.package_sha256.clone(),
                scope_id: scope_id.to_string(),
                descriptor_digest: DESCRIPTOR_DIGEST.to_string(),
                provider: provider.clone(),
                generation: envelope.plan.state.capability_generation + 1,
                kind,
            })
        })
        .collect();
    RuntimeBindingOperationIntent::new(
        &envelope.plan.operation_id,
        &envelope.plan_digest,
        grant_change_set_digest,
        scope_id,
        envelope.plan.state.state_revision,
        envelope.plan.state.capability_generation,
        TRANSITIONED_AT_MS,
        candidates,
        Vec::new(),
    )
    .unwrap()
}

pub(super) fn prepared_receipt(candidate: &RuntimeBindingCandidatePlan) -> RuntimeBindingReceipt {
    let receipt = match &candidate.kind {
        RuntimeBindingCandidateKind::Task {
            artifact_digest,
            artifact_media_type,
        } => RuntimeBindingReceipt::Task(RuntimePreparedTaskBinding {
            schema: RUNTIME_TASK_BINDING_SCHEMA.to_string(),
            surface: candidate.surface.clone(),
            package_digest: candidate.package_digest.clone(),
            scope_id: candidate.scope_id.clone(),
            descriptor_digest: candidate.descriptor_digest.clone(),
            provider_id: candidate.provider.provider_id.clone(),
            provider_build_id: candidate.provider.provider_build_id.clone(),
            capability_digest: candidate.provider.capability_digest.clone(),
            enforcement: candidate.provider.enforcement,
            artifact_digest: artifact_digest.clone(),
            artifact_media_type: artifact_media_type.clone(),
            generation: candidate.generation,
            semantics_profile_digest: candidate.provider.semantics_profile_digest.clone(),
        }),
        RuntimeBindingCandidateKind::Service {
            unit_id,
            spec_digest,
            contract,
        } => {
            let readiness = match contract {
                RuntimeSurfaceContract::ToolService { .. } => {
                    RuntimeServiceReadinessEvidence::HttpHealthy
                }
                RuntimeSurfaceContract::McpService {
                    protocol_version, ..
                } => RuntimeServiceReadinessEvidence::McpInitialized {
                    initialize: RuntimeMcpInitializeEvidence::new(protocol_version, 1).unwrap(),
                },
                RuntimeSurfaceContract::ToolTask { .. } => unreachable!(),
            };
            RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
                schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
                surface: candidate.surface.clone(),
                package_digest: candidate.package_digest.clone(),
                scope_id: candidate.scope_id.clone(),
                descriptor_digest: candidate.descriptor_digest.clone(),
                provider_id: candidate.provider.provider_id.clone(),
                provider_build_id: candidate.provider.provider_build_id.clone(),
                capability_digest: candidate.provider.capability_digest.clone(),
                enforcement: candidate.provider.enforcement,
                unit_id: unit_id.clone(),
                generation: candidate.generation,
                spec_digest: spec_digest.clone(),
                semantics_profile_digest: candidate.provider.semantics_profile_digest.clone(),
                endpoint_ref: RuntimeEndpointRef::parse(format!(
                    "gateway:lifecycle-{}",
                    candidate.surface.surface.id
                ))
                .unwrap(),
                runtime_started_at_ms: 1,
                observation_revision: 1,
                last_healthy_at_ms: 1,
                contract: contract.clone(),
                readiness,
            })
        }
    };
    receipt.validate().unwrap();
    receipt
}

pub(super) fn grant_fixture(
    envelope: &PluginOperationPlanEnvelope,
) -> (
    ResolvedWorkspaceGrantChangeSet,
    Vec<WorkspaceGrantCandidateCeiling>,
) {
    let scope_id = &envelope.plan.workspace_impacts[0].scope_id;
    let change_set_digest = envelope.plan.workspace_impacts[0]
        .grant_after_digest
        .clone()
        .unwrap();
    let package = &envelope.plan.packages[0];
    let state = package.after.as_ref().unwrap();
    let ceiling = state.permissions.clone();
    let authority = WorkspaceGrantAuthority {
        actor: envelope.plan.authority.actor,
        decision: envelope.plan.authority.decision,
        policy_digest: envelope.plan.authority.policy_digest.clone(),
        confirmation_digest: (envelope.plan.authority.decision == PlanPolicyDecision::Ask)
            .then(|| CONFIRMATION_DIGEST.to_string()),
    };
    let grant = PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: scope_id.clone(),
        package_id: package.package_id.clone(),
        package_digest: state.release.package_sha256.clone(),
        permission_ceiling_digest: state.release.permission_ceiling_digest.clone(),
        permissions_digest: ceiling.descriptor_digest().unwrap(),
        permissions: ceiling.clone(),
        authority: authority.clone(),
        granted_at_ms: TRANSITIONED_AT_MS - 1,
        expires_at_ms: None,
    };
    let resolved = ResolvedWorkspaceGrantChangeSet {
        operation_id: envelope.plan.operation_id.clone(),
        plan_digest: envelope.plan_digest.clone(),
        change_set_digest,
        scope_id: scope_id.clone(),
        state_revision_before: envelope.plan.state.state_revision,
        revision: envelope.plan.state.state_revision + 1,
        capability_generation_before: envelope.plan.state.capability_generation,
        capability_generation_after: envelope.plan.state.capability_generation + 1,
        before_snapshot_digest: None,
        transitioned_at_ms: TRANSITIONED_AT_MS,
        revocation_authority: authority,
        grants: vec![ResolvedWorkspaceGrant {
            proposal_digest: PROPOSAL_DIGEST.to_string(),
            grant,
        }],
        revocations: Vec::new(),
    };
    resolved.validate().unwrap();
    let ceilings = vec![WorkspaceGrantCandidateCeiling {
        package_id: package.package_id.clone(),
        package_digest: state.release.package_sha256.clone(),
        ceiling,
    }];
    ceilings[0].validate().unwrap();
    (resolved, ceilings)
}
