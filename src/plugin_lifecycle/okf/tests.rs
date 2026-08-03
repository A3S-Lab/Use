use std::path::PathBuf;
use std::sync::Arc;

use a3s_use_core::{
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt,
    OkfSelectedGeneration, UseResult, OKF_KNOWLEDGE_OBSERVATION_SCHEMA,
    OKF_PROJECTION_RECEIPT_SCHEMA,
};
use a3s_use_extension::ExtensionManifest;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::okf_knowledge::{
    OkfKnowledgeAdapter, OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient,
    OkfKnowledgeStageRequest,
};
use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleIntent, PluginLifecycleIntentSpec,
};

use super::*;

const MANIFEST: &str = include_str!(
    "../../../crates/extension/fixtures/packages/plugin-v3-okf/package/a3s-use-extension.acl"
);
const PACKAGE_DIGEST: &str =
    include_str!("../../../crates/extension/fixtures/packages/plugin-v3-okf/package.sha256");

#[derive(Default)]
struct FakeKnowledgeAdapter {
    calls: Mutex<Vec<String>>,
}

impl FakeKnowledgeAdapter {
    async fn calls(&self) -> Vec<String> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl OkfKnowledgeAdapter for FakeKnowledgeAdapter {
    async fn stage(&self, request: &OkfKnowledgeStageRequest) -> UseResult<OkfKnowledgeBinding> {
        self.calls.lock().await.push("stage".to_string());
        let spec = request.spec();
        let receipt = OkfProjectionReceipt {
            schema: OKF_PROJECTION_RECEIPT_SCHEMA.to_string(),
            operation_id: spec.operation_id.clone(),
            scope_id: spec.scope_id.clone(),
            surface: spec.surface.clone(),
            generation: spec.generation,
            package_digest: spec.package_digest.clone(),
            manifest_digest: spec.manifest_digest.clone(),
            bundle: spec.bundle.clone(),
            projection_id: format!("projection-{}", spec.generation),
            index_schema: "okf-v1".to_string(),
            index_build_id: format!("build-{}", spec.generation),
            staged_at_ms: 1_000 + spec.generation,
        };
        binding(&receipt, OkfKnowledgeObservedState::Staged, None, 2_000)
    }

    async fn promote(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        self.calls.lock().await.push("promote".to_string());
        observation(
            receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(receipt),
            3_000,
        )
    }

    async fn observe(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        observation(
            receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(receipt),
            4_000,
        )
    }

    async fn remove(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        self.calls.lock().await.push("remove".to_string());
        observation(receipt, OkfKnowledgeObservedState::Removed, None, 5_000)
    }
}

#[tokio::test]
async fn stages_promotes_hides_and_receipt_removes_the_real_okf_fixture() {
    let temporary = tempfile::tempdir().unwrap();
    let adapter = Arc::new(FakeKnowledgeAdapter::default());
    let store = OkfKnowledgeBindingStore::new(temporary.path());
    let host = OkfKnowledgeLifecycleHost::new(
        package_root(),
        OkfKnowledgeClient::new(adapter.clone()),
        store.clone(),
    );
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest, PluginLifecycleAction::Install);
    let surface = &manifest.okf[0];
    let key = &intent
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint
                .surface
                .as_ref()
                .is_some_and(|value| value.id == surface.id)
        })
        .unwrap()
        .idempotency_key;

    let first = host.prepare_okf(&intent, surface, key).await.unwrap();
    assert!(first.digest().starts_with("sha256:"));
    assert_eq!(adapter.calls().await, ["stage", "promote"]);
    let qualified = intent
        .surfaces
        .iter()
        .find(|candidate| candidate.surface.id == surface.id)
        .map(|candidate| a3s_use_core::PlanQualifiedSurfaceRef {
            package_id: intent.package_id.clone(),
            surface: candidate.surface.clone(),
        })
        .unwrap();
    assert!(store
        .snapshot(&intent.scope_id, &qualified)
        .await
        .unwrap()
        .projection
        .is_some());

    let replay = host.prepare_okf(&intent, surface, key).await.unwrap();
    assert_eq!(replay, first);
    assert_eq!(adapter.calls().await, ["stage", "promote"]);

    host.stop_okf(&intent, surface, key).await.unwrap();
    assert!(store
        .snapshot(&intent.scope_id, &qualified)
        .await
        .unwrap()
        .projection
        .is_some());
    assert_eq!(adapter.calls().await, ["stage", "promote"]);

    host.remove_okf(&intent, surface, key).await.unwrap();
    host.remove_okf(&intent, surface, key).await.unwrap();
    assert_eq!(adapter.calls().await, ["stage", "promote", "remove"]);
    let snapshot = store.snapshot(&intent.scope_id, &qualified).await.unwrap();
    assert_eq!(
        snapshot.latest.unwrap().observation.state,
        OkfKnowledgeObservedState::Removed
    );
    assert!(snapshot.projection.is_none());
}

