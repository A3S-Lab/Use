use super::*;

#[tokio::test]
async fn registry_tuf_receipts_require_verified_catalog_evidence() {
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
    let identity = lifecycle_identity(&candidate, 6);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();

    let catalog = verified_knowledge_catalog(&source, "acme/cognitive", &[], 'c').await;
    let mut receipt = registry
        .get("acme/cognitive")
        .await
        .unwrap()
        .unwrap()
        .receipt;
    receipt.trust = ExtensionTrust::RegistryTuf;
    receipt.registry = Some(ResolvedRemotePackage::from_verified_catalog(&catalog).unwrap());
    receipt.verified_catalog = None;
    write_receipt(&registry.paths().receipt_path("acme/cognitive"), &receipt)
        .await
        .unwrap();

    let error = registry.get("acme/cognitive").await.unwrap_err();
    assert_eq!(error.code, "use.extension.receipt_invalid");
    assert!(error.message.contains("inconsistent trust provenance"));
}

#[tokio::test]
async fn lifecycle_receipts_require_exact_selected_surface_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let manifest_path = source.join(MANIFEST_NAME);
    let manifest = fs::read_to_string(&manifest_path).await.unwrap();
    let manifest = manifest.replace(
        "    bind_flow = [\"reason\"]\n    optional  = false",
        "    bind_flow = [\"reason\"]\n    optional  = true",
    );
    fs::write(&manifest_path, &manifest).await.unwrap();
    let manifest = ExtensionManifest::parse_acl(&manifest).unwrap();
    let mut selected_surfaces = manifest
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface)
        .filter(|surface| surface.kind != PluginSurfaceKind::Ui)
        .collect::<Vec<_>>();
    selected_surfaces.sort();
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 6);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package_selection(&identity, &candidate, &selected_surfaces)
        .await
        .unwrap();
    let installed = registry.get("acme/cognitive").await.unwrap().unwrap();
    assert_eq!(
        installed.receipt.schema_version,
        EXTENSION_RECEIPT_SCHEMA_VERSION
    );
    assert_eq!(installed.selected_surfaces().unwrap(), selected_surfaces);
    assert!(!installed.surfaces().contains(&"ui"));

    let receipt_path = registry.paths().receipt_path("acme/cognitive");
    let current: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).await.unwrap()).unwrap();
    for (case, receipt, expected_code) in [
        {
            let mut receipt = current.clone();
            receipt.as_object_mut().unwrap().remove("selectedSurfaces");
            (
                "missing selection",
                receipt,
                "use.extension.receipt_invalid",
            )
        },
        {
            let mut receipt = current.clone();
            receipt["selectedSurfaces"] = serde_json::json!([]);
            ("empty selection", receipt, "use.extension.receipt_invalid")
        },
        {
            let mut receipt = current.clone();
            receipt["schemaVersion"] = serde_json::json!(EXTENSION_RECEIPT_SCHEMA_VERSION - 1);
            (
                "superseded schema",
                receipt,
                "use.extension.receipt_incompatible",
            )
        },
        {
            let mut receipt = current.clone();
            receipt["unexpectedAuthority"] = serde_json::json!(true);
            ("unknown field", receipt, "use.extension.receipt_invalid")
        },
        {
            let mut receipt = current.clone();
            receipt["selectedSurfaces"]
                .as_array_mut()
                .unwrap()
                .reverse();
            (
                "non-canonical selection",
                receipt,
                "use.extension.receipt_invalid",
            )
        },
    ] {
        fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
            .await
            .unwrap();
        let error = registry.get("acme/cognitive").await.unwrap_err();
        assert_eq!(error.code, expected_code, "unexpected error for {case}");
    }
}

#[tokio::test]
async fn lifecycle_snapshot_cannot_steal_the_reviewed_atomic_cutover() {
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
    let before = registry.snapshot().await.unwrap();

    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    assert_eq!(
        committed.extension.receipt.schema_version,
        EXTENSION_RECEIPT_SCHEMA_VERSION
    );
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

    // This is the deterministic watcher race: a capability observer runs
    // after package staging but before the lifecycle coordinator publishes
    // the reviewed cutover. Staging must not consume a Registry generation.
    let staged_observation = registry.snapshot().await.unwrap();
    assert_eq!(staged_observation, before);
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
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

    let cutover_key = format!("sha256:{}", "7".repeat(64));
    let publication = registry
        .publish_lifecycle_package_with_durable_cutover(&identity, &cutover_key)
        .await
        .unwrap();
    assert_eq!(publication.registry_generation, before.generation + 1);
    let published = &publication.packages[0];
    assert!(published.changed);
    assert!(published.extension.receipt.enabled);
    assert_eq!(published.extension.receipt.lifecycle_generation, Some(7));
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_some());

    let replay = registry
        .publish_lifecycle_package_with_durable_cutover(&identity, &cutover_key)
        .await
        .unwrap();
    assert_eq!(replay.registry_generation, publication.registry_generation);
    assert!(!replay.packages[0].changed);
    assert_eq!(
        replay.packages[0]
            .extension
            .receipt
            .descriptor_digest()
            .unwrap(),
        published.extension.receipt.descriptor_digest().unwrap()
    );
}

