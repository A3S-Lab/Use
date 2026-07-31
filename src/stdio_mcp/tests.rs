use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    CatalogArchive, CatalogAvailability, CatalogMcpTransport, CatalogPackage, CatalogSurface,
    PlanActor, PlanEnforcementProfile, PlanPolicyDecision, PluginCatalogRecord,
    PluginPermissionCeiling, PluginReleaseChannel, PluginSurfaceKind, PluginSurfaceRef,
    ResourcePermissionCeiling, SurfacePermissionCeiling, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, WorkspaceGrantAuthority, PLUGIN_CATALOG_SCHEMA_V2,
    PLUGIN_PERMISSION_SCHEMA, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};
use a3s_use_extension::{
    ExtensionManifest, ExtensionReceipt, ExtensionTrust, InstalledExtension, ResolvedRemotePackage,
    WorkspaceGrantReceipt, WorkspaceGrantRevocation, WorkspaceGrantStore,
};
use async_trait::async_trait;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::DuplexStream;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::settlement::LeaseSettlement;
use super::*;
use crate::{CapabilityHostSurfaceOwner, CapabilitySurfaceObservedState};

const MANIFEST: &str = r#"
extension "acme/local-mcp" {
  schema_version = 3
  version        = "1.0.0"
  route          = "local-mcp"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  repository {
    url      = "https://github.com/acme/local-mcp"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  mcp "local" {
    transport  = "stdio"
    executable = "bin/local-mcp"
    args       = ["serve", "--stdio"]
    activation = "lazy"
    optional   = false
  }
}
"#;

// Live-change tests exercise durable state transitions, not the accepted
// lower bound. Keep their file-observation deadline above normal scheduler
// granularity when real child-process tests run concurrently on Windows.
const LIVE_GRANT_RECHECK_INTERVAL_MS: u64 = 100;

#[tokio::test]
async fn prepare_binds_package_grant_provider_and_scoped_observation() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host);
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-prepare", 1_000, 1_000))
        .await
        .unwrap();
    let plan = prepared.plan();
    let observation = prepared.host_observation().unwrap();

    assert_eq!(plan.schema(), STDIO_MCP_SESSION_PLAN_SCHEMA);
    assert_eq!(plan.package_id(), "acme/local-mcp");
    assert_eq!(plan.scope_id(), "workspace-a");
    assert_eq!(plan.surface().id, "local");
    assert_eq!(plan.args(), ["serve", "--stdio"]);
    assert_eq!(plan.provider().provider_build_id(), "build-1");
    assert_eq!(plan.grant_authority().decision, PlanPolicyDecision::Ask);
    assert_eq!(plan.authorization_recheck_interval_ms(), 1_000);
    assert_eq!(plan.plan_digest().len(), 71);
    assert_eq!(
        plan.non_secret_environment()["A3S_USE_SCOPE_ID"],
        "workspace-a"
    );
    assert_eq!(observation.owner(), CapabilityHostSurfaceOwner::McpHost);
    assert_eq!(
        observation.state(),
        CapabilitySurfaceObservedState::Prepared
    );
    assert!(!fixture.lease_dropped.load(Ordering::SeqCst));

    drop(lease);
    assert!(fixture.lease_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn initialized_session_is_healthy_and_shutdown_waits_for_process_exit() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-live", 1_000, 1_000))
        .await
        .unwrap();
    let session = supervisor.start(prepared, lease).await.unwrap();

    assert_eq!(session.identity().process_id(), "fake-process-1");
    assert_eq!(
        session.initialize_evidence().server_name(),
        "fixture-stdio-mcp"
    );
    assert_eq!(
        session.host_observation().await.unwrap().state(),
        CapabilitySurfaceObservedState::Healthy
    );
    assert!(!fixture.lease_dropped.load(Ordering::SeqCst));

    let plan = session.plan().clone();
    let shutdown = session.shutdown().await.unwrap();
    assert!(matches!(
        shutdown.process().state(),
        StdioMcpProcessState::Exited { .. }
    ));
    assert_eq!(
        shutdown.host_observation(&plan).unwrap().state(),
        CapabilitySurfaceObservedState::Stopped
    );
    assert!(fixture.lease_dropped.load(Ordering::SeqCst));
    assert!(host.last_control().unwrap().is_exited());
}

