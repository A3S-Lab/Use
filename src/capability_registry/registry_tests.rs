//! Capability registry unit tests.

use super::*;

#[cfg(feature = "extensions")]
const SKILL_ONLY_PLUGIN: &str = r#"
extension "acme/guide" {
  schema_version = 3
  version        = "1.0.0"
  route          = "guide"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/guide"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  skill "guide" {
    path          = "skills/guide/SKILL.md"
    requires_tool = []
    requires_mcp  = []
    optional      = false
  }
}
"#;

#[cfg(feature = "extensions")]
const SKILL_UI_PLUGIN: &str = r#"
extension "acme/workbench" {
  schema_version = 3
  version        = "1.0.0"
  route          = "workbench"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/workbench"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  skill "guide" {
    path          = "skills/guide/SKILL.md"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }

  ui "review" {
    title       = "Evidence Review"
    description = "Review the cognitive package evidence."
    icon        = "flask-conical"
    order       = 80
    entry        = "ui/review.html"
    styles       = ["ui/review.css"]
    scripts      = ["ui/review.js"]
    skill        = "guide"
    bind_tool    = []
    bind_mcp     = []
    optional     = false
  }

  ui "standalone" {
    entry     = "ui/standalone.html"
    styles    = []
    scripts   = []
    bind_tool = []
    bind_mcp  = []
    optional  = false
  }
}
"#;

#[cfg(feature = "extensions")]
const FLOW_PLUGIN: &str = r#"
extension "acme/workflow" {
  schema_version = 3
  version        = "1.0.0"
  route          = "workflow"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/workflow"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  flow "review" {
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/review.ts"
    export        = "run"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }
}
"#;

#[cfg(feature = "extensions")]
fn installed_extension(
    manifest: a3s_use_extension::ExtensionManifest,
    package_root: PathBuf,
    enabled: bool,
) -> a3s_use_extension::InstalledExtension {
    let mut selected_surfaces = manifest
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface)
        .collect::<Vec<_>>();
    selected_surfaces.sort();
    let receipt = a3s_use_extension::ExtensionReceipt {
        schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
        installation: crate::test_installation(),
        package_id: manifest.package_id.clone(),
        component_id: format!("use/{}", manifest.package_id),
        route_alias: manifest.route_alias.clone(),
        version: manifest.version.clone(),
        package_root,
        manifest_sha256: "0".repeat(64),
        package_sha256: Some("0".repeat(64)),
        trust: a3s_use_extension::ExtensionTrust::LocalExplicit,
        registry: None,
        verified_catalog: None,
        planning_bundle: None,
        selected_surfaces,
        installed_at_unix: 0,
        enabled,
        lifecycle_generation: Some(1),
    };
    a3s_use_extension::InstalledExtension { receipt, manifest }
}

#[tokio::test]
async fn universal_engine_registry_projects_no_hardcoded_first_party_domains() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::User,
        "capability-seed-engine",
    )
    .unwrap();
    #[cfg(feature = "extensions")]
    let registry = {
        let paths = a3s_use_extension::ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        crate::cognitive_package::open_control_lifecycle(
            &paths,
            Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        CapabilityRegistry::new(a3s_use_extension::ExtensionRegistry::new(paths))
    };
    #[cfg(not(feature = "extensions"))]
    let registry = CapabilityRegistry::new(installation).unwrap();
    let snapshot = registry.snapshot().await.unwrap();
    assert!(
        snapshot.capabilities.iter().all(|capability| {
            !matches!(
                capability.id.as_str(),
                "use/browser" | "use/ocr" | "use/box"
            )
        }),
        "bare CapabilityRegistry::new must not hardcode Browser/OCR/Box domains"
    );
    assert!(snapshot
        .capabilities
        .iter()
        .all(|capability| capability.origin != CapabilityOrigin::BuiltIn));
}

#[tokio::test]
async fn bundled_product_profile_projects_browser_ocr_box_as_injected_seeds() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::User,
        "capability-seed-product",
    )
    .unwrap();
    #[cfg(feature = "extensions")]
    let registry = {
        let paths = a3s_use_extension::ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation.clone(),
        )
        .unwrap();
        crate::cognitive_package::open_control_lifecycle(
            &paths,
            Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        CapabilityRegistry::with_seeds(
            a3s_use_extension::ExtensionRegistry::new(paths),
            Arc::new(BundledFirstPartyCapabilitySeeds),
        )
    };
    #[cfg(not(feature = "extensions"))]
    let registry = CapabilityRegistry::with_seeds(
        installation.clone(),
        Arc::new(BundledFirstPartyCapabilitySeeds),
    )
    .unwrap();
    let snapshot = registry.snapshot().await.unwrap();
    assert_eq!(snapshot.schema_version, 5);
    assert_eq!(snapshot.installation, installation);
    assert!(snapshot.installation_generation.is_none());
    assert!(snapshot.installation_snapshot_digest.is_none());
    let browser = snapshot
        .capabilities
        .iter()
        .find(|capability| capability.id == "use/browser")
        .unwrap();
    let ocr = snapshot
        .capabilities
        .iter()
        .find(|capability| capability.id == "use/ocr")
        .unwrap();
    let boxed = snapshot
        .capabilities
        .iter()
        .find(|capability| capability.id == "use/box")
        .unwrap();

    assert_eq!(browser.origin, CapabilityOrigin::BuiltIn);
    assert_eq!(ocr.origin, CapabilityOrigin::BuiltIn);
    assert_eq!(boxed.origin, CapabilityOrigin::BuiltIn);
    assert!(snapshot
        .capabilities
        .iter()
        .filter(|capability| capability.alias.as_deref() == Some("office"))
        .all(|capability| capability.origin == CapabilityOrigin::Extension));
    #[cfg(feature = "browser")]
    {
        assert!(browser.surfaces.iter().any(|surface| surface == "skill"));
        assert!(browser
            .skills
            .iter()
            .any(|skill| skill.path.ends_with("a3s-use-browser/SKILL.md")));
        assert!(browser.skills.iter().all(|skill| skill.sha256.len() == 64));
    }
    #[cfg(not(feature = "browser"))]
    {
        assert!(!browser.enabled);
        assert!(browser.surfaces.is_empty());
        assert!(browser.skills.is_empty());
    }
    #[cfg(feature = "ocr")]
    {
        assert!(ocr.enabled);
        assert!(ocr.surfaces.iter().any(|surface| surface == "skill"));
        assert!(ocr
            .skills
            .iter()
            .any(|skill| skill.path.ends_with("a3s-use-ocr/SKILL.md")));
        assert!(ocr.skills.iter().all(|skill| skill.sha256.len() == 64));
        #[cfg(feature = "mcp")]
        assert_eq!(
            ocr.mcp.as_ref().map(|surface| surface.target.as_str()),
            Some("ocr-native")
        );
    }
    #[cfg(not(feature = "ocr"))]
    {
        assert!(!ocr.enabled);
        assert!(ocr.surfaces.is_empty());
        assert!(ocr.skills.is_empty());
    }
    assert_eq!(snapshot.revision.len(), 64);
}

