use std::path::PathBuf;

use a3s_use_core::UseResult;

use crate::{
    StoredWorkspaceGrant, WorkspaceGrantLifecyclePhase, WorkspaceGrantOperationJournal,
    WorkspaceGrantStore,
};

#[allow(dead_code)]
#[path = "../tests/support/workspace_grant_lifecycle_fixtures.rs"]
mod fixtures;

use fixtures::{cutover, digest, install_fixture, upgrade_fixture, LifecycleFixture};

pub(crate) const INTENT_RECORDED: &str = "intent-recorded";
pub(crate) const PREPARING: &str = "preparing";
pub(crate) const PREPARED: &str = "prepared";
pub(crate) const CUTOVER_COMMITTED: &str = "cutover-committed";
pub(crate) const RETIRING: &str = "retiring";
pub(crate) const COMPLETED: &str = "completed";
pub(crate) const ROLLING_BACK: &str = "rolling-back";
pub(crate) const ROLLED_BACK: &str = "rolled-back";

const CHECKPOINT_CRASH_EXIT_CODE: i32 = 88;
const CHILD_ROOT_ENV: &str = "A3S_USE_TEST_GRANT_CHECKPOINT_ROOT";
const CHILD_SCENARIO_ENV: &str = "A3S_USE_TEST_GRANT_CHECKPOINT_SCENARIO";
const FAULT_CHECKPOINT_ENV: &str = "A3S_USE_TEST_GRANT_CHECKPOINT";
const PREPARED_AT_MS: u64 = 1_250;
const ROLLED_BACK_AT_MS: u64 = 1_275;

pub(crate) fn candidate_prepared(package_id: &str) -> String {
    format!("candidate-prepared:{package_id}")
}

pub(crate) fn grant_retired(package_id: &str) -> String {
    format!("grant-retired:{package_id}")
}

pub(crate) fn candidate_restored(package_id: &str) -> String {
    format!("candidate-restored:{package_id}")
}

pub(crate) fn crash_after_checkpoint(checkpoint: &str) {
    if std::env::var(FAULT_CHECKPOINT_ENV).as_deref() == Ok(checkpoint) {
        std::process::exit(CHECKPOINT_CRASH_EXIT_CODE);
    }
}

#[derive(Debug, Clone, Copy)]
enum Scenario {
    Install,
    Upgrade,
    Rollback,
}

struct FaultCase {
    scenario: Scenario,
    checkpoint: String,
}

#[tokio::test]
async fn every_workspace_grant_durable_checkpoint_recovers_after_process_exit() {
    for case in fault_cases() {
        exercise_checkpoint_crash(case).await;
    }
}

fn fault_cases() -> Vec<FaultCase> {
    [
        INTENT_RECORDED.to_string(),
        PREPARING.to_string(),
        candidate_prepared("acme/helper"),
        candidate_prepared("acme/research"),
        PREPARED.to_string(),
    ]
    .into_iter()
    .map(|checkpoint| FaultCase {
        scenario: Scenario::Install,
        checkpoint,
    })
    .chain(
        [
            CUTOVER_COMMITTED.to_string(),
            RETIRING.to_string(),
            grant_retired("acme/helper"),
            grant_retired("acme/research"),
            COMPLETED.to_string(),
        ]
        .into_iter()
        .map(|checkpoint| FaultCase {
            scenario: Scenario::Upgrade,
            checkpoint,
        }),
    )
    .chain(
        [
            ROLLING_BACK.to_string(),
            candidate_restored("acme/research"),
            candidate_restored("acme/helper"),
            ROLLED_BACK.to_string(),
        ]
        .into_iter()
        .map(|checkpoint| FaultCase {
            scenario: Scenario::Rollback,
            checkpoint,
        }),
    )
    .collect()
}

