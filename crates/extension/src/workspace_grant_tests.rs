use std::fs;

use a3s_use_core::{
    PlanActor, PlanPolicyDecision, PluginPermissionCeiling, PluginWorkspaceGrant,
    WorkspaceGrantAuthority, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::{
    StoredWorkspaceGrant, WorkspaceGrantReceipt, WorkspaceGrantRevocation, WorkspaceGrantStore,
};

const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEXT_DIGEST: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const POLICY_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CONFIRMATION_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[tokio::test]
async fn grant_store_round_trips_active_authority_and_persists_revocation() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let receipt = receipt(1, grant(&ceiling, 1_000, Some(3_000)));

    assert!(store.put(&receipt, &ceiling, 1_500).await.unwrap());
    assert!(!store.put(&receipt, &ceiling, 1_500).await.unwrap());
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                PACKAGE_DIGEST,
                &ceiling,
                1_500,
            )
            .await
            .unwrap(),
        Some(receipt.clone())
    );

    let revocation = WorkspaceGrantRevocation::new(2, &receipt, authority(), 1_700).unwrap();
    assert!(store.revoke(&receipt, &revocation).await.unwrap());
    assert!(!store.revoke(&receipt, &revocation).await.unwrap());
    assert_eq!(
        store
            .observe("workspace-01", "acme/research", PACKAGE_DIGEST)
            .await
            .unwrap(),
        Some(StoredWorkspaceGrant::Revoked(revocation))
    );
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                PACKAGE_DIGEST,
                &ceiling,
                1_800,
            )
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn grant_store_rejects_stale_conflicting_and_pre_revocation_regrants() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let first = receipt(7, grant(&ceiling, 1_000, Some(3_000)));
    store.put(&first, &ceiling, 1_500).await.unwrap();

    let stale = receipt(6, grant(&ceiling, 1_100, Some(3_000)));
    assert_eq!(
        store.put(&stale, &ceiling, 1_500).await.unwrap_err().code,
        "use.plugin.grant_store.stale"
    );

    let conflict = receipt(7, grant(&ceiling, 1_100, Some(3_000)));
    assert_eq!(
        store
            .put(&conflict, &ceiling, 1_500)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.conflict"
    );

    let revocation = WorkspaceGrantRevocation::new(8, &first, authority(), 1_700).unwrap();
    store.revoke(&first, &revocation).await.unwrap();
    let pre_revocation = receipt(9, grant(&ceiling, 1_600, Some(3_000)));
    assert_eq!(
        store
            .put(&pre_revocation, &ceiling, 1_800)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.stale"
    );

    let regranted = receipt(9, grant(&ceiling, 1_800, Some(3_000)));
    assert!(store.put(&regranted, &ceiling, 1_800).await.unwrap());
}

#[tokio::test]
async fn grant_store_revalidates_ceiling_lifetime_and_package_generation() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();

    let expired = receipt(1, grant(&ceiling, 1_000, Some(2_000)));
    assert_eq!(
        store.put(&expired, &ceiling, 2_000).await.unwrap_err().code,
        "use.plugin.grant_expired"
    );

    let mut escalated_permissions = ceiling.clone();
    escalated_permissions.surfaces[0]
        .resources
        .as_mut()
        .unwrap()
        .cpu_millis += 1;
    let escalated = receipt(
        1,
        grant_with_permissions(
            &ceiling,
            escalated_permissions,
            PACKAGE_DIGEST,
            1_000,
            Some(3_000),
        ),
    );
    assert_eq!(
        store
            .put(&escalated, &ceiling, 1_500)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_exceeds_ceiling"
    );

    let active = receipt(1, grant(&ceiling, 1_000, Some(3_000)));
    store.put(&active, &ceiling, 1_500).await.unwrap();
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                NEXT_DIGEST,
                &ceiling,
                1_500,
            )
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn revocation_requires_exact_current_grant_ownership() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let first = receipt(1, grant(&ceiling, 1_000, Some(3_000)));
    let second = receipt(2, grant(&ceiling, 1_100, Some(3_000)));
    store.put(&first, &ceiling, 1_500).await.unwrap();
    store.put(&second, &ceiling, 1_500).await.unwrap();

    let stale_revocation = WorkspaceGrantRevocation::new(3, &first, authority(), 1_700).unwrap();
    assert_eq!(
        store
            .revoke(&first, &stale_revocation)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.ownership_changed"
    );
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                PACKAGE_DIGEST,
                &ceiling,
                1_800,
            )
            .await
            .unwrap(),
        Some(second)
    );
}

#[tokio::test]
async fn grant_store_fails_closed_on_non_regular_or_tampered_records() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let receipt = receipt(1, grant(&ceiling, 1_000, Some(3_000)));
    store.put(&receipt, &ceiling, 1_500).await.unwrap();
    let path = record_path(&store);

    fs::write(&path, b"{\"state\":\"granted\",\"record\":{}}").unwrap();
    assert_eq!(
        store
            .observe("workspace-01", "acme/research", PACKAGE_DIGEST)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.record_invalid"
    );

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(
        store
            .observe("workspace-01", "acme/research", PACKAGE_DIGEST)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.record_invalid"
    );
}