#[tokio::test]
async fn unexpected_process_exit_fails_liveness_and_releases_generation_lease() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-exit", 1_000, 1_000))
        .await
        .unwrap();
    let session = supervisor.start(prepared, lease).await.unwrap();

    host.last_control().unwrap().force_exit(Some(17));
    assert_eq!(
        session.host_observation().await.unwrap().state(),
        CapabilitySurfaceObservedState::Failed
    );
    wait_until(|| fixture.lease_dropped.load(Ordering::SeqCst)).await;
    drop(session);
}

#[tokio::test]
async fn initialize_timeout_terminates_host_and_preserves_primary_error() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Silent,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-timeout", 20, 500))
        .await
        .unwrap();
    let error = supervisor.start(prepared, lease).await.unwrap_err();

    assert_eq!(error.code, "use.plugin.stdio_mcp.initialize_timeout");
    assert!(host.last_control().unwrap().is_exited());
    assert!(fixture.lease_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn spawn_timeout_releases_unstarted_package_lease() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::SpawnPending,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host);
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-spawn-timeout", 20, 500))
        .await
        .unwrap();
    let error = supervisor.start(prepared, lease).await.unwrap_err();

    assert_eq!(error.code, "use.plugin.stdio_mcp.spawn_timeout");
    assert!(fixture.lease_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn provider_drift_after_spawn_fails_closed_and_cleans_up() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![
            host_capabilities("build-1"),
            host_capabilities("build-1"),
            host_capabilities("build-2"),
        ],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-drift", 1_000, 1_000))
        .await
        .unwrap();
    let error = supervisor.start(prepared, lease).await.unwrap_err();

    assert_eq!(error.code, "use.plugin.stdio_mcp.provider_changed");
    assert!(host.last_control().unwrap().is_exited());
    assert!(fixture.lease_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn revoked_grant_cannot_start_a_prepared_session() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-revoked", 1_000, 1_000))
        .await
        .unwrap();
    let revocation = WorkspaceGrantRevocation::new(
        2,
        &fixture.grant,
        fixture.grant.grant.authority.clone(),
        now_ms(),
    )
    .unwrap();
    fixture
        .grants
        .revoke(&fixture.grant, &revocation)
        .await
        .unwrap();

    let error = supervisor.start(prepared, lease).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.stdio_mcp.grant_revoked");
    assert_eq!(host.spawn_count(), 0);
    assert!(fixture.lease_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn live_grant_revocation_terminates_process_and_blocks_peer() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let request = fixture
        .request("session-live-revocation", 1_000, 1_000)
        .with_authorization_recheck_interval(LIVE_GRANT_RECHECK_INTERVAL_MS)
        .unwrap();
    let prepared = supervisor.prepare(&lease, request).await.unwrap();
    let session = supervisor.start(prepared, lease).await.unwrap();
    assert_eq!(
        session.authorization_observation().unwrap().state(),
        StdioMcpAuthorizationState::Active
    );

    let revocation = WorkspaceGrantRevocation::new(
        2,
        &fixture.grant,
        fixture.grant.grant.authority.clone(),
        now_ms(),
    )
    .unwrap();
    fixture
        .grants
        .revoke(&fixture.grant, &revocation)
        .await
        .unwrap();

    wait_until(|| {
        session
            .authorization_observation()
            .is_ok_and(|observation| observation.state() == StdioMcpAuthorizationState::Revoked)
    })
    .await;
    wait_until(|| host.last_control().unwrap().is_exited()).await;
    assert_eq!(
        session.peer().unwrap_err().code,
        "use.plugin.stdio_mcp.grant_revoked"
    );
    assert_eq!(
        session.host_observation().await.unwrap().state(),
        CapabilitySurfaceObservedState::Failed
    );
    wait_until(|| fixture.lease_dropped.load(Ordering::SeqCst)).await;
    drop(session);
}

#[tokio::test]
async fn live_grant_replacement_terminates_exact_planned_revision() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let request = fixture
        .request("session-live-grant-change", 1_000, 1_000)
        .with_authorization_recheck_interval(LIVE_GRANT_RECHECK_INTERVAL_MS)
        .unwrap();
    let prepared = supervisor.prepare(&lease, request).await.unwrap();
    let session = supervisor.start(prepared, lease).await.unwrap();

    let replacement = WorkspaceGrantReceipt::new(2, fixture.grant.grant.clone()).unwrap();
    fixture
        .grants
        .put(&replacement, &fixture.grant.grant.permissions, now_ms())
        .await
        .unwrap();

    wait_until(|| {
        session
            .authorization_observation()
            .is_ok_and(|observation| {
                observation.state() == StdioMcpAuthorizationState::Changed
                    && observation.observed_revision() == Some(2)
            })
    })
    .await;
    wait_until(|| host.last_control().unwrap().is_exited()).await;
    assert_eq!(
        session.peer().unwrap_err().code,
        "use.plugin.stdio_mcp.grant_changed"
    );
    wait_until(|| fixture.lease_dropped.load(Ordering::SeqCst)).await;
    drop(session);
}

#[tokio::test]
async fn native_unconfined_requires_explicit_user_confirmation() {
    let fixture = Fixture::new(PlanPolicyDecision::Allow).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host);
    let lease = fixture.lease();
    let error = supervisor
        .prepare(&lease, fixture.request("session-unconfirmed", 1_000, 1_000))
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.plugin.stdio_mcp.native_confirmation_required"
    );
}

