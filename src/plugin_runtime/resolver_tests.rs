use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{PlannedProviderEvidence, PluginSurfaceKind, UseError, UseResult};
use async_trait::async_trait;

use super::test_support::*;
use super::*;

struct MemoryPlanSource {
    plans: Mutex<BTreeMap<RuntimeSurfacePlanKey, Vec<u8>>>,
}

impl MemoryPlanSource {
    fn new(key: RuntimeSurfacePlanKey, plan: &RuntimeSurfacePlan) -> Self {
        Self {
            plans: Mutex::new(BTreeMap::from([(key, plan.to_canonical_bytes().unwrap())])),
        }
    }
}

#[async_trait]
impl RuntimeSurfacePlanSource for MemoryPlanSource {
    async fn read_plan(&self, key: &RuntimeSurfacePlanKey) -> UseResult<Vec<u8>> {
        self.plans
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| UseError::new("use.plugin.runtime.plan_not_found", "missing test plan"))
    }
}

struct StaticFactory {
    provider_id: ProviderId,
    client: Arc<dyn RuntimeClient>,
}

#[async_trait]
impl RuntimeProviderFactory for StaticFactory {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
        Ok(self.client.clone())
    }
}

fn key_for(plan: &RuntimeSurfacePlan, evidence: &PlannedProviderEvidence) -> RuntimeSurfacePlanKey {
    RuntimeSurfacePlanKey::from_plan(plan, evidence).unwrap()
}

fn registry_for(plan: &RuntimeSurfacePlan) -> Arc<RuntimeClientRegistry> {
    let capabilities = capabilities(plan);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let mut registry = RuntimeClientRegistry::new();
    registry
        .register(Arc::new(StaticFactory {
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            client: runtime,
        }))
        .unwrap();
    Arc::new(registry)
}

#[test]
fn canonical_plan_payload_round_trips_and_rejects_semantics_tampering() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let bytes = plan.to_canonical_bytes().unwrap();
    assert!(bytes.len() <= MAX_RUNTIME_SURFACE_PLAN_BYTES);
    assert_eq!(
        RuntimeSurfacePlan::from_canonical_bytes(&bytes).unwrap(),
        plan
    );
    assert_eq!(plan.to_canonical_bytes().unwrap(), bytes);

    let noncanonical =
        serde_json::to_vec_pretty(&serde_json::from_slice::<serde_json::Value>(&bytes).unwrap())
            .unwrap();
    let error = RuntimeSurfacePlan::from_canonical_bytes(&noncanonical).unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.contract_invalid");

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["plan"]["spec"]["resources"]["memoryBytes"] = serde_json::json!(1);
    let tampered = serde_json::to_vec(&value).unwrap();
    let error = RuntimeSurfacePlan::from_canonical_bytes(&tampered).unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.contract_invalid");
}

#[tokio::test]
async fn committed_resolver_reconnects_from_path_free_payload_after_restart() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = key_for(&plan, &provider);
    let source = Arc::new(MemoryPlanSource::new(key.clone(), &plan));
    let resolver = CommittedRuntimeSurfaceResolver::new(source, registry_for(&plan));

    let selected = resolver.resolve(&key, &provider).await.unwrap();
    assert_eq!(selected.plan(), &plan);
    assert_eq!(selected.provider(), &provider);
    assert_eq!(
        selected
            .client()
            .verify_plan(&plan, &provider)
            .await
            .unwrap()
            .provider_id,
        ProviderId::parse("test-runtime").unwrap()
    );
}

#[tokio::test]
async fn resolver_rejects_key_and_provider_evidence_drift_before_runtime_effects() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let key = key_for(&plan, &provider);
    let source = Arc::new(MemoryPlanSource::new(key.clone(), &plan));
    let resolver = CommittedRuntimeSurfaceResolver::new(source, registry_for(&plan));

    let mut wrong_key = key.clone();
    wrong_key.generation += 1;
    let error = resolver.resolve(&wrong_key, &provider).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.plan_not_found");

    let mut changed = provider.clone();
    changed.provider_build_id = "build-unreviewed".to_owned();
    let error = resolver.resolve(&key, &changed).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");
}
