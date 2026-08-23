use std::collections::BTreeSet;

use a3s_use_core::{PlanScope, PlanScopeKind, PluginOperationAction, PluginReleaseChannel};
use a3s_use_extension::{
    PackageRegistryResolutionObserver, TrustedRegistry, VerifiedRegistryMetadata,
};

use super::download_attempt::{PackageDownloadAttemptStore, PendingPackageDownloadAttempt};
use super::download_attempt_tests::package_lock;
use super::resolution_attempt::{
    PackageRegistryResolutionStatus, PackageResolutionAccess, PackageResolutionAttemptStatus,
    PackageResolutionAttemptStore, PendingPackageResolutionAttempt,
};

fn digest(seed: char) -> String {
    seed.to_string().repeat(64)
}

fn scope() -> PlanScope {
    PlanScope {
        kind: PlanScopeKind::User,
        id: "user/current".to_owned(),
    }
}

fn registry(temp: &std::path::Path, name: &str, seed: char) -> TrustedRegistry {
    TrustedRegistry::new(
        name,
        format!("https://{name}.example.test/a3s/"),
        digest(seed),
        None,
        temp.join(format!("registry-{name}")),
    )
    .unwrap()
}

fn metadata(
    registry: &TrustedRegistry,
    version: u64,
    package_targets: u64,
) -> VerifiedRegistryMetadata {
    VerifiedRegistryMetadata {
        registry_name: registry.name().to_owned(),
        registry_url: registry.base_url().to_string(),
        root_sha256: registry.root_sha256().to_owned(),
        root_version: version,
        timestamp_version: version + 1,
        snapshot_version: version + 2,
        targets_version: version + 3,
        package_targets,
    }
}

fn attempt(
    root: &TrustedRegistry,
    dependencies: &[TrustedRegistry],
    started_at_ms: u64,
) -> PendingPackageResolutionAttempt {
    PendingPackageResolutionAttempt::new(
        scope(),
        PluginOperationAction::Install,
        "acme/knowledge",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PackageResolutionAccess::Refreshed,
        root,
        dependencies,
        started_at_ms,
    )
    .unwrap()
}

#[tokio::test]
async fn resolution_store_tracks_each_verified_registry_and_the_exact_terminal_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = registry(temp.path(), "packages", 'd');
    let dependency = registry(temp.path(), "dependency", 'e');
    let store = PackageResolutionAttemptStore::new(temp.path());
    let active = store
        .begin(attempt(&root, std::slice::from_ref(&dependency), 10))
        .await
        .unwrap();

    active
        .registry_resolution_started("dependency")
        .await
        .unwrap();
    active
        .registry_resolution_verified(&metadata(&dependency, 10, 2))
        .await
        .unwrap();
    active
        .registry_resolution_started("packages")
        .await
        .unwrap();
    active
        .registry_resolution_verified(&metadata(&root, 20, 1))
        .await
        .unwrap();
    let lock = package_lock();
    active.mark_resolved(&lock).await.unwrap();

    let retained = store
        .get_for_package("acme/knowledge")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, PackageResolutionAttemptStatus::Resolved);
    assert_eq!(
        retained.package_lock_digest,
        Some(lock.descriptor_digest().unwrap())
    );
    assert_eq!(retained.package_count, Some(1));
    assert!(retained
        .registries
        .iter()
        .all(|registry| registry.status == PackageRegistryResolutionStatus::Verified));
    assert_eq!(retained.registries[0].registry_name, "dependency");
    assert_eq!(retained.registries[1].registry_name, "packages");
    assert_eq!(
        retained.registries[1].source_identity_digest,
        format!("sha256:{}", root.source_identity())
    );

    active.finish().await.unwrap();
    assert!(store
        .get_for_package("acme/knowledge")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn resolution_to_download_handoff_keeps_one_cross_process_package_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = registry(temp.path(), "packages", 'd');
    let resolution_store = PackageResolutionAttemptStore::new(temp.path());
    let active = resolution_store
        .begin(attempt(&root, &[], 10))
        .await
        .unwrap();
    active
        .registry_resolution_started("packages")
        .await
        .unwrap();
    active
        .registry_resolution_verified(&metadata(&root, 1, 1))
        .await
        .unwrap();
    let lock = package_lock();
    active.mark_resolved(&lock).await.unwrap();

    let download_store = PackageDownloadAttemptStore::new(temp.path());
    let download = PendingPackageDownloadAttempt::new(
        scope(),
        PluginOperationAction::Install,
        lock.clone(),
        BTreeSet::from([lock.root_package_id.clone()]),
        10,
    )
    .unwrap();
    let download = active
        .into_download(&download_store, download)
        .await
        .unwrap();
    assert!(resolution_store
        .get_for_package("acme/knowledge")
        .await
        .unwrap()
        .is_none());
    assert!(download_store
        .get_for_package("acme/knowledge")
        .await
        .unwrap()
        .is_some());

    assert_eq!(
        resolution_store
            .begin(attempt(&root, &[], 20))
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_resolution_attempt_busy"
    );
    download.finish().await.unwrap();
    let replacement = resolution_store
        .begin(attempt(&root, &[], 20))
        .await
        .unwrap();
    replacement.finish().await.unwrap();
}