#[tokio::test]
async fn shutdown_timeout_keeps_lease_until_late_terminal_evidence() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::StickyProcess,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-sticky", 1_000, 20))
        .await
        .unwrap();
    let session = supervisor.start(prepared, lease).await.unwrap();
    let error = session.shutdown().await.unwrap_err();

    assert_eq!(error.code, "use.plugin.stdio_mcp.shutdown_timeout");
    assert!(!fixture.lease_dropped.load(Ordering::SeqCst));
    host.last_control().unwrap().force_exit(None);
    wait_until(|| fixture.lease_dropped.load(Ordering::SeqCst)).await;
}

#[tokio::test]
async fn provider_wait_error_keeps_lease_until_late_terminal_evidence() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host);
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-wait-error", 1_000, 1_000))
        .await
        .unwrap();
    let plan = prepared.plan().clone();
    let identity =
        StdioMcpProcessIdentity::new(&plan, "fake-process-wait-error", now_ms()).unwrap();
    let control = Arc::new(FakeControl::new(identity, HostMode::WaitErrorThenSticky));
    let mut settlement = LeaseSettlement::start(lease, plan, control.clone());

    let error = settlement
        .wait(std::time::Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.stdio_mcp.lease_monitor_failed");
    assert!(!fixture.lease_dropped.load(Ordering::SeqCst));

    control.force_exit(None);
    wait_until(|| fixture.lease_dropped.load(Ordering::SeqCst)).await;
    let terminal = settlement
        .wait(std::time::Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        terminal.state(),
        StdioMcpProcessState::Exited { .. }
    ));
}

#[tokio::test]
async fn grant_expiry_terminates_process_and_releases_generation_lease() {
    let fixture = Fixture::new_with_expiry(PlanPolicyDecision::Ask, 500).await;
    let host = Arc::new(FakeHost::new(
        vec![host_capabilities("build-1")],
        HostMode::Responsive,
    ));
    let supervisor = StdioMcpSupervisor::new(&fixture.grants, host.clone());
    let lease = fixture.lease();
    let prepared = supervisor
        .prepare(&lease, fixture.request("session-expiry", 1_000, 1_000))
        .await
        .unwrap();
    let session = supervisor.start(prepared, lease).await.unwrap();

    wait_until(|| host.last_control().unwrap().is_exited()).await;
    wait_until(|| fixture.lease_dropped.load(Ordering::SeqCst)).await;
    assert_eq!(
        session.peer().unwrap_err().code,
        "use.plugin.stdio_mcp.grant_expired"
    );
    assert_eq!(
        session.host_observation().await.unwrap().state(),
        CapabilitySurfaceObservedState::Failed
    );
    drop(session);
}

