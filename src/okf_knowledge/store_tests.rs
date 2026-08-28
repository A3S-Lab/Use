use std::fs;

use a3s_use_core::{OkfKnowledgeObservedState, PlanScope, PlanScopeKind};
use tempfile::TempDir;

use super::test_support::{binding, receipt, scope, surface};
use super::*;

#[tokio::test]
async fn store_round_trips_idempotently_and_promotes_one_generation() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let receipt = receipt(1);
    let staged = binding(&receipt, OkfKnowledgeObservedState::Staged, None, 1_001);
    assert!(store.put(&staged).await.unwrap());
    assert!(!store.put(&staged).await.unwrap());
    assert_eq!(
        store
            .get(&scope(PlanScopeKind::Workspace), &surface(), 1)
            .await
            .unwrap(),
        Some(staged)
    );

    let promoted = binding(
        &receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&receipt),
        1_002,
    );
    assert!(store.put(&promoted).await.unwrap());
    let snapshot = store
        .snapshot(&scope(PlanScopeKind::Workspace), &surface())
        .await
        .unwrap();
    assert_eq!(snapshot.latest, Some(promoted.clone()));
    assert_eq!(snapshot.selected, Some(promoted));
    assert_eq!(snapshot.projection.unwrap().generation, 1);
}

#[tokio::test]
async fn identical_scope_ids_are_isolated_by_scope_kind() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let workspace_receipt = receipt(1);
    let workspace = binding(
        &workspace_receipt,
        OkfKnowledgeObservedState::Staged,
        None,
        1_001,
    );
    let mut user_receipt = workspace_receipt;
    user_receipt.scope.kind = PlanScopeKind::User;
    let user = binding(
        &user_receipt,
        OkfKnowledgeObservedState::Staged,
        None,
        1_001,
    );

    assert!(store.put(&workspace).await.unwrap());
    assert!(store.put(&user).await.unwrap());
    assert_ne!(
        binding_path(&store, &workspace.receipt.scope, 1),
        binding_path(&store, &user.receipt.scope, 1)
    );
    assert_eq!(
        store
            .get(&workspace.receipt.scope, &surface(), 1)
            .await
            .unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store.get(&user.receipt.scope, &surface(), 1).await.unwrap(),
        Some(user)
    );
}

#[tokio::test]
async fn failed_candidate_retains_last_good_and_next_promotion_switches_atomically() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    let first = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&first_receipt),
        1_001,
    );
    store.put(&first).await.unwrap();

    let second_receipt = receipt(2);
    let failed = binding(
        &second_receipt,
        OkfKnowledgeObservedState::Failed,
        Some(&first_receipt),
        2_001,
    );
    store.put(&failed).await.unwrap();
    let snapshot = store
        .snapshot(&scope(PlanScopeKind::Workspace), &surface())
        .await
        .unwrap();
    assert_eq!(snapshot.latest, Some(failed));
    assert_eq!(snapshot.selected, Some(first));
    assert_eq!(snapshot.projection.unwrap().generation, 1);

    let third_receipt = receipt(3);
    let third = binding(
        &third_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&third_receipt),
        3_001,
    );
    store.put(&third).await.unwrap();
    let snapshot = store
        .snapshot(&scope(PlanScopeKind::Workspace), &surface())
        .await
        .unwrap();
    assert_eq!(snapshot.selected, Some(third));
    assert_eq!(snapshot.projection.unwrap().generation, 3);
}

#[tokio::test]
async fn removed_latest_generation_suppresses_fallback() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    store
        .put(&binding(
            &first_receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(&first_receipt),
            1_001,
        ))
        .await
        .unwrap();
    let removed_receipt = receipt(2);
    let removed = binding(
        &removed_receipt,
        OkfKnowledgeObservedState::Removed,
        None,
        2_001,
    );
    store.put(&removed).await.unwrap();

    let snapshot = store
        .snapshot(&scope(PlanScopeKind::Workspace), &surface())
        .await
        .unwrap();
    assert_eq!(snapshot.latest, Some(removed));
    assert!(snapshot.selected.is_none());
    assert!(snapshot.projection.is_none());
}