#[tokio::test]
async fn skill_content_changes_revision_without_changing_its_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("SKILL.md");
    tokio::fs::write(&path, b"first").await.unwrap();
    let first = skill_surface("guide", path.clone()).await.unwrap();
    tokio::fs::write(&path, b"second").await.unwrap();
    let second = skill_surface("guide", path).await.unwrap();
    assert_ne!(first.sha256, second.sha256);

    let mut capability = product_seeds::box_capability();
    capability.skills = vec![first];
    let first_revision = revision(
        &crate::test_installation(),
        None,
        None,
        &[capability.clone()],
    )
    .unwrap();
    capability.skills = vec![second];
    let second_revision = revision(&crate::test_installation(), None, None, &[capability]).unwrap();
    assert_ne!(first_revision, second_revision);
}

#[test]
fn installation_snapshot_evidence_is_part_of_the_capability_revision() {
    let capability = product_seeds::box_capability();
    let first = revision(
        &crate::test_installation(),
        Some(4),
        Some(&format!("sha256:{}", "a".repeat(64))),
        std::slice::from_ref(&capability),
    )
    .unwrap();
    let second = revision(
        &crate::test_installation(),
        Some(5),
        Some(&format!("sha256:{}", "b".repeat(64))),
        &[capability],
    )
    .unwrap();

    assert_ne!(first, second);
}

