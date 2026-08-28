use super::*;

#[tokio::test]
async fn host_manager_fetches_only_media_bound_to_verified_presentation() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let package_target = cognitive_okf_target(
        temporary.path(),
        "1.0.0",
        "Verified presentation media stays inside the Host trust boundary.",
        &target,
    );
    let record: PluginCatalogRecord = serde_json::from_value(
        package_target
            .custom
            .clone()
            .expect("catalog custom metadata"),
    )
    .unwrap();
    let media_name = "cognitive/media/acme-knowledge-cover.png";
    let media = b"verified-host-media".to_vec();
    let media_digest = format!("sha256:{:x}", Sha256::digest(&media));
    let descriptor_name = "cognitive/descriptors/acme-knowledge-1.0.0-en.json";
    let descriptor = CognitivePackagePresentationV1 {
        schema: COGNITIVE_PACKAGE_PRESENTATION_SCHEMA.to_owned(),
        package_id: record.package_id.clone(),
        locale: "en".to_owned(),
        short_title: "Knowledge Builder".to_owned(),
        short_summary: "Verified cognition for one exact workspace task.".to_owned(),
        form_factors: vec![CognitivePackageFormFactor::Desktop],
        media: vec![CognitivePackagePresentationMediaV1 {
            kind: CognitivePackageMediaKind::Image,
            target_name: media_name.to_owned(),
            sha256: media_digest,
            media_type: "image/png".to_owned(),
            width: 1280,
            height: 720,
            byte_length: media.len() as u64,
            alt: "Knowledge Builder preview".to_owned(),
        }],
        accent: Some("#8a7cff".to_owned()),
    };
    let descriptor = serde_json::to_vec(&descriptor).unwrap();
    let descriptor_digest = format!("sha256:{:x}", Sha256::digest(&descriptor));
    let index = serde_json::to_vec(&CognitivePackagePresentationIndexV1 {
        schema: COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA.to_owned(),
        entries: vec![CognitivePackagePresentationRecordV1 {
            package_id: record.package_id.clone(),
            version: record.version.clone(),
            channel: record.channel.as_str().to_owned(),
            host_target: target,
            catalog_record_digest: record.descriptor_digest().unwrap(),
            descriptor_target_name: descriptor_name.to_owned(),
            descriptor_sha256: descriptor_digest,
            descriptor_byte_length: descriptor.len() as u64,
        }],
    })
    .unwrap();
    let repository = TestRepository::with_targets(
        vec![
            package_target,
            TestTarget::with_signed_custom(
                "cognitive/presentation-index-v1.json",
                index,
                serde_json::json!({
                    "a3sCognitivePresentationIndex": {
                        "schema": COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA
                    }
                }),
            ),
            TestTarget::raw(descriptor_name, descriptor),
            TestTarget::raw(media_name, media.clone()),
        ],
        67,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("presentation-host-home");
    RegistrySourceStore::new(use_paths(&home))
        .add(RegistrySourceInput::new(
            "fixture",
            server.base_url(),
            &repository.root_sha256,
            None,
            VerifiedTargetCachePolicy::default(),
        ))
        .await
        .unwrap();
    let scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_string(),
        host_id: "host:workbaby".to_string(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_string(),
        authority_id: "workbaby:user".to_string(),
        fence_generation: 11,
        fence_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let paths = managed_extension_paths(&home, &scope);
    let host = CognitivePackageHostManager::new(
        scope,
        "use:presentation-test",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let searched = host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "knowledge".to_string(),
                kind: Some(PluginSurfaceKind::Okf),
                channel: Some(PluginReleaseChannel::Stable),
                publisher: None,
                category: None,
                availability: None,
                cursor: None,
                limit: 20,
            },
        )
        .await
        .unwrap();
    let presentation = host
        .inspect_cognitive_package_presentation(
            CognitiveRegistryAccess::Refreshed,
            &searched.plugins[0],
        )
        .await
        .unwrap()
        .unwrap();
    let verified = host
        .fetch_cognitive_package_media(
            CognitiveRegistryAccess::Refreshed,
            &presentation,
            media_name,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(verified.path()).unwrap(), media);
    let error = host
        .fetch_cognitive_package_media(
            CognitiveRegistryAccess::Refreshed,
            &presentation,
            "cognitive/media/not-declared.png",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.presentation_invalid");
}

#[tokio::test]
async fn embedded_catalog_plan_apply_and_workspace_okf_lease_are_exact() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        vec![cognitive_okf_target(
            temporary.path(),
            "1.0.0",
            "Embedded workspace cognition stays bound to its exact package generation.",
            &target,
        )],
        67,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("embedded-host-home");
    let sources = RegistrySourceStore::new(use_paths(&home));
    sources
        .add(RegistrySourceInput::new(
            "fixture",
            server.base_url(),
            &repository.root_sha256,
            None,
            VerifiedTargetCachePolicy::default(),
        ))
        .await
        .unwrap();
    let scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_string(),
        host_id: "host:workbaby".to_string(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_string(),
        authority_id: "workbaby:user".to_string(),
        fence_generation: 11,
        fence_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let paths = managed_extension_paths(&home, &scope);
    let host = CognitivePackageHostManager::new(
        scope.clone(),
        "use:embedded-test",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let searched = host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "knowledge".to_string(),
                kind: Some(PluginSurfaceKind::Okf),
                channel: Some(PluginReleaseChannel::Stable),
                publisher: None,
                category: None,
                availability: None,
                cursor: None,
                limit: 20,
            },
        )
        .await
        .unwrap();
    assert_eq!(searched.plugins.len(), 1);
    assert!(searched.source_revision.starts_with("sha256:"));
    let candidate = searched.plugins[0].clone();
    let inspection = host
        .inspect_cognitive_package(CognitiveRegistryAccess::Refreshed, &candidate)
        .await
        .unwrap();
    assert_eq!(inspection.plugin, candidate);
    let package_lock = host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
        .await
        .unwrap();

    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let selected_surfaces = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Okf,
        id: "domain-knowledge".to_string(),
    }];
    let plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_string(),
        request_id: "plan:knowledge:embedded".to_string(),
        assignment_generation: 5,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/knowledge".to_string()).unwrap(),
        candidate: Some(candidate),
        package_lock: Some(package_lock),
        selected_surfaces,
    };
    let planned = host.plan(plan_request.clone()).await.unwrap();
    let applied = host
        .apply(PluginHostApplyRequest {
            schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_string(),
            request_id: "apply:knowledge:embedded".to_string(),
            assignment_generation: plan_request.assignment_generation,
            capabilities_digest,
            scope: scope.clone(),
            package_id: plan_request.package_id,
            operation_id: planned.plan.plan.operation_id.clone(),
            plan_digest: planned.plan.plan_digest.clone(),
            confirmation: Some(PluginOperationConfirmation {
                schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
                operation_id: planned.plan.plan.operation_id.clone(),
                plan_digest: planned.plan.plan_digest.clone(),
                confirmed_by: PlanActor::User,
                confirmed_at_ms: planned.plan.plan.created_at_ms + 1,
            }),
        })
        .await
        .unwrap();
    assert_eq!(applied.state.desired, PluginDesiredState::Enabled);

    let lease = host
        .acquire_cognitive_capability(&scope, "acme/knowledge", "domain-knowledge")
        .await
        .unwrap()
        .unwrap();
    let evidence = lease.evidence();
    assert_eq!(evidence.scope, scope.plan_scope());
    assert_eq!(evidence.lifecycle_generation, 1);
    assert_eq!(evidence.package_version, "1.0.0");
    assert_eq!(
        evidence.capability_generation,
        applied.state.capability_generation
    );
    assert!(evidence.generation_digest.starts_with("sha256:"));
    assert!(evidence.capability_snapshot_digest.starts_with("sha256:"));
    let found = lease
        .knowledge()
        .search("embedded workspace cognition", 4)
        .await
        .unwrap();
    assert!(!found.hits.is_empty());

    let identity = a3s_use_extension::ExtensionLifecycleIdentity::new(
        "acme/knowledge",
        &evidence.package_digest,
        &evidence.manifest_digest,
        evidence.lifecycle_generation,
    )
    .unwrap();
    ExtensionRegistry::new(paths.clone())
        .hide_lifecycle_package(&identity)
        .await
        .unwrap();
    assert!(host
        .acquire_cognitive_capability(&scope, "acme/knowledge", "domain-knowledge")
        .await
        .unwrap()
        .is_none());
    let draining = lease
        .knowledge()
        .search("embedded workspace cognition", 4)
        .await
        .unwrap();
    assert!(!draining.hits.is_empty());

    let mut stale_scope = scope.clone();
    stale_scope.fence_generation -= 1;
    let error = match host
        .acquire_cognitive_capability(&stale_scope, "acme/knowledge", "domain-knowledge")
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("a stale managed fence must not acquire a cognitive capability"),
    };
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    let package_root = paths
        .data_root()
        .join("extensions/acme/knowledge")
        .join(format!(
            "lifecycle-{}-{}",
            evidence.lifecycle_generation,
            evidence.package_digest.strip_prefix("sha256:").unwrap()
        ));
    std::fs::write(package_root.join("README.md"), "drifted after lease\n").unwrap();
    let error = lease
        .knowledge()
        .search("embedded workspace cognition", 4)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_digest_mismatch");
}