#[tokio::test]
async fn authorization_recheck_interval_rejects_pathological_bounds() {
    let fixture = Fixture::new(PlanPolicyDecision::Ask).await;
    for interval_ms in [9, 10_001] {
        let error = fixture
            .request("session-invalid-recheck", 1_000, 1_000)
            .with_authorization_recheck_interval(interval_ms)
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.stdio_mcp.input_invalid");
    }
    fixture
        .request("session-valid-recheck", 1_000, 1_000)
        .with_authorization_recheck_interval(10)
        .unwrap();
}

#[test]
fn public_stdio_contracts_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StdioMcpHostCapabilities>();
    assert_send_sync::<StdioMcpHostRoots>();
    assert_send_sync::<StdioMcpAuthorizationObservation>();
    assert_send_sync::<StdioMcpAuthorizationState>();
    assert_send_sync::<StdioMcpSessionRequest>();
    assert_send_sync::<StdioMcpSessionPlan>();
    assert_send_sync::<StdioMcpProcessIdentity>();
    assert_send_sync::<StdioMcpProcessObservation>();
    assert_send_sync::<StdioMcpPackageLease>();
    assert_send_sync::<StdioMcpSupervisor<'static>>();
}

struct Fixture {
    _root: TempDir,
    extension: InstalledExtension,
    grants: WorkspaceGrantStore,
    grant: WorkspaceGrantReceipt,
    roots: StdioMcpHostRoots,
    lease_dropped: Arc<AtomicBool>,
}

impl Fixture {
    async fn new(decision: PlanPolicyDecision) -> Self {
        Self::new_with_expiry(decision, 60_000).await
    }

    async fn new_with_expiry(decision: PlanPolicyDecision, expires_in_ms: u64) -> Self {
        let root = TempDir::new().unwrap();
        let package_root = root.path().join("package");
        tokio::fs::create_dir_all(package_root.join("bin"))
            .await
            .unwrap();
        tokio::fs::write(package_root.join("bin/local-mcp"), b"fixture")
            .await
            .unwrap();
        let package_sha256 = "a".repeat(64);
        let manifest_sha256 = format!("{:x}", Sha256::digest(MANIFEST.as_bytes()));
        let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
        let permission = stdio_permission();
        let permissions = PluginPermissionCeiling {
            schema: PLUGIN_PERMISSION_SCHEMA.to_string(),
            surfaces: vec![permission],
        };
        let record = catalog_record(&package_sha256, &manifest_sha256, permissions.clone());
        let verified = verified_catalog(record);
        let resolved = ResolvedRemotePackage::from_verified_catalog(&verified).unwrap();
        let receipt = ExtensionReceipt {
            schema_version: 2,
            package_id: manifest.package_id.clone(),
            component_id: "use/acme/local-mcp".to_string(),
            route: manifest.route.clone(),
            version: manifest.version.clone(),
            package_root: package_root.clone(),
            manifest_sha256,
            package_sha256: Some(package_sha256.clone()),
            trust: ExtensionTrust::RegistryTuf,
            registry: Some(resolved),
            verified_catalog: Some(verified),
            installed_at_unix: 1,
            enabled: true,
        };
        let extension = InstalledExtension { receipt, manifest };
        extension.plan_ready_catalog().unwrap();
        let now = now_ms();
        let authority = WorkspaceGrantAuthority {
            actor: PlanActor::User,
            decision,
            policy_digest: format!("sha256:{}", "b".repeat(64)),
            confirmation_digest: (decision == PlanPolicyDecision::Ask)
                .then(|| format!("sha256:{}", "c".repeat(64))),
        };
        let grant = a3s_use_core::PluginWorkspaceGrant {
            schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
            scope_id: "workspace-a".to_string(),
            package_id: "acme/local-mcp".to_string(),
            package_digest: format!("sha256:{package_sha256}"),
            permission_ceiling_digest: permissions.descriptor_digest().unwrap(),
            permissions_digest: permissions.descriptor_digest().unwrap(),
            permissions: permissions.clone(),
            authority,
            granted_at_ms: now.saturating_sub(1_000).max(1),
            expires_at_ms: Some(now + expires_in_ms),
        };
        let grant = WorkspaceGrantReceipt::new(1, grant).unwrap();
        let grants = WorkspaceGrantStore::new(root.path().join("state"));
        grants.put(&grant, &permissions, now).await.unwrap();
        let roots = StdioMcpHostRoots::new(
            root.path().join("plugin-data"),
            root.path().join("temporary"),
            root.path().join("workspace"),
        )
        .unwrap();
        Self {
            _root: root,
            extension,
            grants,
            grant,
            roots,
            lease_dropped: Arc::new(AtomicBool::new(false)),
        }
    }

