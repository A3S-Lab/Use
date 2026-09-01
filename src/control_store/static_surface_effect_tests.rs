use a3s_use_core::{
    CatalogSurface, InstallationPackageSelection, LockedPluginPackage, PluginOperationAction,
    PluginPackageLockHost, PluginSurfaceKind, PluginSurfaceRef, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{
    ArtifactStore, ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionRegistry,
};

use super::effect_owner::static_surface::ControlStaticSurfaceEffectPort;
use super::effect_port::{
    ControlEffectPortOutcome, ControlEffectRequestIdentity, ControlSkillEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
    ControlUiEffectPort,
};
use super::model::{
    ControlEffectIntent, ControlEffectKind, ControlEffectOwner, ControlEffectSubject,
    ControlPackageEffectAuthority,
};
use crate::plugin_lifecycle::PluginLifecycleAction;

struct StaticOwnerFixture {
    _temporary: tempfile::TempDir,
    store: ArtifactStore,
    package_root: std::path::PathBuf,
    authority: ControlPackageEffectAuthority,
}

#[tokio::test]
async fn static_skill_and_ui_prepare_use_verified_artifact_evidence() {
    let fixture = static_owner_fixture().await;
    let owner = ControlStaticSurfaceEffectPort::new(fixture.store.clone());
    let skill_request = request(
        &fixture.authority,
        PluginSurfaceKind::Skill,
        "guide",
        ControlSurfaceEffectAction::Prepare,
    );
    let ui_request = request(
        &fixture.authority,
        PluginSurfaceKind::Ui,
        "panel",
        ControlSurfaceEffectAction::Prepare,
    );

    let first_skill = applied_skill(&owner, &skill_request).await;
    let second_skill = applied_skill(&owner, &skill_request).await;
    let mut retried_skill_request = skill_request.clone();
    retried_skill_request.identity.attempt = 2;
    retried_skill_request.identity.deadline_at_ms = 30_000;
    let retried_skill = applied_skill(&owner, &retried_skill_request).await;
    let ui = applied_ui(&owner, &ui_request).await;

    assert_eq!(first_skill, second_skill);
    assert_eq!(first_skill, retried_skill);
    assert!(first_skill.materialization_digest.is_some());
    assert!(ui.materialization_digest.is_some());
    assert_ne!(
        first_skill.materialization_digest,
        ui.materialization_digest
    );
    assert_ne!(first_skill.receipt_digest, ui.receipt_digest);
}

#[tokio::test]
async fn static_prepare_rejects_tampering_without_unknown_effect_state() {
    let fixture = static_owner_fixture().await;
    let owner = ControlStaticSurfaceEffectPort::new(fixture.store.clone());
    let request = request(
        &fixture.authority,
        PluginSurfaceKind::Skill,
        "guide",
        ControlSurfaceEffectAction::Prepare,
    );
    std::fs::write(
        fixture.package_root.join("skills/guide/SKILL.md"),
        b"# Substituted\n",
    )
    .unwrap();

    let outcome = ControlSkillEffectPort::apply_surface(&owner, &request).await;

    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("a read-only static owner must safely reject tampered content");
    };
    assert_eq!(failure.error_code, "use.artifact_store.package_mismatch");
}

#[tokio::test]
async fn static_owner_rejects_surface_or_authority_substitution_before_reading() {
    let fixture = static_owner_fixture().await;
    let owner = ControlStaticSurfaceEffectPort::new(fixture.store.clone());
    let wrong_kind = request(
        &fixture.authority,
        PluginSurfaceKind::Ui,
        "panel",
        ControlSurfaceEffectAction::Prepare,
    );
    let mut wrong_package = request(
        &fixture.authority,
        PluginSurfaceKind::Skill,
        "guide",
        ControlSurfaceEffectAction::Prepare,
    );
    let mut wrong_host = wrong_package.clone();
    let mut wrong_key = wrong_package.clone();
    wrong_package.package_id = "acme/substituted".to_string();
    wrong_host.authority.host.target = "windows-x86_64".to_string();
    wrong_key.identity.idempotency_key = digest('9');

    let wrong_kind = ControlSkillEffectPort::apply_surface(&owner, &wrong_kind).await;
    let wrong_package = ControlSkillEffectPort::apply_surface(&owner, &wrong_package).await;
    let wrong_host = ControlSkillEffectPort::apply_surface(&owner, &wrong_host).await;
    let wrong_key = ControlSkillEffectPort::apply_surface(&owner, &wrong_key).await;

    for outcome in [wrong_kind, wrong_package, wrong_host, wrong_key] {
        let ControlEffectPortOutcome::Rejected(failure) = outcome else {
            panic!("substituted static authority must be rejected before Artifact I/O");
        };
        assert_eq!(
            failure.error_code,
            "use.control_store.static_authority_invalid"
        );
    }
}

