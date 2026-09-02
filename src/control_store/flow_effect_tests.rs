use std::path::{Path, PathBuf};

use a3s_use_core::{
    CatalogSurface, InstallationPackageSelection, LockedPluginPackage, PluginOperationAction,
    PluginPackageLockHost, PluginSurfaceKind, PluginSurfaceRef, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{ExtensionLifecyclePackage, PluginSurfaceFileEvidence};

use super::effect_owner::flow::ControlA3sFlowEffectPort;
use super::effect_port::{
    ControlEffectPortOutcome, ControlEffectRequestIdentity, ControlFlowEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
};
use super::model::{
    ControlEffectIntent, ControlEffectKind, ControlEffectOwner, ControlEffectSubject,
    ControlPackageEffectAuthority,
};
use crate::plugin_lifecycle::PluginLifecycleAction;

struct FlowOwnerFixture {
    _temporary: tempfile::TempDir,
    package_root: PathBuf,
    store: a3s_use_extension::ArtifactStore,
    compiler: PathBuf,
    cache_dir: PathBuf,
    authority: ControlPackageEffectAuthority,
}

#[test]
fn flow_owner_requires_lexically_stable_absolute_paths() {
    let store = a3s_use_extension::ExtensionPaths::new(
        std::env::temp_dir().join("flow-owner-data"),
        std::env::temp_dir().join("flow-owner-state"),
        crate::test_installation(),
    )
    .unwrap()
    .artifact_store();
    let error = ControlA3sFlowEffectPort::new(
        store.clone(),
        PathBuf::from("relative/compiler"),
        PathBuf::from("C:/flow-cache/../cache"),
    )
    .unwrap_err();
    assert_eq!(error.code, "use.control_store.flow_authority_invalid");
}

#[tokio::test]
async fn flow_prepare_uses_verified_bytes_and_owner_materialization() {
    let fixture = flow_owner_fixture(false).await;
    let owner = ControlA3sFlowEffectPort::new(
        fixture.store.clone(),
        fixture.compiler.clone(),
        fixture.cache_dir.clone(),
    )
    .unwrap();
    let request = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);

    let first = applied(&owner, &request).await;
    let mut retry_request = request.clone();
    retry_request.identity.attempt = 2;
    retry_request.identity.deadline_at_ms = 30_000;
    let retry = applied(&owner, &retry_request).await;

    assert_eq!(first, retry);
    assert!(first.materialization_digest.is_some());
    let materialized = std::fs::read_dir(fixture.cache_dir.join("control-sources"))
        .unwrap()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "ts"))
        .expect("the Flow owner must retain one materialized source");
    assert!(materialized.starts_with(fixture.cache_dir.join("control-sources")));
    assert!(!materialized.starts_with(&fixture.package_root));
    assert_eq!(
        std::fs::read(materialized).unwrap(),
        std::fs::read(fixture.package_root.join("flows/reason.ts")).unwrap()
    );
}

#[tokio::test]
async fn flow_prepare_rejects_tampered_package_before_preflight() {
    let fixture = flow_owner_fixture(false).await;
    let owner = ControlA3sFlowEffectPort::new(
        fixture.store.clone(),
        fixture.compiler.clone(),
        fixture.cache_dir.clone(),
    )
    .unwrap();
    std::fs::write(
        fixture.package_root.join("flows/reason.ts"),
        b"export function run() { return { type: 'tampered' }; }\n",
    )
    .unwrap();

    let outcome = ControlFlowEffectPort::apply_surface(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("tampered Flow source must be rejected before preflight");
    };
    assert_eq!(failure.error_code, "use.artifact_store.package_mismatch");
}

#[tokio::test]
async fn flow_prepare_rejects_authority_substitution_before_artifact_io() {
    let fixture = flow_owner_fixture(false).await;
    let owner = ControlA3sFlowEffectPort::new(
        fixture.store.clone(),
        fixture.compiler.clone(),
        fixture.cache_dir.clone(),
    )
    .unwrap();
    let mut request = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);
    request.package_id = "acme/other".to_string();

    let outcome = ControlFlowEffectPort::apply_surface(&owner, &request).await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("authority substitution must be rejected before Artifact Store access");
    };
    assert_eq!(
        failure.error_code,
        "use.control_store.flow_authority_invalid"
    );
}

