use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{PlanEnforcementProfile, PlanPolicyDecision};
use a3s_use_extension::PluginMcpLaunch;
use serde_json::Value;
use tokio::io;

use super::Fixture;
use crate::stdio_mcp::{
    NativeUnconfinedStdioMcpHost, StdioMcpHostFeature, StdioMcpHostProvider, StdioMcpProcessState,
    StdioMcpSessionPlan, StdioMcpSupervisor,
};

const ENV_SESSION_ID: &str = "native-session-env";
const TREE_SESSION_ID: &str = "native-session-tree";
const ENV_CHILD_TEST: &str = "stdio_mcp::tests::native_host::native_child_reports_environment";
const TREE_CHILD_TEST: &str = "stdio_mcp::tests::native_host::native_child_spawns_descendant";
const DESCENDANT_TEST: &str = "stdio_mcp::tests::native_host::native_descendant_heartbeats";

#[test]
fn native_host_capabilities_are_explicit_and_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<NativeUnconfinedStdioMcpHost>();
    let host = NativeUnconfinedStdioMcpHost::new("native-test-build").unwrap();
    let capabilities = host.capability_evidence();
    assert_eq!(
        capabilities.enforcement(),
        PlanEnforcementProfile::NativeUnconfined
    );
    assert_eq!(
        capabilities.features(),
        &[
            StdioMcpHostFeature::SanitizedEnvironment,
            StdioMcpHostFeature::OwnedFilesystemRoots,
            StdioMcpHostFeature::ProcessIdentity,
            StdioMcpHostFeature::StderrDrain,
            StdioMcpHostFeature::ProcessTreeCleanup,
        ]
    );
}

#[tokio::test]
async fn native_host_runs_with_sanitized_environment_and_exact_working_directory() {
    let (fixture, host, plan) =
        prepared_native_plan(ENV_SESSION_ID, ENV_CHILD_TEST, "native-test-build").await;
    let spawned = host.spawn(&plan).await.unwrap();
    let (mut reader, writer, control) = spawned.into_parts();
    drop(writer);
    let stdout = tokio::spawn(async move { io::copy(&mut reader, &mut io::sink()).await });

    let observation = tokio::time::timeout(Duration::from_secs(10), control.wait_for_exit())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        observation.state(),
        StdioMcpProcessState::Exited { exit_code: Some(0) }
    );
    stdout.await.unwrap().unwrap();

    let marker = plan.roots().temporary_root().join("environment.json");
    let value: Value = serde_json::from_slice(&tokio::fs::read(marker).await.unwrap()).unwrap();
    assert_eq!(
        value["currentDirectory"].as_str().map(PathBuf::from),
        Some(plan.package_root().to_path_buf())
    );
    assert_eq!(value["pathPresent"], false);
    assert_eq!(value["sessionId"], ENV_SESSION_ID);
    assert_eq!(
        value["packageRoot"].as_str().map(PathBuf::from),
        Some(plan.package_root().to_path_buf())
    );
    drop(fixture);
}

#[tokio::test]
async fn native_host_terminates_the_complete_descendant_group_before_terminal_evidence() {
    let (fixture, host, plan) =
        prepared_native_plan(TREE_SESSION_ID, TREE_CHILD_TEST, "native-test-build").await;
    let spawned = host.spawn(&plan).await.unwrap();
    let (mut reader, writer, control) = spawned.into_parts();
    drop(writer);
    let stdout = tokio::spawn(async move { io::copy(&mut reader, &mut io::sink()).await });
    let ready = plan.roots().temporary_root().join("tree-ready");
    let heartbeat = plan.roots().temporary_root().join("descendant-heartbeat");
    wait_for_file(&ready).await;
    wait_for_growth(&heartbeat, 3).await;

    control.terminate();
    let observation = tokio::time::timeout(Duration::from_secs(10), control.wait_for_exit())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        observation.state(),
        StdioMcpProcessState::Exited { .. }
    ));
    stdout.await.unwrap().unwrap();

    let settled_size = tokio::fs::metadata(&heartbeat).await.unwrap().len();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        tokio::fs::metadata(&heartbeat).await.unwrap().len(),
        settled_size,
        "terminal evidence was published while a descendant remained alive"
    );
    drop(fixture);
}