#[tokio::test]
async fn activity_asset_content_and_dependencies_are_integrity_bound_to_the_registry_revision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.html");
    tokio::fs::write(&path, b"<main>first</main>")
        .await
        .unwrap();
    let first = activity_asset(path.clone(), "text/html").await.unwrap();
    tokio::fs::write(&path, b"<main>second</main>")
        .await
        .unwrap();
    let second = activity_asset(path, "text/html").await.unwrap();
    assert_ne!(first.sha256, second.sha256);

    let mut capability = product_seeds::box_capability();
    capability.activity_bar = vec![ActivityBarContribution {
        id: "science".to_string(),
        title: "Science".to_string(),
        description: "Scientific workspace".to_string(),
        icon: "flask-conical".to_string(),
        entry: first,
        styles: Vec::new(),
        scripts: Vec::new(),
        skill: Some("science".to_string()),
        dependency_evidence_schema: UI_DEPENDENCY_EVIDENCE_SCHEMA.to_owned(),
        dependencies: vec![PluginSurfaceRef {
            kind: a3s_use_core::PluginSurfaceKind::Skill,
            id: "science".to_string(),
        }],
        order: 120,
    }];
    let first_revision = revision(
        &crate::test_installation(),
        None,
        None,
        &[capability.clone()],
    )
    .unwrap();
    capability.activity_bar[0].entry = second;
    let second_revision = revision(
        &crate::test_installation(),
        None,
        None,
        &[capability.clone()],
    )
    .unwrap();
    assert_ne!(first_revision, second_revision);

    capability.activity_bar[0].dependencies[0].id = "science-v2".to_string();
    let dependency_revision =
        revision(&crate::test_installation(), None, None, &[capability]).unwrap();
    assert_ne!(second_revision, dependency_revision);
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn promoted_sqlite_knowledge_enters_the_scope_aware_capability_projection() {
    const MANIFEST: &str = include_str!(
        "../../crates/extension/fixtures/packages/plugin-v3-okf/package/a3s-use-extension.acl"
    );
    const PACKAGE_DIGEST: &str =
        include_str!("../../crates/extension/fixtures/packages/plugin-v3-okf/package.sha256");
    let temporary = tempfile::tempdir().unwrap();
    let package_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let mut extension = installed_extension(manifest.clone(), package_root.clone(), true);
    let package_digest = PACKAGE_DIGEST.trim().to_owned();
    extension.receipt.package_sha256 =
        Some(package_digest.strip_prefix("sha256:").unwrap().to_owned());
    extension.receipt.manifest_sha256 = format!("{:x}", Sha256::digest(MANIFEST.as_bytes()));
    extension.receipt.lifecycle_generation = Some(7);
    let paths = crate::test_extension_paths(temporary.path());
    let adapter = std::sync::Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(&paths));
    let client = OkfKnowledgeClient::new(adapter);
    let store = OkfKnowledgeBindingStore::from_extension_paths(&paths);
    let surface = &manifest.okf[0];
    let files = a3s_use_extension::load_okf_bundle_files(surface, &package_root)
        .await
        .unwrap();
    let scope = crate::test_installation();
    let staged = client
        .stage(
            crate::okf_knowledge::OkfKnowledgeStageRequest::new(
                crate::okf_knowledge::OkfKnowledgeStageSpec {
                    operation_id: "capability-knowledge-stage".to_owned(),
                    scope: scope.clone(),
                    surface: PlanQualifiedSurfaceRef {
                        package_id: manifest.package_id.clone(),
                        surface: PluginSurfaceRef {
                            kind: PluginSurfaceKind::Okf,
                            id: surface.id.clone(),
                        },
                    },
                    generation: 7,
                    package_digest: package_digest.clone(),
                    manifest_digest: format!("sha256:{}", extension.receipt.manifest_sha256),
                    bundle: surface.bundle.clone(),
                },
                files,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store.put(&staged).await.unwrap();
    let promoted = client.promote(&staged.receipt).await.unwrap();
    store.put(&promoted).await.unwrap();

    let evidence = knowledge_evidence_from_store(&extension, &store, &client, &scope)
        .await
        .unwrap();
    assert!(evidence.failures.is_empty());
    let binding = project_extension_for_host_with_evidence(
        &extension,
        extension
            .surfaces()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        CapabilityHostProjectionContext {
            desired_enabled: extension.receipt.enabled,
            host_version: "0.3.0",
            host_observations: &SurfaceObservations::new(),
            knowledge_bindings: &evidence.bindings,
            runtime_tasks: &[],
            mcp_projections: &[],
            executable_tools: &[],
        },
    )
    .await
    .unwrap();
    assert!(binding.enabled);
    assert_eq!(binding.knowledge.len(), 1);
    assert_eq!(binding.knowledge[0].scope, scope);
    assert_eq!(binding.knowledge[0].generation, 7);

    let response = client
        .search(
            &crate::okf_knowledge::OkfKnowledgeSearchRequest::new(
                scope,
                "package activation",
                5,
                binding.knowledge,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.hits[0].citation.path,
        "concepts/package-lifecycle.md"
    );
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn healthy_okf_projects_knowledge_while_sibling_runtime_surfaces_reconcile() {
    const MANIFEST: &str = include_str!(
            "../../crates/extension/fixtures/packages/plugin-v3-cognitive/package/a3s-use-extension.acl"
        );
    const PACKAGE_DIGEST: &str =
        include_str!("../../crates/extension/fixtures/packages/plugin-v3-cognitive/package.sha256");
    let temporary = tempfile::tempdir().unwrap();
    let package_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-cognitive/package");
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let mut extension = installed_extension(manifest.clone(), package_root.clone(), true);
    let package_digest = PACKAGE_DIGEST.trim().to_owned();
    extension.receipt.package_sha256 =
        Some(package_digest.strip_prefix("sha256:").unwrap().to_owned());
    extension.receipt.manifest_sha256 = format!("{:x}", Sha256::digest(MANIFEST.as_bytes()));
    extension.receipt.lifecycle_generation = Some(3);
    let paths = crate::test_extension_paths(temporary.path());
    let adapter = std::sync::Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(&paths));
    let client = OkfKnowledgeClient::new(adapter);
    let store = OkfKnowledgeBindingStore::from_extension_paths(&paths);
    let surface = &manifest.okf[0];
    let files = a3s_use_extension::load_okf_bundle_files(surface, &package_root)
        .await
        .unwrap();
    let scope = crate::test_installation();
    let staged = client
        .stage(
            crate::okf_knowledge::OkfKnowledgeStageRequest::new(
                crate::okf_knowledge::OkfKnowledgeStageSpec {
                    operation_id: "capability-knowledge-partial".to_owned(),
                    scope: scope.clone(),
                    surface: PlanQualifiedSurfaceRef {
                        package_id: manifest.package_id.clone(),
                        surface: PluginSurfaceRef {
                            kind: PluginSurfaceKind::Okf,
                            id: surface.id.clone(),
                        },
                    },
                    generation: 3,
                    package_digest: package_digest.clone(),
                    manifest_digest: format!("sha256:{}", extension.receipt.manifest_sha256),
                    bundle: surface.bundle.clone(),
                },
                files,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store.put(&staged).await.unwrap();
    let promoted = client.promote(&staged.receipt).await.unwrap();
    store.put(&promoted).await.unwrap();

    let evidence = knowledge_evidence_from_store(&extension, &store, &client, &scope)
        .await
        .unwrap();
    assert!(evidence.failures.is_empty());
    // No Tool/MCP runtime observations → package stays reconciling.
    let binding = project_extension_for_host_with_evidence(
        &extension,
        extension
            .surfaces()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        CapabilityHostProjectionContext {
            desired_enabled: extension.receipt.enabled,
            host_version: "0.3.0",
            host_observations: &SurfaceObservations::new(),
            knowledge_bindings: &evidence.bindings,
            runtime_tasks: &[],
            mcp_projections: &[],
            executable_tools: &[],
        },
    )
    .await
    .unwrap();
    let reconciliation = binding.reconciliation.as_ref().expect("reconciliation");
    assert!(
        !reconciliation.capability_ready,
        "sibling Tool/MCP must keep the package from becoming capability-ready"
    );
    assert_eq!(binding.knowledge.len(), 1);
    assert!(
        binding.enabled,
        "healthy promoted OKF must remain queryable while siblings reconcile"
    );
    assert!(binding.mcp_servers.is_empty());
    assert!(binding.tool_tasks.is_empty());
    assert!(binding.skills.is_empty());
    assert!(binding.flows.is_empty());
    assert!(binding.activity_bar.is_empty());

    let response = client
        .search(
            &crate::okf_knowledge::OkfKnowledgeSearchRequest::new(
                scope,
                "package activation",
                5,
                binding.knowledge,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.hits[0].citation.path, "concepts/lifecycle.md");
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn schema_three_projects_only_dependency_ready_named_skills() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("skills").join("guide").join("SKILL.md");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, b"# Guide\n").await.unwrap();
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_ONLY_PLUGIN).unwrap();
    let mut extension = installed_extension(manifest, temp.path().to_path_buf(), true);
    extension.receipt.package_sha256 = Some("a".repeat(64));
    extension.receipt.lifecycle_generation = Some(7);
    let surfaces = extension
        .surfaces()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

    let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
        .await
        .unwrap();
    let reconciliation = binding.reconciliation.as_ref().unwrap();

    assert!(binding.enabled);
    assert_eq!(binding.readiness, Readiness::Ready);
    assert_eq!(binding.skills.len(), 1);
    assert_eq!(binding.skills[0].id, "guide");
    assert_eq!(binding.skills[0].path, path);
    assert_eq!(binding.skills[0].sha256.len(), 64);
    assert_eq!(binding.lifecycle_generation, Some(7));
    assert_eq!(reconciliation.observed, PluginObservedState::Ready);
    assert!(reconciliation.publishes(PluginSurfaceKind::Skill, "guide"));

    let json = serde_json::to_value(&binding).unwrap();
    assert_eq!(json["reconciliation"]["desired"], "enabled");
    assert_eq!(json["reconciliation"]["observed"], "ready");
    assert_eq!(json["lifecycleGeneration"], 7);
    assert_eq!(
        json["reconciliation"]["surfaces"][0]["surface"]["id"],
        "guide"
    );
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn installation_intent_withholds_an_enabled_receipt_from_publication() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("skills").join("guide").join("SKILL.md");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, b"# Guide\n").await.unwrap();
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_ONLY_PLUGIN).unwrap();
    let mut extension = installed_extension(manifest, temp.path().to_path_buf(), true);
    extension.receipt.package_sha256 = Some("a".repeat(64));
    extension.receipt.lifecycle_generation = Some(7);

    let binding = project_extension_for_host_with_evidence(
        &extension,
        extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect(),
        CapabilityHostProjectionContext {
            desired_enabled: false,
            host_version: "0.3.0",
            host_observations: &SurfaceObservations::new(),
            knowledge_bindings: &[],
            runtime_tasks: &[],
            mcp_projections: &[],
            executable_tools: &[],
        },
    )
    .await
    .unwrap();
    let reconciliation = binding.reconciliation.as_ref().unwrap();

    assert!(extension.receipt.enabled);
    assert!(!binding.enabled);
    assert!(binding.skills.is_empty());
    assert_eq!(
        reconciliation.desired,
        PluginDesiredState::InstalledDisabled
    );
    assert!(reconciliation
        .surfaces
        .iter()
        .all(|surface| !surface.published));
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn schema_three_projects_ready_ui_assets_with_optional_skill_guidance() {
    let temp = tempfile::tempdir().unwrap();
    let skill = temp.path().join("skills/guide/SKILL.md");
    let review = temp.path().join("ui/review.html");
    let style = temp.path().join("ui/review.css");
    let script = temp.path().join("ui/review.js");
    let standalone = temp.path().join("ui/standalone.html");
    tokio::fs::create_dir_all(skill.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(review.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&skill, b"# Guide\n").await.unwrap();
    tokio::fs::write(&review, b"<main>review</main>")
        .await
        .unwrap();
    tokio::fs::write(&style, b"main { color: purple; }")
        .await
        .unwrap();
    tokio::fs::write(&script, b"window.reviewReady = true;")
        .await
        .unwrap();
    tokio::fs::write(&standalone, b"<main>standalone</main>")
        .await
        .unwrap();
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_UI_PLUGIN).unwrap();
    let mut extension = installed_extension(manifest, temp.path().to_path_buf(), true);
    extension.receipt.package_sha256 = Some("a".repeat(64));
    extension.receipt.lifecycle_generation = Some(9);
    let surfaces = extension
        .surfaces()
        .into_iter()
        .map(str::to_string)
        .collect();

    let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
        .await
        .unwrap();
    assert_eq!(binding.activity_bar.len(), 2);
    let review = binding
        .activity_bar
        .iter()
        .find(|activity| activity.id == "review")
        .unwrap();
    assert_eq!(review.title, "Evidence Review");
    assert_eq!(review.description, "Review the cognitive package evidence.");
    assert_eq!(review.icon, "flask-conical");
    assert_eq!(review.order, 80);
    assert_eq!(review.skill.as_deref(), Some("guide"));
    assert_eq!(
        review.dependency_evidence_schema,
        UI_DEPENDENCY_EVIDENCE_SCHEMA
    );
    assert_eq!(
        review.dependencies,
        vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "guide".to_string(),
        }]
    );
    assert_eq!(review.entry.media_type, "text/html");
    assert_eq!(review.styles[0].media_type, "text/css");
    assert_eq!(review.scripts[0].media_type, "text/javascript");

    let standalone = binding
        .activity_bar
        .iter()
        .find(|activity| activity.id == "standalone")
        .unwrap();
    assert_eq!(standalone.title, "standalone");
    assert_eq!(standalone.icon, "package");
    assert_eq!(standalone.order, 100);
    assert!(standalone.skill.is_none());
    assert!(standalone.dependencies.is_empty());
    assert!(binding
        .reconciliation
        .as_ref()
        .unwrap()
        .publishes(PluginSurfaceKind::Ui, "review"));
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn schema_three_requires_a3s_flow_host_preflight_before_catalog_publication() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("flows/review.ts");
    tokio::fs::create_dir_all(source.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &source,
        b"export async function run() { return { type: 'complete', output: null }; }\n",
    )
    .await
    .unwrap();
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(FLOW_PLUGIN).unwrap();
    let extension = installed_extension(manifest, temp.path().to_path_buf(), true);
    let surfaces = extension
        .surfaces()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

    let source_only = project_extension_for_host(&extension, surfaces.clone(), "0.3.0")
        .await
        .unwrap();
    let source_only_flow = source_only
        .reconciliation
        .as_ref()
        .unwrap()
        .surfaces
        .iter()
        .find(|surface| surface.surface.kind == PluginSurfaceKind::Flow)
        .unwrap();
    assert!(!source_only.enabled);
    assert_eq!(source_only.readiness, Readiness::Unknown);
    assert!(source_only.flows.is_empty());
    assert_eq!(source_only_flow.observed, SurfaceObservedState::Pending);
    assert_eq!(
        source_only_flow.reason,
        Some(crate::surface_reconciler::SurfaceStateReason::FlowObservationMissing)
    );

    let mut flow_observations = SurfaceObservations::new();
    flow_observations.insert(
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "review".to_owned(),
        },
        SurfaceObservedState::Prepared,
    );
    let binding = project_extension_for_host_with_evidence(
        &extension,
        surfaces,
        CapabilityHostProjectionContext {
            desired_enabled: extension.receipt.enabled,
            host_version: "0.3.0",
            host_observations: &flow_observations,
            knowledge_bindings: &[],
            runtime_tasks: &[],
            mcp_projections: &[],
            executable_tools: &[],
        },
    )
    .await
    .unwrap();
    assert!(binding.enabled);
    assert_eq!(binding.readiness, Readiness::Ready);
    assert_eq!(binding.flows.len(), 1);
    let flow = &binding.flows[0];
    assert_eq!(flow.id, "review");
    assert_eq!(flow.engine, FlowEngine::A3sFlow);
    assert_eq!(flow.runtime, FlowRuntime::NativeTs);
    assert_eq!(flow.source.path, source);
    assert_eq!(flow.source.sha256.len(), 64);
    assert_eq!(flow.source.media_type, "text/typescript");
    assert_eq!(flow.export_name, "run");
    assert!(binding
        .reconciliation
        .as_ref()
        .unwrap()
        .publishes(PluginSurfaceKind::Flow, "review"));

    let json = serde_json::to_value(&binding).unwrap();
    assert_eq!(json["flows"][0]["engine"], "a3s-flow");
    assert_eq!(json["flows"][0]["runtime"], "native-ts");
    assert_eq!(json["flows"][0]["exportName"], "run");
    assert_eq!(json["flows"][0]["source"]["mediaType"], "text/typescript");
}