#[tokio::test]
async fn store_rejects_stale_and_conflicting_same_generation_writes() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    let promoted = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&first_receipt),
        1_010,
    );
    store.put(&promoted).await.unwrap();

    let stale = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&first_receipt),
        1_009,
    );
    assert_eq!(
        store.put(&stale).await.unwrap_err().code,
        "use.okf.knowledge_binding_stale"
    );
    let staged = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Staged,
        None,
        1_011,
    );
    assert_eq!(
        store.put(&staged).await.unwrap_err().code,
        "use.okf.knowledge_binding_conflict"
    );
    let mut conflicting_receipt = first_receipt.clone();
    conflicting_receipt.projection_id = "other-projection".to_owned();
    let conflicting = binding(
        &conflicting_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&conflicting_receipt),
        1_012,
    );
    assert_eq!(
        store.put(&conflicting).await.unwrap_err().code,
        "use.okf.knowledge_binding_conflict"
    );

    let second_receipt = receipt(2);
    store
        .put(&binding(
            &second_receipt,
            OkfKnowledgeObservedState::Staged,
            Some(&first_receipt),
            2_001,
        ))
        .await
        .unwrap();
    let late_refresh = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&first_receipt),
        1_013,
    );
    assert_eq!(
        store.put(&late_refresh).await.unwrap_err().code,
        "use.okf.knowledge_binding_stale"
    );
}

#[tokio::test]
async fn receipt_owned_cleanup_can_remove_an_older_superseded_generation() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    store
        .put(&binding(
            &first_receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(&first_receipt),
            1_001,
        ))
        .await
        .unwrap();
    let second_receipt = receipt(2);
    let second = binding(
        &second_receipt,
        OkfKnowledgeObservedState::Promoted,
        Some(&second_receipt),
        2_001,
    );
    store.put(&second).await.unwrap();

    let removed = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Removed,
        None,
        2_002,
    );
    store.put(&removed).await.unwrap();
    assert_eq!(
        store
            .get(&scope(PlanScopeKind::Workspace), &surface(), 1)
            .await
            .unwrap(),
        Some(removed)
    );
    let snapshot = store
        .snapshot(&scope(PlanScopeKind::Workspace), &surface())
        .await
        .unwrap();
    assert_eq!(snapshot.selected, Some(second));
    assert_eq!(snapshot.projection.unwrap().generation, 2);
}

#[tokio::test]
async fn next_stage_prunes_retired_tombstones_at_the_generation_bound() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    store
        .put(&binding(
            &first_receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(&first_receipt),
            1_001,
        ))
        .await
        .unwrap();

    for generation in 2..=MAX_OKF_KNOWLEDGE_GENERATIONS as u64 {
        let current = receipt(generation);
        let previous = receipt(generation - 1);
        store
            .put(&binding(
                &current,
                OkfKnowledgeObservedState::Staged,
                Some(&previous),
                generation * 1_000,
            ))
            .await
            .unwrap();
        store
            .put(&binding(
                &current,
                OkfKnowledgeObservedState::Promoted,
                Some(&current),
                generation * 1_000 + 1,
            ))
            .await
            .unwrap();
        store
            .put(&binding(
                &previous,
                OkfKnowledgeObservedState::Removed,
                None,
                generation * 1_000 + 2,
            ))
            .await
            .unwrap();
    }

    let next = receipt(MAX_OKF_KNOWLEDGE_GENERATIONS as u64 + 1);
    let selected = receipt(MAX_OKF_KNOWLEDGE_GENERATIONS as u64);
    store
        .put(&binding(
            &next,
            OkfKnowledgeObservedState::Staged,
            Some(&selected),
            next.staged_at_ms + 1,
        ))
        .await
        .unwrap();
    assert!(store
        .get(&scope(PlanScopeKind::Workspace), &surface(), 1)
        .await
        .unwrap()
        .is_none());
    let snapshot = store
        .snapshot(&scope(PlanScopeKind::Workspace), &surface())
        .await
        .unwrap();
    assert_eq!(
        snapshot.latest.unwrap().receipt.generation,
        MAX_OKF_KNOWLEDGE_GENERATIONS as u64 + 1
    );
    assert_eq!(
        snapshot.selected.unwrap().receipt.generation,
        MAX_OKF_KNOWLEDGE_GENERATIONS as u64
    );
}

