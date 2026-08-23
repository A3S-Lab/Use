use a3s_use_core::{PlanScope, PlanScopeKind, PluginPackageId};
use tokio::fs;
use tokio::time::{timeout, Duration};

use super::{
    CognitivePackageEnablementStore, StoredCognitivePackageEnablement, MAX_ENABLEMENT_RECORD_BYTES,
};

fn scope() -> PlanScope {
    PlanScope {
        kind: PlanScopeKind::Workspace,
        id: "workspace:test".to_string(),
    }
}

fn package_id() -> PluginPackageId {
    PluginPackageId::parse("acme/calendar").unwrap()
}

#[tokio::test]
async fn enablement_state_rejects_tampered_and_unbounded_records() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = CognitivePackageEnablementStore::new(&state_root);
    let scope = scope();
    let package_id = package_id();
    let state = StoredCognitivePackageEnablement::new(
        scope.clone(),
        package_id.to_string(),
        1,
        None,
        false,
        1,
    )
    .unwrap();
    store.put_state(&state).await.unwrap();

    let path = store.state_path(&scope, &package_id).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
    value["stateGeneration"] = serde_json::json!(0);
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap())
        .await
        .unwrap();
    let error = store.get_state(&scope, &package_id).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_enablement_store_invalid");

    let file = fs::File::create(&path).await.unwrap();
    file.set_len(MAX_ENABLEMENT_RECORD_BYTES + 1).await.unwrap();
    drop(file);
    let error = store.get_state(&scope, &package_id).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_enablement_store_invalid");
}

#[tokio::test]
async fn enablement_state_rejects_a_non_directory_path_component() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = CognitivePackageEnablementStore::new(&state_root);
    let scope = scope();
    let package_id = package_id();
    let package_directory = store.package_directory(&scope, &package_id).unwrap();
    let publisher_directory = package_directory.parent().unwrap();
    fs::create_dir_all(publisher_directory.parent().unwrap())
        .await
        .unwrap();
    fs::write(publisher_directory, b"not a directory")
        .await
        .unwrap();

    let error = store.get_state(&scope, &package_id).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_enablement_path_invalid");
}

#[tokio::test]
async fn operation_lock_serializes_the_same_identity_across_packages() {
    let temp = tempfile::tempdir().unwrap();
    let store = CognitivePackageEnablementStore::new(temp.path().join("state"));
    let scope = scope();
    let operation_id = "01JTESTENABLEMENTLOCK000000";
    let first = store.lock_operation(&scope, operation_id).await.unwrap();

    let waiting_store = store.clone();
    let waiting_scope = scope.clone();
    let mut waiting = tokio::spawn(async move {
        waiting_store
            .lock_operation(&waiting_scope, operation_id)
            .await
    });
    assert!(timeout(Duration::from_millis(50), &mut waiting)
        .await
        .is_err());

    drop(first);
    let second = timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(second);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn enablement_state_and_lock_reject_directory_links() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = CognitivePackageEnablementStore::new(&state_root);
    let scope = scope();
    let package_id = package_id();
    let package_directory = store.package_directory(&scope, &package_id).unwrap();
    let external = temp.path().join("external");
    fs::create_dir_all(&external).await.unwrap();
    fs::write(external.join("state.json"), b"{}").await.unwrap();
    fs::create_dir_all(package_directory.parent().unwrap())
        .await
        .unwrap();
    crate::test_filesystem::create_directory_link(&external, &package_directory);

    let error = store.get_state(&scope, &package_id).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_enablement_path_invalid");

    crate::test_filesystem::remove_directory_link(&package_directory);
    fs::create_dir(&package_directory).await.unwrap();
    let external_lock = temp.path().join("external-lock");
    fs::create_dir(&external_lock).await.unwrap();
    crate::test_filesystem::create_directory_link(
        &external_lock,
        &package_directory.join(".state.lock"),
    );
    let error = store.lock_package(&scope, &package_id).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_enablement_path_invalid");
}
