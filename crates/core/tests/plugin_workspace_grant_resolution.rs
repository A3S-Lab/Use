use a3s_use_core::{
    PlanActor, PlanPolicyDecision, PluginGrantConfirmation, PluginPermissionCeiling,
    PluginWorkspaceGrantProposal, WorkspaceGrantProposalAuthority,
    PLUGIN_GRANT_CONFIRMATION_SCHEMA, PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA,
};

const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const POLICY_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PLAN_DIGEST: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const OTHER_DIGEST: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const GRANT_PROPOSAL: &[u8] =
    include_bytes!("../fixtures/plugins/workspace-grant-proposal-v1.json");
const GRANT_PROPOSAL_DIGEST: &str =
    include_str!("../fixtures/plugins/workspace-grant-proposal-v1.sha256").trim_ascii_end();
const GRANT_CONFIRMATION: &[u8] = include_bytes!("../fixtures/plugins/grant-confirmation-v1.json");
const GRANT_CONFIRMATION_DIGEST: &str =
    include_str!("../fixtures/plugins/grant-confirmation-v1.sha256").trim_ascii_end();

#[test]
fn confirmed_proposal_deterministically_finalizes_a_secret_grant() {
    let ceiling = permission_ceiling();
    let proposal = proposal(ceiling.clone(), PlanActor::User, PlanPolicyDecision::Ask);
    proposal.validate_against(&ceiling).unwrap();
    let proposal_digest = proposal.descriptor_digest().unwrap();
    let confirmation = PluginGrantConfirmation {
        schema: PLUGIN_GRANT_CONFIRMATION_SCHEMA.to_string(),
        operation_id: proposal.operation_id.clone(),
        plan_digest: PLAN_DIGEST.to_string(),
        proposal_digest,
        confirmed_by: PlanActor::User,
        confirmed_at_ms: 1_500,
    };
    assert_eq!(
        proposal.canonical_bytes().unwrap(),
        canonical_fixture(GRANT_PROPOSAL)
    );
    assert_eq!(
        PluginWorkspaceGrantProposal::from_json(GRANT_PROPOSAL).unwrap(),
        proposal
    );
    assert_eq!(proposal.descriptor_digest().unwrap(), GRANT_PROPOSAL_DIGEST);
    assert_eq!(
        confirmation.canonical_bytes().unwrap(),
        canonical_fixture(GRANT_CONFIRMATION)
    );
    assert_eq!(
        PluginGrantConfirmation::from_json(GRANT_CONFIRMATION).unwrap(),
        confirmation
    );
    assert_eq!(
        confirmation.descriptor_digest().unwrap(),
        GRANT_CONFIRMATION_DIGEST
    );

    assert_eq!(
        proposal
            .finalize(&ceiling, PLAN_DIGEST, None, 1_600)
            .unwrap_err()
            .code,
        "use.plugin.grant_confirmation_required"
    );
    let grant = proposal
        .finalize(&ceiling, PLAN_DIGEST, Some(&confirmation), 1_600)
        .unwrap();
    assert_eq!(grant.scope_id, proposal.scope_id);
    assert_eq!(grant.package_digest, PACKAGE_DIGEST);
    assert_eq!(grant.granted_at_ms, confirmation.confirmed_at_ms);
    assert_eq!(grant.expires_at_ms, Some(5_000));
    assert_eq!(
        grant.authority.confirmation_digest,
        Some(confirmation.descriptor_digest().unwrap())
    );
    grant.validate_active_against(&ceiling, 1_600).unwrap();
}

#[test]
fn unattended_allow_proposal_finalizes_only_without_confirmation() {
    let ceiling = permission_ceiling();
    let mut permissions = ceiling.clone();
    permissions.surfaces[1].secrets.clear();
    let proposal = proposal(permissions, PlanActor::Agent, PlanPolicyDecision::Allow);

    let grant = proposal
        .finalize(&ceiling, PLAN_DIGEST, None, 1_600)
        .unwrap();
    assert_eq!(grant.authority.actor, PlanActor::Agent);
    assert_eq!(grant.authority.decision, PlanPolicyDecision::Allow);
    assert_eq!(grant.authority.confirmation_digest, None);
    assert_eq!(grant.granted_at_ms, 1_600);

    let confirmation = confirmation(&proposal, PLAN_DIGEST, 1_500);
    assert_eq!(
        proposal
            .finalize(&ceiling, PLAN_DIGEST, Some(&confirmation), 1_600)
            .unwrap_err()
            .code,
        "use.plugin.grant_confirmation_mismatch"
    );
}

#[test]
fn confirmation_binds_the_exact_operation_plan_proposal_and_time() {
    let ceiling = permission_ceiling();
    let proposal = proposal(ceiling.clone(), PlanActor::User, PlanPolicyDecision::Ask);

    let wrong_plan = confirmation(&proposal, OTHER_DIGEST, 1_500);
    assert_confirmation_mismatch(&proposal, &ceiling, &wrong_plan, 1_600);

    let mut wrong_proposal = confirmation(&proposal, PLAN_DIGEST, 1_500);
    wrong_proposal.proposal_digest = OTHER_DIGEST.to_string();
    assert_confirmation_mismatch(&proposal, &ceiling, &wrong_proposal, 1_600);

    let mut wrong_operation = confirmation(&proposal, PLAN_DIGEST, 1_500);
    wrong_operation.operation_id = "install:acme-research:0002".to_string();
    assert_confirmation_mismatch(&proposal, &ceiling, &wrong_operation, 1_600);

    let future = confirmation(&proposal, PLAN_DIGEST, 1_700);
    assert_confirmation_mismatch(&proposal, &ceiling, &future, 1_600);

    let mut wrong_actor = confirmation(&proposal, PLAN_DIGEST, 1_500);
    wrong_actor.confirmed_by = PlanActor::Agent;
    assert_eq!(
        wrong_actor.validate().unwrap_err().code,
        "use.plugin.grant_confirmation_invalid"
    );
}

