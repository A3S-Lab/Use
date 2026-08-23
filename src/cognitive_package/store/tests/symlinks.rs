use super::*;

#[tokio::test]
async fn installed_graph_reads_reject_a_linked_publisher_directory() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let external = temp.path().join("external");
    fs::create_dir_all(&external).await.unwrap();
    fs::write(external.join("root.json"), b"{}").await.unwrap();
    let graph_root = state_root.join("package-graphs");
    fs::create_dir_all(&graph_root).await.unwrap();
    crate::test_filesystem::create_directory_link(&external, &graph_root.join("acme"));

    let error = InstalledPackageGraphStore::new(&state_root)
        .get("acme/root")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
}

#[tokio::test]
async fn pending_graph_reads_reject_a_linked_publisher_directory() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let external = temp.path().join("external");
    fs::create_dir_all(&external).await.unwrap();
    fs::write(external.join("root.json"), b"{}").await.unwrap();
    let operation_root = state_root
        .join("operations")
        .join("package-graphs")
        .join("uninstall");
    fs::create_dir_all(&operation_root).await.unwrap();
    crate::test_filesystem::create_directory_link(&external, &operation_root.join("acme"));

    let error = PendingPackageGraphStore::new(&state_root)
        .get(PluginOperationAction::Uninstall, "acme/root")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
}
