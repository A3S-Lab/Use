use super::*;

#[tokio::test]
async fn lifecycle_graph_publication_is_one_cutover_and_recovers_partial_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base");
    let root_source = temp.path().join("root");
    cognitive_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    cognitive_package_with_dependencies(
        &root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    let base = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/base",
        &base_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let base_identity = lifecycle_identity(&base, 31);
    let root_identity = lifecycle_identity(&root, 32);
    let identities = [base_identity.clone(), root_identity.clone()];
    let registry = registry(temp.path());
    for (identity, candidate) in [(&base_identity, &base), (&root_identity, &root)] {
        registry
            .commit_lifecycle_package(identity, candidate)
            .await
            .unwrap();
    }
    let before = registry.snapshot().await.unwrap();
    assert!(before.packages.iter().all(|package| !package.enabled));

    // Model a process crash after one receipt was enabled but before the
    // complete dependency closure reached the snapshot commit point.
    let mut partial = registry.get("acme/base").await.unwrap().unwrap().receipt;
    partial.enabled = true;
    let artifact_store = registry.paths().artifact_store();
    let artifact_admission = artifact_store.acquire_reference_admission().await.unwrap();
    write_receipt(
        &artifact_store,
        &artifact_admission,
        &registry.paths().receipt_path("acme/base"),
        &partial,
    )
    .await
    .unwrap();
    let guarded = registry.snapshot().await.unwrap();
    assert_eq!(guarded, before);
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("root", "0.3.0")
        .await
        .unwrap()
        .is_none());

    let published = registry
        .publish_lifecycle_packages_for_test_host_version(&identities, "0.3.0")
        .await
        .unwrap();
    assert_eq!(published.len(), 2);
    assert!(published.iter().all(|result| result.extension.enabled()));
    assert!(published
        .iter()
        .all(|result| result.registry_generation == before.generation + 1));
    let after = registry.snapshot().await.unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert!(after.packages.iter().all(|package| package.enabled));
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("root", "0.3.0")
        .await
        .unwrap()
        .is_some());

    let replay = registry
        .publish_lifecycle_packages_for_test_host_version(&identities, "0.3.0")
        .await
        .unwrap();
    assert!(replay.iter().all(|result| !result.changed));
    assert!(replay
        .iter()
        .all(|result| result.registry_generation == after.generation));
}