#[test]
fn proposal_lifetime_and_secret_authority_fail_closed() {
    let ceiling = permission_ceiling();
    let user_proposal = proposal(ceiling.clone(), PlanActor::User, PlanPolicyDecision::Ask);
    let confirmation = confirmation(&user_proposal, PLAN_DIGEST, 1_500);
    assert_eq!(
        user_proposal
            .finalize(&ceiling, PLAN_DIGEST, Some(&confirmation), 2_000)
            .unwrap_err()
            .code,
        "use.plugin.grant_proposal_expired"
    );

    let agent_secret = proposal(ceiling, PlanActor::Agent, PlanPolicyDecision::Ask);
    assert_eq!(
        agent_secret.validate().unwrap_err().code,
        "use.plugin.grant_proposal_invalid"
    );
}

#[test]
fn proposal_rejects_ceiling_escalation_and_unknown_privileged_fields() {
    let ceiling = permission_ceiling();
    let mut permissions = ceiling.clone();
    permissions.surfaces[1]
        .resources
        .as_mut()
        .unwrap()
        .cpu_millis += 1;
    let escalated = proposal(permissions, PlanActor::User, PlanPolicyDecision::Ask);
    assert_eq!(
        escalated.validate_against(&ceiling).unwrap_err().code,
        "use.plugin.grant_exceeds_ceiling"
    );

    let mut proposal_value: serde_json::Value = serde_json::from_slice(GRANT_PROPOSAL).unwrap();
    proposal_value["authority"]["secretValue"] = serde_json::json!("do-not-echo");
    let proposal_error =
        PluginWorkspaceGrantProposal::from_json(&serde_json::to_vec(&proposal_value).unwrap())
            .unwrap_err();
    assert_eq!(proposal_error.code, "use.plugin.grant_proposal_invalid");
    assert!(!proposal_error.message.contains("do-not-echo"));

    let mut confirmation_value: serde_json::Value =
        serde_json::from_slice(GRANT_CONFIRMATION).unwrap();
    confirmation_value["userToken"] = serde_json::json!("do-not-echo");
    let confirmation_error =
        PluginGrantConfirmation::from_json(&serde_json::to_vec(&confirmation_value).unwrap())
            .unwrap_err();
    assert_eq!(
        confirmation_error.code,
        "use.plugin.grant_confirmation_invalid"
    );
    assert!(!confirmation_error.message.contains("do-not-echo"));
}

fn permission_ceiling() -> PluginPermissionCeiling {
    PluginPermissionCeiling::from_json(include_bytes!(
        "../fixtures/plugins/permission-ceiling-v1.json"
    ))
    .unwrap()
}

fn proposal(
    permissions: PluginPermissionCeiling,
    actor: PlanActor,
    decision: PlanPolicyDecision,
) -> PluginWorkspaceGrantProposal {
    let ceiling = permission_ceiling();
    PluginWorkspaceGrantProposal {
        schema: PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA.to_string(),
        operation_id: "install:acme-research:0001".to_string(),
        scope_id: "workspace-01".to_string(),
        package_id: "acme/research".to_string(),
        package_digest: PACKAGE_DIGEST.to_string(),
        permission_ceiling_digest: ceiling.descriptor_digest().unwrap(),
        permissions_digest: permissions.descriptor_digest().unwrap(),
        permissions,
        authority: WorkspaceGrantProposalAuthority {
            actor,
            decision,
            policy_digest: POLICY_DIGEST.to_string(),
        },
        created_at_ms: 1_000,
        apply_expires_at_ms: 2_000,
        grant_expires_at_ms: Some(5_000),
    }
}

fn confirmation(
    proposal: &PluginWorkspaceGrantProposal,
    plan_digest: &str,
    confirmed_at_ms: u64,
) -> PluginGrantConfirmation {
    PluginGrantConfirmation {
        schema: PLUGIN_GRANT_CONFIRMATION_SCHEMA.to_string(),
        operation_id: proposal.operation_id.clone(),
        plan_digest: plan_digest.to_string(),
        proposal_digest: proposal.descriptor_digest().unwrap(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms,
    }
}

fn assert_confirmation_mismatch(
    proposal: &PluginWorkspaceGrantProposal,
    ceiling: &PluginPermissionCeiling,
    confirmation: &PluginGrantConfirmation,
    applied_at_ms: u64,
) {
    assert_eq!(
        proposal
            .finalize(ceiling, PLAN_DIGEST, Some(confirmation), applied_at_ms,)
            .unwrap_err()
            .code,
        "use.plugin.grant_confirmation_mismatch"
    );
}

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
