use a3s_use_core::{
    CatalogSurface, PlanPackageChangeKind, PlanPackageRole, PlanQualifiedSurfaceRef,
    PlannedPackageState, PlannedPackageTransition, PlannedProviderEvidence,
    PlannedSecretChangeKind, PlannedSurfaceChange, PlannedWorkspaceGrantChange,
    PluginGrantConfirmation, PluginOperationAction, PluginOperationConfirmation,
    PluginOperationPlan, PluginPermissionCeiling, PluginPlanSource, PluginSurfaceKind,
    PluginSurfaceRef, PluginWorkspaceGrantChangeSet, PluginWorkspaceGrantProposal,
    PluginWorkspaceGrantSnapshot, SurfaceChangeKind, ToolWorkloadClass, WorkspaceGrantEvidence,
    WorkspaceGrantProposalAuthority, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
    PLUGIN_PERMISSION_SCHEMA, PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};

const INSTALL_PLAN: &[u8] = include_bytes!("../../fixtures/plugins/operation-plan-install-v1.json");
pub const DIGEST_C: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
pub const DIGEST_E: &str =
    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const DIGEST_F: &str = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

pub fn multi_package_install() -> (PluginOperationPlan, PluginWorkspaceGrantChangeSet) {
    let mut plan = PluginOperationPlan::from_json(INSTALL_PLAN).unwrap();
    let helper = helper_transition(&plan);
    plan.packages.insert(0, helper);
    plan.providers.insert(0, helper_provider());

    let changes = change_set_for_after(&plan, None);
    plan.workspace_impacts[0].grant_after_digest = Some(changes.descriptor_digest().unwrap());
    plan.validate().unwrap();
    (plan, changes)
}

pub fn multi_package_uninstall() -> (
    PluginOperationPlan,
    PluginWorkspaceGrantChangeSet,
    PluginWorkspaceGrantSnapshot,
) {
    let (mut plan, _) = multi_package_install();
    plan.action = PluginOperationAction::Uninstall;
    plan.operation_id = "uninstall:acme-research:0002".to_string();
    plan.created_at_ms += 1_000;
    plan.expires_at_ms += 1_000;
    plan.providers.clear();
    plan.state.state_revision = 10;
    plan.state.receipt_digest = Some(DIGEST_C.to_string());
    plan.impact.download_bytes = 0;
    plan.impact.installed_bytes_after = 0;
    plan.impact.reclaimed_bytes = 4_194_304;
    plan.impact.drain_required = true;
    plan.impact.retained_data = true;
    plan.secret_changes[0].change = PlannedSecretChangeKind::Revoke;
    plan.workspace_impacts[0].enabled_before = true;
    plan.workspace_impacts[0].enabled_after = false;

    for package in &mut plan.packages {
        let before = package.after.take().unwrap();
        package.before = Some(before.clone());
        package.change = PlanPackageChangeKind::Remove;
        package.source = None;
        package.surfaces = before
            .release
            .surfaces
            .iter()
            .map(|surface| PlannedSurfaceChange {
                surface: PluginSurfaceRef {
                    kind: surface.kind,
                    id: surface.id.clone(),
                },
                change: SurfaceChangeKind::Remove,
                before_digest: Some(surface.descriptor_digest().unwrap()),
                after_digest: None,
            })
            .collect();
    }

    let snapshot = PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: plan.scope.id.clone(),
        state_revision: plan.state.state_revision,
        grants: vec![
            WorkspaceGrantEvidence {
                package_id: "acme/helper".to_string(),
                package_digest: DIGEST_D.to_string(),
                receipt_revision: 8,
                grant_digest: DIGEST_E.to_string(),
            },
            WorkspaceGrantEvidence {
                package_id: "acme/research".to_string(),
                package_digest: package_digest(&plan, "acme/research", true).to_string(),
                receipt_revision: 9,
                grant_digest: DIGEST_F.to_string(),
            },
        ],
    };
    let before_digest = snapshot.descriptor_digest().unwrap();
    let changes = PluginWorkspaceGrantChangeSet {
        schema: PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA.to_string(),
        operation_id: plan.operation_id.clone(),
        scope_id: plan.scope.id.clone(),
        state_revision: plan.state.state_revision,
        before_snapshot_digest: Some(before_digest.clone()),
        changes: snapshot
            .grants
            .iter()
            .map(|evidence| PlannedWorkspaceGrantChange {
                package_id: evidence.package_id.clone(),
                before: Some(evidence.clone()),
                after: None,
            })
            .collect(),
    };
    plan.workspace_impacts[0].grant_before_digest = Some(before_digest);
    plan.workspace_impacts[0].grant_after_digest = Some(changes.descriptor_digest().unwrap());
    plan.validate().unwrap();
    (plan, changes, snapshot)
}

pub fn confirmations(
    changes: &PluginWorkspaceGrantChangeSet,
    plan_digest: &str,
) -> Vec<PluginGrantConfirmation> {
    changes
        .changes
        .iter()
        .filter_map(|change| change.after.as_ref())
        .map(|proposal| PluginGrantConfirmation {
            schema: a3s_use_core::PLUGIN_GRANT_CONFIRMATION_SCHEMA.to_string(),
            operation_id: proposal.operation_id.clone(),
            plan_digest: plan_digest.to_string(),
            proposal_digest: proposal.descriptor_digest().unwrap(),
            confirmed_by: a3s_use_core::PlanActor::User,
            confirmed_at_ms: proposal.created_at_ms + 100,
        })
        .collect()
}