#[cfg(all(feature = "extensions", unix))]
#[tokio::test]
async fn exact_generation_flow_binding_drives_production_observation() {
    use std::os::unix::fs::PermissionsExt;

    use crate::flow_runtime::{A3sFlowLifecycleHost, FlowRuntimeBindingStore};
    use crate::plugin_lifecycle::{
        PluginFlowLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
        PluginLifecycleIntentSpec,
    };

    let temp = tempfile::tempdir().unwrap();
    let package_root = temp.path().join("package");
    let source = package_root.join("flows/review.ts");
    tokio::fs::create_dir_all(source.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &source,
        b"export async function run() { return { type: 'complete', output: null }; }\n",
    )
    .await
    .unwrap();
    let compiler = temp.path().join("a3s-flow-native-compiler");
    tokio::fs::write(
            &compiler,
            b"#!/bin/sh\nset -eu\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nshift\nprintf '#!/bin/sh\\nexit 0\\n' > \"$1\"\nchmod +x \"$1\"\n",
        )
        .await
        .unwrap();
    let mut permissions = std::fs::metadata(&compiler).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&compiler, permissions).unwrap();

    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(FLOW_PLUGIN).unwrap();
    let mut extension = installed_extension(manifest, package_root.clone(), true);
    extension.receipt.package_sha256 = Some("a".repeat(64));
    extension.receipt.lifecycle_generation = Some(12);
    let intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: "flow-observation-install".to_string(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope: crate::test_installation(),
            package_id: extension.receipt.package_id.clone(),
            package_digest: format!("sha256:{}", "a".repeat(64)),
            manifest_digest: format!("sha256:{}", extension.receipt.manifest_sha256),
            generation: 12,
            action: PluginLifecycleAction::Install,
            retained_ui_state_surfaces: Vec::new(),
        },
        &extension.manifest,
    )
    .unwrap();
    let key = &intent
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint
                .surface
                .as_ref()
                .is_some_and(|surface| surface.kind == PluginSurfaceKind::Flow)
        })
        .unwrap()
        .idempotency_key;
    let store = FlowRuntimeBindingStore::new(temp.path().join("state"), crate::test_installation())
        .unwrap();
    let host = A3sFlowLifecycleHost::new(
        &package_root,
        &compiler,
        temp.path().join("cache"),
        store.clone(),
    )
    .unwrap();
    host.prepare_flow(&intent, &extension.manifest.flows[0], key)
        .await
        .unwrap();

    let observations =
        flow_observations_from_store(&extension, &store, &crate::test_installation())
            .await
            .unwrap();
    assert_eq!(
        observations.get(&PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "review".to_string(),
        }),
        Some(&SurfaceObservedState::Prepared)
    );
    let binding = store
        .get(
            &crate::test_installation(),
            &PlanQualifiedSurfaceRef {
                package_id: extension.receipt.package_id.clone(),
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Flow,
                    id: "review".to_string(),
                },
            },
            12,
        )
        .await
        .unwrap()
        .unwrap();
    tokio::fs::write(binding.artifact(), b"substituted")
        .await
        .unwrap();
    let failed = flow_observations_from_store(&extension, &store, &crate::test_installation())
        .await
        .unwrap();
    assert_eq!(
        failed.get(&PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "review".to_string(),
        }),
        Some(&SurfaceObservedState::Failed)
    );
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn required_a3s_flow_source_corruption_withholds_the_generation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("flows/review.ts");
    tokio::fs::create_dir_all(source.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&source, [0xff_u8]).await.unwrap();
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(FLOW_PLUGIN).unwrap();
    let extension = installed_extension(manifest, temp.path().to_path_buf(), true);
    let mut flow_observations = SurfaceObservations::new();
    flow_observations.insert(
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "review".to_owned(),
        },
        SurfaceObservedState::Prepared,
    );
    let binding = project_extension_for_host_with_evidence(
        &extension,
        extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect(),
        CapabilityHostProjectionContext {
            desired_enabled: extension.receipt.enabled,
            host_version: "0.3.0",
            host_observations: &flow_observations,
            knowledge_bindings: &[],
            runtime_tasks: &[],
            mcp_projections: &[],
            executable_tools: &[],
        },
    )
    .await
    .unwrap();
    let flow = binding
        .reconciliation
        .as_ref()
        .unwrap()
        .surfaces
        .iter()
        .find(|surface| surface.surface.kind == PluginSurfaceKind::Flow)
        .unwrap();

    assert!(!binding.enabled);
    assert_eq!(binding.readiness, Readiness::Broken);
    assert!(binding.flows.is_empty());
    assert_eq!(flow.observed, SurfaceObservedState::Failed);
    assert!(!flow.published);
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn schema_three_ui_integrity_failure_blocks_only_required_surfaces() {
    let temp = tempfile::tempdir().unwrap();
    let skill = temp.path().join("skills/guide/SKILL.md");
    let review = temp.path().join("ui/review.html");
    tokio::fs::create_dir_all(skill.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(review.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&skill, b"# Guide\n").await.unwrap();
    tokio::fs::write(&review, b"<main>review</main>")
        .await
        .unwrap();
    tokio::fs::write(
        temp.path().join("ui/review.css"),
        b"main { color: purple; }",
    )
    .await
    .unwrap();
    tokio::fs::write(
        temp.path().join("ui/review.js"),
        b"window.reviewReady = true;",
    )
    .await
    .unwrap();

    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_UI_PLUGIN).unwrap();
    let required = installed_extension(manifest.clone(), temp.path().to_path_buf(), true);
    let binding = project_extension_for_host(
        &required,
        required
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect(),
        "0.3.0",
    )
    .await
    .unwrap();
    assert!(!binding.enabled);
    assert_eq!(binding.readiness, Readiness::Broken);
    assert!(binding.activity_bar.is_empty());
    assert_eq!(
        binding.reconciliation.as_ref().unwrap().observed,
        PluginObservedState::Broken
    );

    let mut optional_manifest = manifest;
    optional_manifest
        .ui
        .iter_mut()
        .find(|surface| surface.id == "standalone")
        .unwrap()
        .optional = true;
    let optional = installed_extension(optional_manifest, temp.path().to_path_buf(), true);
    let binding = project_extension_for_host(
        &optional,
        optional
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect(),
        "0.3.0",
    )
    .await
    .unwrap();
    let reconciliation = binding.reconciliation.as_ref().unwrap();
    let standalone = reconciliation
        .surfaces
        .iter()
        .find(|surface| {
            surface.surface.kind == PluginSurfaceKind::Ui && surface.surface.id == "standalone"
        })
        .unwrap();
    assert!(binding.enabled);
    assert_eq!(binding.readiness, Readiness::Ready);
    assert_eq!(binding.activity_bar.len(), 1);
    assert_eq!(binding.activity_bar[0].id, "review");
    assert_eq!(reconciliation.observed, PluginObservedState::Degraded);
    assert_eq!(standalone.observed, SurfaceObservedState::Failed);
    assert!(!standalone.published);
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn schema_three_with_unobserved_runtime_surfaces_stays_unpublished() {
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(include_str!(
        "../../crates/extension/fixtures/manifests/plugin-v3.acl"
    ))
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let ui = temp.path().join("ui/review");
    tokio::fs::create_dir_all(&ui).await.unwrap();
    tokio::fs::write(ui.join("index.html"), b"<main>review</main>")
        .await
        .unwrap();
    tokio::fs::write(ui.join("index.css"), b"main { color: purple; }")
        .await
        .unwrap();
    tokio::fs::write(ui.join("index.js"), b"window.reviewReady = true;")
        .await
        .unwrap();
    for skill in ["review", "quick-look"] {
        let directory = temp.path().join("skills").join(skill);
        tokio::fs::create_dir_all(&directory).await.unwrap();
        tokio::fs::write(
            directory.join("SKILL.md"),
            format!("# {skill}\n\nVerified test skill.\n"),
        )
        .await
        .unwrap();
    }
    let extension = installed_extension(manifest, temp.path().to_path_buf(), true);
    let surfaces = extension
        .surfaces()
        .into_iter()
        .map(str::to_string)
        .collect();

    let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
        .await
        .unwrap();
    let reconciliation = binding.reconciliation.as_ref().unwrap();

    assert!(!binding.enabled);
    assert_eq!(binding.readiness, Readiness::Unknown);
    assert!(binding.skills.is_empty());
    assert_eq!(reconciliation.observed, PluginObservedState::Reconciling);
    assert!(!reconciliation.capability_ready);
    assert!(reconciliation
        .surfaces
        .iter()
        .all(|surface| !surface.published));
}

#[tokio::test]
async fn matching_revision_times_out_without_reporting_a_change() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::User,
        "capability-watch-timeout",
    )
    .unwrap();
    #[cfg(feature = "extensions")]
    let registry = {
        let paths = a3s_use_extension::ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation.clone(),
        )
        .unwrap();
        crate::cognitive_package::open_control_lifecycle(
            &paths,
            std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        CapabilityRegistry::new(a3s_use_extension::ExtensionRegistry::new(paths))
    };
    #[cfg(not(feature = "extensions"))]
    let registry = CapabilityRegistry::new(installation.clone()).unwrap();
    let current = registry.snapshot().await.unwrap();
    let changed = registry
        .wait_for_change(
            current.generation,
            Some(current.revision.as_str()),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
    assert!(changed.is_none());
}

