use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::{io, path::Path, path::PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::*;
use crate::plugin_lifecycle::test_support::{intent, manifest};
use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleCheckpointOutcome, PluginLifecycleIntentSpec,
    PluginLifecycleOperationStatus,
};

#[derive(Default)]
struct RecordingHosts {
    calls: Mutex<Vec<String>>,
    fail_once: Mutex<Option<String>>,
    durable_root: Option<PathBuf>,
    crash_after_effect: Option<String>,
}

impl RecordingHosts {
    async fn with_failure(label: &str) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_once: Mutex::new(Some(label.to_string())),
            durable_root: None,
            crash_after_effect: None,
        }
    }

    fn with_durable_checkpoint_crash(root: PathBuf, crash_after_effect: Option<String>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_once: Mutex::new(None),
            durable_root: Some(root),
            crash_after_effect,
        }
    }

    async fn execute(&self, label: String, key: &str) -> UseResult<PluginLifecycleEvidence> {
        self.calls.lock().await.push(label.clone());
        let mut failure = self.fail_once.lock().await;
        if failure.as_deref() == Some(label.as_str()) {
            *failure = None;
            return Err(UseError::new(
                "use.plugin.injected_failure",
                "Injected lifecycle host failure.",
            ));
        }
        self.record_durable_effect(&label, key).await?;
        PluginLifecycleEvidence::new(format!("sha256:{:x}", Sha256::digest(key.as_bytes())))
    }

    async fn record_durable_effect(&self, label: &str, key: &str) -> UseResult<()> {
        let Some(root) = self.durable_root.as_ref() else {
            return Ok(());
        };
        let effects = root.join("effects");
        tokio::fs::create_dir_all(&effects)
            .await
            .map_err(|error| durable_host_io("create durable effect directory", error))?;

        let attempt = format!("{key}\t{label}\n");
        let mut attempts = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("attempts.log"))
            .await
            .map_err(|error| durable_host_io("open durable attempt log", error))?;
        attempts
            .write_all(attempt.as_bytes())
            .await
            .map_err(|error| durable_host_io("append durable attempt log", error))?;
        attempts
            .sync_all()
            .await
            .map_err(|error| durable_host_io("sync durable attempt log", error))?;
        drop(attempts);

        let identity = format!("{:x}", Sha256::digest(key.as_bytes()));
        let path = effects.join(format!("{identity}.effect"));
        let effect = format!("{key}\n{label}\n");
        match tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                file.write_all(effect.as_bytes())
                    .await
                    .map_err(|error| durable_host_io("write durable effect", error))?;
                file.sync_all()
                    .await
                    .map_err(|error| durable_host_io("sync durable effect", error))?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = tokio::fs::read(&path)
                    .await
                    .map_err(|error| durable_host_io("read replayed durable effect", error))?;
                if existing != effect.as_bytes() {
                    return Err(UseError::new(
                        "use.plugin.test_durable_effect_conflict",
                        "A replay reused a lifecycle idempotency key with different host evidence.",
                    ));
                }
            }
            Err(error) => return Err(durable_host_io("create durable effect", error)),
        }

        if self.crash_after_effect.as_deref() == Some(key) {
            std::process::exit(fault_matrix::CHECKPOINT_CRASH_EXIT_CODE);
        }
        Ok(())
    }

    async fn calls(&self) -> Vec<String> {
        self.calls.lock().await.clone()
    }
}

fn durable_host_io(action: &str, error: io::Error) -> UseError {
    UseError::new(
        "use.plugin.test_durable_host_io",
        format!("Failed to {action}: {error}"),
    )
}

#[async_trait]
impl PluginPackageLifecycleHost for RecordingHosts {
    async fn commit_package(
        &self,
        _intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute("package.commit".to_string(), key).await
    }

    async fn remove_package(
        &self,
        _intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute("package.remove".to_string(), key).await
    }
}