#[tokio::test]
async fn static_stop_and_remove_are_path_independent_idempotent_receipts() {
    let fixture = static_owner_fixture().await;
    let absent =
        crate::test_extension_paths(&fixture._temporary.path().join("absent")).artifact_store();
    let owner = ControlStaticSurfaceEffectPort::new(absent);
    let stop = request(
        &fixture.authority,
        PluginSurfaceKind::Skill,
        "guide",
        ControlSurfaceEffectAction::Stop,
    );
    let remove = request(
        &fixture.authority,
        PluginSurfaceKind::Skill,
        "guide",
        ControlSurfaceEffectAction::Remove,
    );

    let first_stop = applied_skill(&owner, &stop).await;
    let second_stop = applied_skill(&owner, &stop).await;
    let remove = applied_skill(&owner, &remove).await;

    assert_eq!(first_stop, second_stop);
    assert!(first_stop.materialization_digest.is_none());
    assert!(remove.materialization_digest.is_none());
    assert_ne!(first_stop.receipt_digest, remove.receipt_digest);
}

#[tokio::test]
async fn static_prepare_defers_while_artifact_collection_owns_the_boundary() {
    let fixture = static_owner_fixture().await;
    let owner = ControlStaticSurfaceEffectPort::new(fixture.store.clone());
    let collection = fixture.store.acquire_collection().await.unwrap();
    let request = request(
        &fixture.authority,
        PluginSurfaceKind::Skill,
        "guide",
        ControlSurfaceEffectAction::Prepare,
    );

    let outcome = ControlSkillEffectPort::apply_surface(&owner, &request).await;

    drop(collection);
    let ControlEffectPortOutcome::Deferred(failure) = outcome else {
        panic!("Artifact Store contention must be a safe same-key deferral");
    };
    assert_eq!(failure.error_code, "use.artifact_store.busy");
}

async fn applied_skill(
    owner: &ControlStaticSurfaceEffectPort,
    request: &ControlSurfaceEffectRequest,
) -> ControlSurfaceApplication {
    let ControlEffectPortOutcome::Applied(application) =
        ControlSkillEffectPort::apply_surface(owner, request).await
    else {
        panic!("the Skill owner must apply the valid fixture");
    };
    application
}

async fn applied_ui(
    owner: &ControlStaticSurfaceEffectPort,
    request: &ControlSurfaceEffectRequest,
) -> ControlSurfaceApplication {
    let ControlEffectPortOutcome::Applied(application) =
        ControlUiEffectPort::apply_surface(owner, request).await
    else {
        panic!("the UI owner must apply the valid fixture");
    };
    application
}

