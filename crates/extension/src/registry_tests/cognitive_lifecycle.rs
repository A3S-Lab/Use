use super::*;

#[tokio::test]
async fn lifecycle_commit_keeps_all_six_surfaces_installed_disabled_until_atomic_publish() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 7);
    let registry = registry(temp.path());

    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    assert_eq!(committed.extension.receipt.schema_version, 3);
    assert_eq!(committed.extension.receipt.lifecycle_generation, Some(7));
    assert!(!committed.extension.receipt.enabled);
    assert_eq!(
        committed.extension.surfaces(),
        ["tool", "mcp", "okf", "flow", "skill", "ui"]
    );
    assert_eq!(
        committed.extension.receipt.package_root,
        registry.lifecycle_package_root(&identity)
    );

    let installed_disabled = registry.snapshot().await.unwrap();
    assert_eq!(installed_disabled.routes.len(), 1);
    assert_eq!(installed_disabled.routes[0].lifecycle_generation, Some(7));
    assert!(!installed_disabled.routes[0].enabled);
    assert!(registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_none());

    let commit_replay = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(!commit_replay.changed);
    assert_eq!(
        commit_replay.extension.receipt.descriptor_digest().unwrap(),
        committed.extension.receipt.descriptor_digest().unwrap()
    );

    for error in [
        registry.enable("acme/cognitive").await.unwrap_err(),
        registry.disable("acme/cognitive").await.unwrap_err(),
        registry.uninstall("acme/cognitive").await.unwrap_err(),
    ] {
        assert_eq!(error.code, "use.extension.lifecycle_managed");
    }

    let published = registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    assert!(published.changed);
    assert!(published.extension.receipt.enabled);
    assert_eq!(published.extension.receipt.lifecycle_generation, Some(7));
    assert!(registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_some());

    let replay = registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(
        replay.extension.receipt.descriptor_digest().unwrap(),
        published.extension.receipt.descriptor_digest().unwrap()
    );
}

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
    assert!(before.routes.iter().all(|route| !route.enabled));

    // Model a process crash after one receipt was enabled but before the
    // complete dependency closure reached the snapshot commit point.
    let mut partial = registry.get("acme/base").await.unwrap().unwrap().receipt;
    partial.enabled = true;
    write_receipt(&registry.paths().receipt_path("acme/base"), &partial)
        .await
        .unwrap();
    let guarded = registry.snapshot().await.unwrap();
    assert_eq!(guarded, before);
    assert!(registry
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("root", "0.3.0")
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
    assert!(after.routes.iter().all(|route| route.enabled));
    assert!(registry
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("root", "0.3.0")
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
    assert!(before.routes.iter().all(|route| route.enabled));

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
    assert!(after.routes.is_empty());
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
    let lock_host = a3s_use_core::PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap();
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
        "0.3.0",
    )
    .await
    .unwrap();
    let prior_root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &prior_root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let candidate_root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &candidate_root_source,
        true,
        "0.3.0",
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
            "0.3.0",
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
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
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
        .routes
        .iter()
        .all(|route| route.package_id != "acme/base"));
    assert!(after.routes.iter().any(|route| {
        route.package_id == "acme/root"
            && route.lifecycle_generation == Some(candidate_root_identity.generation())
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
    assert!(!registry.lifecycle_package_root(&base_identity).exists());
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
async fn lifecycle_graph_requires_the_exact_published_retained_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base");
    let root_source = temp.path().join("root");
    knowledge_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    knowledge_package_with_dependencies(
        &root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    let base_catalog = verified_knowledge_catalog(&base_source, "acme/base", &[], 'a').await;
    let root_catalog =
        verified_knowledge_catalog(&root_source, "acme/root", &[("acme/base", "^1.0.0")], 'b')
            .await;
    let package_lock = a3s_use_core::PluginPackageResolver::new(
        a3s_use_core::PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap(),
    )
    .resolve(root_catalog.clone(), vec![base_catalog.clone()])
    .unwrap();
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
    let registry = registry(temp.path());

    registry
        .commit_lifecycle_package(&base_identity, &base)
        .await
        .unwrap();
    bind_remote_catalog_receipt(&registry, "acme/base", &base_catalog).await;
    registry
        .publish_lifecycle_package_for_host_version(&base_identity, "0.3.0")
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&root_identity, &root)
        .await
        .unwrap();
    bind_remote_catalog_receipt(&registry, "acme/root", &root_catalog).await;

    registry
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap();
    let error = registry
        .publish_lifecycle_package_graph_for_test_host_version(
            &package_lock,
            std::slice::from_ref(&root_identity),
            "0.3.0",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_package_graph_invalid");
    assert!(!registry.get("acme/root").await.unwrap().unwrap().enabled());

    registry
        .publish_lifecycle_package_for_host_version(&base_identity, "0.3.0")
        .await
        .unwrap();
    let published = registry
        .publish_lifecycle_package_graph_for_test_host_version(
            &package_lock,
            std::slice::from_ref(&root_identity),
            "0.3.0",
        )
        .await
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(published[0].extension.enabled());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("root", "0.3.0")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn lifecycle_hide_drains_accepted_calls_before_exact_idempotent_removal() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 11);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    let lease = registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .unwrap();

    let hidden = registry.hide_lifecycle_package(&identity).await.unwrap();
    assert!(hidden.changed);
    assert!(!hidden.extension.receipt.enabled);
    assert!(registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_none());
    let hide_replay = registry.hide_lifecycle_package(&identity).await.unwrap();
    assert!(!hide_replay.changed);

    let error = registry
        .drain_lifecycle_package(&identity, Duration::from_millis(50))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.drain_timeout");
    drop(lease);

    let drained = registry
        .drain_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!drained.extension.receipt.enabled);
    let drain_replay = registry
        .drain_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        drain_replay.extension.receipt.descriptor_digest().unwrap(),
        drained.extension.receipt.descriptor_digest().unwrap()
    );
    let package_root = drained.extension.receipt.package_root.clone();

    let removed = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(removed.changed);
    assert!(!package_root.exists());
    assert!(registry.get("acme/cognitive").await.unwrap().is_none());
    assert!(registry.snapshot().await.unwrap().routes.is_empty());

    let replay = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!replay.changed);
}