    fn lease(&self) -> StdioMcpPackageLease {
        self.lease_dropped.store(false, Ordering::SeqCst);
        StdioMcpPackageLease::for_test(
            self.extension.clone(),
            LeaseDrop(Arc::clone(&self.lease_dropped)),
        )
    }

    fn request(
        &self,
        session_id: &str,
        initialize_timeout_ms: u64,
        shutdown_timeout_ms: u64,
    ) -> StdioMcpSessionRequest {
        StdioMcpSessionRequest::new(
            session_id,
            "workspace-a",
            "acme/local-mcp",
            "local",
            self.roots.clone(),
        )
        .unwrap()
        .with_timeouts(initialize_timeout_ms, shutdown_timeout_ms)
        .unwrap()
    }
}

struct LeaseDrop(Arc<AtomicBool>);

impl Drop for LeaseDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn stdio_permission() -> SurfacePermissionCeiling {
    SurfacePermissionCeiling {
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Mcp,
            id: "local".to_string(),
        },
        native_execution: true,
        child_process: false,
        filesystem: Vec::new(),
        network_egress: Vec::new(),
        private_service: false,
        secrets: Vec::new(),
        resources: Some(ResourcePermissionCeiling {
            cpu_millis: 500,
            memory_bytes: 256 * 1024 * 1024,
            pids: 32,
            ephemeral_storage_bytes: 128 * 1024 * 1024,
            task_timeout_ms: None,
            max_stdout_bytes: None,
            max_stderr_bytes: None,
        }),
        ui_http: Vec::new(),
    }
}

fn catalog_record(
    package_sha256: &str,
    manifest_sha256: &str,
    permissions: PluginPermissionCeiling,
) -> PluginCatalogRecord {
    PluginCatalogRecord {
        schema: PLUGIN_CATALOG_SCHEMA_V2.to_string(),
        package_id: "acme/local-mcp".to_string(),
        display_name: "Local MCP".to_string(),
        description: "A supervised local standard MCP fixture.".to_string(),
        publisher: "acme".to_string(),
        keywords: vec!["mcp".to_string()],
        categories: vec!["productivity".to_string()],
        version: "1.0.0".to_string(),
        channel: PluginReleaseChannel::Stable,
        requires_use: ">=0.3.0, <0.4.0".to_string(),
        target: "any".to_string(),
        surfaces: vec![CatalogSurface {
            kind: PluginSurfaceKind::Mcp,
            id: "local".to_string(),
            optional: false,
            workload: None,
            mcp_transport: Some(CatalogMcpTransport::Stdio),
            mcp_tool_count: Some(0),
            requires: Vec::new(),
        }],
        permission_ceiling_digest: permissions.descriptor_digest().unwrap(),
        permission_ceiling: permissions,
        planning: None,
        archive: CatalogArchive {
            target_name: "extensions/acme/local-mcp/1.0.0/stable/any/local-mcp-1.0.0-any.tar.gz"
                .to_string(),
            length: 1,
            sha256: format!("sha256:{}", "d".repeat(64)),
        },
        package: CatalogPackage {
            expanded_bytes: 1,
            file_count: 2,
            sha256: Some(format!("sha256:{package_sha256}")),
            manifest_sha256: Some(format!("sha256:{manifest_sha256}")),
        },
        license: "Apache-2.0".to_string(),
        repository: "https://github.com/acme/local-mcp".to_string(),
        availability: CatalogAvailability::Available,
    }
}