async fn exercise_checkpoint_crash(case: FaultCase) {
    let temporary = tempfile::tempdir().expect("create Grant checkpoint fault directory");
    let root = temporary.path().to_path_buf();
    let output = tokio::process::Command::new(
        std::env::current_exe().expect("resolve Grant lifecycle test executable"),
    )
    .arg("workspace_grant_checkpoint_crash_child")
    .arg("--ignored")
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(CHILD_ROOT_ENV, &root)
    .env(CHILD_SCENARIO_ENV, scenario_name(case.scenario))
    .env(FAULT_CHECKPOINT_ENV, &case.checkpoint)
    .output()
    .await
    .expect("run Grant checkpoint fault child");
    assert_eq!(
        output.status.code(),
        Some(CHECKPOINT_CRASH_EXIT_CODE),
        "Grant fault child did not exit for {:?} checkpoint {}: status={:?}, stdout={}, stderr={}",
        case.scenario,
        case.checkpoint,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    match case.scenario {
        Scenario::Install => recover_completed(&root, install_fixture(), false).await,
        Scenario::Upgrade => recover_completed(&root, upgrade_fixture(), true).await,
        Scenario::Rollback => recover_rollback(&root, upgrade_fixture()).await,
    }
}

async fn recover_completed(root: &std::path::Path, fixture: LifecycleFixture, has_priors: bool) {
    let store = WorkspaceGrantStore::new(root);
    let completed = run_forward(&store, &fixture)
        .await
        .expect("recover Grant forward lifecycle");
    assert_eq!(completed.phase, WorkspaceGrantLifecyclePhase::Completed);
    assert_candidates_granted(&store, &completed).await;
    if has_priors {
        assert_priors_revoked(&store, &fixture).await;
    }

    let replayed = run_forward(&store, &fixture)
        .await
        .expect("replay completed Grant lifecycle");
    assert_eq!(replayed, completed);
    assert_candidates_granted(&store, &replayed).await;
    if has_priors {
        assert_priors_revoked(&store, &fixture).await;
    }
}

async fn recover_rollback(root: &std::path::Path, fixture: LifecycleFixture) {
    let store = WorkspaceGrantStore::new(root);
    let rollback_digest = digest('7');
    let rolled_back = store
        .rollback_change_set(
            &fixture.resolved.operation_id,
            rollback_digest.clone(),
            ROLLED_BACK_AT_MS,
            ROLLED_BACK_AT_MS,
        )
        .await
        .expect("recover Grant rollback lifecycle");
    assert_eq!(rolled_back.phase, WorkspaceGrantLifecyclePhase::RolledBack);
    assert_candidates_removed(&store, &rolled_back).await;
    assert_priors_granted(&store, &fixture).await;

    let replayed = store
        .rollback_change_set(
            &fixture.resolved.operation_id,
            rollback_digest,
            ROLLED_BACK_AT_MS,
            ROLLED_BACK_AT_MS + 100,
        )
        .await
        .expect("replay completed Grant rollback");
    assert_eq!(replayed, rolled_back);
    assert_candidates_removed(&store, &replayed).await;
    assert_priors_granted(&store, &fixture).await;
}

async fn run_forward(
    store: &WorkspaceGrantStore,
    fixture: &LifecycleFixture,
) -> UseResult<WorkspaceGrantOperationJournal> {
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await?;
    store
        .prepare_change_set(&fixture.resolved.operation_id, PREPARED_AT_MS)
        .await?;
    let cutover = cutover(&fixture.resolved);
    store
        .commit_change_set_cutover(
            &fixture.resolved.operation_id,
            cutover.clone(),
            cutover.committed_at_ms,
        )
        .await?;
    store
        .retire_change_set(&fixture.resolved.operation_id)
        .await
}

async fn run_rollback(
    store: &WorkspaceGrantStore,
    fixture: &LifecycleFixture,
) -> UseResult<WorkspaceGrantOperationJournal> {
    store
        .begin_change_set(&fixture.resolved, &fixture.ceilings)
        .await?;
    store
        .prepare_change_set(&fixture.resolved.operation_id, PREPARED_AT_MS)
        .await?;
    store
        .rollback_change_set(
            &fixture.resolved.operation_id,
            digest('7'),
            ROLLED_BACK_AT_MS,
            ROLLED_BACK_AT_MS,
        )
        .await
}

async fn initialize_priors(store: &WorkspaceGrantStore, fixture: &LifecycleFixture) {
    for prior in &fixture.priors {
        store
            .put(prior, &fixture.ceiling, 1_000)
            .await
            .expect("initialize prior Grant receipt");
    }
}

async fn assert_candidates_granted(
    store: &WorkspaceGrantStore,
    journal: &WorkspaceGrantOperationJournal,
) {
    for candidate in &journal.intent.candidates {
        assert_eq!(
            store
                .observe(
                    &journal.intent.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                )
                .await
                .unwrap(),
            Some(StoredWorkspaceGrant::Granted(candidate.receipt.clone())),
        );
    }
}

async fn assert_candidates_removed(
    store: &WorkspaceGrantStore,
    journal: &WorkspaceGrantOperationJournal,
) {
    for candidate in &journal.intent.candidates {
        assert_eq!(
            store
                .observe(
                    &journal.intent.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                )
                .await
                .unwrap(),
            None,
        );
    }
}

async fn assert_priors_revoked(store: &WorkspaceGrantStore, fixture: &LifecycleFixture) {
    for prior in &fixture.priors {
        assert!(matches!(
            store
                .observe(
                    &fixture.resolved.scope_id,
                    &prior.grant.package_id,
                    &prior.grant.package_digest,
                )
                .await
                .unwrap(),
            Some(StoredWorkspaceGrant::Revoked(_))
        ));
    }
}

async fn assert_priors_granted(store: &WorkspaceGrantStore, fixture: &LifecycleFixture) {
    for prior in &fixture.priors {
        assert_eq!(
            store
                .observe(
                    &fixture.resolved.scope_id,
                    &prior.grant.package_id,
                    &prior.grant.package_digest,
                )
                .await
                .unwrap(),
            Some(StoredWorkspaceGrant::Granted(prior.clone())),
        );
    }
}

#[tokio::test]
#[ignore = "subprocess helper for Grant checkpoint crash injection"]
async fn workspace_grant_checkpoint_crash_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let scenario = parse_scenario(
        &std::env::var(CHILD_SCENARIO_ENV).expect("Grant fault child scenario is missing"),
    );
    let store = WorkspaceGrantStore::new(root);
    let fixture = match scenario {
        Scenario::Install => install_fixture(),
        Scenario::Upgrade | Scenario::Rollback => upgrade_fixture(),
    };
    if !matches!(scenario, Scenario::Install) {
        initialize_priors(&store, &fixture).await;
    }
    let outcome = match scenario {
        Scenario::Install | Scenario::Upgrade => run_forward(&store, &fixture).await,
        Scenario::Rollback => run_rollback(&store, &fixture).await,
    };
    panic!("Grant checkpoint fault child completed without exiting for {scenario:?}: {outcome:?}");
}

fn scenario_name(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::Install => "install",
        Scenario::Upgrade => "upgrade",
        Scenario::Rollback => "rollback",
    }
}

fn parse_scenario(value: &str) -> Scenario {
    match value {
        "install" => Scenario::Install,
        "upgrade" => Scenario::Upgrade,
        "rollback" => Scenario::Rollback,
        _ => panic!("unsupported Grant checkpoint fault child scenario {value:?}"),
    }
}