#[tokio::test]
async fn lifecycle_snapshot_does_not_reconcile_during_state_restore_maintenance() {
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
    let identity = lifecycle_identity(&candidate, 8);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry.publish_lifecycle_package(&identity).await.unwrap();

    let snapshot_path = registry.paths().registry_snapshot_path();
    let mut stale = registry.snapshot().await.unwrap();
    stale.packages[0].surfaces.pop();
    crate::registry_io::write_registry_snapshot(registry.paths(), &stale)
        .await
        .unwrap();

    let maintenance =
        crate::state_maintenance::StateMaintenanceLock::new(registry.paths().state_root())
            .acquire_exclusive()
            .await
            .unwrap();
    let bytes_before = fs::read(&snapshot_path).await.unwrap();
    let observed = registry.snapshot().await.unwrap();
    let bytes_after = fs::read(&snapshot_path).await.unwrap();
    assert_eq!(observed, stale);
    assert_eq!(bytes_after, bytes_before);

    drop(maintenance);
    fs::write(
        registry
            .paths()
            .state_root()
            .join(crate::ACTIVE_STATE_RESTORE_MARKER),
        b"active restore",
    )
    .await
    .unwrap();
    let observed = registry.snapshot().await.unwrap();
    assert_eq!(observed, stale);
    assert_eq!(fs::read(&snapshot_path).await.unwrap(), bytes_before);
    fs::remove_file(
        registry
            .paths()
            .state_root()
            .join(crate::ACTIVE_STATE_RESTORE_MARKER),
    )
    .await
    .unwrap();

    let repaired = registry.snapshot().await.unwrap();
    assert_eq!(repaired.generation, stale.generation + 1);
    assert_ne!(repaired.packages, stale.packages);
}