fn verified_catalog(record: PluginCatalogRecord) -> VerifiedPluginCatalogRecord {
    VerifiedPluginCatalogRecord::new(
        record.clone(),
        VerifiedCatalogProvenance {
            registry_name: "fixture".to_string(),
            registry_url: "http://127.0.0.1:43111/".to_string(),
            root_sha256: format!("sha256:{}", "e".repeat(64)),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest: record.descriptor_digest().unwrap(),
        },
    )
    .unwrap()
}

fn host_capabilities(build: &str) -> StdioMcpHostCapabilities {
    StdioMcpHostCapabilities::new(
        "fixture-stdio-host",
        build,
        PlanEnforcementProfile::NativeUnconfined,
        vec![
            StdioMcpHostFeature::SanitizedEnvironment,
            StdioMcpHostFeature::OwnedFilesystemRoots,
            StdioMcpHostFeature::ProcessIdentity,
            StdioMcpHostFeature::StderrDrain,
            StdioMcpHostFeature::ProcessTreeCleanup,
        ],
    )
    .unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostMode {
    Responsive,
    Silent,
    StickyProcess,
    WaitErrorThenSticky,
    SpawnPending,
}

struct FakeHost {
    capabilities: Mutex<VecDeque<StdioMcpHostCapabilities>>,
    mode: HostMode,
    last_control: Mutex<Option<Arc<FakeControl>>>,
    spawn_count: Mutex<u64>,
}

impl FakeHost {
    fn new(capabilities: Vec<StdioMcpHostCapabilities>, mode: HostMode) -> Self {
        Self {
            capabilities: Mutex::new(capabilities.into()),
            mode,
            last_control: Mutex::new(None),
            spawn_count: Mutex::new(0),
        }
    }

    fn last_control(&self) -> Option<Arc<FakeControl>> {
        self.last_control.lock().unwrap().clone()
    }

    fn spawn_count(&self) -> u64 {
        *self.spawn_count.lock().unwrap()
    }
}

#[async_trait]
impl StdioMcpHostProvider for FakeHost {
    async fn capabilities(&self) -> a3s_use_core::UseResult<StdioMcpHostCapabilities> {
        let mut capabilities = self.capabilities.lock().unwrap();
        if capabilities.len() > 1 {
            Ok(capabilities.pop_front().unwrap())
        } else {
            capabilities.front().cloned().ok_or_else(|| {
                a3s_use_core::UseError::new(
                    "fixture.capabilities_missing",
                    "Fixture host has no capabilities.",
                )
            })
        }
    }

    async fn spawn(
        &self,
        plan: &StdioMcpSessionPlan,
    ) -> a3s_use_core::UseResult<SpawnedStdioMcpSession> {
        *self.spawn_count.lock().unwrap() += 1;
        if self.mode == HostMode::SpawnPending {
            std::future::pending::<()>().await;
            unreachable!("the pending fixture spawn is cancelled by its timeout");
        }
        let identity = StdioMcpProcessIdentity::new(plan, "fake-process-1", now_ms())?;
        let control = Arc::new(FakeControl::new(identity, self.mode));
        *self.last_control.lock().unwrap() = Some(control.clone());
        let (client, server) = tokio::io::duplex(64 * 1024);
        spawn_server(server, control.clone(), self.mode);
        let (reader, writer) = tokio::io::split(client);
        SpawnedStdioMcpSession::new(reader, writer, control)
    }
}

struct FakeControl {
    identity: StdioMcpProcessIdentity,
    state: watch::Sender<StdioMcpProcessState>,
    cancellation: CancellationToken,
    sticky: bool,
    wait_error_once: AtomicBool,
}

impl FakeControl {
    fn new(identity: StdioMcpProcessIdentity, mode: HostMode) -> Self {
        let (state, _) = watch::channel(StdioMcpProcessState::Running);
        Self {
            identity,
            state,
            cancellation: CancellationToken::new(),
            sticky: matches!(
                mode,
                HostMode::StickyProcess | HostMode::WaitErrorThenSticky
            ),
            wait_error_once: AtomicBool::new(mode == HostMode::WaitErrorThenSticky),
        }
    }

    fn force_exit(&self, exit_code: Option<i32>) {
        self.cancellation.cancel();
        self.state
            .send_replace(StdioMcpProcessState::Exited { exit_code });
    }

    fn is_exited(&self) -> bool {
        matches!(*self.state.borrow(), StdioMcpProcessState::Exited { .. })
    }
}

#[async_trait]
impl StdioMcpProcessControl for FakeControl {
    fn identity(&self) -> &StdioMcpProcessIdentity {
        &self.identity
    }

    async fn observe(&self) -> a3s_use_core::UseResult<StdioMcpProcessObservation> {
        observation(&self.identity, *self.state.borrow())
    }

    async fn wait_for_exit(&self) -> a3s_use_core::UseResult<StdioMcpProcessObservation> {
        if self.wait_error_once.swap(false, Ordering::SeqCst) {
            return Err(a3s_use_core::UseError::new(
                "fixture.wait_failed",
                "Fixture provider could not observe the process tree.",
            ));
        }
        let mut state = self.state.subscribe();
        loop {
            let current = *state.borrow_and_update();
            if matches!(current, StdioMcpProcessState::Exited { .. }) {
                return observation(&self.identity, current);
            }
            state.changed().await.map_err(|_| {
                a3s_use_core::UseError::new(
                    "fixture.process_state_closed",
                    "Fixture process state closed before exit.",
                )
            })?;
        }
    }

    fn terminate(&self) {
        self.cancellation.cancel();
        if !self.sticky {
            self.state
                .send_replace(StdioMcpProcessState::Exited { exit_code: None });
        }
    }
}

fn spawn_server(server: DuplexStream, control: Arc<FakeControl>, mode: HostMode) {
    tokio::spawn(async move {
        match mode {
            HostMode::Silent | HostMode::SpawnPending => {
                let _server = server;
                control.cancellation.cancelled().await;
            }
            HostMode::Responsive | HostMode::StickyProcess | HostMode::WaitErrorThenSticky => {
                if let Ok(service) = TestServer.serve(server).await {
                    tokio::select! {
                        _ = service.waiting() => {}
                        _ = control.cancellation.cancelled() => {}
                    }
                }
            }
        }
        if !matches!(
            mode,
            HostMode::StickyProcess | HostMode::WaitErrorThenSticky
        ) {
            control
                .state
                .send_replace(StdioMcpProcessState::Exited { exit_code: Some(0) });
        }
    });
}

#[derive(Debug, Clone, Default)]
struct TestServer;

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::default(),
            server_info: Implementation {
                name: "fixture-stdio-mcp".to_string(),
                title: Some("Fixture stdio MCP".to_string()),
                version: "1.0.0".to_string(),
                icons: None,
                website_url: None,
            },
            instructions: None,
            ..Default::default()
        }
    }
}

fn observation(
    identity: &StdioMcpProcessIdentity,
    state: StdioMcpProcessState,
) -> a3s_use_core::UseResult<StdioMcpProcessObservation> {
    match state {
        StdioMcpProcessState::Running => {
            StdioMcpProcessObservation::running(identity.clone(), now_ms())
        }
        StdioMcpProcessState::Exited { exit_code } => {
            StdioMcpProcessObservation::exited(identity.clone(), exit_code, now_ms())
        }
    }
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

mod native_host;
