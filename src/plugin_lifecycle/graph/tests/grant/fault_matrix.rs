use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use a3s_use_extension::{
    StoredWorkspaceGrant, WorkspaceGrantLifecyclePhase, WorkspaceGrantReceipt, WorkspaceGrantStore,
};

use super::{
    grant_install_fixture_at, grant_uninstall_fixture_at, grant_upgrade_fixture_at, SCOPE_ID,
    TRANSITIONED_AT_MS,
};
use crate::plugin_lifecycle::graph::tests::durable_graph_host::GRAPH_CUTOVER_CRASH_EXIT_CODE;
use crate::plugin_lifecycle::graph::tests::{hide_key, publication_key, RecordingHost};
use crate::plugin_lifecycle::{
    PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
    PluginPackageGraphLifecycleCoordinator,
};

const CHILD_ROOT_ENV: &str = "A3S_USE_TEST_GRAPH_CUTOVER_FAULT_ROOT";
const CHILD_ACTION_ENV: &str = "A3S_USE_TEST_GRAPH_CUTOVER_FAULT_ACTION";

#[derive(Debug, Clone, Copy)]
enum GraphAction {
    Install,
    Upgrade,
    Uninstall,
}

#[tokio::test]
async fn every_grant_graph_cutover_recovers_after_an_ambiguous_host_effect() {
    for action in [
        GraphAction::Install,
        GraphAction::Upgrade,
        GraphAction::Uninstall,
    ] {
        exercise_graph_cutover_crash(action).await;
    }
}

async fn exercise_graph_cutover_crash(action: GraphAction) {
    let temporary = tempfile::tempdir().expect("create graph cutover fault directory");
    let root = temporary.path().to_path_buf();
    let output = tokio::process::Command::new(
        std::env::current_exe().expect("resolve graph lifecycle test executable"),
    )
    .arg("grant_graph_cutover_crash_child")
    .arg("--ignored")
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(CHILD_ROOT_ENV, &root)
    .env(CHILD_ACTION_ENV, action_name(action))
    .output()
    .await
    .expect("run graph cutover fault child");
    assert_eq!(
        output.status.code(),
        Some(GRAPH_CUTOVER_CRASH_EXIT_CODE),
        "graph cutover fault child did not exit for {action:?}: status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let host_root = root.join("graph-host");
    assert_eq!(effect_count(&host_root.join("effects")).await, 1);
    assert_eq!(attempt_lines(&host_root).await.len(), 1);

    let host = Arc::new(RecordingHost::with_durable_graph_root(host_root.clone()));
    let key = match action {
        GraphAction::Install => recover_install(&root, host).await,
        GraphAction::Upgrade => recover_upgrade(&root, host).await,
        GraphAction::Uninstall => recover_uninstall(&root, host).await,
    };
    let expected_kind = match action {
        GraphAction::Install | GraphAction::Upgrade => "publish",
        GraphAction::Uninstall => "hide",
    };
    assert_eq!(
        attempt_lines(&host_root).await,
        vec![
            format!("{expected_kind}\t{key}"),
            format!("{expected_kind}\t{key}"),
        ],
        "graph cutover recovery changed the exact {action:?} idempotency key",
    );
    assert_eq!(
        effect_count(&host_root.join("effects")).await,
        1,
        "graph cutover recovery duplicated the durable {action:?} effect",
    );
}

async fn recover_install(root: &Path, host: Arc<RecordingHost>) -> String {
    let fixture = grant_install_fixture_at(root.to_path_buf(), None, host.clone());
    let grants = fixture.grants();
    assert_prepared(&grants.observe().await.unwrap());
    let graph = PluginPackageGraphLifecycleCoordinator::new(host);
    let clock = AtomicU64::new(10_000);
    let completed = graph
        .apply_install_with_grants(&fixture.envelope, &fixture.units, &grants, || {
            clock.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .expect("recover install graph cutover");
    assert_completed(&completed);
    assert_grants_completed(&grants).await;

    let candidate = WorkspaceGrantReceipt::new(
        fixture.resolved.revision,
        fixture.resolved.grants[0].grant.clone(),
    )
    .unwrap();
    assert_eq!(
        WorkspaceGrantStore::new(&fixture.grant_root)
            .observe(
                SCOPE_ID,
                &candidate.grant.package_id,
                &candidate.grant.package_digest,
            )
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(candidate)),
    );

    let attempts = read_attempt_bytes(root).await;
    let replayed = graph
        .apply_install_with_grants(&fixture.envelope, &fixture.units, &fixture.grants(), || {
            20_000
        })
        .await
        .expect("replay completed install graph");
    assert_eq!(replayed, completed);
    assert_eq!(read_attempt_bytes(root).await, attempts);
    publication_key(&fixture.envelope).unwrap()
}

async fn recover_upgrade(root: &Path, host: Arc<RecordingHost>) -> String {
    let fixture = grant_upgrade_fixture_at(root.to_path_buf(), None, host.clone(), false).await;
    let grants = fixture.grants();
    assert_prepared(&grants.observe().await.unwrap());
    let graph = PluginPackageGraphLifecycleCoordinator::new(host);
    let clock = AtomicU64::new(10_000);
    let completed = graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &grants,
            || clock.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .expect("recover upgrade graph cutover");
    assert_completed(&completed);
    assert_grants_completed(&grants).await;

    let candidate = WorkspaceGrantReceipt::new(
        fixture.resolved.revision,
        fixture.resolved.grants[0].grant.clone(),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store()
            .observe(
                SCOPE_ID,
                &candidate.grant.package_id,
                &candidate.grant.package_digest,
            )
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Granted(candidate)),
    );
    assert!(matches!(
        fixture
            .store()
            .observe(
                SCOPE_ID,
                &fixture.prior.grant.package_id,
                &fixture.prior.grant.package_digest,
            )
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Revoked(_))
    ));

    let attempts = read_attempt_bytes(root).await;
    let replayed = graph
        .apply_upgrade_with_grants(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            &fixture.grants(),
            || 20_000,
        )
        .await
        .expect("replay completed upgrade graph");
    assert_eq!(replayed, completed);
    assert_eq!(read_attempt_bytes(root).await, attempts);
    publication_key(&fixture.envelope).unwrap()
}

