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
    ExtensionManifest, PluginFlowSurface, PluginMcpSurface, PluginOkfSurface, PluginSkillSurface,
    PluginUiSurface, ToolSurface,
};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::*;
use crate::plugin_lifecycle::{
    PluginCapabilityLifecycleHost, PluginFlowLifecycleHost, PluginLifecycleHosts,
    PluginLifecycleIntentSpec, PluginLifecycleJournalStore, PluginLifecycleOperationStatus,
    PluginMcpLifecycleHost, PluginOkfLifecycleHost, PluginPackageLifecycleHost,
    PluginSkillLifecycleHost, PluginToolLifecycleHost, PluginUiLifecycleHost,
};

const CATALOG: &[u8] =
    include_bytes!("../../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");
const MANIFEST: &str =
    include_str!("../../../crates/extension/fixtures/manifests/plugin-v3-okf.acl");

#[derive(Default)]
struct RecordingHost {
    calls: Mutex<Vec<String>>,
    fail_once: Mutex<Option<String>>,
    publication_fault: Mutex<Option<PublicationFault>>,
    fail_exact_publication_once: Mutex<bool>,
    drift_cutover_generation_once: Mutex<bool>,
    cutover_generation_before: AtomicU64,
}

#[derive(Clone, Copy)]
enum PublicationFault {
    ReverseEvidence,
}

