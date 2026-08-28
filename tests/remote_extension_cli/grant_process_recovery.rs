use super::graph_grants::cognitive_tool_targets_version_with_dependencies_and_payload;
use super::*;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use a3s_use::cognitive_package::{
    CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider,
    StandaloneCognitivePackageLifecycleFactory,
};
use a3s_use_core::{
    PlanActor, PlanAuthority, PlanPolicyDecision, PlanScope, PlanScopeKind, PluginOperationPlan,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginWorkspaceGrantChangeSet, UseError,
    UseResult,
};
use a3s_use_extension::{StoredWorkspaceGrant, WorkspaceGrantStore};
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

const CHILD_HOME_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_HOME";
const CHILD_REGISTRY_URL_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_REGISTRY_URL";
const CHILD_ROOT_SHA256_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_ROOT_SHA256";
const CHILD_AUTHORIZATION_MARKER_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_AUTH_MARKER";
const CHILD_ALLOW_AUTHORIZATION_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_ALLOW_AUTH";
const CHILD_OFFLINE_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_OFFLINE";
const CHILD_ACTION_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_ACTION";
const CHILD_VERSION_ENV: &str = "A3S_USE_TEST_GRANT_PROCESS_VERSION";
const MANAGED_SCOPE_ID: &str = "workspace:grant-process-recovery";
const PACKAGE_ID: &str = "acme/worker";
const DEPENDENCY_COUNT: usize = 4;
const PAYLOAD_FILES: usize = 8;
const POLICY_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[path = "grant_process_recovery/host_disable.rs"]
mod host_disable;
#[path = "grant_process_recovery/host_enable.rs"]
mod host_enable;
#[path = "grant_process_recovery/host_install.rs"]
mod host_install;
#[path = "grant_process_recovery/host_support.rs"]
mod host_support;
#[path = "grant_process_recovery/host_uninstall.rs"]
mod host_uninstall;
#[path = "grant_process_recovery/host_upgrade.rs"]
mod host_upgrade;
#[path = "grant_process_recovery/install.rs"]
mod install;
#[path = "grant_process_recovery/uninstall.rs"]
mod uninstall;
#[path = "grant_process_recovery/upgrade.rs"]
mod upgrade;

#[derive(Debug)]
struct ProcessAuthorization {
    marker: PathBuf,
    allow_authorization: bool,
}