#[tokio::test]
async fn store_rejects_missing_selected_generation_and_tampered_json() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    let second_receipt = receipt(2);
    let dangling = binding(
        &second_receipt,
        OkfKnowledgeObservedState::Failed,
        Some(&first_receipt),
        2_001,
    );
    assert_eq!(
        store.put(&dangling).await.unwrap_err().code,
        "use.okf.knowledge_binding_selection_invalid"
    );

    let staged = binding(
        &first_receipt,
        OkfKnowledgeObservedState::Staged,
        None,
        1_001,
    );
    store.put(&staged).await.unwrap();
    fs::write(
        binding_path(&store, &scope(PlanScopeKind::Workspace), 1),
        b"{}",
    )
    .unwrap();
    assert_eq!(
        store
            .get(&scope(PlanScopeKind::Workspace), &surface(), 1)
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_binding_record_invalid"
    );
}

#[tokio::test]
async fn store_rejects_wrong_scope_and_surface_identity() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let receipt = receipt(1);
    store
        .put(&binding(
            &receipt,
            OkfKnowledgeObservedState::Staged,
            None,
            1_001,
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .get(
                &PlanScope {
                    kind: PlanScopeKind::Workspace,
                    id: "../workspace".to_owned(),
                },
                &surface(),
                1,
            )
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_binding_path_invalid"
    );
    let mut wrong_kind = surface();
    wrong_kind.surface.kind = a3s_use_core::PluginSurfaceKind::Skill;
    assert_eq!(
        store
            .snapshot(&scope(PlanScopeKind::Workspace), &wrong_kind)
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_binding_path_invalid"
    );
}

#[tokio::test]
async fn store_detects_a_valid_record_moved_to_another_scope_path() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let first_receipt = receipt(1);
    store
        .put(&binding(
            &first_receipt,
            OkfKnowledgeObservedState::Staged,
            None,
            1_001,
        ))
        .await
        .unwrap();

    let mut other_receipt = receipt(1);
    other_receipt.scope.id = "other-scope".to_owned();
    store
        .put(&binding(
            &other_receipt,
            OkfKnowledgeObservedState::Staged,
            None,
            1_001,
        ))
        .await
        .unwrap();
    fs::copy(
        binding_path(&store, &other_receipt.scope, 1),
        binding_path(&store, &scope(PlanScopeKind::Workspace), 1),
    )
    .unwrap();

    assert_eq!(
        store
            .get(&scope(PlanScopeKind::Workspace), &surface(), 1)
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_binding_ownership_mismatch"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn store_rejects_linked_owned_directories() {
    let temporary = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    fs::create_dir_all(store.root().parent().unwrap()).unwrap();
    crate::test_filesystem::create_directory_link(outside.path(), store.root());

    let receipt = receipt(1);
    let error = store
        .put(&binding(
            &receipt,
            OkfKnowledgeObservedState::Staged,
            None,
            1_001,
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_binding_path_invalid");
}

#[tokio::test]
async fn store_fails_closed_at_the_generation_bound() {
    let temporary = TempDir::new().unwrap();
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    for generation in 1..=MAX_OKF_KNOWLEDGE_GENERATIONS as u64 {
        let receipt = receipt(generation);
        store
            .put(&binding(
                &receipt,
                OkfKnowledgeObservedState::Staged,
                None,
                generation * 1_000 + 1,
            ))
            .await
            .unwrap();
    }
    let overflow = receipt(MAX_OKF_KNOWLEDGE_GENERATIONS as u64 + 1);
    assert_eq!(
        store
            .put(&binding(
                &overflow,
                OkfKnowledgeObservedState::Staged,
                None,
                overflow.staged_at_ms + 1,
            ))
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_binding_limit_exceeded"
    );
}

#[test]
fn binding_store_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OkfKnowledgeBindingStore>();
    assert_send_sync::<OkfKnowledgeBindingSnapshot>();
}

fn binding_path(
    store: &OkfKnowledgeBindingStore,
    scope: &PlanScope,
    generation: u64,
) -> std::path::PathBuf {
    let scope_digest = scope.storage_key().unwrap();
    store
        .root()
        .join(scope.kind.as_str())
        .join(scope_digest)
        .join("acme")
        .join("research")
        .join("okf-domain-knowledge")
        .join(format!("{generation:020}.json"))
}
