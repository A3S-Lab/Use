use a3s_use_core::{
    PlanActor, PlanPolicyDecision, PluginPermissionCeiling, PluginWorkspaceGrant,
    WorkspaceGrantAuthority, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};

const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const POLICY_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CONFIRMATION_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const WORKSPACE_GRANT: &[u8] = include_bytes!("../fixtures/plugins/workspace-grant-v1.json");
const WORKSPACE_GRANT_DIGEST: &str =
    include_str!("../fixtures/plugins/workspace-grant-v1.sha256").trim_ascii_end();

#[test]
fn workspace_grant_is_canonical_scope_and_package_bound_evidence() {
    let ceiling = permission_ceiling();
    let grant = grant(
        ceiling.clone(),
        PlanActor::User,
        PlanPolicyDecision::Ask,
        Some(CONFIRMATION_DIGEST),
    );

    grant.validate_active_against(&ceiling, 1_500).unwrap();
    grant.validate_at(1_500).unwrap();
    assert_eq!(
        grant.validate_at(2_000).unwrap_err().code,
        "use.plugin.grant_expired"
    );
    let canonical = grant.canonical_bytes().unwrap();
    assert_eq!(canonical, canonical_fixture(WORKSPACE_GRANT));
    assert_eq!(
        PluginWorkspaceGrant::from_json(WORKSPACE_GRANT).unwrap(),
        grant
    );
    assert_eq!(grant.descriptor_digest().unwrap(), WORKSPACE_GRANT_DIGEST);
}

#[test]
fn secret_grants_require_an_explicit_user_confirmation() {
    let ceiling = permission_ceiling();
    let agent = grant(
        ceiling.clone(),
        PlanActor::Agent,
        PlanPolicyDecision::Allow,
        None,
    );
    assert_eq!(
        agent.validate_against(&ceiling).unwrap_err().code,
        "use.plugin.grant_invalid"
    );

    let user_without_confirmation = grant(
        ceiling.clone(),
        PlanActor::User,
        PlanPolicyDecision::Allow,
        None,
    );
    assert_eq!(
        user_without_confirmation
            .validate_against(&ceiling)
            .unwrap_err()
            .code,
        "use.plugin.grant_invalid"
    );
}

#[test]
fn resolved_permissions_cannot_exceed_the_signed_ceiling() {
    let ceiling = permission_ceiling();
    let mut permissions = ceiling.clone();
    let convert = permissions
        .surfaces
        .iter_mut()
        .find(|permission| permission.surface.id == "convert")
        .unwrap();
    convert.resources.as_mut().unwrap().cpu_millis = 2_000;
    let escalated = grant(
        permissions,
        PlanActor::User,
        PlanPolicyDecision::Ask,
        Some(CONFIRMATION_DIGEST),
    );

    assert!(!escalated.permissions.is_within(&ceiling).unwrap());
    assert_eq!(
        escalated.validate_against(&ceiling).unwrap_err().code,
        "use.plugin.grant_exceeds_ceiling"
    );
}

#[test]
fn a_narrow_secret_free_agent_grant_is_within_the_ceiling() {
    let ceiling = permission_ceiling();
    let mut permissions = ceiling.clone();
    let convert = permissions
        .surfaces
        .iter_mut()
        .find(|permission| permission.surface.id == "convert")
        .unwrap();
    convert.secrets.clear();
    convert.filesystem.retain(|path| path.path == "inputs");
    convert.resources.as_mut().unwrap().cpu_millis = 500;
    let grant = grant(
        permissions,
        PlanActor::Agent,
        PlanPolicyDecision::Allow,
        None,
    );

    grant.validate_against(&ceiling).unwrap();
    assert!(grant.permissions.is_within(&ceiling).unwrap());
}

#[test]
fn filesystem_network_and_ui_authority_must_only_narrow() {
    let ceiling = permission_ceiling();

    let mut filesystem = ceiling.clone();
    let convert = filesystem
        .surfaces
        .iter_mut()
        .find(|permission| permission.surface.id == "convert")
        .unwrap();
    convert
        .filesystem
        .iter_mut()
        .find(|permission| permission.path == "inputs")
        .unwrap()
        .path = ".".to_string();
    assert_exceeds(filesystem, &ceiling);

    let mut network = ceiling.clone();
    let convert = network
        .surfaces
        .iter_mut()
        .find(|permission| permission.surface.id == "convert")
        .unwrap();
    convert.network_egress[0].ports = vec![80];
    assert_exceeds(network, &ceiling);

    let mut ui = ceiling.clone();
    let review = ui
        .surfaces
        .iter_mut()
        .find(|permission| permission.surface.id == "review")
        .unwrap();
    review.ui_http[0].path_prefixes = vec!["/".to_string()];
    assert_exceeds(ui, &ceiling);
}

fn permission_ceiling() -> PluginPermissionCeiling {
    PluginPermissionCeiling::from_json(include_bytes!(
        "../fixtures/plugins/permission-ceiling-v1.json"
    ))
    .unwrap()
}

fn grant(
    permissions: PluginPermissionCeiling,
    actor: PlanActor,
    decision: PlanPolicyDecision,
    confirmation_digest: Option<&str>,
) -> PluginWorkspaceGrant {
    let ceiling = permission_ceiling();
    PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: "workspace-01".to_string(),
        package_id: "acme/research".to_string(),
        package_digest: PACKAGE_DIGEST.to_string(),
        permission_ceiling_digest: ceiling.descriptor_digest().unwrap(),
        permissions_digest: permissions.descriptor_digest().unwrap(),
        permissions,
        authority: WorkspaceGrantAuthority {
            actor,
            decision,
            policy_digest: POLICY_DIGEST.to_string(),
            confirmation_digest: confirmation_digest.map(str::to_string),
        },
        granted_at_ms: 1_000,
        expires_at_ms: Some(2_000),
    }
}

fn assert_exceeds(permissions: PluginPermissionCeiling, ceiling: &PluginPermissionCeiling) {
    let grant = grant(
        permissions,
        PlanActor::User,
        PlanPolicyDecision::Ask,
        Some(CONFIRMATION_DIGEST),
    );
    assert!(!grant.permissions.is_within(ceiling).unwrap());
    assert_eq!(
        grant.validate_against(ceiling).unwrap_err().code,
        "use.plugin.grant_exceeds_ceiling"
    );
}

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