#[tokio::test]
async fn failed_resolution_retains_only_the_stable_error_code_and_rejects_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = registry(temp.path(), "packages", 'd');
    let store = PackageResolutionAttemptStore::new(temp.path());
    let active = store.begin(attempt(&root, &[], 10)).await.unwrap();
    active
        .registry_resolution_started("packages")
        .await
        .unwrap();
    active
        .registry_resolution_failed("packages", "use.extension.registry_untrusted")
        .await
        .unwrap();
    active
        .mark_failed("use.extension.registry_untrusted")
        .await
        .unwrap();
    drop(active);

    let retained = store
        .get_for_package("acme/knowledge")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, PackageResolutionAttemptStatus::Failed);
    assert_eq!(
        retained.error_code.as_deref(),
        Some("use.extension.registry_untrusted")
    );
    assert_eq!(
        retained.registries[0].error_code.as_deref(),
        Some("use.extension.registry_untrusted")
    );

    let path = temp
        .path()
        .join("operations/package-resolutions/install/acme/knowledge.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["token"] = serde_json::json!("secret-sentinel");
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        store
            .get_for_package("acme/knowledge")
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_resolution_attempt_store_invalid"
    );
}

#[tokio::test]
async fn retry_does_not_delete_valid_download_evidence_when_resolution_state_is_damaged() {
    let temp = tempfile::tempdir().unwrap();
    let root = registry(temp.path(), "packages", 'd');
    let lock = package_lock();
    let download_store = PackageDownloadAttemptStore::new(temp.path());
    let download = download_store
        .begin(
            PendingPackageDownloadAttempt::new(
                scope(),
                PluginOperationAction::Install,
                lock.clone(),
                BTreeSet::from([lock.root_package_id.clone()]),
                10,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    drop(download);
    let download_path = temp
        .path()
        .join("operations/package-downloads/install/acme/knowledge.json");
    assert!(download_path.is_file());

    let resolution_path = temp
        .path()
        .join("operations/package-resolutions/install/acme/knowledge.json");
    std::fs::create_dir_all(resolution_path.parent().unwrap()).unwrap();
    let mut damaged = serde_json::to_value(attempt(&root, &[], 20)).unwrap();
    damaged["credential"] = serde_json::json!("secret-sentinel");
    std::fs::write(resolution_path, serde_json::to_vec(&damaged).unwrap()).unwrap();

    assert_eq!(
        PackageResolutionAttemptStore::new(temp.path())
            .begin(attempt(&root, &[], 30))
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_resolution_attempt_store_invalid"
    );
    assert!(download_path.is_file());
    assert!(download_store
        .get_for_package("acme/knowledge")
        .await
        .unwrap()
        .is_some());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn resolution_store_rejects_a_linked_owned_directory() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    crate::test_filesystem::create_directory_link(outside.path(), &temp.path().join("operations"));
    let root = registry(temp.path(), "packages", 'd');

    assert_eq!(
        PackageResolutionAttemptStore::new(temp.path())
            .begin(attempt(&root, &[], 10))
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_resolution_attempt_store_invalid"
    );
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}