#[tokio::test]
async fn lifecycle_uninstall_rejects_a_dependency_until_dependents_are_removed() {
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
    let base_identity = lifecycle_identity(&base, 21);
    let root_identity = lifecycle_identity(&root, 22);
    let registry = registry(temp.path());
    for (identity, candidate) in [(&base_identity, &base), (&root_identity, &root)] {
        registry
            .commit_lifecycle_package(identity, candidate)
            .await
            .unwrap();
        registry
            .publish_lifecycle_package_for_host_version(identity, "0.3.0")
            .await
            .unwrap();
    }

    assert_eq!(
        registry.dependent_packages("acme/base").await.unwrap(),
        ["acme/root"]
    );
    registry
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap();
    let error = registry
        .remove_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_required");
    assert_eq!(
        error.details["requiredBy"],
        serde_json::json!(["acme/root"])
    );
    assert!(registry.get("acme/base").await.unwrap().is_some());

    registry
        .hide_lifecycle_package(&root_identity)
        .await
        .unwrap();
    registry
        .remove_lifecycle_package(&root_identity, Duration::from_secs(1))
        .await
        .unwrap();
    registry
        .remove_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn verified_catalog_dependencies_must_match_the_admitted_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("knowledge");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    crate::package::copy_package(&fixture, &source)
        .await
        .unwrap();
    let manifest_path = source.join(MANIFEST_NAME);
    let manifest = fs::read_to_string(&manifest_path).await.unwrap();
    let manifest = manifest.replace(
        "  repository {",
        "  dependency \"acme/base\" {\n    version = \"^1.0.0\"\n  }\n\n  repository {",
    );
    fs::write(&manifest_path, manifest).await.unwrap();

    let (manifest, manifest_bytes) = read_manifest(&source).await.unwrap();
    let package_digest = package_sha256(&source).await.unwrap();
    let manifest_digest = sha256(&manifest_bytes);
    let mut catalog = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    catalog.target = "any".to_string();
    catalog.archive.target_name = catalog.archive.target_name.replace("linux-x86_64", "any");
    catalog.package.sha256 = Some(format!("sha256:{package_digest}"));
    catalog.package.manifest_sha256 = Some(format!("sha256:{manifest_digest}"));
    catalog.validate().unwrap();
    let verified = VerifiedPluginCatalogRecord::new(
        catalog.clone(),
        a3s_use_core::VerifiedCatalogProvenance {
            registry_name: "fixture".to_string(),
            registry_url: "https://packages.example.test/catalog/".to_string(),
            root_sha256: format!("sha256:{}", "a".repeat(64)),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest: catalog.descriptor_digest().unwrap(),
        },
    )
    .unwrap();
    let resolved = ResolvedRemotePackage::from_verified_catalog(&verified).unwrap();

    let error = validate_catalog_binding(
        &verified,
        Some(&resolved),
        &manifest,
        &manifest_digest,
        &package_digest,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_package_mismatch");
    assert!(error.message.contains("dependency graph"));
}

#[test]
fn verified_catalog_flow_inventory_and_dependencies_match_the_admitted_manifest() {
    let manifest = ExtensionManifest::parse_acl(include_str!(
        "../../fixtures/packages/plugin-v3-cognitive/package/a3s-use-extension.acl"
    ))
    .unwrap();
    let graph = manifest.plugin_surfaces().unwrap();
    let mut record = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    record.surfaces = graph
        .iter()
        .map(|surface| a3s_use_core::CatalogSurface {
            kind: surface.surface.kind,
            id: surface.surface.id.clone(),
            optional: surface.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: manifest
                .okf
                .iter()
                .find(|okf| {
                    surface.surface.kind == PluginSurfaceKind::Okf && okf.id == surface.surface.id
                })
                .map(|okf| okf.bundle.clone()),
            requires: surface.dependencies.clone(),
        })
        .collect();

    validate_surface_catalog_binding(&record, &manifest).unwrap();

    record
        .surfaces
        .iter_mut()
        .find(|surface| surface.kind == PluginSurfaceKind::Flow)
        .unwrap()
        .requires
        .clear();
    let error = validate_surface_catalog_binding(&record, &manifest).unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_package_mismatch");
    assert!(error.message.contains("surface dependency graph"));

    record
        .surfaces
        .retain(|surface| surface.kind != PluginSurfaceKind::Flow);
    let error = validate_surface_catalog_binding(&record, &manifest).unwrap_err();
    assert!(error.message.contains("surface inventory"));
}

#[tokio::test]
async fn lifecycle_generation_binding_fails_closed_and_snapshot_repairs_tampered_projection() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 13);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();

    registry.snapshot().await.unwrap();
    let snapshot_path = registry.paths().registry_snapshot_path();
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).await.unwrap()).unwrap();
    snapshot["routes"][0]["lifecycleGeneration"] = serde_json::json!(99);
    fs::write(
        &snapshot_path,
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    let repaired = registry.snapshot().await.unwrap();
    assert_eq!(repaired.routes[0].lifecycle_generation, Some(13));

    let receipt_path = registry.paths().receipt_path("acme/cognitive");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).await.unwrap()).unwrap();
    receipt["lifecycleGeneration"] = serde_json::json!(14);
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
        .await
        .unwrap();
    let error = registry.get("acme/cognitive").await.unwrap_err();
    assert!(matches!(
        error.code.as_str(),
        "use.extension.lifecycle_receipt_invalid" | "use.extension.ownership_invalid"
    ));
}

