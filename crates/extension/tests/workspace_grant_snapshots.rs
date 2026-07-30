use a3s_use_core::{
    PlanActor, PlanPolicyDecision, PluginPermissionCeiling, PluginWorkspaceGrant,
    WorkspaceGrantAuthority, WorkspaceGrantEvidence, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};
use a3s_use_extension::{
    StoredWorkspaceGrant, WorkspaceGrantReceipt, WorkspaceGrantRevocation, WorkspaceGrantStore,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::fs;

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

#[tokio::test]
async fn stable_scope_snapshot_is_sorted_and_binds_exact_receipts() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let beta = receipt(4, grant(&ceiling, "beta/helper", DIGEST_D, 1_100));
    let acme = receipt(5, grant(&ceiling, "acme/research", DIGEST_A, 1_200));
    let retired = receipt(2, grant(&ceiling, "gamma/retired", DIGEST_B, 900));

    store.put(&beta, &ceiling, 1_500).await.unwrap();
    store.put(&acme, &ceiling, 1_500).await.unwrap();
    store.put(&retired, &ceiling, 1_500).await.unwrap();
    let revocation = WorkspaceGrantRevocation::new(3, &retired, authority(), 1_300).unwrap();
    store.revoke(&retired, &revocation).await.unwrap();

    let snapshot = store.snapshot_scope("workspace-01", 5).await.unwrap();
    assert_eq!(snapshot.scope_id, "workspace-01");
    assert_eq!(snapshot.state_revision, 5);
    assert_eq!(snapshot.grants, vec![evidence(&acme), evidence(&beta)]);
    assert_eq!(
        store
            .snapshot_scope("workspace-01", 5)
            .await
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        snapshot.descriptor_digest().unwrap()
    );

    let empty = store.snapshot_scope("workspace-02", 5).await.unwrap();
    assert!(empty.grants.is_empty());
}

#[tokio::test]
async fn snapshot_rejects_stale_state_and_parallel_granted_generations() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let current = receipt(7, grant(&ceiling, "acme/research", DIGEST_A, 1_000));
    store.put(&current, &ceiling, 1_500).await.unwrap();

    assert_eq!(
        store
            .snapshot_scope("workspace-01", 6)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.snapshot_stale"
    );

    let candidate = receipt(8, grant(&ceiling, "acme/research", DIGEST_D, 1_100));
    store.put(&candidate, &ceiling, 1_500).await.unwrap();
    assert_eq!(
        store
            .snapshot_scope("workspace-01", 8)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.snapshot_unstable"
    );
}

#[tokio::test]
async fn snapshot_tracks_revocation_revisions_and_ignores_abandoned_temporary_files() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let active = receipt(1, grant(&ceiling, "acme/research", DIGEST_A, 1_000));
    store.put(&active, &ceiling, 1_500).await.unwrap();
    let revocation = WorkspaceGrantRevocation::new(2, &active, authority(), 1_600).unwrap();
    store.revoke(&active, &revocation).await.unwrap();
    let parent = record_path(&store, "acme/research", DIGEST_A)
        .parent()
        .unwrap()
        .to_path_buf();

    fs::write(parent.join(".grant-abandoned.tmp"), b"incomplete")
        .await
        .unwrap();
    assert_eq!(
        store
            .snapshot_scope("workspace-01", 1)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.snapshot_stale"
    );
    assert_eq!(
        store
            .snapshot_scope("workspace-01", 2)
            .await
            .unwrap()
            .grants,
        Vec::<WorkspaceGrantEvidence>::new()
    );

    fs::write(parent.join("notes.txt"), b"unexpected")
        .await
        .unwrap();
    assert_eq!(
        store
            .snapshot_scope("workspace-01", 2)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.path_invalid"
    );
}

#[tokio::test]
async fn snapshot_rejects_a_valid_record_moved_to_another_package_path() {
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());
    let ceiling = permission_ceiling();
    let expected = receipt(1, grant(&ceiling, "acme/research", DIGEST_A, 1_000));
    store.put(&expected, &ceiling, 1_500).await.unwrap();

    let moved = receipt(2, grant(&ceiling, "acme/other", DIGEST_A, 1_100));
    fs::write(
        record_path(&store, "acme/research", DIGEST_A),
        serde_json::to_vec_pretty(&StoredWorkspaceGrant::Granted(moved)).unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(
        store
            .snapshot_scope("workspace-01", 2)
            .await
            .unwrap_err()
            .code,
        "use.plugin.grant_store.ownership_mismatch"
    );
}

fn permission_ceiling() -> PluginPermissionCeiling {
    PluginPermissionCeiling::from_json(include_bytes!(
        "../../core/fixtures/plugins/permission-ceiling-v1.json"
    ))
    .unwrap()
}

fn grant(
    ceiling: &PluginPermissionCeiling,
    package_id: &str,
    package_digest: &str,
    granted_at_ms: u64,
) -> PluginWorkspaceGrant {
    PluginWorkspaceGrant {
        schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
        scope_id: "workspace-01".to_string(),
        package_id: package_id.to_string(),
        package_digest: package_digest.to_string(),
        permission_ceiling_digest: ceiling.descriptor_digest().unwrap(),
        permissions_digest: ceiling.descriptor_digest().unwrap(),
        permissions: ceiling.clone(),
        authority: authority(),
        granted_at_ms,
        expires_at_ms: None,
    }
}

fn receipt(revision: u64, grant: PluginWorkspaceGrant) -> WorkspaceGrantReceipt {
    WorkspaceGrantReceipt::new(revision, grant).unwrap()
}

fn evidence(receipt: &WorkspaceGrantReceipt) -> WorkspaceGrantEvidence {
    WorkspaceGrantEvidence {
        package_id: receipt.grant.package_id.clone(),
        package_digest: receipt.grant.package_digest.clone(),
        receipt_revision: receipt.revision,
        grant_digest: receipt.grant_digest.clone(),
    }
}

fn authority() -> WorkspaceGrantAuthority {
    WorkspaceGrantAuthority {
        actor: PlanActor::User,
        decision: PlanPolicyDecision::Ask,
        policy_digest: DIGEST_B.to_string(),
        confirmation_digest: Some(DIGEST_C.to_string()),
    }
}

fn record_path(
    store: &WorkspaceGrantStore,
    package_id: &str,
    package_digest: &str,
) -> std::path::PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(b"workspace-01"));
    let mut package_segments = package_id.split('/');
    store
        .root()
        .join(scope_digest)
        .join(package_segments.next().unwrap())
        .join(package_segments.next().unwrap())
        .join(format!(
            "{}.json",
            package_digest.strip_prefix("sha256:").unwrap()
        ))
}