async fn recover_uninstall(root: &Path, host: Arc<RecordingHost>) -> String {
    let fixture = grant_uninstall_fixture_at(root.to_path_buf(), None, host.clone(), false).await;
    let grants = fixture.grants();
    assert_prepared(&grants.observe().await.unwrap());
    let graph = PluginPackageGraphLifecycleCoordinator::new(host);
    let clock = AtomicU64::new(10_000);
    let completed = graph
        .apply_uninstall_with_grants(&fixture.envelope, &fixture.units, &grants, || {
            clock.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .expect("recover uninstall graph cutover");
    assert_completed(&completed);
    assert_grants_completed(&grants).await;
    assert!(matches!(
        fixture
            .store()
            .observe(
                SCOPE_ID,
                &fixture.prior.grant.package_id,
                &fixture.prior.grant.package_digest,
            )
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Revoked(_))
    ));

    let attempts = read_attempt_bytes(root).await;
    let replayed = graph
        .apply_uninstall_with_grants(&fixture.envelope, &fixture.units, &fixture.grants(), || {
            20_000
        })
        .await
        .expect("replay completed uninstall graph");
    assert_eq!(replayed, completed);
    assert_eq!(read_attempt_bytes(root).await, attempts);
    hide_key(&fixture.envelope).unwrap()
}

fn assert_prepared(journal: &Option<a3s_use_extension::WorkspaceGrantOperationJournal>) {
    assert_eq!(
        journal.as_ref().map(|journal| journal.phase),
        Some(WorkspaceGrantLifecyclePhase::Prepared),
        "the fault must happen after Grant preparation and before Grant cutover",
    );
}

async fn assert_grants_completed(grants: &crate::plugin_lifecycle::PluginGrantLifecycleUnit) {
    assert_eq!(
        grants.observe().await.unwrap().unwrap().phase,
        WorkspaceGrantLifecyclePhase::Completed,
    );
}

fn assert_completed(records: &[PluginLifecycleOperationRecord]) {
    assert!(!records.is_empty());
    assert!(records
        .iter()
        .all(|record| record.status == PluginLifecycleOperationStatus::Completed));
}

async fn read_attempt_bytes(root: &Path) -> Vec<u8> {
    tokio::fs::read(root.join("graph-host").join("attempts.log"))
        .await
        .expect("read graph cutover attempt bytes")
}

async fn attempt_lines(root: &Path) -> Vec<String> {
    tokio::fs::read_to_string(root.join("attempts.log"))
        .await
        .expect("read graph cutover attempts")
        .lines()
        .map(str::to_string)
        .collect()
}

async fn effect_count(root: &Path) -> usize {
    let mut entries = tokio::fs::read_dir(root)
        .await
        .expect("open durable graph effect directory");
    let mut count = 0;
    while let Some(entry) = entries
        .next_entry()
        .await
        .expect("read durable graph effect entry")
    {
        assert!(
            entry
                .file_type()
                .await
                .expect("inspect durable graph effect")
                .is_file(),
            "durable graph effect must be a regular file",
        );
        count += 1;
    }
    count
}

#[tokio::test]
#[ignore = "subprocess helper for graph cutover crash injection"]
async fn grant_graph_cutover_crash_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let action = parse_action(
        &std::env::var(CHILD_ACTION_ENV).expect("graph cutover fault child action is missing"),
    );
    let host = Arc::new(RecordingHost::with_durable_graph_root(
        root.join("graph-host"),
    ));
    let graph = PluginPackageGraphLifecycleCoordinator::new(host.clone());
    let clock = AtomicU64::new(TRANSITIONED_AT_MS);
    let outcome = match action {
        GraphAction::Install => {
            let fixture = grant_install_fixture_at(root, None, host.clone());
            host.crash_after_graph_effect(publication_key(&fixture.envelope).unwrap())
                .await;
            graph
                .apply_install_with_grants(
                    &fixture.envelope,
                    &fixture.units,
                    &fixture.grants(),
                    || clock.fetch_add(1, Ordering::Relaxed) + 1,
                )
                .await
        }
        GraphAction::Upgrade => {
            let fixture = grant_upgrade_fixture_at(root, None, host.clone(), true).await;
            host.crash_after_graph_effect(publication_key(&fixture.envelope).unwrap())
                .await;
            graph
                .apply_upgrade_with_grants(
                    &fixture.envelope,
                    &fixture.prior_lock,
                    &fixture.candidates,
                    &fixture.retirements,
                    &fixture.grants(),
                    || clock.fetch_add(1, Ordering::Relaxed) + 1,
                )
                .await
        }
        GraphAction::Uninstall => {
            let fixture = grant_uninstall_fixture_at(root, None, host.clone(), true).await;
            host.crash_after_graph_effect(hide_key(&fixture.envelope).unwrap())
                .await;
            graph
                .apply_uninstall_with_grants(
                    &fixture.envelope,
                    &fixture.units,
                    &fixture.grants(),
                    || clock.fetch_add(1, Ordering::Relaxed) + 1,
                )
                .await
        }
    };
    panic!("graph cutover fault child completed without exiting for {action:?}: {outcome:?}");
}

fn action_name(action: GraphAction) -> &'static str {
    match action {
        GraphAction::Install => "install",
        GraphAction::Upgrade => "upgrade",
        GraphAction::Uninstall => "uninstall",
    }
}

fn parse_action(value: &str) -> GraphAction {
    match value {
        "install" => GraphAction::Install,
        "upgrade" => GraphAction::Upgrade,
        "uninstall" => GraphAction::Uninstall,
        _ => panic!("unsupported graph cutover fault child action {value:?}"),
    }
}
