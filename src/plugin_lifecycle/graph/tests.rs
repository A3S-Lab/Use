use std::sync::atomic::{AtomicU64, Ordering};

use a3s_use_core::{
    CatalogAvailability, PlanActor, PlanAuthority, PlanPackageChangeKind, PlanPackageRole,
    PlanPolicyDecision, PlanScope, PlanScopeKind, PlannedOperationImpact, PlannedPackageTransition,
    PlannedStateEvidence, PluginCatalogRecord, PluginOperationAction, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginPackageDependency,
    PluginPackageLockHost, PluginPackageResolver, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{
    ExtensionManifest, PluginMcpSurface, PluginOkfSurface, PluginSkillSurface, PluginUiSurface,
    ToolSurface,
};
use tokio::sync::Mutex;

use super::*;
use crate::plugin_lifecycle::{
    PluginCapabilityLifecycleHost, PluginLifecycleHosts, PluginLifecycleIntentSpec,
    PluginLifecycleJournalStore, PluginLifecycleOperationStatus, PluginMcpLifecycleHost,
    PluginOkfLifecycleHost, PluginPackageLifecycleHost, PluginSkillLifecycleHost,
    PluginToolLifecycleHost, PluginUiLifecycleHost,
};

const CATALOG: &[u8] =
    include_bytes!("../../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");
const MANIFEST: &str =
    include_str!("../../../crates/extension/fixtures/manifests/plugin-v3-okf.acl");

#[derive(Default)]
struct RecordingHost {
    calls: Mutex<Vec<String>>,
}

impl RecordingHost {
    async fn evidence(
        &self,
        label: &str,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.calls
            .lock()
            .await
            .push(format!("{}:{label}", intent.package_id));
        PluginLifecycleEvidence::new(format!(
            "sha256:{:x}",
            Sha256::digest(format!("{}\n{label}\n{key}", intent.package_id).as_bytes())
        ))
    }
}

#[async_trait]
impl PluginPackageLifecycleHost for RecordingHost {
    async fn commit_package(
        &self,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("commit", intent, key).await
    }

    async fn remove_package(
        &self,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("remove", intent, key).await
    }
}

#[async_trait]
impl PluginCapabilityLifecycleHost for RecordingHost {
    async fn publish_capability(
        &self,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("single-publish", intent, key).await
    }

    async fn hide_capability(
        &self,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("hide", intent, key).await
    }

    async fn drain_calls(
        &self,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("drain", intent, key).await
    }
}

#[async_trait]
impl PluginToolLifecycleHost for RecordingHost {
    async fn prepare_tool(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &ToolSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("tool-prepare", intent, key).await
    }
    async fn stop_tool(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &ToolSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("tool-stop", intent, key).await
    }
    async fn remove_tool(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &ToolSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("tool-remove", intent, key).await
    }
}

#[async_trait]
impl PluginMcpLifecycleHost for RecordingHost {
    async fn prepare_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginMcpSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("mcp-prepare", intent, key).await
    }
    async fn stop_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginMcpSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("mcp-stop", intent, key).await
    }
    async fn remove_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginMcpSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("mcp-remove", intent, key).await
    }
}

#[async_trait]
impl PluginOkfLifecycleHost for RecordingHost {
    async fn prepare_okf(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginOkfSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("okf-prepare", intent, key).await
    }
    async fn stop_okf(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginOkfSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("okf-stop", intent, key).await
    }
    async fn remove_okf(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginOkfSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("okf-remove", intent, key).await
    }
}

#[async_trait]
impl PluginSkillLifecycleHost for RecordingHost {
    async fn prepare_skill(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginSkillSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("skill-prepare", intent, key).await
    }
    async fn stop_skill(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginSkillSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("skill-stop", intent, key).await
    }
    async fn remove_skill(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginSkillSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("skill-remove", intent, key).await
    }
}

#[async_trait]
impl PluginUiLifecycleHost for RecordingHost {
    async fn prepare_ui(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginUiSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("ui-prepare", intent, key).await
    }
    async fn stop_ui(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginUiSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("ui-stop", intent, key).await
    }
    async fn remove_ui(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginUiSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("ui-remove", intent, key).await
    }
}

