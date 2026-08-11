use std::path::{Path, PathBuf};

use super::provisioning_fault_support::{FaultFixture, SurfaceCase};
use crate::plugin_runtime::provisioning_fault_matrix::{
    BINDING_SYNCED, CHECKPOINT_CRASH_EXIT_CODE, FAULT_CHECKPOINT_ENV, GATEWAY_EFFECT,
    GATEWAY_READY_SYNCED, REQUESTED_SYNCED, RUNTIME_APPLIED_SYNCED, RUNTIME_EFFECT,
};

const CHILD_ROOT_ENV: &str = "A3S_USE_TEST_RUNTIME_PROVISIONING_ROOT";
const CHILD_SURFACE_ENV: &str = "A3S_USE_TEST_RUNTIME_PROVISIONING_SURFACE";

const CHECKPOINTS: [&str; 6] = [
    REQUESTED_SYNCED,
    RUNTIME_EFFECT,
    RUNTIME_APPLIED_SYNCED,
    GATEWAY_EFFECT,
    GATEWAY_READY_SYNCED,
    BINDING_SYNCED,
];

#[tokio::test]
async fn every_service_provisioning_window_recovers_after_process_exit() {
    for surface in [SurfaceCase::Tool, SurfaceCase::Mcp] {
        for checkpoint in CHECKPOINTS {
            exercise_process_exit(surface, checkpoint).await;
        }
    }
}

async fn exercise_process_exit(surface: SurfaceCase, checkpoint: &str) {
    let temporary = tempfile::tempdir().expect("create Runtime provisioning fault directory");
    let root = temporary.path().to_path_buf();
    let output = tokio::process::Command::new(
        std::env::current_exe().expect("resolve Runtime provisioning test executable"),
    )
    .arg("runtime_service_provisioning_crash_child")
    .arg("--ignored")
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(CHILD_ROOT_ENV, &root)
    .env(CHILD_SURFACE_ENV, surface.name())
    .env(FAULT_CHECKPOINT_ENV, checkpoint)
    .output()
    .await
    .expect("run Runtime provisioning fault child");
    assert_eq!(
        output.status.code(),
        Some(CHECKPOINT_CRASH_EXIT_CODE),
        "fault child did not exit for {} at {checkpoint}: status={:?}, stdout={}, stderr={}",
        surface.name(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let fixture = FaultFixture::new(&root, surface).await;
    let recovered = fixture.prepare().await.unwrap_or_else(|error| {
        panic!(
            "failed to recover {} at {checkpoint}: {} ({})",
            surface.name(),
            error.message,
            error.code
        )
    });
    assert!(matches!(
        fixture.binding().await,
        Some(crate::plugin_runtime::RuntimeBindingReceipt::Service(_))
    ));
    assert!(fixture.provisioning().await.is_none());
    assert!(fixture.runtime_effect_path().is_file());
    assert!(fixture.gateway_effect_path().is_file());

    let expected = expected_attempts(checkpoint);
    assert_eq!(
        line_count(&fixture.runtime_attempt_path()).await,
        expected.runtime,
        "unexpected Runtime apply attempts for {} at {checkpoint}",
        surface.name(),
    );
    assert_eq!(
        line_count(&fixture.gateway_attempt_path()).await,
        expected.gateway,
        "unexpected Gateway bind attempts for {} at {checkpoint}",
        surface.name(),
    );

    let replayed = fixture.prepare().await.unwrap();
    assert_eq!(replayed, recovered);
    assert_eq!(
        line_count(&fixture.runtime_attempt_path()).await,
        expected.runtime,
        "terminal prepare replay invoked Runtime for {} at {checkpoint}",
        surface.name(),
    );
    assert_eq!(
        line_count(&fixture.gateway_attempt_path()).await,
        expected.gateway,
        "terminal prepare replay invoked Gateway for {} at {checkpoint}",
        surface.name(),
    );

    fixture.remove().await.unwrap();
    assert!(fixture.binding().await.is_none());
    assert!(fixture.provisioning().await.is_none());
    assert!(!fixture.runtime_effect_path().exists());
    assert!(!fixture.gateway_effect_path().exists());
}

#[derive(Debug, Clone, Copy)]
struct ExpectedAttempts {
    runtime: usize,
    gateway: usize,
}

fn expected_attempts(checkpoint: &str) -> ExpectedAttempts {
    match checkpoint {
        REQUESTED_SYNCED => ExpectedAttempts {
            runtime: 1,
            gateway: 1,
        },
        RUNTIME_EFFECT | RUNTIME_APPLIED_SYNCED => ExpectedAttempts {
            runtime: 2,
            gateway: 1,
        },
        GATEWAY_EFFECT => ExpectedAttempts {
            runtime: 2,
            gateway: 2,
        },
        GATEWAY_READY_SYNCED | BINDING_SYNCED => ExpectedAttempts {
            runtime: 1,
            gateway: 1,
        },
        _ => panic!("unknown Runtime provisioning checkpoint {checkpoint}"),
    }
}

async fn line_count(path: &Path) -> usize {
    tokio::fs::read_to_string(path)
        .await
        .unwrap_or_else(|error| panic!("read attempt log '{}': {error}", path.display()))
        .lines()
        .count()
}

#[tokio::test]
#[ignore = "subprocess helper for Runtime Service provisioning crash injection"]
async fn runtime_service_provisioning_crash_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let surface = match std::env::var(CHILD_SURFACE_ENV).as_deref() {
        Ok("tool") => SurfaceCase::Tool,
        Ok("mcp") => SurfaceCase::Mcp,
        value => panic!("invalid Runtime provisioning child surface {value:?}"),
    };
    let checkpoint = std::env::var(FAULT_CHECKPOINT_ENV)
        .expect("Runtime provisioning child checkpoint is missing");
    assert!(CHECKPOINTS.contains(&checkpoint.as_str()));
    let fixture = FaultFixture::new(&root, surface).await;
    let outcome = fixture.prepare().await;
    panic!(
        "Runtime provisioning fault child completed without exiting for {} at {checkpoint}: {outcome:?}",
        surface.name(),
    );
}