impl RecordingHost {
    async fn evidence(
        &self,
        label: &str,
        intent: &PluginLifecycleIntent,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let call = format!("{}:{label}", intent.package_id);
        self.calls.lock().await.push(call.clone());
        let mut failure = self.fail_once.lock().await;
        if failure.as_deref() == Some(&call) {
            *failure = None;
            return Err(UseError::new(
                "use.plugin.test_injected_failure",
                "The lifecycle test host injected a candidate preparation failure.",
            ));
        }
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
impl PluginFlowLifecycleHost for RecordingHost {
    async fn prepare_flow(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginFlowSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("flow-prepare", intent, key).await
    }
    async fn stop_flow(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginFlowSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("flow-stop", intent, key).await
    }
    async fn remove_flow(
        &self,
        intent: &PluginLifecycleIntent,
        _surface: &PluginFlowSurface,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.evidence("flow-remove", intent, key).await
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
        let mut evidence = intents
            .iter()
            .map(|intent| {
                let evidence = PluginLifecycleEvidence::new(format!(
                    "sha256:{:x}",
                    Sha256::digest(format!("{}\n{key}", intent.package_id).as_bytes())
                ))?;
                PluginPackagePublicationEvidence::new(&intent.package_id, evidence)
            })
            .collect::<UseResult<Vec<_>>>()?;
        if matches!(
            self.publication_fault.lock().await.take(),
            Some(PublicationFault::ReverseEvidence)
        ) {
            evidence.reverse();
        }
        Ok(evidence)
    }

    async fn publish_capabilities_with_cutover(
        &self,
        package_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        let mut fail = self.fail_exact_publication_once.lock().await;
        if *fail {
            *fail = false;
            return Err(UseError::new(
                "use.plugin.test_publication_failure",
                "The lifecycle test host rejected capability publication.",
            ));
        }
        drop(fail);
        let packages = self
            .publish_capabilities(package_lock, intents, key)
            .await?;
        let mut drift = self.drift_cutover_generation_once.lock().await;
        let configured = self.cutover_generation_before.load(Ordering::Relaxed);
        let mut generation_before = configured.max(1);
        if *drift {
            generation_before = generation_before.saturating_add(1);
        }
        *drift = false;
        Ok(PluginGraphCapabilityPublication::new(
            packages,
            PluginCapabilityCutoverEvidence::new(
                generation_before,
                generation_before + 1,
                digest('6'),
            )?,
        ))
    }

    async fn publish_upgrade_capabilities_with_cutover(
        &self,
        package_lock: &a3s_use_core::PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        removed_intents: &[PluginLifecycleIntent],
        key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        if !removed_intents.is_empty() {
            return Err(UseError::new(
                "use.plugin.test_removed_cutover_unsupported",
                "The lifecycle test host does not model removed-only cutover here.",
            ));
        }
        self.publish_capabilities_with_cutover(package_lock, candidate_intents, key)
            .await
    }

    async fn hide_capabilities_with_cutover(
        &self,
        _package_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        self.calls.lock().await.push(format!(
            "hide-batch:{}",
            intents
                .iter()
                .map(|intent| intent.package_id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ));
        let packages = intents
            .iter()
            .map(|intent| {
                PluginPackagePublicationEvidence::new(
                    &intent.package_id,
                    PluginLifecycleEvidence::new(format!(
                        "sha256:{:x}",
                        Sha256::digest(format!("{}\n{key}\nhide", intent.package_id).as_bytes())
                    ))?,
                )
            })
            .collect::<UseResult<Vec<_>>>()?;
        let configured = self.cutover_generation_before.load(Ordering::Relaxed);
        let generation_before = configured.max(1);
        Ok(PluginGraphCapabilityPublication::new(
            packages,
            PluginCapabilityCutoverEvidence::new(
                generation_before,
                generation_before + 1,
                digest('6'),
            )?,
        ))
    }

    async fn rollback_candidates(
        &self,
        candidate_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        _prior_intents: &[PluginLifecycleIntent],
        key: &str,
    ) -> UseResult<Vec<PluginPackageRollbackEvidence>> {
        let by_package = intents
            .iter()
            .map(|intent| (intent.package_id.as_str(), intent))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut evidence = Vec::new();
        for package in candidate_lock.removal_order()? {
            let Some(intent) = by_package.get(package.package_id()).copied() else {
                continue;
            };
            self.calls
                .lock()
                .await
                .push(format!("{}:candidate-rollback", intent.package_id));
            evidence.push(PluginPackageRollbackEvidence::new(
                &intent.package_id,
                PluginLifecycleEvidence::new(format!(
                    "sha256:{:x}",
                    Sha256::digest(format!("{}\n{key}\nrollback", intent.package_id).as_bytes())
                ))?,
            )?);
        }
        Ok(evidence)
    }
}

mod grant;

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
    catalog_version(package_id, dependencies, "1.0.0", seed)
}

fn catalog_version(
    package_id: &str,
    dependencies: Vec<PluginPackageDependency>,
    version: &str,
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    record.package_id = package_id.to_string();
    record.publisher = publisher.to_string();
    record.display_name = format!("{publisher} {name}");
    record.description = format!("Graph fixture for {package_id}.");
    record.version = version.to_string();
    record.dependencies = dependencies;
    record.repository = format!("https://github.com/{publisher}/{name}");
    record.archive.target_name = format!(
        "extensions/{package_id}/{version}/stable/linux-x86_64/{publisher}-{name}-{version}.tar.gz"
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
    manifest_version(package_id, dependency, "1.0.0")
}

fn manifest_version(
    package_id: &str,
    dependency: Option<&str>,
    version: &str,
) -> ExtensionManifest {
    let name = package_id.split_once('/').unwrap().1;
    let mut input = MANIFEST
        .replace("acme/knowledge", package_id)
        .replace(
            "route          = \"knowledge\"",
            &format!("route          = \"{name}\""),
        )
        .replace(
            "version        = \"1.0.0\"",
            &format!("version        = \"{version}\""),
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

struct UpgradeGraphFixture {
    _temp: tempfile::TempDir,
    envelope: PluginOperationPlanEnvelope,
    prior_lock: a3s_use_core::PluginPackageLock,
    candidates: Vec<PluginPackageLifecycleUnit>,
    retirements: Vec<PluginPackageLifecycleUnit>,
    host: Arc<RecordingHost>,
}

fn upgrade_graph_fixture() -> UpgradeGraphFixture {
    let old_root = catalog_version("acme/root", vec![dependency("acme/base")], "1.0.0", 'c');
    let old_base = catalog_version("acme/base", Vec::new(), "1.0.0", 'd');
    let prior_lock =
        PluginPackageResolver::new(PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap())
            .resolve(old_root, vec![old_base])
            .unwrap();
    let next_root = catalog_version("acme/root", vec![dependency("acme/base")], "1.1.0", 'a');
    let next_base = catalog_version("acme/base", Vec::new(), "1.1.0", 'b');
    let next_lock =
        PluginPackageResolver::new(PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap())
            .resolve(next_root, vec![next_base])
            .unwrap();
    let mut transitions = next_lock
        .packages
        .iter()
        .map(|package| {
            let prior = prior_lock.package(package.package_id()).unwrap();
            let role = if package.package_id() == next_lock.root_package_id {
                PlanPackageRole::Root
            } else {
                PlanPackageRole::Dependency
            };
            package
                .catalog
                .replace_transition(&prior.catalog, role, &[], &[])
        })
        .collect::<UseResult<Vec<_>>>()
        .unwrap();
    transitions.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Upgrade,
        "acme/root",
        "runtime:local",
        transitions,
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: next_lock
                .packages
                .iter()
                .map(|package| package.catalog.record.archive.length)
                .sum(),
            installed_bytes_after: next_lock
                .packages
                .iter()
                .map(|package| package.catalog.record.package.expanded_bytes)
                .sum(),
            reclaimed_bytes: 1,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 2,
            capability_generation: 2,
            receipt_digest: Some(digest('8')),
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: "upgrade:acme-root:graph-2".to_string(),
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
    let envelope =
        PluginOperationPlanEnvelope::new_with_package_lock(plan, next_lock.clone()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHost::default());
    let package_root = |package_id: &str| temp.path().join(package_id.replace('/', "-"));
    let candidates = next_lock
        .install_order()
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, package)| {
            let dependency = (package.package_id() == "acme/root").then_some("acme/base");
            let manifest = manifest_version(package.package_id(), dependency, "1.1.0");
            let transition = envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
                .unwrap();
            let state = transition.after.as_ref().unwrap();
            let intent = PluginLifecycleIntent::from_manifest(
                PluginLifecycleIntentSpec {
                    operation_id: envelope.plan.operation_id.clone(),
                    plan_digest: envelope.plan_digest.clone(),
                    scope_id: envelope.plan.scope.id.clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: state.release.package_sha256.clone(),
                    manifest_digest: state.release.manifest_sha256.clone(),
                    generation: index as u64 + 11,
                    action: PluginLifecycleAction::Upgrade,
                },
                &manifest,
            )
            .unwrap();
            PluginPackageLifecycleUnit::new(
                coordinator(&package_root(package.package_id()), host.clone()),
                intent,
                manifest,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let retirements = prior_lock
        .removal_order()
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, package)| {
            let dependency = (package.package_id() == "acme/root").then_some("acme/base");
            let manifest = manifest_version(package.package_id(), dependency, "1.0.0");
            let transition = envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
                .unwrap();
            let state = transition.before.as_ref().unwrap();
            let intent = PluginLifecycleIntent::from_manifest(
                PluginLifecycleIntentSpec {
                    operation_id: envelope.plan.operation_id.clone(),
                    plan_digest: envelope.plan_digest.clone(),
                    scope_id: envelope.plan.scope.id.clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: state.release.package_sha256.clone(),
                    manifest_digest: state.release.manifest_sha256.clone(),
                    generation: 2_u64.saturating_sub(index as u64),
                    action: PluginLifecycleAction::Uninstall,
                },
                &manifest,
            )
            .unwrap();
            PluginPackageLifecycleUnit::new(
                coordinator(&package_root(package.package_id()), host.clone()),
                intent,
                manifest,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    UpgradeGraphFixture {
        _temp: temp,
        envelope,
        prior_lock,
        candidates,
        retirements,
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

#[tokio::test]
async fn publication_identity_mismatch_stays_replayable_without_repreparing_packages() {
    let fixture = install_graph_fixture(false);
    *fixture.host.publication_fault.lock().await = Some(PublicationFault::ReverseEvidence);
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());

    let error = graph
        .apply_install(&fixture.envelope, &fixture.units, || 1)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_invalid");
    assert!(error.message.contains("order or identity"));

    let records = graph
        .apply_install(&fixture.envelope, &fixture.units, || 2)
        .await
        .unwrap();
    assert!(records
        .iter()
        .all(|record| record.status == PluginLifecycleOperationStatus::Completed));
    let calls = fixture.host.calls.lock().await;
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "batch:acme/base,acme/root")
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.ends_with(":commit"))
            .count(),
        2
    );
}

mod upgrade;
