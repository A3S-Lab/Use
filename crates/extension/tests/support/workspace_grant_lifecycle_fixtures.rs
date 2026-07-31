use a3s_use_core::{
    PlanActor, PlanPolicyDecision, PluginPermissionCeiling, PluginWorkspaceGrant,
    PluginWorkspaceGrantSnapshot, ResolvedWorkspaceGrant, ResolvedWorkspaceGrantChangeSet,
    WorkspaceGrantAuthority, WorkspaceGrantEvidence, PLUGIN_WORKSPACE_GRANT_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};
use a3s_use_extension::{
    WorkspaceGrantCandidateCeiling, WorkspaceGrantCutoverEvidence, WorkspaceGrantReceipt,
    WORKSPACE_GRANT_CUTOVER_SCHEMA,
};

pub struct LifecycleFixture {
    pub ceiling: PluginPermissionCeiling,
    pub resolved: ResolvedWorkspaceGrantChangeSet,
    pub ceilings: Vec<WorkspaceGrantCandidateCeiling>,
    pub priors: Vec<WorkspaceGrantReceipt>,
}

pub fn install_fixture() -> LifecycleFixture {
    let ceiling = permission_ceiling();
    let grants = vec![
        resolved_grant(&ceiling, "acme/helper", &digest('d'), &digest('4'), 1_100),
        resolved_grant(&ceiling, "acme/research", &digest('e'), &digest('5'), 1_100),
    ];
    fixture("install:acme-research:0003", 5, 10, grants, Vec::new())
}

pub fn upgrade_fixture() -> LifecycleFixture {
    let ceiling = permission_ceiling();
    let priors = vec![
        WorkspaceGrantReceipt::new(4, grant(&ceiling, "acme/helper", &digest('a'), 900)).unwrap(),
        WorkspaceGrantReceipt::new(5, grant(&ceiling, "acme/research", &digest('b'), 950)).unwrap(),
    ];
    let grants = vec![
        resolved_grant(&ceiling, "acme/helper", &digest('d'), &digest('4'), 1_100),
        resolved_grant(&ceiling, "acme/research", &digest('e'), &digest('5'), 1_100),
    ];
    fixture("upgrade:acme-research:0004", 5, 10, grants, priors)
}

pub fn in_place_fixture() -> LifecycleFixture {
    let ceiling = permission_ceiling();
    let prior =
        WorkspaceGrantReceipt::new(5, grant(&ceiling, "acme/research", &digest('a'), 900)).unwrap();
    let candidate = resolved_grant(&ceiling, "acme/research", &digest('a'), &digest('4'), 1_100);
    fixture(
        "upgrade:acme-research:in-place",
        5,
        10,
        vec![candidate],
        vec![prior],
    )
}

pub fn cutover(resolved: &ResolvedWorkspaceGrantChangeSet) -> WorkspaceGrantCutoverEvidence {
    WorkspaceGrantCutoverEvidence {
        schema: WORKSPACE_GRANT_CUTOVER_SCHEMA.to_string(),
        capability_generation_before: resolved.capability_generation_before,
        capability_generation_after: resolved.capability_generation_after,
        capability_snapshot_digest: digest('6'),
        committed_at_ms: resolved.transitioned_at_ms + 100,
    }
}

pub fn evidence(receipt: &WorkspaceGrantReceipt) -> WorkspaceGrantEvidence {
    WorkspaceGrantEvidence {
        package_id: receipt.grant.package_id.clone(),
        package_digest: receipt.grant.package_digest.clone(),
        receipt_revision: receipt.revision,
        grant_digest: receipt.grant_digest.clone(),
    }
}

pub fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

fn fixture(
    operation_id: &str,
    state_revision_before: u64,
    capability_generation_before: u64,
    grants: Vec<ResolvedWorkspaceGrant>,
    priors: Vec<WorkspaceGrantReceipt>,
) -> LifecycleFixture {
    let ceiling = permission_ceiling();
    let ceilings = grants
        .iter()
        .map(|candidate| WorkspaceGrantCandidateCeiling {
            package_id: candidate.grant.package_id.clone(),
            package_digest: candidate.grant.package_digest.clone(),
            ceiling: ceiling.clone(),
        })
        .collect();
    let revocations = priors.iter().map(evidence).collect::<Vec<_>>();
    let before_snapshot_digest = (!revocations.is_empty())
        .then(|| PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
            scope_id: "workspace-01".to_string(),
            state_revision: state_revision_before,
            grants: revocations.clone(),
        })
        .map(|snapshot| snapshot.descriptor_digest().unwrap());
    let resolved = ResolvedWorkspaceGrantChangeSet {
        operation_id: operation_id.to_string(),
        plan_digest: digest('1'),
        change_set_digest: digest('2'),
        scope_id: "workspace-01".to_string(),
        state_revision_before,
        revision: state_revision_before + 1,
        capability_generation_before,
        capability_generation_after: capability_generation_before + 1,
        before_snapshot_digest,
        transitioned_at_ms: 1_200,
        revocation_authority: authority(),
        grants,
        revocations,
    };
    resolved.validate().unwrap();
    LifecycleFixture {
        ceiling,
        resolved,
        ceilings,
        priors,
    }
}

fn resolved_grant(
    ceiling: &PluginPermissionCeiling,
    package_id: &str,
    package_digest: &str,
    proposal_digest: &str,
    granted_at_ms: u64,
) -> ResolvedWorkspaceGrant {
    ResolvedWorkspaceGrant {
        proposal_digest: proposal_digest.to_string(),
        grant: grant(ceiling, package_id, package_digest, granted_at_ms),
    }
}

fn permission_ceiling() -> PluginPermissionCeiling {
    PluginPermissionCeiling::from_json(include_bytes!(
        "../../../core/fixtures/plugins/permission-ceiling-v1.json"
    ))
    .unwrap()
}

fn grant(
    ceiling: &PluginPermissionCeiling,
    package_id: &str,
    package_digest: &str,
    granted_at_ms: u64,
) -> PluginWorkspaceGrant {
    PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: "workspace-01".to_string(),
        package_id: package_id.to_string(),
        package_digest: package_digest.to_string(),
        permission_ceiling_digest: ceiling.descriptor_digest().unwrap(),
        permissions_digest: ceiling.descriptor_digest().unwrap(),
        permissions: ceiling.clone(),
        authority: authority(),
        granted_at_ms,
        expires_at_ms: None,
    }
}

fn authority() -> WorkspaceGrantAuthority {
    WorkspaceGrantAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: digest('b'),
        confirmation_digest: Some(digest('c')),
    }
}