#[tokio::test]
async fn missing_receipt_removal_is_idempotent_without_calling_knowledge() {
    let temporary = tempfile::tempdir().unwrap();
    let adapter = Arc::new(FakeKnowledgeAdapter::default());
    let host = OkfKnowledgeLifecycleHost::new(
        package_root(),
        OkfKnowledgeClient::new(adapter.clone()),
        OkfKnowledgeBindingStore::new(temporary.path()),
    );
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest, PluginLifecycleAction::Uninstall);

    host.remove_okf(&intent, &manifest.okf[0], "missing")
        .await
        .unwrap();
    assert!(adapter.calls().await.is_empty());
}

fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package")
}

fn intent(manifest: &ExtensionManifest, action: PluginLifecycleAction) -> PluginLifecycleIntent {
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: format!("okf-{}", action_name(action)),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope_id: "workspace:knowledge".to_string(),
            package_id: manifest.package_id.clone(),
            package_digest: PACKAGE_DIGEST.trim().to_string(),
            manifest_digest: format!("sha256:{:x}", Sha256::digest(MANIFEST.as_bytes())),
            generation: 7,
            action,
        },
        manifest,
    )
    .unwrap()
}

fn action_name(action: PluginLifecycleAction) -> &'static str {
    match action {
        PluginLifecycleAction::Install => "install",
        PluginLifecycleAction::Upgrade => "upgrade",
        PluginLifecycleAction::Enable => "enable",
        PluginLifecycleAction::Disable => "disable",
        PluginLifecycleAction::Uninstall => "uninstall",
    }
}

fn binding(
    receipt: &OkfProjectionReceipt,
    state: OkfKnowledgeObservedState,
    selected: Option<&OkfProjectionReceipt>,
    observed_at_ms: u64,
) -> UseResult<OkfKnowledgeBinding> {
    OkfKnowledgeBinding::new(
        receipt.clone(),
        observation(receipt, state, selected, observed_at_ms)?,
    )
}

fn observation(
    receipt: &OkfProjectionReceipt,
    state: OkfKnowledgeObservedState,
    selected: Option<&OkfProjectionReceipt>,
    observed_at_ms: u64,
) -> UseResult<OkfKnowledgeObservation> {
    let index_digest = format!("sha256:{:064x}", receipt.generation);
    let selected = selected
        .map(|selected| {
            Ok::<_, a3s_use_core::UseError>(OkfSelectedGeneration {
                generation: selected.generation,
                package_digest: selected.package_digest.clone(),
                bundle_digest: selected.bundle.content_digest.clone(),
                projection_receipt_digest: selected.descriptor_digest()?,
                index_schema: selected.index_schema.clone(),
                index_build_id: selected.index_build_id.clone(),
                index_digest: format!("sha256:{:064x}", selected.generation),
            })
        })
        .transpose()?;
    let observation = OkfKnowledgeObservation {
        schema: OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_string(),
        scope_id: receipt.scope_id.clone(),
        surface: receipt.surface.clone(),
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest()?,
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        state,
        observed_at_ms,
        index_digest: match state {
            OkfKnowledgeObservedState::Removed => None,
            OkfKnowledgeObservedState::Failed => None,
            OkfKnowledgeObservedState::Promoted | OkfKnowledgeObservedState::Staged => {
                Some(index_digest)
            }
        },
        selected,
    };
    observation.validate_for_receipt(receipt)?;
    Ok(observation)
}
