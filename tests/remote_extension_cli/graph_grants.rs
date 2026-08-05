use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_use::cognitive_package::{
    CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider,
};
use a3s_use_core::{
    CatalogPlanningTarget, ExecutablePlanningSurface, PlanActor, PlanAuthority, PlanPolicyDecision,
    PlanningArtifactRef, PlanningSurfaceActivation, PluginOperationPlan, PluginOperationPlanDraft,
    PluginOperationPlanEnvelope, PluginPermissionCeiling, PluginPlanningBundle,
    PluginWorkspaceGrantChangeSet, ToolReleaseDescriptor, ToolWorkloadClass, UseResult,
    PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use a3s_use_extension::{StoredWorkspaceGrant, WorkspaceGrantStore};
use async_trait::async_trait;

const POLICY_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PERMISSIONS: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/permission-ceiling-v1.json");

#[derive(Debug)]
struct ConfirmAllPlans {
    authorization_count: Arc<AtomicUsize>,
}

#[async_trait]
impl CognitivePackageAuthorizationProvider for ConfirmAllPlans {
    fn name(&self) -> &'static str {
        "integration-confirm-all"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        draft.validate()?;
        Ok(test_authority())
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        plan.validate()?;
        if plan.authority != test_authority() {
            return Err(a3s_use_core::UseError::new(
                "test.plugin.authority_changed",
                "The test authorization authority changed after planning.",
            ));
        }
        Ok(())
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        self.authorization_count.fetch_add(1, Ordering::SeqCst);
        CognitivePackageAuthorizationEvidence::confirmed(envelope, changes, now_ms)
    }
}

