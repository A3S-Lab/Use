use super::*;
use crate::ArtifactStore;

#[tokio::test]
async fn prepared_package_admission_is_idempotent_and_creates_no_lifecycle_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        a3s_use_core::InstallationId::new(
            a3s_use_core::InstallationKind::User,
            "artifact-admission",
        )
        .unwrap(),
    )
    .unwrap();
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/knowledge", &source, true)
        .await
        .unwrap();
    let store = paths.artifact_store();
    let admission = store.acquire_reference_admission().await.unwrap();

    store
        .admit_prepared_package(&admission, &candidate)
        .await
        .unwrap();
    store
        .admit_prepared_package(&admission, &candidate)
        .await
        .unwrap();

    assert!(store
        .expanded_package_path(candidate.package_digest())
        .unwrap()
        .is_dir());
    assert!(!paths.installation_state_root().exists());
}

#[tokio::test]
async fn prepared_package_admission_revalidates_the_source_before_writing() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("package");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    crate::package::copy_package(&fixture, &source)
        .await
        .unwrap();
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/knowledge", &source, true)
        .await
        .unwrap();
    std::fs::write(source.join("README.md"), b"substituted after preparation").unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let admission = store.acquire_reference_admission().await.unwrap();

    let error = store
        .admit_prepared_package(&admission, &candidate)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.extension.package_changed");
    assert!(!store
        .expanded_package_path(candidate.package_digest())
        .unwrap()
        .exists());
}