#[cfg(feature = "extensions")]
#[tokio::test]
async fn capability_snapshot_rejects_legacy_registry_json_beside_control() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::User,
        "capability-watch-tests",
    )
    .unwrap();
    let paths = a3s_use_extension::ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation.clone(),
    )
    .unwrap();
    crate::cognitive_package::open_control_lifecycle(
        &paths,
        std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
        None,
    )
    .await
    .unwrap();
    let extensions = a3s_use_extension::ExtensionRegistry::new(paths);
    let registry = CapabilityRegistry::new(extensions.clone());
    let initial = registry.snapshot().await.unwrap();
    assert!(initial.capabilities.is_empty());

    let mut published = a3s_use_extension::ExtensionRegistrySnapshot::empty(installation).unwrap();
    published.generation = initial.generation + 1;
    let parent = extensions.paths().installation_state_root();
    tokio::fs::create_dir_all(&parent).await.unwrap();
    let staging = parent.join(".registry-capability-watch.tmp");
    tokio::fs::write(&staging, serde_json::to_vec(&published).unwrap())
        .await
        .unwrap();
    tokio::fs::rename(staging, parent.join("registry.json"))
        .await
        .unwrap();

    let error = registry.snapshot().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
}

fn gateway_projection_descriptor(
    package_id: &str,
    generation: u64,
    package_digest: &str,
    manifest_digest: &str,
    catalog_record_digest: &str,
) -> CapabilityDescriptor {
    let package_id = a3s_use_core::PluginPackageId::parse(package_id).unwrap();
    let surface = PluginSurfaceRef {
        kind: a3s_use_core::PluginSurfaceKind::Tool,
        id: "search".to_owned(),
    };
    let input_schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "query": { "type": "string" } },
        "required": ["query"]
    });
    let output_schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "ok": { "type": "boolean" } },
        "required": ["ok"]
    });
    CapabilityDescriptor {
        schema: a3s_use_core::CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        package_id: package_id.clone(),
        surface: surface.clone(),
        generation,
        package_digest: package_digest.to_owned(),
        manifest_digest: manifest_digest.to_owned(),
        title: "Search".to_owned(),
        description: "Search verified data.".to_owned(),
        invocation_ref: a3s_use_core::InvocationRef::derive(
            &package_id,
            &surface,
            generation,
            &format!("sha256:{}", "c".repeat(64)),
        )
        .unwrap(),
        artifact_ref: None,
        endpoint_ref: None,
        dependencies: Vec::new(),
        required_extensions: Vec::new(),
        publication: a3s_use_core::CapabilityPublicationEvidence {
            catalog_record_digest: catalog_record_digest.to_owned(),
            signature_digest: format!("sha256:{}", "d".repeat(64)),
        },
        capability: a3s_use_core::CapabilityDescriptorKind::Tool {
            name: "search".to_owned(),
            input_schema,
            output_schema,
            annotations: a3s_use_core::CapabilityToolAnnotations::new(true, false, true, false),
            runtime_descriptor_digest: None,
        },
    }
}