#[tokio::test]
async fn lifecycle_graph_hide_returns_exact_stable_snapshot_evidence_in_one_cutover() {
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base-hide");
    let root_source = temp.path().join("root-hide");
    cognitive_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    cognitive_package_with_dependencies(
        &root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    let base = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/base",
        &base_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let base_identity = lifecycle_identity(&base, 41);
    let root_identity = lifecycle_identity(&root, 42);
    let identities = [base_identity.clone(), root_identity.clone()];
    let registry = registry(temp.path());
    for (identity, candidate) in [(&base_identity, &base), (&root_identity, &root)] {
        registry
            .commit_lifecycle_package(identity, candidate)
            .await
            .unwrap();
    }
    registry
        .publish_lifecycle_packages_for_test_host_version(&identities, "0.3.0")
        .await
        .unwrap();
    let before = registry.snapshot().await.unwrap();
    assert!(before.packages.iter().all(|package| package.enabled));

    let hidden = registry
        .hide_lifecycle_package_graph_with_evidence(&identities)
        .await
        .unwrap();
    let after = registry.snapshot().await.unwrap();
    assert_eq!(hidden.registry_generation, before.generation + 1);
    assert_eq!(hidden.registry_generation, after.generation);
    assert_eq!(
        hidden.registry_snapshot_digest,
        after.descriptor_digest().unwrap()
    );
    assert!(after.packages.is_empty());
    for identity in &identities {
        assert!(
            !registry
                .get_lifecycle_generation(identity)
                .await
                .unwrap()
                .unwrap()
                .receipt
                .enabled
        );
        registry
            .drain_lifecycle_package(identity, Duration::from_secs(1))
            .await
            .unwrap();
    }

    let replay = registry
        .hide_lifecycle_package_graph_with_evidence(&identities)
        .await
        .unwrap();
    assert_eq!(replay, hidden);
    assert_eq!(registry.snapshot().await.unwrap(), after);
}

#[tokio::test]
async fn lifecycle_graph_transition_atomically_publishes_candidates_and_hides_removed_nodes() {
    let host_version = env!("CARGO_PKG_VERSION");
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base");
    let prior_root_source = temp.path().join("prior-root");
    let candidate_root_source = temp.path().join("candidate-root");
    knowledge_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    knowledge_package_with_dependencies(
        &prior_root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    knowledge_package_with_dependencies(&candidate_root_source, "acme/root", "root", &[]).await;

    let base_catalog = verified_knowledge_catalog(&base_source, "acme/base", &[], 'a').await;
    let prior_root_catalog = verified_knowledge_catalog(
        &prior_root_source,
        "acme/root",
        &[("acme/base", "^1.0.0")],
        'b',
    )
    .await;
    let candidate_root_catalog =
        verified_knowledge_catalog(&candidate_root_source, "acme/root", &[], 'c').await;
    let lock_host = a3s_use_core::PluginPackageLockHost::new("linux-x86_64", host_version).unwrap();
    let prior_lock = a3s_use_core::PluginPackageResolver::new(lock_host.clone())
        .resolve(prior_root_catalog.clone(), vec![base_catalog.clone()])
        .unwrap();
    let candidate_lock = a3s_use_core::PluginPackageResolver::new(lock_host)
        .resolve(candidate_root_catalog.clone(), Vec::new())
        .unwrap();

    let base = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/base",
        &base_source,
        true,
        host_version,
    )
    .await
    .unwrap();
    let prior_root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &prior_root_source,
        true,
        host_version,
    )
    .await
    .unwrap();
    let candidate_root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &candidate_root_source,
        true,
        host_version,
    )
    .await
    .unwrap();
    let base_identity = lifecycle_identity(&base, 51);
    let prior_root_identity = lifecycle_identity(&prior_root, 52);
    let candidate_root_identity = lifecycle_identity(&candidate_root, 53);
    let registry = registry(temp.path());

    for (identity, package, catalog) in [
        (&base_identity, &base, &base_catalog),
        (&prior_root_identity, &prior_root, &prior_root_catalog),
    ] {
        registry
            .commit_lifecycle_package(identity, package)
            .await
            .unwrap();
        bind_remote_catalog_receipt(&registry, identity.package_id(), catalog).await;
    }
    registry
        .publish_lifecycle_package_graph_for_test_host_version(
            &prior_lock,
            &[base_identity.clone(), prior_root_identity],
            host_version,
        )
        .await
        .unwrap();
    let before = registry.snapshot().await.unwrap();

    registry
        .commit_lifecycle_package(&candidate_root_identity, &candidate_root)
        .await
        .unwrap();
    bind_remote_catalog_receipt(&registry, "acme/root", &candidate_root_catalog).await;

    let wrong_removed = ExtensionLifecycleIdentity::new(
        base_identity.package_id(),
        base_identity.package_digest(),
        base_identity.manifest_digest(),
        base_identity.generation() + 1,
    )
    .unwrap();
    let error = registry
        .publish_lifecycle_package_graph_transition(
            &candidate_lock,
            std::slice::from_ref(&candidate_root_identity),
            &[wrong_removed],
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_package_graph_invalid");
    assert_eq!(registry.snapshot().await.unwrap(), before);
    assert!(registry.get("acme/base").await.unwrap().unwrap().enabled());
    assert!(!registry.get("acme/root").await.unwrap().unwrap().enabled());
    let base_lease = registry
        .acquire_lifecycle_alias_for_host_version("base", host_version)
        .await
        .unwrap()
        .unwrap();

    // Recreate a process crash after the removed generation was copied to
    // retained storage and its selected receipt was deleted, but before the
    // candidate snapshot was published. The prior snapshot must remain the
    // visibility commit point and exact replay must finish the cutover.
    let selected_receipt = registry.paths().receipt_path(base_identity.package_id());
    let retained_receipt = registry.paths().retained_lifecycle_receipt_path(
        base_identity.package_id(),
        base_identity.generation(),
        base_identity
            .package_digest()
            .strip_prefix("sha256:")
            .unwrap(),
    );
    fs::create_dir_all(retained_receipt.parent().unwrap())
        .await
        .unwrap();
    fs::copy(&selected_receipt, &retained_receipt)
        .await
        .unwrap();
    fs::remove_file(&selected_receipt).await.unwrap();
    assert_eq!(registry.snapshot().await.unwrap(), before);
    assert!(registry.get("acme/base").await.unwrap().is_none());
    assert!(registry
        .get_lifecycle_generation(&base_identity)
        .await
        .unwrap()
        .unwrap()
        .enabled());

    let published = registry
        .publish_lifecycle_package_graph_transition(
            &candidate_lock,
            std::slice::from_ref(&candidate_root_identity),
            std::slice::from_ref(&base_identity),
        )
        .await
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(published[0].extension.enabled());
    let after = registry.snapshot().await.unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert!(after
        .packages
        .iter()
        .all(|package| package.package_id != "acme/base"));
    assert!(after.packages.iter().any(|package| {
        package.package_id == "acme/root"
            && package.lifecycle_generation == Some(candidate_root_identity.generation())
    }));
    assert!(registry.get("acme/base").await.unwrap().is_none());
    assert!(registry
        .get_lifecycle_generation(&base_identity)
        .await
        .unwrap()
        .unwrap()
        .enabled());
    assert_eq!(registry.snapshot().await.unwrap(), after);

    let replay = registry
        .publish_lifecycle_package_graph_transition(
            &candidate_lock,
            std::slice::from_ref(&candidate_root_identity),
            std::slice::from_ref(&base_identity),
        )
        .await
        .unwrap();
    assert!(replay.iter().all(|result| !result.changed));
    assert_eq!(
        registry.snapshot().await.unwrap().generation,
        after.generation
    );

    let hidden = registry
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap();
    assert!(hidden.changed);
    assert!(!hidden.extension.enabled());
    assert_eq!(hidden.registry_generation, after.generation);
    let error = registry
        .drain_lifecycle_package(&base_identity, Duration::from_millis(1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.drain_timeout");
    drop(base_lease);
    registry
        .drain_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap();
    let removed = registry
        .remove_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(removed.changed);
    assert!(registry
        .get_lifecycle_generation(&base_identity)
        .await
        .unwrap()
        .is_none());
    assert!(registry.lifecycle_package_root(&base_identity).is_dir());
    let removal_replay = registry
        .remove_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!removal_replay.changed);
    assert_eq!(
        registry.snapshot().await.unwrap().generation,
        after.generation
    );
}

#[tokio::test]
async fn lifecycle_graph_hide_revalidates_dependents_outside_the_reviewed_removal_set() {
    let host_version = env!("CARGO_PKG_VERSION");
    let temp = tempfile::tempdir().unwrap();
    let dependency_source = temp.path().join("dependency");
    let first_source = temp.path().join("first");
    let second_source = temp.path().join("second");
    knowledge_package_with_dependencies(&dependency_source, "acme/dependency", "dependency", &[])
        .await;
    knowledge_package_with_dependencies(
        &first_source,
        "acme/first",
        "first",
        &[("acme/dependency", "^1.0.0")],
    )
    .await;
    knowledge_package_with_dependencies(
        &second_source,
        "acme/second",
        "second",
        &[("acme/dependency", "^1.0.0")],
    )
    .await;

    let dependency_catalog =
        verified_knowledge_catalog(&dependency_source, "acme/dependency", &[], 'd').await;
    let first_catalog = verified_knowledge_catalog(
        &first_source,
        "acme/first",
        &[("acme/dependency", "^1.0.0")],
        'e',
    )
    .await;
    let second_catalog = verified_knowledge_catalog(
        &second_source,
        "acme/second",
        &[("acme/dependency", "^1.0.0")],
        'f',
    )
    .await;
    let reviewed_removal_lock = a3s_use_core::PluginPackageResolver::new(
        a3s_use_core::PluginPackageLockHost::new("linux-x86_64", host_version).unwrap(),
    )
    .resolve(first_catalog.clone(), vec![dependency_catalog.clone()])
    .unwrap();

    let dependency = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/dependency",
        &dependency_source,
        true,
        host_version,
    )
    .await
    .unwrap();
    let first = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/first",
        &first_source,
        true,
        host_version,
    )
    .await
    .unwrap();
    let second = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/second",
        &second_source,
        true,
        host_version,
    )
    .await
    .unwrap();
    let dependency_identity = lifecycle_identity(&dependency, 61);
    let first_identity = lifecycle_identity(&first, 62);
    let second_identity = lifecycle_identity(&second, 63);
    let identities = [
        dependency_identity.clone(),
        first_identity.clone(),
        second_identity.clone(),
    ];
    let registry = registry(temp.path());
    for (identity, package, catalog) in [
        (&dependency_identity, &dependency, &dependency_catalog),
        (&first_identity, &first, &first_catalog),
        (&second_identity, &second, &second_catalog),
    ] {
        registry
            .commit_lifecycle_package(identity, package)
            .await
            .unwrap();
        bind_remote_catalog_receipt(&registry, identity.package_id(), catalog).await;
    }
    registry
        .publish_lifecycle_packages_for_test_host_version(&identities, host_version)
        .await
        .unwrap();

    let snapshot_before = registry.snapshot().await.unwrap();
    let first_receipt_path = registry.paths().receipt_path(first_identity.package_id());
    let dependency_receipt_path = registry
        .paths()
        .receipt_path(dependency_identity.package_id());
    let first_receipt_before = std::fs::read(&first_receipt_path).unwrap();
    let dependency_receipt_before = std::fs::read(&dependency_receipt_path).unwrap();
    let key = format!("sha256:{}", "7".repeat(64));

    let error = registry
        .hide_lifecycle_package_graph_with_durable_cutover(
            &reviewed_removal_lock,
            &[first_identity.clone(), dependency_identity.clone()],
            snapshot_before.generation,
            &key,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.extension.package_required");
    assert_eq!(
        error.details["requiredBy"],
        serde_json::json!(["acme/second"])
    );
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before);
    assert_eq!(
        std::fs::read(&first_receipt_path).unwrap(),
        first_receipt_before
    );
    assert_eq!(
        std::fs::read(&dependency_receipt_path).unwrap(),
        dependency_receipt_before
    );
    assert!(registry.get("acme/first").await.unwrap().unwrap().enabled());
    assert!(registry
        .get("acme/dependency")
        .await
        .unwrap()
        .unwrap()
        .enabled());
}