#[tokio::test]
async fn flow_source_materialization_is_immutable_and_no_clobber() {
    let fixture = flow_owner_fixture(false).await;
    let owner = ControlA3sFlowEffectPort::new(
        fixture.store.clone(),
        fixture.compiler.clone(),
        fixture.cache_dir.clone(),
    )
    .unwrap();
    let request = request(&fixture.authority, ControlSurfaceEffectAction::Prepare);
    applied(&owner, &request).await;
    let materialized = std::fs::read_dir(fixture.cache_dir.join("control-sources"))
        .unwrap()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "ts"))
        .expect("materialized source");
    std::fs::write(&materialized, b"substituted").unwrap();

    let outcome = ControlFlowEffectPort::apply_surface(&owner, &request).await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("a changed content-addressed source must not be reused");
    };
    assert_eq!(failure.error_code, "use.control_store.flow_source_conflict");
}

#[tokio::test]
async fn flow_prepare_defers_while_artifact_store_is_busy() {
    let fixture = flow_owner_fixture(false).await;
    let owner =
        ControlA3sFlowEffectPort::new(fixture.store.clone(), fixture.compiler, fixture.cache_dir)
            .unwrap();
    let collection = fixture.store.acquire_collection().await.unwrap();
    let outcome = ControlFlowEffectPort::apply_surface(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    drop(collection);
    let ControlEffectPortOutcome::Deferred(failure) = outcome else {
        panic!("Artifact Store contention must be a safe Flow deferral");
    };
    assert_eq!(failure.error_code, "use.artifact_store.busy");
}

#[tokio::test]
async fn flow_stop_and_remove_are_path_independent_receipts() {
    let fixture = flow_owner_fixture(true).await;
    let absent_store = a3s_use_extension::ExtensionPaths::new(
        fixture._temporary.path().join("absent-data"),
        fixture._temporary.path().join("absent-state"),
        crate::test_installation(),
    )
    .unwrap()
    .artifact_store();
    let owner = ControlA3sFlowEffectPort::new(
        absent_store,
        fixture._temporary.path().join("missing-compiler"),
        fixture._temporary.path().join("missing-cache"),
    )
    .unwrap();
    let stop = applied(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Stop),
    )
    .await;
    let remove = applied(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Remove),
    )
    .await;
    assert_ne!(stop.receipt_digest, remove.receipt_digest);
    assert!(stop.materialization_digest.is_none());
    assert!(remove.materialization_digest.is_none());
}

#[tokio::test]
async fn flow_preflight_failure_is_proved_no_effect() {
    let fixture = flow_owner_fixture(true).await;
    let owner =
        ControlA3sFlowEffectPort::new(fixture.store, fixture.compiler, fixture.cache_dir).unwrap();
    let outcome = ControlFlowEffectPort::apply_surface(
        &owner,
        &request(&fixture.authority, ControlSurfaceEffectAction::Prepare),
    )
    .await;
    let ControlEffectPortOutcome::Rejected(failure) = outcome else {
        panic!("a compiler that exits unsuccessfully must reject without publication");
    };
    assert_eq!(
        failure.error_code,
        "use.control_store.flow_preflight_failed"
    );
}

async fn applied(
    owner: &ControlA3sFlowEffectPort,
    request: &ControlSurfaceEffectRequest,
) -> ControlSurfaceApplication {
    let outcome = ControlFlowEffectPort::apply_surface(owner, request).await;
    let ControlEffectPortOutcome::Applied(application) = outcome else {
        match outcome {
            ControlEffectPortOutcome::Deferred(failure)
            | ControlEffectPortOutcome::Rejected(failure)
            | ControlEffectPortOutcome::Unknown(failure) => {
                panic!(
                    "the Flow owner must apply the valid fixture: {}",
                    failure.error_code
                )
            }
            ControlEffectPortOutcome::Applied(_) => unreachable!(),
        }
    };
    application
}