#[tokio::test]
async fn lifecycle_receipt_persists_the_exact_selected_surface_closure() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("selected-surfaces");
    compatible_cognitive_package(&source).await;
    let manifest_path = source.join(MANIFEST_NAME);
    let manifest = fs::read_to_string(&manifest_path).await.unwrap();
    let manifest = manifest.replace("    optional  = false", "    optional  = true");
    fs::write(&manifest_path, manifest).await.unwrap();

    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 8);
    let mut selected_surfaces = candidate
        .manifest()
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface)
        .filter(|surface| surface.kind != PluginSurfaceKind::Ui)
        .collect::<Vec<_>>();
    selected_surfaces.sort();

    let registry = registry(temp.path());
    let committed = registry
        .commit_lifecycle_package_selection(&identity, &candidate, &selected_surfaces)
        .await
        .unwrap();

    assert_eq!(
        committed.extension.receipt.selected_surfaces,
        selected_surfaces
    );
    assert_eq!(
        committed.extension.selected_surfaces().unwrap(),
        selected_surfaces
    );
    assert_eq!(
        committed.extension.surfaces(),
        ["tool", "mcp", "okf", "flow", "skill"]
    );

    let replay = registry
        .commit_lifecycle_package_selection(&identity, &candidate, &selected_surfaces)
        .await
        .unwrap();
    assert!(!replay.changed);
    let error = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_state_invalid");
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
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&root_identity, &root)
        .await
        .unwrap();
    bind_remote_catalog_receipt(&registry, "acme/root", &root_catalog).await;
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
        .acquire_lifecycle_alias_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("root", "0.3.0")
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
        .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .unwrap();

    let hidden = registry.hide_lifecycle_package(&identity).await.unwrap();
    assert!(hidden.changed);
    assert!(!hidden.extension.receipt.enabled);
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
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
    assert!(package_root.is_dir());
    assert!(registry.get("acme/cognitive").await.unwrap().is_none());
    assert!(registry.snapshot().await.unwrap().packages.is_empty());

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
    let error = registry
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_required");
    assert_eq!(
        error.details["requiredBy"],
        serde_json::json!(["acme/root"])
    );
    assert!(registry.get("acme/base").await.unwrap().unwrap().enabled());

    registry
        .hide_lifecycle_package(&root_identity)
        .await
        .unwrap();
    registry
        .remove_lifecycle_package(&root_identity, Duration::from_secs(1))
        .await
        .unwrap();
    registry
        .hide_lifecycle_package(&base_identity)
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
async fn lifecycle_generation_binding_fails_closed_at_the_publication_boundary() {
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
    registry.publish_lifecycle_package(&identity).await.unwrap();

    let snapshot_path = registry.paths().registry_snapshot_path();
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).await.unwrap()).unwrap();
    snapshot["packages"][0]["lifecycleGeneration"] = serde_json::json!(99);
    fs::write(
        &snapshot_path,
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    let observed = registry.snapshot().await.unwrap();
    assert_eq!(observed.packages[0].lifecycle_generation, Some(99));
    assert!(registry
        .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_none());

    let receipt_path = registry.paths().receipt_path("acme/cognitive");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).await.unwrap()).unwrap();
    receipt["lifecycleGeneration"] = serde_json::json!(14);
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
        .await
        .unwrap();
    let selected = registry.get("acme/cognitive").await.unwrap().unwrap();
    assert_eq!(selected.receipt.lifecycle_generation, Some(14));
    assert!(registry
        .get_snapshot_binding(&observed.packages[0])
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn lifecycle_commit_replay_defers_receipt_cutover_to_the_journal() {
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
    // publication. Replaying the package checkpoint restores staged state,
    // while only the lifecycle journal may publish the reviewed cutover.
    assert!(registry.snapshot().await.unwrap().packages.is_empty());
    let replay = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert!(registry.snapshot().await.unwrap().packages.is_empty());

    let cutover_key = format!("sha256:{}", "f".repeat(64));
    let publication = registry
        .publish_lifecycle_package_with_durable_cutover(&identity, &cutover_key)
        .await
        .unwrap();
    assert_eq!(publication.registry_generation, 1);
    assert_eq!(registry.snapshot().await.unwrap().packages.len(), 1);
}

#[tokio::test]
async fn lifecycle_upgrade_retains_packages_until_cutover_and_retires_the_exact_prior_generation() {
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
        .acquire_published_lifecycle_generation(&first)
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
        .get_snapshot_binding(&staged_snapshot.packages[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(staged_binding.receipt.lifecycle_generation, Some(17));
    assert!(staged_binding.receipt.enabled);
    assert_eq!(
        registry
            .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
            .await
            .unwrap()
            .unwrap()
            .extension()
            .receipt
            .lifecycle_generation,
        Some(17)
    );

    let error = registry
        .retire_hidden_lifecycle_package(&first)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_state_invalid");
    assert!(error.message.contains("atomic graph cutover"));

    registry
        .publish_lifecycle_package_for_host_version(&next, "0.3.0")
        .await
        .unwrap();
    assert!(registry
        .acquire_published_lifecycle_generation(&first)
        .await
        .unwrap()
        .is_none());
    let next_lease = registry
        .acquire_published_lifecycle_generation(&next)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next_lease.extension().receipt.lifecycle_generation,
        Some(18)
    );
    drop(next_lease);
    assert_eq!(
        registry
            .acquire_lifecycle_alias_for_host_version("cognitive", "0.3.0")
            .await
            .unwrap()
            .unwrap()
            .extension()
            .receipt
            .lifecycle_generation,
        Some(18)
    );

    let retired = registry
        .retire_hidden_lifecycle_package(&first)
        .await
        .unwrap();
    assert!(retired.changed);
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
    assert!(registry.lifecycle_package_root(&first).is_dir());
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
    assert!(registry.lifecycle_package_root(&next).is_dir());
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
        registry.snapshot().await.unwrap().packages[0].lifecycle_generation,
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
    assert_eq!(staged.packages.len(), 1);
    assert_eq!(staged.packages[0].lifecycle_generation, Some(41));

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
    assert_eq!(restored.packages.len(), 1);
    assert_eq!(restored.packages[0].lifecycle_generation, Some(41));
    assert!(restored.packages[0].enabled);
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
    assert!(registry.lifecycle_package_root(&replacement).is_dir());
    assert!(registry.lifecycle_package_root(&addition).is_dir());

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