#[tokio::test]
async fn grant_store_rejects_valid_records_moved_across_scope_paths() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let expected = receipt(1, grant(&ceiling, 1_000, Some(3_000)));
    store.put(&expected, &ceiling, 1_500).await.unwrap();

    let mut moved_grant = grant(&ceiling, 1_100, Some(3_000));
    moved_grant.scope_id = "workspace-02".to_string();
    let moved = receipt(2, moved_grant);
    fs::write(
        record_path(&store),
        serde_json::to_vec_pretty(&StoredWorkspaceGrant::Granted(moved)).unwrap(),
    )
    .unwrap();

    assert_eq!(
        store
            .observe("workspace-01", "acme/research", PACKAGE_DIGEST)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.ownership_mismatch"
    );
    let replacement = receipt(3, grant(&ceiling, 1_200, Some(3_000)));
    assert_eq!(
        store
            .put(&replacement, &ceiling, 1_500)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.ownership_mismatch"
    );
}

#[tokio::test]
async fn package_generations_can_coexist_during_blue_green_upgrade() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let current = receipt(7, grant(&ceiling, 1_000, Some(3_000)));
    let candidate = receipt(
        1,
        grant_for_digest(&ceiling, NEXT_DIGEST, 1_100, Some(3_000)),
    );

    store.put(&current, &ceiling, 1_500).await.unwrap();
    store.put(&candidate, &ceiling, 1_500).await.unwrap();
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                PACKAGE_DIGEST,
                &ceiling,
                1_500,
            )
            .await
            .unwrap(),
        Some(current)
    );
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                NEXT_DIGEST,
                &ceiling,
                1_500,
            )
            .await
            .unwrap(),
        Some(candidate)
    );
}

#[tokio::test]
async fn concurrent_grant_writes_converge_on_the_highest_revision() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let first = receipt(1, grant(&ceiling, 1_000, Some(3_000)));
    store.put(&first, &ceiling, 1_500).await.unwrap();
    let second = receipt(2, grant(&ceiling, 1_100, Some(3_000)));
    let third = receipt(3, grant(&ceiling, 1_200, Some(3_000)));

    let (second_result, third_result) = tokio::join!(
        store.put(&second, &ceiling, 1_500),
        store.put(&third, &ceiling, 1_500),
    );
    assert!(third_result.is_ok());
    if let Err(error) = second_result {
        assert_eq!(error.code, "use.plugin.grant_store.stale");
    }
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                PACKAGE_DIGEST,
                &ceiling,
                1_500,
            )
            .await
            .unwrap(),
        Some(third)
    );
}

#[test]
fn grant_store_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorkspaceGrantStore>();
    assert_send_sync::<StoredWorkspaceGrant>();
    assert_send_sync::<WorkspaceGrantReceipt>();
    assert_send_sync::<WorkspaceGrantRevocation>();
}

fn permission_ceiling() -> PluginPermissionCeiling {
    PluginPermissionCeiling::from_json(include_bytes!(
        "../../core/fixtures/plugins/permission-ceiling-v1.json"
    ))
    .unwrap()
}

fn grant(
    ceiling: &PluginPermissionCeiling,
    granted_at_ms: u64,
    expires_at_ms: Option<u64>,
) -> PluginWorkspaceGrant {
    grant_for_digest(ceiling, PACKAGE_DIGEST, granted_at_ms, expires_at_ms)
}

fn grant_for_digest(
    ceiling: &PluginPermissionCeiling,
    package_digest: &str,
    granted_at_ms: u64,
    expires_at_ms: Option<u64>,
) -> PluginWorkspaceGrant {
    grant_with_permissions(
        ceiling,
        ceiling.clone(),
        package_digest,
        granted_at_ms,
        expires_at_ms,
    )
}

fn grant_with_permissions(
    ceiling: &PluginPermissionCeiling,
    permissions: PluginPermissionCeiling,
    package_digest: &str,
    granted_at_ms: u64,
    expires_at_ms: Option<u64>,
) -> PluginWorkspaceGrant {
    PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: "workspace-01".to_string(),
        package_id: "acme/research".to_string(),
        package_digest: package_digest.to_string(),
        permission_ceiling_digest: ceiling.descriptor_digest().unwrap(),
        permissions_digest: permissions.descriptor_digest().unwrap(),
        permissions,
        authority: authority(),
        granted_at_ms,
        expires_at_ms,
    }
}

fn receipt(revision: u64, grant: PluginWorkspaceGrant) -> WorkspaceGrantReceipt {
    WorkspaceGrantReceipt::new(revision, grant).unwrap()
}

fn authority() -> WorkspaceGrantAuthority {
    WorkspaceGrantAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: POLICY_DIGEST.to_string(),
        confirmation_digest: Some(CONFIRMATION_DIGEST.to_string()),
    }
}

fn record_path(store: &WorkspaceGrantStore) -> std::path::PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(b"workspace-01"));
    store
        .root()
        .join(scope_digest)
        .join("acme")
        .join("research")
        .join(format!(
            "{}.json",
            PACKAGE_DIGEST.strip_prefix("sha256:").unwrap()
        ))
}