#[async_trait]
impl PluginCapabilityLifecycleHost for RecordingHosts {
    async fn publish_capability_with_cutover(
        &self,
        _intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginCapabilityPublication> {
        let evidence = self.execute("capability.publish".to_string(), key).await?;
        Ok(PluginCapabilityPublication::new(
            evidence,
            PluginCapabilityCutoverEvidence::new(1, 2, format!("sha256:{}", "a".repeat(64)))?,
        ))
    }

    async fn hide_capability_with_cutover(
        &self,
        _intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginCapabilityPublication> {
        let evidence = self.execute("capability.hide".to_string(), key).await?;
        Ok(PluginCapabilityPublication::new(
            evidence,
            PluginCapabilityCutoverEvidence::new(1, 2, format!("sha256:{}", "b".repeat(64)))?,
        ))
    }

    async fn retire_hidden_capability(
        &self,
        _intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute("capability.hide".to_string(), key).await
    }

    async fn drain_calls(
        &self,
        _intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute("capability.drain".to_string(), key).await
    }
}

#[async_trait]
impl PluginToolLifecycleHost for RecordingHosts {
    async fn prepare_tool(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("tool.prepare:{}", surface.id), key)
            .await
    }

    async fn stop_tool(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("tool.stop:{}", surface.id), key).await
    }

    async fn remove_tool(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("tool.remove:{}", surface.id), key)
            .await
    }
}

#[async_trait]
impl PluginMcpLifecycleHost for RecordingHosts {
    async fn prepare_mcp(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("mcp.prepare:{}", surface.id), key)
            .await
    }

    async fn stop_mcp(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("mcp.stop:{}", surface.id), key).await
    }

    async fn remove_mcp(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("mcp.remove:{}", surface.id), key)
            .await
    }
}

#[async_trait]
impl PluginOkfLifecycleHost for RecordingHosts {
    async fn prepare_okf(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("okf.prepare:{}", surface.id), key)
            .await
    }

    async fn stop_okf(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("okf.stop:{}", surface.id), key).await
    }

    async fn remove_okf(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("okf.remove:{}", surface.id), key)
            .await
    }
}

#[async_trait]
impl PluginFlowLifecycleHost for RecordingHosts {
    async fn prepare_flow(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("flow.prepare:{}", surface.id), key)
            .await
    }

    async fn stop_flow(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("flow.stop:{}", surface.id), key).await
    }

    async fn remove_flow(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("flow.remove:{}", surface.id), key)
            .await
    }
}

#[async_trait]
impl PluginSkillLifecycleHost for RecordingHosts {
    async fn prepare_skill(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("skill.prepare:{}", surface.id), key)
            .await
    }

    async fn stop_skill(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("skill.stop:{}", surface.id), key)
            .await
    }

    async fn remove_skill(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("skill.remove:{}", surface.id), key)
            .await
    }
}

#[async_trait]
impl PluginUiLifecycleHost for RecordingHosts {
    async fn prepare_ui(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("ui.prepare:{}", surface.id), key)
            .await
    }

    async fn stop_ui(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("ui.stop:{}", surface.id), key).await
    }

    async fn remove_ui(
        &self,
        _intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.execute(format!("ui.remove:{}", surface.id), key).await
    }
}

fn coordinator(temp: &tempfile::TempDir, host: Arc<RecordingHosts>) -> PluginLifecycleCoordinator {
    coordinator_at(&temp.path().join("state"), host)
}

fn coordinator_at(state: &Path, host: Arc<RecordingHosts>) -> PluginLifecycleCoordinator {
    let hosts = PluginLifecycleHosts::new(
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host,
    );
    PluginLifecycleCoordinator::new(PluginLifecycleJournalStore::new(state), hosts)
}

fn clock() -> impl Fn() -> u64 {
    clock_from(0)
}

fn clock_from(start: u64) -> impl Fn() -> u64 {
    let clock = Arc::new(AtomicU64::new(start));
    move || clock.fetch_add(1, Ordering::Relaxed) + 1
}

