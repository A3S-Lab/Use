use a3s_use_core::{
    PluginGrantConfirmation, PluginPermissionCeiling, PluginWorkspaceGrantProposal,
};
use a3s_use_extension::{WorkspaceGrantReceipt, WorkspaceGrantStore};
use tempfile::TempDir;

const PLAN_DIGEST: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::test]
async fn confirmed_proposal_flows_into_exact_generation_storage() {
    let ceiling = PluginPermissionCeiling::from_json(include_bytes!(
        "../../core/fixtures/plugins/permission-ceiling-v1.json"
    ))
    .unwrap();
    let proposal = PluginWorkspaceGrantProposal::from_json(include_bytes!(
        "../../core/fixtures/plugins/workspace-grant-proposal-v1.json"
    ))
    .unwrap();
    let confirmation = PluginGrantConfirmation::from_json(include_bytes!(
        "../../core/fixtures/plugins/grant-confirmation-v1.json"
    ))
    .unwrap();
    let grant = proposal
        .finalize(&ceiling, PLAN_DIGEST, Some(&confirmation), 1_600)
        .unwrap();
    let receipt = WorkspaceGrantReceipt::new(1, grant).unwrap();
    let temporary = TempDir::new().unwrap();
    let store = WorkspaceGrantStore::new(temporary.path());

    assert!(store.put(&receipt, &ceiling, 1_600).await.unwrap());
    assert_eq!(
        store
            .resolve_active(
                "workspace-01",
                "acme/research",
                PACKAGE_DIGEST,
                &ceiling,
                1_600,
            )
            .await
            .unwrap(),
        Some(receipt)
    );
}