fn request(
    authority: &ControlPackageEffectAuthority,
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
        kind: PluginSurfaceKind::Flow,
        id: "reason".to_string(),
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
    let installation = crate::test_installation();
    let plan_digest = digest('1');
    let effect_kind = match action {
        ControlSurfaceEffectAction::Prepare => ControlEffectKind::SurfacePrepare,
        ControlSurfaceEffectAction::Stop => ControlEffectKind::SurfaceStop,
        ControlSurfaceEffectAction::Remove => ControlEffectKind::SurfaceRemove,
    };
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
        ControlEffectOwner::FlowHost,
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

async fn flow_owner_fixture(failing_compiler: bool) -> FlowOwnerFixture {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("source");
    std::fs::create_dir_all(source_root.join("flows")).unwrap();
    std::fs::write(source_root.join("README.md"), "# Flow fixture\n").unwrap();
    std::fs::write(source_root.join("flows/reason.ts"), flow_source()).unwrap();
    std::fs::write(source_root.join("a3s-use-extension.acl"), flow_manifest()).unwrap();

    let candidate = ExtensionLifecyclePackage::prepare_local("acme/flow", &source_root, true)
        .await
        .unwrap();
    let catalog = verified_catalog(&candidate);
    let paths = a3s_use_extension::ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        crate::test_installation(),
    )
    .unwrap();
    let store = paths.artifact_store();
    let admission = store.acquire_reference_admission().await.unwrap();
    store
        .admit_prepared_package(&admission, &candidate)
        .await
        .unwrap();
    drop(admission);
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
    let compiler = if cfg!(windows) {
        temporary.path().join("compiler.cmd")
    } else {
        temporary.path().join("compiler")
    };
    write_compiler(&compiler, failing_compiler);
    let cache_dir = temporary.path().join("flow-cache");
    FlowOwnerFixture {
        package_root: store
            .expanded_package_path(candidate.package_digest())
            .unwrap(),
        store,
        compiler,
        cache_dir,
        authority: ControlPackageEffectAuthority {
            generation_operation_id: "operation:flow-owner".to_string(),
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
    let manifest = candidate.manifest();
    record.package_id = candidate.package_id().to_string();
    record.display_name = "Flow Fixture".to_string();
    record.description = "Committed Control Flow owner fixture.".to_string();
    record.version = manifest.version.clone();
    record.repository = "https://github.com/acme/flow".to_string();
    record.surfaces = manifest
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
    record.archive.target_name = format!(
        "extensions/acme/flow/{}/stable/linux-x86_64/acme-flow-{}-linux-x86_64.tar.gz",
        manifest.version, manifest.version
    );
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

fn flow_manifest() -> &'static str {
    r#"extension "acme/flow" {
  schema_version = 3
  version        = "1.0.0"
  route          = "flow"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["execute"]

  repository {
    url      = "https://github.com/acme/flow"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  flow "reason" {
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/reason.ts"
    export        = "run"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }
}
"#
}

fn flow_source() -> &'static str {
    "export function run() { return { type: 'complete', output: {} }; }\n"
}

fn write_compiler(path: &Path, failing: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let body = if failing {
            "#!/bin/sh\nexit 7\n"
        } else {
            "#!/bin/sh\nset -eu\n[ \"$1\" = \"compile\" ]\nshift\noutput=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then shift; output=\"$1\"; fi\n  shift\ndone\n[ -n \"$output\" ]\nprintf '#!/bin/sh\\nexit 0\\n' > \"$output\"\nchmod +x \"$output\"\n"
        };
        std::fs::write(path, body).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
    #[cfg(windows)]
    {
        let body = if failing {
            "@echo off\r\nexit /b 7\r\n".to_string()
        } else {
            "@echo off\r\nsetlocal EnableExtensions\r\nif /I not \"%~1\"==\"compile\" exit /b 2\r\nshift\r\nset \"output=\"\r\n:parse\r\nif \"%~1\"==\"\" goto done\r\nif /I \"%~1\"==\"-o\" goto output\r\nshift\r\ngoto parse\r\n:output\r\nshift\r\nset \"output=%~1\"\r\ngoto parse\r\n:done\r\nif not defined output exit /b 3\r\n> \"%output%\" echo @echo off\r\n>> \"%output%\" echo exit /b 0\r\nexit /b 0\r\n".to_string()
        };
        std::fs::write(path, body).unwrap();
    }
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PluginSurfaceFileEvidence>();
};