#[async_trait]
impl CognitivePackageAuthorizationProvider for ProcessAuthorization {
    fn name(&self) -> &'static str {
        "integration-process-authorization"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        draft.validate()?;
        Ok(test_authority())
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        plan.validate()?;
        if plan.authority != test_authority() {
            return Err(UseError::new(
                "test.plugin.authority_changed",
                "The process-test authorization authority changed after planning.",
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
        if !self.allow_authorization {
            return Err(UseError::new(
                "test.plugin.unexpected_reauthorization",
                "Crash recovery attempted to request authorization again.",
            ));
        }
        let mut marker = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&self.marker)
            .await
            .map_err(|error| {
                UseError::new(
                    "test.plugin.authorization_marker",
                    format!("Failed to create the authorization marker: {error}"),
                )
            })?;
        marker
            .write_all(envelope.plan.operation_id.as_bytes())
            .await
            .map_err(|error| {
                UseError::new(
                    "test.plugin.authorization_marker",
                    format!("Failed to write the authorization marker: {error}"),
                )
            })?;
        marker.sync_all().await.map_err(|error| {
            UseError::new(
                "test.plugin.authorization_marker",
                format!("Failed to sync the authorization marker: {error}"),
            )
        })?;
        CognitivePackageAuthorizationEvidence::confirmed(envelope, changes, now_ms)
    }
}

fn expected_package_ids() -> std::collections::BTreeSet<String> {
    (0..DEPENDENCY_COUNT)
        .map(|index| format!("acme/leaf-{index:02}"))
        .chain(std::iter::once(PACKAGE_ID.to_owned()))
        .collect()
}

fn managed_installation() -> PlanScope {
    PlanScope {
        kind: PlanScopeKind::Workspace,
        id: MANAGED_SCOPE_ID.to_owned(),
    }
}

fn managed_state_root(home: &Path) -> PathBuf {
    extension_paths_for(home, managed_installation())
        .state_root()
        .to_path_buf()
}

fn managed_graph_targets(
    fixture_root: &Path,
    version: &str,
    dependency_requirement: &str,
    target: &str,
) -> Vec<TestTarget> {
    let dependencies = (0..DEPENDENCY_COUNT)
        .map(|index| {
            PluginPackageDependency::new(format!("acme/leaf-{index:02}"), dependency_requirement)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let mut targets = cognitive_tool_targets_version_with_dependencies_and_payload(
        &fixture_root.join("worker"),
        PACKAGE_ID,
        "worker",
        version,
        target,
        dependencies.clone(),
        PAYLOAD_FILES,
    );
    targets.extend(dependencies.iter().enumerate().map(|(index, dependency)| {
        cognitive_skill_target_version(
            &fixture_root.join(format!("leaf-{index:02}")),
            &dependency.package_id,
            &format!("leaf-{index:02}"),
            version,
            Vec::new(),
            target,
        )
    }));
    targets
}

fn graph_package_versions(graph: &serde_json::Value) -> std::collections::BTreeMap<String, String> {
    graph["packageLock"]["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            let record = &package["catalog"]["record"];
            (
                record["packageId"].as_str().unwrap().to_owned(),
                record["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn assert_completed_lifecycles(home: &Path) {
    for package_id in expected_package_ids() {
        assert_eq!(
            lifecycle_status(&managed_lifecycle_journal_path(home, &package_id)).as_deref(),
            Some("completed")
        );
    }
}

#[tokio::test]
#[ignore = "subprocess helper for managed graph/Grant process interruption"]
async fn managed_grant_operation_child() {
    let Some(home) = std::env::var_os(CHILD_HOME_ENV).map(PathBuf::from) else {
        return;
    };
    let registry_url = std::env::var(CHILD_REGISTRY_URL_ENV).unwrap();
    let root_sha256 = std::env::var(CHILD_ROOT_SHA256_ENV).unwrap();
    let marker = PathBuf::from(std::env::var_os(CHILD_AUTHORIZATION_MARKER_ENV).unwrap());
    let allow_authorization = std::env::var(CHILD_ALLOW_AUTHORIZATION_ENV).as_deref() == Ok("1");
    let offline = std::env::var(CHILD_OFFLINE_ENV).as_deref() == Ok("1");
    let action = std::env::var(CHILD_ACTION_ENV).unwrap();
    let version = std::env::var(CHILD_VERSION_ENV).unwrap_or_default();
    let installation = managed_installation();
    let paths = extension_paths_for(&home, installation.clone());
    let registry = TrustedRegistry::new(
        "fixture",
        registry_url,
        root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        ExtensionRegistry::new(paths),
        installation,
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ProcessAuthorization {
            marker,
            allow_authorization,
        }),
    )
    .unwrap();
    match action.as_str() {
        "install" => {
            if offline {
                manager
                    .install_cached(
                        &registry,
                        &[],
                        PACKAGE_ID,
                        Some(&version),
                        PluginReleaseChannel::Stable,
                        None,
                    )
                    .await
                    .unwrap();
            } else {
                manager
                    .install_remote(
                        &registry,
                        &[],
                        PACKAGE_ID,
                        Some(&version),
                        PluginReleaseChannel::Stable,
                        None,
                    )
                    .await
                    .unwrap();
            }
        }
        "upgrade" => {
            if offline {
                manager
                    .upgrade_cached(
                        &registry,
                        &[],
                        PACKAGE_ID,
                        Some(&version),
                        PluginReleaseChannel::Stable,
                        None,
                    )
                    .await
                    .unwrap();
            } else {
                manager
                    .upgrade_remote(
                        &registry,
                        &[],
                        PACKAGE_ID,
                        Some(&version),
                        PluginReleaseChannel::Stable,
                        None,
                    )
                    .await
                    .unwrap();
            }
        }
        "uninstall" => {
            manager.uninstall(PACKAGE_ID).await.unwrap();
        }
        _ => panic!("unsupported managed process action: {action}"),
    }
}

struct ManagedChildRequest<'a> {
    home: &'a Path,
    server: &'a TestServer,
    repository: &'a TestRepository,
    authorization_marker: &'a Path,
    action: &'a str,
    version: Option<&'a str>,
    allow_authorization: bool,
    offline: bool,
}

fn spawn_managed_child(request: ManagedChildRequest<'_>) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("managed_grant_operation_child")
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_HOME_ENV, request.home)
        .env(CHILD_REGISTRY_URL_ENV, request.server.base_url())
        .env(CHILD_ROOT_SHA256_ENV, &request.repository.root_sha256)
        .env(CHILD_AUTHORIZATION_MARKER_ENV, request.authorization_marker)
        .env(CHILD_ACTION_ENV, request.action)
        .env(CHILD_VERSION_ENV, request.version.unwrap_or_default())
        .env(
            CHILD_ALLOW_AUTHORIZATION_ENV,
            if request.allow_authorization {
                "1"
            } else {
                "0"
            },
        )
        .env(CHILD_OFFLINE_ENV, if request.offline { "1" } else { "0" })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn terminate_child(mut child: std::process::Child) -> std::process::Output {
    let _ = child.kill();
    child.wait_with_output().unwrap()
}

fn wait_for_grant_phase(home: &Path, action: &str, expected: &str) -> Option<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some((path, journal)) = find_grant_operation(home, action) {
            if journal["phase"] == expected {
                return Some(path);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    find_grant_operation(home, action)
        .and_then(|(path, journal)| (journal["phase"] == expected).then_some(path))
}

fn find_grant_operation(home: &Path, action: &str) -> Option<(PathBuf, serde_json::Value)> {
    let operations = std::fs::read_dir(managed_state_root(home).join("grants/.operations")).ok()?;
    let mut found = operations
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|entry| read_json(&entry.path()).map(|journal| (entry.path(), journal)))
        .filter(|(_, journal)| {
            journal["intent"]["operationId"]
                .as_str()
                .is_some_and(|operation_id| operation_id.starts_with(&format!("{action}:")))
        })
        .collect::<Vec<_>>();
    found.sort_by(|left, right| left.0.cmp(&right.0));
    (found.len() == 1).then(|| found.remove(0))
}

fn grant_phase(path: &Path) -> Option<String> {
    read_json(path)?["phase"].as_str().map(str::to_owned)
}

fn managed_lifecycle_journal_path(home: &Path, package_id: &str) -> PathBuf {
    let scope = managed_installation().storage_key().unwrap();
    managed_state_root(home)
        .join("operations/plugins/workspace")
        .join(scope)
        .join(package_id)
        .join("active.json")
}

fn wait_for_lifecycle_prepare(path: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if lifecycle_is_prepared(path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    lifecycle_is_prepared(path)
}

fn lifecycle_is_prepared(path: &Path) -> bool {
    read_json(path).is_some_and(|journal| {
        let receipt_count = journal["receipts"].as_array().map_or(0, Vec::len);
        let checkpoint_count = journal["intent"]["checkpoints"]
            .as_array()
            .map_or(0, Vec::len);
        journal["status"] == "applying"
            && checkpoint_count > 0
            && receipt_count + 1 == checkpoint_count
    })
}

fn lifecycle_status(path: &Path) -> Option<String> {
    read_json(path)?["status"].as_str().map(str::to_owned)
}

fn lifecycle_summary(path: &Path) -> Option<(String, usize, usize)> {
    let journal = read_json(path)?;
    Some((
        journal["status"].as_str()?.to_owned(),
        journal["receipts"].as_array().map_or(0, Vec::len),
        journal["intent"]["checkpoints"]
            .as_array()
            .map_or(0, Vec::len),
    ))
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn file_length(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn route_package_ids(snapshot: &serde_json::Value) -> std::collections::BTreeSet<String> {
    snapshot["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| route["packageId"].as_str().unwrap().to_owned())
        .collect()
}

fn enabled_route_package_ids(snapshot: &serde_json::Value) -> std::collections::BTreeSet<String> {
    snapshot["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|route| route["enabled"] == true)
        .map(|route| route["packageId"].as_str().unwrap().to_owned())
        .collect()
}

fn observe_grant(home: &Path, package_digest: &str) -> StoredWorkspaceGrant {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(WorkspaceGrantStore::new(managed_state_root(home)).observe(
            MANAGED_SCOPE_ID,
            PACKAGE_ID,
            package_digest,
        ))
        .unwrap()
        .unwrap()
}

fn child_output(output: &std::process::Output) -> String {
    format!(
        "status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn test_authority() -> PlanAuthority {
    PlanAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: POLICY_DIGEST.to_owned(),
        confirmation_required: true,
    }
}
