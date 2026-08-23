use super::*;

#[tokio::test]
async fn snapshot_lease_pins_the_complete_published_generation_atomically() {
    let temporary = tempfile::tempdir().unwrap();
    let alpha_root = temporary.path().join("alpha");
    let beta_root = temporary.path().join("beta");
    cognitive_package_with_dependencies(&alpha_root, "acme/alpha", "alpha", &[]).await;
    cognitive_package_with_dependencies(&beta_root, "acme/beta", "beta", &[]).await;
    let alpha = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/alpha",
        &alpha_root,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let beta = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/beta",
        &beta_root,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let alpha_identity = lifecycle_identity(&alpha, 41);
    let beta_identity = lifecycle_identity(&beta, 42);
    let registry = registry(temporary.path());
    registry
        .commit_lifecycle_package(&alpha_identity, &alpha)
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&beta_identity, &beta)
        .await
        .unwrap();
    registry
        .publish_lifecycle_packages(&[alpha_identity.clone(), beta_identity.clone()])
        .await
        .unwrap();

    let snapshot = registry.snapshot().await.unwrap();
    let cursor = snapshot.cursor().unwrap();
    assert_eq!(cursor.schema, EXTENSION_SNAPSHOT_CURSOR_SCHEMA);
    assert_eq!(cursor.generation, snapshot.generation);
    assert_eq!(cursor.revision, snapshot.descriptor_digest().unwrap());
    assert!(cursor.is_fully_leasable());
    assert_eq!(
        cursor
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/alpha", "acme/beta"]
    );

    let lease = registry
        .acquire_published_snapshot(&cursor)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.cursor(), &cursor);
    assert_eq!(lease.len(), 2);
    assert_eq!(
        lease
            .packages()
            .map(|extension| extension.receipt.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/alpha", "acme/beta"]
    );
    lease.verify_integrity().await.unwrap();

    registry
        .hide_lifecycle_package_with_evidence(&alpha_identity)
        .await
        .unwrap();
    assert!(registry
        .acquire_published_snapshot(&cursor)
        .await
        .unwrap()
        .is_none());
    assert_eq!(lease.len(), 2, "the admitted old generation stays pinned");

    let error = registry
        .drain_lifecycle_package(&alpha_identity, Duration::from_millis(10))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.drain_timeout");
    drop(lease);
    registry
        .drain_lifecycle_package(&alpha_identity, Duration::from_secs(1))
        .await
        .unwrap();

    let current = registry.snapshot().await.unwrap().cursor().unwrap();
    assert_eq!(
        current
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/beta"]
    );
    assert!(registry
        .acquire_published_snapshot(&current)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn snapshot_lease_rejects_stale_and_digest_mismatched_cursors() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 17);
    let registry = registry(temporary.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry.publish_lifecycle_package(&identity).await.unwrap();
    let cursor = registry.snapshot().await.unwrap().cursor().unwrap();

    let mut stale = cursor.clone();
    stale.generation += 1;
    assert!(registry
        .acquire_published_snapshot(&stale)
        .await
        .unwrap()
        .is_none());

    let mut mismatched = cursor.clone();
    mismatched.revision = format!("sha256:{}", "0".repeat(64));
    assert!(registry
        .acquire_published_snapshot(&mismatched)
        .await
        .unwrap()
        .is_none());

    let mut invalid = cursor;
    invalid.revision = "sha256:ABC".to_owned();
    let error = registry
        .acquire_published_snapshot(&invalid)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.snapshot_cursor_invalid");
}

#[test]
fn snapshot_cursor_rejects_noncanonical_unleasable_routes() {
    let canonical = ExtensionSnapshotCursor {
        schema: EXTENSION_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
        generation: 1,
        revision: format!("sha256:{}", "a".repeat(64)),
        packages: Vec::new(),
        unleasable_routes: vec!["alpha".to_owned()],
    };
    canonical.validate().unwrap();

    let mut empty = canonical.clone();
    empty.unleasable_routes = vec![String::new()];
    assert_eq!(
        empty.validate().unwrap_err().code,
        "use.extension.snapshot_cursor_invalid"
    );

    let mut duplicate = canonical.clone();
    duplicate.unleasable_routes = vec!["alpha".to_owned(), "alpha".to_owned()];
    assert_eq!(
        duplicate.validate().unwrap_err().code,
        "use.extension.snapshot_cursor_invalid"
    );

    let mut unsorted = canonical.clone();
    unsorted.unleasable_routes = vec!["beta".to_owned(), "alpha".to_owned()];
    assert_eq!(
        unsorted.validate().unwrap_err().code,
        "use.extension.snapshot_cursor_invalid"
    );

    let mut unbounded = canonical;
    unbounded.unleasable_routes = (0..=a3s_use_core::MAX_PLUGIN_PLAN_ITEMS)
        .map(|index| format!("route-{index:05}"))
        .collect();
    assert_eq!(
        unbounded.validate().unwrap_err().code,
        "use.extension.snapshot_cursor_invalid"
    );
}

#[tokio::test]
async fn snapshot_lease_fails_closed_for_callable_legacy_routes() {
    let registry = registry(tempfile::tempdir().unwrap().path());
    let snapshot = ExtensionRegistrySnapshot {
        schema_version: super::super::REGISTRY_SCHEMA_VERSION,
        generation: 9,
        routes: vec![ExtensionRouteBinding {
            package_id: "acme/legacy".to_owned(),
            component_id: "use/acme-legacy".to_owned(),
            route: "legacy".to_owned(),
            version: "1.0.0".to_owned(),
            package_root: PathBuf::from("/managed/acme/legacy"),
            manifest_sha256: "a".repeat(64),
            package_sha256: Some("b".repeat(64)),
            lifecycle_generation: None,
            enabled: true,
            surfaces: vec!["skill".to_owned()],
        }],
        pending_cutovers: Vec::new(),
    };
    let cursor = snapshot.cursor().unwrap();
    assert!(!cursor.is_fully_leasable());
    assert_eq!(cursor.unleasable_routes, ["legacy"]);
    let error = registry
        .acquire_published_snapshot(&cursor)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.snapshot_unleasable");
}