#[async_trait]
impl PluginGraphCapabilityLifecycleHost for RecordingHost {
    async fn publish_capabilities(
        &self,
        _package_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        key: &str,
    ) -> UseResult<Vec<PluginPackagePublicationEvidence>> {
        self.calls.lock().await.push(format!(
            "batch:{}",
            intents
                .iter()
                .map(|intent| intent.package_id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ));
        intents
            .iter()
            .map(|intent| {
                let evidence = PluginLifecycleEvidence::new(format!(
                    "sha256:{:x}",
                    Sha256::digest(format!("{}\n{key}", intent.package_id).as_bytes())
                ))?;
                PluginPackagePublicationEvidence::new(&intent.package_id, evidence)
            })
            .collect()
    }
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

fn dependency(package_id: &str) -> PluginPackageDependency {
    PluginPackageDependency::new(package_id, "^1.0.0").unwrap()
}

fn catalog(
    package_id: &str,
    dependencies: Vec<PluginPackageDependency>,
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    record.package_id = package_id.to_string();
    record.publisher = publisher.to_string();
    record.display_name = format!("{publisher} {name}");
    record.description = format!("Graph fixture for {package_id}.");
    record.dependencies = dependencies;
    record.repository = format!("https://github.com/{publisher}/{name}");
    record.archive.target_name = format!(
        "extensions/{package_id}/1.0.0/stable/linux-x86_64/{publisher}-{name}-1.0.0.tar.gz"
    );
    record.archive.sha256 = digest(seed);
    record.package.sha256 = Some(digest(seed));
    record.package.manifest_sha256 = Some(digest(seed));
    record.availability = CatalogAvailability::Available;
    record.validate().unwrap();
    let catalog_record_digest = record.descriptor_digest().unwrap();
    VerifiedPluginCatalogRecord::new(
        record,
        VerifiedCatalogProvenance {
            registry_name: "official".to_string(),
            registry_url: "https://packages.example.test/catalog/".to_string(),
            root_sha256: digest('f'),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest,
        },
    )
    .unwrap()
}

fn manifest(package_id: &str, dependency: Option<&str>) -> ExtensionManifest {
    let name = package_id.split_once('/').unwrap().1;
    let mut input = MANIFEST.replace("acme/knowledge", package_id).replace(
        "route          = \"knowledge\"",
        &format!("route          = \"{name}\""),
    );
    if let Some(dependency) = dependency {
        input = input.replace(
            "  repository {",
            &format!(
                "  dependency \"{dependency}\" {{\n    version = \"^1.0.0\"\n  }}\n\n  repository {{"
            ),
        );
    }
    ExtensionManifest::parse_acl(&input).unwrap()
}

fn coordinator(root: &std::path::Path, host: Arc<RecordingHost>) -> PluginLifecycleCoordinator {
    let hosts = PluginLifecycleHosts::new(
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host.clone(),
        host,
    );
    PluginLifecycleCoordinator::new(PluginLifecycleJournalStore::new(root), hosts)
}

struct InstallGraphFixture {
    _temp: tempfile::TempDir,
    envelope: PluginOperationPlanEnvelope,
    units: Vec<PluginPackageLifecycleUnit>,
    host: Arc<RecordingHost>,
}

fn install_graph_fixture(retain_base: bool) -> InstallGraphFixture {
    let root_catalog = catalog("acme/root", vec![dependency("acme/base")], 'a');
    let base_catalog = catalog("acme/base", Vec::new(), 'b');
    let lock =
        PluginPackageResolver::new(PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap())
            .resolve(root_catalog, vec![base_catalog])
            .unwrap();
    let mut transitions = lock
        .packages
        .iter()
        .map(|package| {
            let role = if package.package_id() == lock.root_package_id {
                PlanPackageRole::Root
            } else {
                PlanPackageRole::Dependency
            };
            if retain_base && package.package_id() == "acme/base" {
                let state = package.catalog.selected_state(&[])?;
                PlannedPackageTransition::resolved(
                    package.package_id(),
                    role,
                    PlanPackageChangeKind::Retain,
                    Some(state.clone()),
                    Some(state),
                    None,
                )
            } else {
                package.catalog.install_transition(role, &[])
            }
        })
        .collect::<UseResult<Vec<_>>>()
        .unwrap();
    transitions.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        "acme/root",
        "runtime:local",
        transitions,
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: lock
                .packages
                .iter()
                .map(|package| package.catalog.record.archive.length)
                .sum(),
            installed_bytes_after: lock
                .packages
                .iter()
                .map(|package| package.catalog.record.package.expanded_bytes)
                .sum(),
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 1,
            capability_generation: 1,
            receipt_digest: None,
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: "install:acme-root:graph-1".to_string(),
        created_at_ms: 1,
        expires_at_ms: 2,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "current".to_string(),
        },
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('9'),
            confirmation_required: true,
        },
    })
    .unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(plan, lock.clone()).unwrap();

    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHost::default());
    let units = lock
        .install_order()
        .unwrap()
        .into_iter()
        .enumerate()
        .filter_map(|(index, package)| {
            let transition = envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
                .unwrap();
            if transition.change == PlanPackageChangeKind::Retain {
                return None;
            }
            let dependency = (package.package_id() == "acme/root").then_some("acme/base");
            let manifest = manifest(package.package_id(), dependency);
            let state = transition.after.as_ref().unwrap();
            let intent = PluginLifecycleIntent::from_manifest(
                PluginLifecycleIntentSpec {
                    operation_id: envelope.plan.operation_id.clone(),
                    plan_digest: envelope.plan_digest.clone(),
                    scope_id: envelope.plan.scope.id.clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: state.release.package_sha256.clone(),
                    manifest_digest: state.release.manifest_sha256.clone(),
                    generation: index as u64 + 1,
                    action: PluginLifecycleAction::Install,
                },
                &manifest,
            )
            .unwrap();
            Some(
                PluginPackageLifecycleUnit::new(
                    coordinator(
                        &temp.path().join(package.package_id().replace('/', "-")),
                        host.clone(),
                    ),
                    intent,
                    manifest,
                )
                .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    InstallGraphFixture {
        _temp: temp,
        envelope,
        units,
        host,
    }
}

#[tokio::test]
async fn dependency_closure_prepares_forward_then_publishes_once() {
    let fixture = install_graph_fixture(false);
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(0);
    let records = graph
        .apply_install(&fixture.envelope, &fixture.units, || {
            time.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .unwrap();
    assert!(records
        .iter()
        .all(|record| record.status == PluginLifecycleOperationStatus::Completed));
    assert_eq!(
        fixture.host.calls.lock().await.as_slice(),
        [
            "acme/base:commit",
            "acme/base:okf-prepare",
            "acme/base:skill-prepare",
            "acme/root:commit",
            "acme/root:okf-prepare",
            "acme/root:skill-prepare",
            "batch:acme/base,acme/root",
        ]
    );
}

#[tokio::test]
async fn dependency_closure_reuses_a_reviewed_retained_dependency() {
    let fixture = install_graph_fixture(true);
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let records = graph
        .apply_install(&fixture.envelope, &fixture.units, || 1)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        fixture.host.calls.lock().await.as_slice(),
        [
            "acme/root:commit",
            "acme/root:okf-prepare",
            "acme/root:skill-prepare",
            "batch:acme/root",
        ]
    );
}