fn gateway_projection_snapshot(
    descriptor: &CapabilityDescriptor,
    ready: bool,
    selected: bool,
) -> CapabilityRegistrySnapshot {
    let installation = crate::test_installation();
    let package_id = descriptor.package_id.to_string();
    let mut cursor = CapabilitySnapshotCursor {
        schema: CAPABILITY_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
        installation: installation.clone(),
        installation_generation: None,
        installation_snapshot_digest: None,
        generation: 9,
        revision: "a".repeat(64),
        registry_revision: format!("sha256:{}", "b".repeat(64)),
        packages: vec![CapabilityPackageGeneration {
            package_id: package_id.clone(),
            lifecycle_generation: descriptor.generation,
            package_digest: descriptor.package_digest.clone(),
            manifest_digest: descriptor.manifest_digest.clone(),
        }],
        unleasable_packages: Vec::new(),
    };
    let surface = descriptor.surface.clone();
    let evidence = PluginPlannerEvidence {
        schema_version: 1,
        package_id: package_id.clone(),
        package_sha256: descriptor.package_digest.clone(),
        manifest_sha256: descriptor.manifest_digest.clone(),
        receipt_digest: format!("sha256:{}", "e".repeat(64)),
        catalog_record_digest: descriptor.publication.catalog_record_digest.clone(),
        desired_enabled: true,
        selected_surfaces: selected.then_some(vec![surface]).unwrap_or_default(),
    };
    let binding = CapabilityBinding {
        id: format!("use/{package_id}"),
        alias: None,
        version: "1.0.0".to_owned(),
        origin: CapabilityOrigin::Extension,
        enabled: ready,
        readiness: if ready {
            Readiness::Ready
        } else {
            Readiness::Unknown
        },
        #[cfg(feature = "extensions")]
        reconciliation: None,
        planner_evidence: Some(evidence),
        package_root: None,
        lifecycle_generation: Some(descriptor.generation),
        requires_use: None,
        repository: None,
        surfaces: vec!["tool".to_owned()],
        mcp: None,
        mcp_servers: Vec::new(),
        skills: Vec::new(),
        flows: Vec::new(),
        knowledge: Vec::new(),
        activity_bar: Vec::new(),
        tool_tasks: Vec::new(),

        executable_tools: Vec::new(),
    };
    let capabilities = vec![binding];
    let snapshot_revision = revision(&installation, None, None, &capabilities).unwrap();
    cursor.revision.clone_from(&snapshot_revision);
    CapabilityRegistrySnapshot {
        schema_version: CAPABILITY_REGISTRY_SCHEMA_VERSION,
        installation,
        installation_generation: None,
        installation_snapshot_digest: None,
        generation: 9,
        revision: snapshot_revision,
        capabilities,
        cursor,
    }
}