#[tokio::test]
async fn installs_all_surface_hosts_before_one_capability_publication() {
    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHosts::default());
    let coordinator = coordinator(&temp, host.clone());
    let intent = intent(PluginLifecycleAction::Install);

    let completed = coordinator
        .apply(&intent, &manifest(), clock_from(100))
        .await
        .unwrap();
    assert_eq!(completed.status, PluginLifecycleOperationStatus::Completed);
    assert_eq!(
        host.calls().await,
        [
            "package.commit",
            "mcp.prepare:catalog",
            "okf.prepare:papers",
            "tool.prepare:query",
            "flow.prepare:review",
            "skill.prepare:review",
            "ui.prepare:review",
            "capability.publish",
        ]
    );

    coordinator
        .apply(&intent, &manifest(), clock())
        .await
        .unwrap();
    assert_eq!(host.calls().await.len(), 8);
}

#[tokio::test]
async fn required_failure_withholds_publication_and_retry_resumes_exact_step() {
    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHosts::with_failure("okf.prepare:papers").await);
    let coordinator = coordinator(&temp, host.clone());
    let intent = intent(PluginLifecycleAction::Install);

    let error = coordinator
        .apply(&intent, &manifest(), clock())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.injected_failure");
    assert_eq!(
        host.calls().await,
        [
            "package.commit",
            "mcp.prepare:catalog",
            "okf.prepare:papers"
        ]
    );

    let completed = coordinator
        .apply(&intent, &manifest(), clock_from(100))
        .await
        .unwrap();
    assert_eq!(completed.status, PluginLifecycleOperationStatus::Completed);
    assert_eq!(
        &host.calls().await[3..],
        [
            "okf.prepare:papers",
            "tool.prepare:query",
            "flow.prepare:review",
            "skill.prepare:review",
            "ui.prepare:review",
            "capability.publish",
        ]
    );
}

#[tokio::test]
async fn optional_surface_failure_publishes_degraded_package_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHosts::with_failure("ui.prepare:review").await);
    let coordinator = coordinator(&temp, host.clone());
    let mut manifest = manifest();
    manifest.ui[0].optional = true;
    let intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: "install:acme-research:optional".to_string(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope: a3s_use_core::PlanScope {
                kind: a3s_use_core::PlanScopeKind::Workspace,
                id: "research".to_string(),
            },
            package_id: "acme/research".to_string(),
            package_digest: format!("sha256:{}", "2".repeat(64)),
            manifest_digest: format!("sha256:{}", "3".repeat(64)),
            generation: 8,
            action: PluginLifecycleAction::Install,
            retained_ui_state_surfaces: Vec::new(),
        },
        &manifest,
    )
    .unwrap();

    let completed = coordinator
        .apply(&intent, &manifest, clock())
        .await
        .unwrap();
    assert_eq!(completed.status, PluginLifecycleOperationStatus::Completed);
    assert!(completed.receipts.iter().any(|receipt| {
        receipt.outcome == PluginLifecycleCheckpointOutcome::OptionalFailed
            && receipt.error_code.as_deref() == Some("use.plugin.injected_failure")
    }));
    assert_eq!(host.calls().await.last().unwrap(), "capability.publish");
}

#[tokio::test]
async fn uninstall_hides_then_drains_and_removes_every_surface_before_package() {
    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHosts::default());
    let coordinator = coordinator(&temp, host.clone());
    let intent = intent(PluginLifecycleAction::Uninstall);

    coordinator
        .apply(&intent, &manifest(), clock())
        .await
        .unwrap();
    assert_eq!(
        host.calls().await,
        [
            "capability.hide",
            "capability.drain",
            "ui.remove:review",
            "skill.remove:review",
            "flow.remove:review",
            "tool.remove:query",
            "okf.remove:papers",
            "mcp.remove:catalog",
            "package.remove",
        ]
    );
}

mod fault_matrix;