#[tokio::test]
async fn lifecycle_commit_repairs_crashes_after_root_or_receipt_commit() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 15);
    let registry = registry(temp.path());
    let target = registry.lifecycle_package_root(&identity);

    // Model a crash after the deterministic immutable root was committed but
    // before the authoritative receipt was written.
    crate::package::copy_package(&source, &target)
        .await
        .unwrap();
    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    assert_eq!(committed.extension.receipt.package_root, target);

    // Model a second crash after receipt replacement but before snapshot
    // publication. Replaying the same checkpoint repairs only the projection.
    registry.snapshot().await.unwrap();
    let snapshot_path = registry.paths().registry_snapshot_path();
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).await.unwrap()).unwrap();
    snapshot["routes"] = serde_json::json!([]);
    fs::write(
        &snapshot_path,
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    let replay = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(registry.snapshot().await.unwrap().routes.len(), 1);
}

#[tokio::test]
async fn lifecycle_upgrade_retains_routes_until_cutover_and_retires_the_exact_prior_generation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let first = lifecycle_identity(&candidate, 17);
    let next = lifecycle_identity(&candidate, 18);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&first, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&first, "0.3.0")
        .await
        .unwrap();
    let old_lease = registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_lease.extension().receipt.lifecycle_generation, Some(17));

    let committed = registry
        .commit_lifecycle_package(&next, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    let replayed = registry
        .commit_lifecycle_package(&next, &candidate)
        .await
        .unwrap();
    assert!(!replayed.changed);
    assert_eq!(committed.extension.receipt.lifecycle_generation, Some(18));
    assert!(!committed.extension.receipt.enabled);
    assert_eq!(
        registry
            .get_lifecycle_generation(&first)
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation,
        Some(17)
    );
    let staged_snapshot = registry.snapshot().await.unwrap();
    let staged_binding = registry
        .get_snapshot_binding(&staged_snapshot.routes[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(staged_binding.receipt.lifecycle_generation, Some(17));
    assert!(staged_binding.receipt.enabled);
    assert_eq!(
        registry
            .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
            .await
            .unwrap()
            .unwrap()
            .extension()
            .receipt
            .lifecycle_generation,
        Some(17)
    );

    registry
        .publish_lifecycle_package_for_host_version(&next, "0.3.0")
        .await
        .unwrap();
    assert_eq!(
        registry
            .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
            .await
            .unwrap()
            .unwrap()
            .extension()
            .receipt
            .lifecycle_generation,
        Some(18)
    );

    registry.hide_lifecycle_package(&first).await.unwrap();
    let error = registry
        .drain_lifecycle_package(&first, Duration::from_millis(1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.drain_timeout");
    drop(old_lease);
    registry
        .drain_lifecycle_package(&first, Duration::from_secs(1))
        .await
        .unwrap();
    registry
        .remove_lifecycle_package(&first, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(registry
        .get_lifecycle_generation(&first)
        .await
        .unwrap()
        .is_none());
    assert!(!registry.lifecycle_package_root(&first).exists());
    assert_eq!(
        registry
            .get_lifecycle_generation(&next)
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation,
        Some(18)
    );
}

#[tokio::test]
async fn lifecycle_upgrade_candidate_can_roll_back_before_capability_cutover() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let first = lifecycle_identity(&candidate, 21);
    let next = lifecycle_identity(&candidate, 22);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&first, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&first, "0.3.0")
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&next, &candidate)
        .await
        .unwrap();

    registry
        .rollback_lifecycle_package(&next, &first)
        .await
        .unwrap();
    registry
        .rollback_lifecycle_package(&next, &first)
        .await
        .unwrap();

    assert!(registry
        .get_lifecycle_generation(&next)
        .await
        .unwrap()
        .is_none());
    assert!(!registry.lifecycle_package_root(&next).exists());
    assert_eq!(
        registry
            .get("acme/cognitive")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation,
        Some(21)
    );
    assert_eq!(
        registry.snapshot().await.unwrap().routes[0].lifecycle_generation,
        Some(21)
    );
}

#[tokio::test]
async fn lifecycle_graph_rollback_atomically_restores_replacements_and_discards_additions() {
    let temp = tempfile::tempdir().unwrap();
    let root_source = temp.path().join("root");
    let added_source = temp.path().join("added");
    cognitive_package_with_dependencies(&root_source, "acme/root", "root", &[]).await;
    cognitive_package_with_dependencies(&added_source, "acme/added", "added", &[]).await;
    let root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let added = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/added",
        &added_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let prior = lifecycle_identity(&root, 41);
    let replacement = lifecycle_identity(&root, 42);
    let addition = lifecycle_identity(&added, 43);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&prior, &root)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&prior, "0.3.0")
        .await
        .unwrap();
    let published_before = registry.snapshot().await.unwrap();

    registry
        .commit_lifecycle_package(&replacement, &root)
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&addition, &added)
        .await
        .unwrap();
    let staged = registry.snapshot().await.unwrap();
    assert_eq!(staged, published_before);
    assert_eq!(staged.routes.len(), 1);
    assert_eq!(staged.routes[0].lifecycle_generation, Some(41));

    let results = registry
        .rollback_lifecycle_package_graph(
            &[addition.clone(), replacement.clone()],
            std::slice::from_ref(&prior),
        )
        .await
        .unwrap();
    assert_eq!(
        results
            .iter()
            .map(|result| result.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/added", "acme/root"]
    );
    assert!(results.iter().all(|result| result.changed));
    let restored = registry.snapshot().await.unwrap();
    assert_eq!(restored.routes.len(), 1);
    assert_eq!(restored.routes[0].lifecycle_generation, Some(41));
    assert!(restored.routes[0].enabled);
    assert_eq!(
        registry
            .get("acme/root")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation,
        Some(41)
    );
    assert!(registry.get("acme/added").await.unwrap().is_none());
    assert!(registry
        .get_lifecycle_generation(&replacement)
        .await
        .unwrap()
        .is_none());
    assert!(!registry.lifecycle_package_root(&replacement).exists());
    assert!(!registry.lifecycle_package_root(&addition).exists());

    let replay = registry
        .rollback_lifecycle_package_graph(
            &[addition.clone(), replacement.clone()],
            std::slice::from_ref(&prior),
        )
        .await
        .unwrap();
    assert!(replay.iter().all(|result| !result.changed));
    assert!(replay
        .iter()
        .all(|result| result.registry_generation == restored.generation));
    assert_eq!(registry.snapshot().await.unwrap(), restored);
}

#[tokio::test]
async fn public_lifecycle_candidate_accepts_the_real_v3_host_version() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/packages/plugin-v3-cognitive/package");
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/cognitive", &fixture, true)
        .await
        .unwrap();
    assert_eq!(candidate.package_id(), "acme/cognitive");
}