#[test]
fn gateway_catalog_projection_binds_exact_reviewed_snapshot() {
    let descriptor = gateway_projection_descriptor(
        "acme/assistant",
        7,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
    );
    let snapshot = gateway_projection_snapshot(&descriptor, true, true);
    let catalog = snapshot
        .capability_gateway_catalog(vec![descriptor.clone()])
        .unwrap();
    assert_eq!(catalog.installation(), &snapshot.installation);
    assert_eq!(catalog.generation(), snapshot.generation);
    assert_eq!(catalog.descriptors(), &[descriptor]);
}

#[test]
fn gateway_catalog_projection_accepts_only_a_host_verified_description() {
    let descriptor = gateway_projection_descriptor(
        "acme/assistant",
        7,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
    );
    let snapshot = gateway_projection_snapshot(&descriptor, true, true);
    let proof = a3s_use_core::CapabilityDescriptionProof::from_verified(
        descriptor.clone(),
        "registry/official",
    )
    .unwrap();
    let catalog = snapshot
        .capability_gateway_catalog_from_verified_descriptions(vec![proof])
        .unwrap();
    assert_eq!(catalog.descriptors(), &[descriptor]);
}

#[test]
fn gateway_catalog_projection_rejects_unreviewed_or_unready_surfaces() {
    let descriptor = gateway_projection_descriptor(
        "acme/assistant",
        7,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
    );
    let unselected = gateway_projection_snapshot(&descriptor, true, false);
    assert_eq!(
        unselected
            .capability_gateway_catalog(vec![descriptor.clone()])
            .unwrap_err()
            .code,
        "use.capability.gateway_catalog_projection_invalid"
    );

    let unready = gateway_projection_snapshot(&descriptor, false, true);
    assert_eq!(
        unready
            .capability_gateway_catalog(vec![descriptor])
            .unwrap_err()
            .code,
        "use.capability.gateway_catalog_projection_invalid"
    );
}

#[test]
fn gateway_catalog_projection_rejects_a_stale_package_identity() {
    let descriptor = gateway_projection_descriptor(
        "acme/assistant",
        7,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
    );
    let snapshot = gateway_projection_snapshot(&descriptor, true, true);
    let stale = gateway_projection_descriptor(
        "acme/other",
        7,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
    );
    assert_eq!(
        snapshot
            .capability_gateway_catalog(vec![stale])
            .unwrap_err()
            .code,
        "use.capability.gateway_catalog_projection_invalid"
    );
}

#[test]
fn gateway_catalog_projection_rejects_a_mutated_public_snapshot() {
    let descriptor = gateway_projection_descriptor(
        "acme/assistant",
        7,
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
    );
    let mut snapshot = gateway_projection_snapshot(&descriptor, true, true);
    snapshot.capabilities[0].version = "2.0.0".to_owned();

    assert_eq!(
        snapshot
            .capability_gateway_catalog(vec![descriptor])
            .unwrap_err()
            .code,
        "use.capability.gateway_catalog_projection_invalid"
    );
}