#[tokio::test]
async fn native_host_rejects_a_plan_bound_to_another_provider_build() {
    let (_fixture, _planned_host, plan) =
        prepared_native_plan(ENV_SESSION_ID, ENV_CHILD_TEST, "planned-build").await;
    let changed_host = NativeUnconfinedStdioMcpHost::new("changed-build").unwrap();
    assert_eq!(
        changed_host.spawn(&plan).await.unwrap_err().code,
        "use.plugin.stdio_mcp.native_provider_mismatch"
    );
}

#[tokio::test]
async fn native_host_rejects_a_missing_owned_root_before_spawn() {
    let (_fixture, host, plan) =
        prepared_native_plan(ENV_SESSION_ID, ENV_CHILD_TEST, "native-test-build").await;
    tokio::fs::remove_dir(plan.roots().temporary_root())
        .await
        .unwrap();
    assert_eq!(
        host.spawn(&plan).await.unwrap_err().code,
        "use.plugin.stdio_mcp.native_path_invalid"
    );
}

async fn prepared_native_plan(
    session_id: &str,
    child_test: &str,
    build_id: &str,
) -> (Fixture, NativeUnconfinedStdioMcpHost, StdioMcpSessionPlan) {
    let mut fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    for root in [
        fixture.roots.plugin_data_root(),
        fixture.roots.temporary_root(),
        fixture.roots.workspace_root(),
    ] {
        tokio::fs::create_dir_all(root).await.unwrap();
    }

    let relative_executable = PathBuf::from(format!(
        "bin/native-mcp-fixture{}",
        std::env::consts::EXE_SUFFIX
    ));
    let executable = fixture
        .extension
        .receipt
        .package_root
        .join(&relative_executable);
    tokio::fs::copy(std::env::current_exe().unwrap(), &executable)
        .await
        .unwrap();
    make_executable(&executable);
    fixture.extension.manifest.mcp_servers[0].launch = PluginMcpLaunch::Stdio {
        executable: relative_executable,
        args: vec![
            "--exact".to_string(),
            child_test.to_string(),
            "--nocapture".to_string(),
            "--test-threads=1".to_string(),
        ],
    };

    let host = NativeUnconfinedStdioMcpHost::new(build_id).unwrap();
    let provider: Arc<dyn StdioMcpHostProvider> = Arc::new(host.clone());
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, provider);
    let lease = fixture.lease();
    let plan = supervisor
        .prepare(&lease, fixture.request(session_id, 5_000, 5_000))
        .await
        .unwrap()
        .plan()
        .clone();
    drop(lease);
    (fixture, host, plan)
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(windows)]
fn make_executable(_path: &Path) {}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_growth(path: &Path, minimum_bytes: u64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if tokio::fs::metadata(path)
                .await
                .is_ok_and(|metadata| metadata.len() >= minimum_bytes)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn native_child_reports_environment() {
    if std::env::var("A3S_USE_SESSION_ID").as_deref() != Ok(ENV_SESSION_ID) {
        return;
    }
    let stderr_chunk = [b'x'; 16 * 1024];
    let mut stderr = std::io::stderr().lock();
    for _ in 0..16 {
        stderr.write_all(&stderr_chunk).unwrap();
    }
    stderr.flush().unwrap();
    drop(stderr);
    let temporary = PathBuf::from(std::env::var("A3S_USE_TEMP_ROOT").unwrap());
    let value = serde_json::json!({
        "currentDirectory": std::env::current_dir().unwrap(),
        "packageRoot": std::env::var("A3S_USE_PACKAGE_ROOT").unwrap(),
        "pathPresent": std::env::var_os("PATH").is_some(),
        "sessionId": std::env::var("A3S_USE_SESSION_ID").unwrap(),
    });
    std::fs::write(
        temporary.join("environment.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

#[test]
fn native_child_spawns_descendant() {
    if std::env::var("A3S_USE_SESSION_ID").as_deref() != Ok(TREE_SESSION_ID) {
        return;
    }
    let temporary = PathBuf::from(std::env::var("A3S_USE_TEMP_ROOT").unwrap());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            DESCENDANT_TEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let child_id = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    std::fs::write(temporary.join("tree-ready"), child_id.to_string()).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn native_descendant_heartbeats() {
    if std::env::var("A3S_USE_SESSION_ID").as_deref() != Ok(TREE_SESSION_ID) {
        return;
    }
    let heartbeat =
        PathBuf::from(std::env::var("A3S_USE_TEMP_ROOT").unwrap()).join("descendant-heartbeat");
    loop {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&heartbeat)
            .unwrap();
        file.write_all(b"x").unwrap();
        file.flush().unwrap();
        drop(file);
        std::thread::sleep(Duration::from_millis(20));
    }
}