fn request(
    authority: &ControlPackageEffectAuthority,
    kind: PluginSurfaceKind,
    surface_id: &str,
    action: ControlSurfaceEffectAction,
) -> ControlSurfaceEffectRequest {
    let lifecycle_action = match action {
        ControlSurfaceEffectAction::Prepare => PluginLifecycleAction::Install,
        ControlSurfaceEffectAction::Stop => PluginLifecycleAction::Disable,
        ControlSurfaceEffectAction::Remove => PluginLifecycleAction::Uninstall,
    };
    let operation_action = match action {
        ControlSurfaceEffectAction::Prepare => PluginOperationAction::Install,
        ControlSurfaceEffectAction::Stop => PluginOperationAction::Disable,
        ControlSurfaceEffectAction::Remove => PluginOperationAction::Uninstall,
    };
    let surface = PluginSurfaceRef {
        kind,
        id: surface_id.to_string(),
    };
    let package_id = authority.package.package_id().to_string();
    let package_digest = authority
        .package
        .package
        .catalog
        .record
        .package
        .sha256
        .clone()
        .unwrap();
    let manifest_digest = authority
        .package
        .package
        .catalog
        .record
        .package
        .manifest_sha256
        .clone()
        .unwrap();
    let owner = match kind {
        PluginSurfaceKind::Skill => ControlEffectOwner::SkillHost,
        PluginSurfaceKind::Ui => ControlEffectOwner::UiHost,
        _ => panic!("the static owner fixture supports only Skill and UI"),
    };
    let effect_kind = match action {
        ControlSurfaceEffectAction::Prepare => ControlEffectKind::SurfacePrepare,
        ControlSurfaceEffectAction::Stop => ControlEffectKind::SurfaceStop,
        ControlSurfaceEffectAction::Remove => ControlEffectKind::SurfaceRemove,
    };
    let installation = crate::test_installation();
    let plan_digest = digest('1');
    let intent = ControlEffectIntent::new(
        0,
        installation.clone(),
        plan_digest.clone(),
        operation_action,
        authority.installation_generation,
        ControlEffectSubject::Surface {
            package_id: package_id.clone(),
            lifecycle_generation: authority.lifecycle_generation,
            package_digest: package_digest.clone(),
            manifest_digest: manifest_digest.clone(),
            action: lifecycle_action,
            surface: surface.clone(),
        },
        owner,
        effect_kind,
        true,
    )
    .unwrap();
    ControlSurfaceEffectRequest {
        identity: ControlEffectRequestIdentity {
            operation_id: authority.generation_operation_id.clone(),
            installation,
            plan_digest,
            operation_action,
            installation_generation: authority.installation_generation,
            sequence: 0,
            idempotency_key: intent.idempotency_key,
            required: true,
            attempt: 1,
            deadline_at_ms: 20_000,
        },
        authority: authority.clone(),
        package_id,
        lifecycle_generation: authority.lifecycle_generation,
        package_digest,
        manifest_digest,
        lifecycle_action,
        surface,
        action,
    }
}

async fn static_owner_fixture() -> StaticOwnerFixture {
    let temporary = tempfile::tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-static/package");
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/static", &source, true)
        .await
        .unwrap();
    let catalog = verified_catalog(&candidate);
    let identity = ExtensionLifecycleIdentity::new(
        candidate.package_id(),
        candidate.package_digest(),
        candidate.manifest_digest(),
        1,
    )
    .unwrap();
    let paths = crate::test_extension_paths(temporary.path());
    let registry = ExtensionRegistry::new(paths.clone());
    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    let selected_surfaces = candidate
        .manifest()
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface)
        .collect();
    let package = InstallationPackageSelection::new(
        LockedPluginPackage {
            catalog,
            dependencies: Vec::new(),
        },
        1,
        true,
        selected_surfaces,
    )
    .unwrap();
    StaticOwnerFixture {
        store: paths.artifact_store(),
        package_root: committed.extension.receipt.package_root,
        authority: ControlPackageEffectAuthority {
            generation_operation_id: "operation:static-owner".to_string(),
            installation_generation: 1,
            snapshot_digest: digest('3'),
            committed_at_ms: 1_000,
            host: PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
            package,
            lifecycle_generation: 1,
            grant: None,
        },
        _temporary: temporary,
    }
}

fn verified_catalog(candidate: &ExtensionLifecyclePackage) -> VerifiedPluginCatalogRecord {
    let mut record = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    record.package_id = candidate.package_id().to_string();
    record.display_name = "Static Surface Fixture".to_string();
    record.description = "Committed Control static owner fixture.".to_string();
    record.version = candidate.manifest().version.clone();
    record.dependencies = candidate.manifest().dependencies.clone();
    record.surfaces = candidate
        .manifest()
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| CatalogSurface {
            kind: surface.surface.kind,
            id: surface.surface.id,
            optional: surface.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: surface.dependencies,
        })
        .collect();
    record.archive.target_name =
        "extensions/acme/static/1.0.0/stable/linux-x86_64/acme-static-1.0.0-linux-x86_64.tar.gz"
            .to_string();
    record.package.expanded_bytes = candidate.expanded_bytes();
    record.package.file_count = candidate.file_count();
    record.package.sha256 = Some(candidate.package_digest().to_string());
    record.package.manifest_sha256 = Some(candidate.manifest_digest().to_string());
    let provenance = VerifiedCatalogProvenance {
        registry_name: "fixture".to_string(),
        registry_url: "https://packages.example.test/catalog/".to_string(),
        root_sha256: digest('4'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