pub fn operation_confirmation(plan: &PluginOperationPlan) -> PluginOperationConfirmation {
    PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: plan.operation_id.clone(),
        plan_digest: plan.descriptor_digest().unwrap(),
        confirmed_by: a3s_use_core::PlanActor::User,
        confirmed_at_ms: plan.created_at_ms + 100,
    }
}

fn helper_transition(plan: &PluginOperationPlan) -> PlannedPackageTransition {
    let root = plan
        .packages
        .iter()
        .find(|package| package.role == PlanPackageRole::Root)
        .unwrap();
    let mut permission = root
        .after
        .as_ref()
        .unwrap()
        .permissions
        .surfaces
        .iter()
        .find(|permission| {
            permission.surface.kind == PluginSurfaceKind::Tool && permission.surface.id == "convert"
        })
        .unwrap()
        .clone();
    permission.surface.id = "run".to_string();
    permission.secrets.clear();
    let permissions = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_string(),
        surfaces: vec![permission],
    };
    let surface = CatalogSurface {
        kind: PluginSurfaceKind::Tool,
        id: "run".to_string(),
        optional: false,
        workload: Some(ToolWorkloadClass::Task),
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    };
    let state = PlannedPackageState {
        release: a3s_use_core::PlannedPluginRelease {
            package_id: "acme/helper".to_string(),
            version: "1.0.0".to_string(),
            channel: a3s_use_core::PluginReleaseChannel::Stable,
            target: "linux-x86_64".to_string(),
            package_sha256: DIGEST_D.to_string(),
            manifest_sha256: DIGEST_E.to_string(),
            permission_ceiling_digest: permissions.descriptor_digest().unwrap(),
            surfaces: vec![surface.clone()],
        },
        permissions,
    };
    PlannedPackageTransition {
        package_id: "acme/helper".to_string(),
        role: PlanPackageRole::Dependency,
        change: PlanPackageChangeKind::Add,
        before: None,
        after: Some(state),
        source: Some(PluginPlanSource::ReleaseBundle {
            bundle_digest: DIGEST_F.to_string(),
            package_digest: DIGEST_D.to_string(),
        }),
        surfaces: vec![PlannedSurfaceChange {
            surface: PluginSurfaceRef {
                kind: surface.kind,
                id: surface.id.clone(),
            },
            change: SurfaceChangeKind::Add,
            before_digest: None,
            after_digest: Some(surface.descriptor_digest().unwrap()),
        }],
    }
}

fn helper_provider() -> PlannedProviderEvidence {
    PlannedProviderEvidence {
        surface: PlanQualifiedSurfaceRef {
            package_id: "acme/helper".to_string(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "run".to_string(),
            },
        },
        provider_id: "runtime:helper".to_string(),
        provider_build_id: "runtime:0.3.0:linux-x86_64".to_string(),
        capability_digest: DIGEST_D.to_string(),
        semantics_profile_digest: DIGEST_E.to_string(),
        enforcement: a3s_use_core::PlanEnforcementProfile::Sandbox,
    }
}

fn change_set_for_after(
    plan: &PluginOperationPlan,
    before_snapshot_digest: Option<String>,
) -> PluginWorkspaceGrantChangeSet {
    PluginWorkspaceGrantChangeSet {
        schema: PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA.to_string(),
        operation_id: plan.operation_id.clone(),
        scope_id: plan.scope.id.clone(),
        state_revision: plan.state.state_revision,
        before_snapshot_digest,
        changes: plan
            .packages
            .iter()
            .map(|package| PlannedWorkspaceGrantChange {
                package_id: package.package_id.clone(),
                before: None,
                after: Some(proposal_for(plan, package.after.as_ref().unwrap())),
            })
            .collect(),
    }
}

fn proposal_for(
    plan: &PluginOperationPlan,
    state: &PlannedPackageState,
) -> PluginWorkspaceGrantProposal {
    PluginWorkspaceGrantProposal {
        schema: PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA.to_string(),
        operation_id: plan.operation_id.clone(),
        scope_id: plan.scope.id.clone(),
        package_id: state.release.package_id.clone(),
        package_digest: state.release.package_sha256.clone(),
        permission_ceiling_digest: state.release.permission_ceiling_digest.clone(),
        permissions_digest: state.permissions.descriptor_digest().unwrap(),
        permissions: state.permissions.clone(),
        authority: WorkspaceGrantProposalAuthority {
            actor: plan.authority.actor,
            decision: plan.authority.decision,
            policy_digest: plan.authority.policy_digest.clone(),
        },
        created_at_ms: plan.created_at_ms,
        apply_expires_at_ms: plan.expires_at_ms,
        grant_expires_at_ms: None,
    }
}

fn package_digest<'a>(plan: &'a PluginOperationPlan, package_id: &str, before: bool) -> &'a str {
    let transition = plan
        .packages
        .iter()
        .find(|package| package.package_id == package_id)
        .unwrap();
    let state = if before {
        transition.before.as_ref()
    } else {
        transition.after.as_ref()
    }
    .unwrap();
    &state.release.package_sha256
}
