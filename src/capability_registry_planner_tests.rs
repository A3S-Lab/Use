use a3s_use_core::{
    CatalogArchive, CatalogAvailability, CatalogPackage, CatalogSurface, PluginCatalogRecord,
    PluginPermissionCeiling, PluginReleaseChannel, PluginSurfaceKind, PluginSurfaceRef,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V2,
    PLUGIN_PERMISSION_SCHEMA,
};
use sha2::{Digest, Sha256};

use super::*;

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

#[tokio::test]
async fn plan_ready_projection_binds_receipt_and_named_surface_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("skills").join("guide").join("SKILL.md");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, b"# Guide\n").await.unwrap();
    let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_ONLY_PLUGIN).unwrap();
    let manifest_sha256 = format!("{:x}", Sha256::digest(SKILL_ONLY_PLUGIN.as_bytes()));
    let package_sha256 = "b".repeat(64);
    let permissions = PluginPermissionCeiling {
        schema: PLUGIN_PERMISSION_SCHEMA.to_owned(),
        surfaces: Vec::new(),
    };
    let record = PluginCatalogRecord {
        schema: PLUGIN_CATALOG_SCHEMA_V2.to_owned(),
        package_id: "acme/guide".to_owned(),
        display_name: "Guide".to_owned(),
        description: "Agent guidance for the current workspace.".to_owned(),
        publisher: "acme".to_owned(),
        keywords: vec!["guide".to_owned()],
        categories: vec!["productivity".to_owned()],
        version: "1.0.0".to_owned(),
        channel: PluginReleaseChannel::Stable,
        requires_use: ">=0.3.0, <0.4.0".to_owned(),
        dependencies: Vec::new(),
        target: "any".to_owned(),
        surfaces: vec![CatalogSurface {
            kind: PluginSurfaceKind::Skill,
            id: "guide".to_owned(),
            optional: false,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: Vec::new(),
        }],
        permission_ceiling_digest: permissions.descriptor_digest().unwrap(),
        permission_ceiling: permissions,
        planning: None,
        archive: CatalogArchive {
            target_name: "extensions/acme/guide/1.0.0/stable/any/guide-1.0.0-any.tar.gz".to_owned(),
            length: 1,
            sha256: format!("sha256:{}", "a".repeat(64)),
        },
        package: CatalogPackage {
            expanded_bytes: 1,
            file_count: 1,
            sha256: Some(format!("sha256:{package_sha256}")),
            manifest_sha256: Some(format!("sha256:{manifest_sha256}")),
        },
        license: "Apache-2.0".to_owned(),
        repository: "https://github.com/acme/guide".to_owned(),
        availability: CatalogAvailability::Available,
    };
    let verified = VerifiedPluginCatalogRecord::new(
        record.clone(),
        VerifiedCatalogProvenance {
            registry_name: "fixture".to_owned(),
            registry_url: "http://127.0.0.1:43111/".to_owned(),
            root_sha256: format!("sha256:{}", "f".repeat(64)),
            root_version: 1,
            timestamp_version: 2,
            snapshot_version: 2,
            targets_version: 2,
            catalog_record_digest: record.descriptor_digest().unwrap(),
        },
    )
    .unwrap();
    let registry =
        a3s_use_extension::ResolvedRemotePackage::from_verified_catalog(&verified).unwrap();
    let receipt = a3s_use_extension::ExtensionReceipt {
        schema_version: 2,
        package_id: manifest.package_id.clone(),
        component_id: "use/acme/guide".to_owned(),
        route: manifest.route.clone(),
        version: manifest.version.clone(),
        package_root: temp.path().to_path_buf(),
        manifest_sha256,
        package_sha256: Some(package_sha256),
        trust: a3s_use_extension::ExtensionTrust::RegistryTuf,
        registry: Some(registry),
        verified_catalog: Some(verified),
        installed_at_unix: 7,
        enabled: true,
        lifecycle_generation: None,
    };
    let extension = a3s_use_extension::InstalledExtension { receipt, manifest };
    let surfaces = extension
        .surfaces()
        .into_iter()
        .map(str::to_string)
        .collect();

    let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
        .await
        .unwrap();
    let evidence = binding.planner_evidence.as_ref().unwrap();

    assert_eq!(evidence.schema_version, 1);
    assert_eq!(evidence.package_id, "acme/guide");
    assert_eq!(
        evidence.receipt_digest,
        extension.receipt.descriptor_digest().unwrap()
    );
    assert_eq!(
        evidence.selected_surfaces,
        vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "guide".to_owned(),
        }]
    );
    assert!(evidence.desired_enabled);

    let snapshot = CapabilityRegistrySnapshot {
        schema_version: SCHEMA_VERSION,
        generation: 23,
        revision: "e".repeat(64),
        capabilities: vec![binding.clone()],
    };
    let installed = installed_plugin_plan_evidence_from_snapshot(&snapshot, &extension).unwrap();
    assert_eq!(installed.capability_generation, 23);
    assert_eq!(installed.capability_revision, "e".repeat(64));
    assert_eq!(
        installed.verified_catalog.provenance.catalog_record_digest,
        evidence.catalog_record_digest
    );
    assert_eq!(installed.receipt_digest, evidence.receipt_digest);

    let mut mismatched = extension;
    let catalog = mismatched.receipt.verified_catalog.as_mut().unwrap();
    catalog.record.surfaces[0].id = "other".to_owned();
    catalog.provenance.catalog_record_digest = catalog.record.descriptor_digest().unwrap();
    mismatched.receipt.registry =
        Some(a3s_use_extension::ResolvedRemotePackage::from_verified_catalog(catalog).unwrap());
    let surfaces = mismatched
        .surfaces()
        .into_iter()
        .map(str::to_string)
        .collect();
    let error = project_extension_for_host(&mismatched, surfaces, "0.3.0")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_package_mismatch");
}
