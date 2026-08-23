use std::collections::BTreeSet;

use a3s_use_core::{
    CatalogAvailability, PlanScope, PlanScopeKind, PluginCatalogRecord, PluginOperationAction,
    PluginPackageLock, PluginPackageLockHost, PluginPackageResolver, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V3,
};

use super::download_attempt::{PackageDownloadAttemptStore, PendingPackageDownloadAttempt};

const CATALOG: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

pub(super) fn package_lock() -> PluginPackageLock {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    record.availability = CatalogAvailability::Available;
    record.archive.sha256 = digest('a');
    record.package.sha256 = Some(digest('b'));
    record.package.manifest_sha256 = Some(digest('c'));
    record.validate().unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_owned(),
        registry_url: "https://packages.example.test/a3s/".to_owned(),
        root_sha256: digest('d'),
        root_version: 1,
        timestamp_version: 2,
        snapshot_version: 3,
        targets_version: 4,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(
        VerifiedPluginCatalogRecord::new(record, provenance).unwrap(),
        Vec::new(),
    )
    .unwrap()
}

fn attempt(lock: &PluginPackageLock, started_at_ms: u64) -> PendingPackageDownloadAttempt {
    PendingPackageDownloadAttempt::new(
        PlanScope {
            kind: PlanScopeKind::User,
            id: "user/current".to_owned(),
        },
        PluginOperationAction::Install,
        lock.clone(),
        BTreeSet::from([lock.root_package_id.clone()]),
        started_at_ms,
    )
    .unwrap()
}

#[tokio::test]
async fn attempt_store_retains_process_exit_evidence_and_replaces_it_under_lock() {
    let temp = tempfile::tempdir().unwrap();
    let store = PackageDownloadAttemptStore::new(temp.path());
    let lock = package_lock();
    let first = attempt(&lock, 10);
    let first_guard = store.begin(first.clone()).await.unwrap();
    assert_eq!(
        store.get_for_package(&lock.root_package_id).await.unwrap(),
        Some(first)
    );

    assert_eq!(
        store.begin(attempt(&lock, 20)).await.unwrap_err().code,
        "use.plugin.package_download_attempt_busy"
    );

    drop(first_guard);
    let replacement = attempt(&lock, 20);
    let replacement_guard = store.begin(replacement.clone()).await.unwrap();
    assert_eq!(
        store.get_for_package(&lock.root_package_id).await.unwrap(),
        Some(replacement)
    );
    replacement_guard.finish().await.unwrap();
    assert!(store
        .get_for_package(&lock.root_package_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn attempt_store_fails_closed_on_unknown_privileged_fields() {
    let temp = tempfile::tempdir().unwrap();
    let store = PackageDownloadAttemptStore::new(temp.path());
    let lock = package_lock();
    let guard = store.begin(attempt(&lock, 10)).await.unwrap();
    drop(guard);

    let (publisher, package) = lock.root_package_id.split_once('/').unwrap();
    let path = temp
        .path()
        .join("operations/package-downloads/install")
        .join(publisher)
        .join(format!("{package}.json"));
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["credential"] = serde_json::json!("secret-sentinel-value");
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();

    assert_eq!(
        store
            .get_for_package(&lock.root_package_id)
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_download_attempt_store_invalid"
    );
}

#[tokio::test]
async fn attempt_store_does_not_reconcile_ambiguous_action_records() {
    let temp = tempfile::tempdir().unwrap();
    let store = PackageDownloadAttemptStore::new(temp.path());
    let lock = package_lock();
    let guard = store.begin(attempt(&lock, 10)).await.unwrap();
    drop(guard);

    let (publisher, package) = lock.root_package_id.split_once('/').unwrap();
    let upgrade_path = temp
        .path()
        .join("operations/package-downloads/upgrade")
        .join(publisher)
        .join(format!("{package}.json"));
    std::fs::create_dir_all(upgrade_path.parent().unwrap()).unwrap();
    let upgrade = PendingPackageDownloadAttempt::new(
        PlanScope {
            kind: PlanScopeKind::User,
            id: "user/current".to_owned(),
        },
        PluginOperationAction::Upgrade,
        lock.clone(),
        BTreeSet::from([lock.root_package_id.clone()]),
        20,
    )
    .unwrap();
    std::fs::write(&upgrade_path, serde_json::to_vec(&upgrade).unwrap()).unwrap();

    assert_eq!(
        store.begin(attempt(&lock, 30)).await.unwrap_err().code,
        "use.plugin.package_download_attempt_store_invalid"
    );
    assert!(upgrade_path.is_file());
    assert!(temp
        .path()
        .join("operations/package-downloads/install")
        .join(publisher)
        .join(format!("{package}.json"))
        .is_file());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn attempt_store_rejects_a_linked_owned_directory() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    crate::test_filesystem::create_directory_link(outside.path(), &temp.path().join("operations"));
    let lock = package_lock();

    assert_eq!(
        PackageDownloadAttemptStore::new(temp.path())
            .begin(attempt(&lock, 10))
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_download_attempt_store_invalid"
    );
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}