#[tokio::test]
async fn permission_grants_follow_install_upgrade_uninstall_and_survive_replay() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v1",
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v2",
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 53, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("home");
    let registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let extension_registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let manager = CognitivePackageManager::with_authorization(
        extension_registry.clone(),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();

    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    let interrupted = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let pending_path = home.join("state/operations/package-graphs/install/acme/worker.json");
    let pending_bytes = std::fs::read(&pending_path).unwrap();
    let pending: serde_json::Value = serde_json::from_slice(&pending_bytes).unwrap();
    let mut tampered = Vec::new();

    let mut missing_resolved = pending.clone();
    missing_resolved["authorization"]
        .as_object_mut()
        .unwrap()
        .remove("resolvedGrants");
    tampered.push((
        "missing resolved Grant",
        "use.plugin.package_authorization_invalid",
        missing_resolved,
    ));

    let mut changed_confirmation = pending.clone();
    let confirmed_at = changed_confirmation["authorization"]["operationConfirmation"]
        ["confirmedAtMs"]
        .as_u64()
        .unwrap();
    changed_confirmation["authorization"]["operationConfirmation"]["confirmedAtMs"] =
        serde_json::json!(confirmed_at + 1);
    tampered.push((
        "changed operation confirmation",
        "use.plugin.plan_confirmation_mismatch",
        changed_confirmation,
    ));

    let mut changed_snapshot = pending.clone();
    changed_snapshot["authorization"]["grantSnapshot"]["stateRevision"] = serde_json::json!(999);
    tampered.push((
        "changed Grant snapshot",
        "use.plugin.package_authorization_invalid",
        changed_snapshot,
    ));

    let mut changed_change_set = pending.clone();
    changed_change_set["authorization"]["grantChangeSet"]["stateRevision"] = serde_json::json!(999);
    tampered.push((
        "changed Grant change set",
        "use.plugin.grant_changes_plan_mismatch",
        changed_change_set,
    ));

    let mut changed_ceiling = pending.clone();
    changed_ceiling["authorization"]["grantCeilings"][0]["packageDigest"] =
        serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    tampered.push((
        "changed signed ceiling",
        "use.plugin.package_authorization_invalid",
        changed_ceiling,
    ));

    let mut legacy_permission_operation = pending;
    legacy_permission_operation["schema"] =
        serde_json::json!("a3s.use.pending-package-graph-operation.v1");
    tampered.push((
        "permission-bearing legacy pending schema",
        "use.plugin.package_graph_store_invalid",
        legacy_permission_operation,
    ));

    for (case, expected_code, value) in tampered {
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let error = manager
            .install_remote(
                &registry,
                &[],
                "acme/worker",
                Some("1.0.0"),
                PluginReleaseChannel::Stable,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected_code, "unexpected error for {case}");
        assert_eq!(
            authorization_count.load(Ordering::SeqCst),
            1,
            "tampered pending evidence must not trigger reauthorization: {case}"
        );
    }
    std::fs::write(&pending_path, &pending_bytes).unwrap();

    let installed = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(installed.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    let install_plan = installed.plan.as_ref().unwrap();
    assert_eq!(install_plan.plan.authority, test_authority());
    assert_eq!(install_plan.plan.workspace_impacts.len(), 1);
    let first_state = install_plan.plan.packages[0].after.as_ref().unwrap();
    assert_granted(
        &home,
        &first_state.release.package_sha256,
        &first_state.permissions,
    )
    .await;

    let upgraded = manager
        .upgrade_remote(
            &registry,
            &[],
            "acme/worker",
            Some("2.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(upgraded.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 2);
    let upgrade_plan = upgraded.plan.as_ref().unwrap();
    let transition = &upgrade_plan.plan.packages[0];
    let prior = transition.before.as_ref().unwrap();
    let candidate = transition.after.as_ref().unwrap();
    assert_revoked(&home, &prior.release.package_sha256).await;
    assert_granted(
        &home,
        &candidate.release.package_sha256,
        &candidate.permissions,
    )
    .await;

    let uninstalled = manager.uninstall("acme/worker").await.unwrap();
    assert!(uninstalled.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 3);
    assert_revoked(&home, &candidate.release.package_sha256).await;
    assert!(!home
        .join("state/operations/package-graphs/install/acme/worker.json")
        .exists());
    assert!(!home
        .join("state/operations/package-graphs/upgrade/acme/worker.json")
        .exists());
    assert!(!home
        .join("state/operations/package-graphs/uninstall/acme/worker.json")
        .exists());
}

fn cognitive_tool_targets_version(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    target: &str,
) -> Vec<TestTarget> {
    let package_root = fixture_root.join("packages").join(route);
    std::fs::create_dir_all(package_root.join("tools/convert/bin")).unwrap();
    let manifest = format!(
        "extension \"{package_id}\" {{\n  schema_version = 3\n  version = \"{version}\"\n  route = \"{route}\"\n  requires_use = \">=0.3.0, <0.4.0\"\n  actions = [\"read\", \"execute\"]\n\n  repository {{\n    url = \"https://github.com/acme/worker\"\n    revision = \"0123456789abcdef0123456789abcdef01234567\"\n  }}\n\n  tool \"convert\" {{\n    workload = \"task\"\n    interface = \"cli\"\n    executable = \"tools/convert/bin/convert\"\n    command = \"acme-worker-convert\"\n    json_output = true\n    interactive = false\n    timeout_ms = 120000\n    activation = \"lazy\"\n    optional = false\n  }}\n}}\n"
    );
    std::fs::write(package_root.join("a3s-use-extension.acl"), &manifest).unwrap();
    std::fs::write(
        package_root.join("README.md"),
        "# Worker\n\nPermission-bearing cognitive package fixture.\n",
    )
    .unwrap();
    std::fs::write(
        package_root.join("tools/convert/bin/convert"),
        "#!/bin/sh\nset -eu\nprintf '{\"status\":\"ok\"}\\n'\n",
    )
    .unwrap();

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let manifest_sha256 = format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()));
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    catalog.package_id = package_id.to_string();
    catalog.display_name = format!("Worker {version}");
    catalog.description = "Permission-bearing cognitive package fixture.".to_string();
    catalog.publisher = "acme".to_string();
    catalog.keywords = vec!["fixture".to_string()];
    catalog.categories = vec!["test".to_string()];
    catalog.version = version.to_string();
    catalog.channel = PluginReleaseChannel::Stable;
    catalog.requires_use = ">=0.3.0, <0.4.0".to_string();
    catalog.dependencies.clear();
    catalog.target = target.to_string();
    catalog.surfaces = vec![CatalogSurface {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_string(),
        optional: false,
        workload: Some(ToolWorkloadClass::Task),
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    }];
    let mut permissions = PluginPermissionCeiling::from_json(PERMISSIONS).unwrap();
    permissions
        .surfaces
        .retain(|permission| permission.surface.id == "convert");
    permissions.validate().unwrap();
    catalog.permission_ceiling = permissions;
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    catalog.archive.target_name = format!(
        "extensions/{package_id}/{version}/stable/{target}/{route}-{version}-{target}.tar.gz"
    );
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.package.manifest_sha256 = Some(manifest_sha256);
    let descriptor = ToolReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/tool-task-release-v1.json"
    ))
    .unwrap();
    let planning_target =
        format!("extensions/{package_id}/{version}/stable/{target}/planning-v1.json");
    let planning = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_string(),
        package_id: package_id.to_string(),
        version: version.to_string(),
        channel: PluginReleaseChannel::Stable,
        target: target.to_string(),
        archive_sha256: catalog.archive.sha256.clone(),
        package_sha256: catalog.package.sha256.clone().unwrap(),
        manifest_sha256: catalog.package.manifest_sha256.clone().unwrap(),
        permission_ceiling_digest: catalog.permission_ceiling_digest.clone(),
        surfaces: vec![ExecutablePlanningSurface::ToolTask {
            id: "convert".to_string(),
            activation: PlanningSurfaceActivation::Lazy,
            command: "acme-worker-convert".to_string(),
            json_output: true,
            timeout_ms: 120_000,
            artifact: PlanningArtifactRef {
                uri: format!(
                    "oci://registry.example/acme/worker@{}",
                    descriptor.artifact.digest
                ),
                digest: descriptor.artifact.digest.clone(),
                media_type: descriptor.artifact.media_type.clone(),
            },
            descriptor,
        }],
    };
    let planning_bytes = planning.canonical_bytes().unwrap();
    catalog.planning = Some(CatalogPlanningTarget {
        target_name: planning_target.clone(),
        length: planning_bytes.len() as u64,
        sha256: format!("sha256:{:x}", Sha256::digest(&planning_bytes)),
    });
    catalog.license = "MIT".to_string();
    catalog.repository = "https://github.com/acme/worker".to_string();
    catalog.availability = CatalogAvailability::Available;
    catalog.validate().unwrap();

    vec![
        TestTarget {
            target_name: catalog.archive.target_name.clone(),
            custom: Some(serde_json::to_value(catalog).unwrap()),
            archive,
        },
        TestTarget {
            target_name: planning_target,
            custom: None,
            archive: planning_bytes,
        },
    ]
}

async fn assert_granted(
    home: &std::path::Path,
    package_digest: &str,
    ceiling: &PluginPermissionCeiling,
) {
    let record = WorkspaceGrantStore::new(home.join("state"))
        .observe("user/current", "acme/worker", package_digest)
        .await
        .unwrap()
        .unwrap();
    let StoredWorkspaceGrant::Granted(receipt) = record else {
        panic!("expected an active Grant receipt");
    };
    receipt.grant.validate_against(ceiling).unwrap();
    assert_eq!(receipt.grant.package_digest, package_digest);
    assert!(receipt.grant.authority.confirmation_digest.is_some());
}

async fn assert_revoked(home: &std::path::Path, package_digest: &str) {
    let record = WorkspaceGrantStore::new(home.join("state"))
        .observe("user/current", "acme/worker", package_digest)
        .await
        .unwrap()
        .unwrap();
    let StoredWorkspaceGrant::Revoked(revocation) = record else {
        panic!("expected an exact Grant revocation");
    };
    assert_eq!(revocation.package_digest, package_digest);
    assert!(revocation.authority.confirmation_digest.is_some());
}

fn test_authority() -> PlanAuthority {
    PlanAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: POLICY_DIGEST.to_string(),
        confirmation_required: true,
    }
}
