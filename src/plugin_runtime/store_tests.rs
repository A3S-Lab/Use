use std::collections::BTreeMap;
use std::fs;

use a3s_runtime::contract::{
    RuntimeEvidence, RuntimeHealthObservation, RuntimeHealthState, RuntimeObservation,
    RuntimeServiceEndpoint, RuntimeUnitState,
};

use a3s_use_core::{
    PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind,
    PlannedProviderEvidence, PluginSurfaceKind, PluginSurfaceRef,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::test_support::{
    artifact, capabilities, evidence, policy, service_descriptor, service_surface, task_descriptor,
    task_surface,
};
use super::*;

const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DESCRIPTOR_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CAPABILITY_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SPEC_DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const SEMANTICS_DIGEST: &str =
    "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

fn surface(kind: PluginSurfaceKind, id: &str) -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_string(),
        surface: PluginSurfaceRef {
            kind,
            id: id.to_string(),
        },
    }
}

fn task_receipt(generation: u64) -> RuntimeBindingReceipt {
    task_receipt_for_scope(generation, workspace_scope())
}

fn task_receipt_for_scope(generation: u64, scope: PlanScope) -> RuntimeBindingReceipt {
    let descriptor = task_descriptor();
    let context = RuntimeSurfaceContext::new(
        "acme/research",
        PACKAGE_DIGEST,
        scope,
        DESCRIPTOR_DIGEST,
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_string(),
        },
        generation,
    )
    .unwrap();
    let plan = plan_tool_task_release(
        context,
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("store-template", Vec::new()).unwrap(),
        policy(),
        a3s_runtime::contract::NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    RuntimeBindingReceipt::Task(
        RuntimePreparedTaskBinding::from_plan(&plan, &evidence(&plan, &capabilities)).unwrap(),
    )
}

fn service_receipt(observation_revision: u64) -> RuntimeBindingReceipt {
    RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
        schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
        surface: surface(PluginSurfaceKind::Tool, "index"),
        package_digest: PACKAGE_DIGEST.to_string(),
        scope: workspace_scope(),
        descriptor_digest: DESCRIPTOR_DIGEST.to_string(),
        provider_id: "test-runtime".to_string(),
        provider_build_id: "build-1".to_string(),
        capability_digest: CAPABILITY_DIGEST.to_string(),
        enforcement: PlanEnforcementProfile::Container,
        unit_id: "use:service:0123456789abcdef".to_string(),
        generation: 7,
        spec_digest: SPEC_DIGEST.to_string(),
        semantics_profile_digest: SEMANTICS_DIGEST.to_string(),
        endpoint_ref: RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap(),
        runtime_started_at_ms: 900,
        observation_revision,
        last_healthy_at_ms: observation_revision,
        contract: RuntimeSurfaceContract::ToolService {
            port_name: "http".to_string(),
            base_path: "/api".to_string(),
            shutdown_grace_ms: 30_000,
            api_contract_digest: None,
        },
        readiness: RuntimeServiceReadinessEvidence::HttpHealthy,
    })
}

fn service_plan(generation: u64) -> (RuntimeSurfacePlan, PlannedProviderEvidence) {
    let descriptor = service_descriptor();
    let context = RuntimeSurfaceContext::new(
        "acme/research",
        PACKAGE_DIGEST,
        workspace_scope(),
        DESCRIPTOR_DIGEST,
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "index".to_string(),
        },
        generation,
    )
    .unwrap();
    let plan = plan_tool_service_release(
        context,
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    (plan, provider)
}

fn provisioning_receipt(generation: u64) -> RuntimeServiceProvisioningReceipt {
    let (plan, provider) = service_plan(generation);
    RuntimeServiceProvisioningReceipt::from_plan(
        &plan,
        &provider,
        format!("sha256:{}", "9".repeat(64)),
        format!("use:apply-tool:{}", "8".repeat(64)),
    )
    .unwrap()
}

fn running_observation(
    plan: &RuntimeSurfacePlan,
    provider: &PlannedProviderEvidence,
) -> RuntimeObservation {
    let spec_digest = plan.spec().digest().unwrap();
    let mut claims = BTreeMap::new();
    RuntimeServiceEndpoint::node_local_tcp("http", 31_337)
        .unwrap()
        .insert_claim(&mut claims)
        .unwrap();
    RuntimeObservation {
        schema: RuntimeObservation::SCHEMA.to_string(),
        unit_id: plan.spec().unit_id.clone(),
        generation: plan.spec().generation,
        spec_digest: spec_digest.clone(),
        class: plan.spec().class,
        state: RuntimeUnitState::Running,
        provider_resource_id: Some("resource-01".to_string()),
        provider_build: Some(provider.provider_build_id.clone()),
        observed_at_ms: 1_000,
        started_at_ms: Some(900),
        finished_at_ms: None,
        health: Some(RuntimeHealthObservation {
            state: RuntimeHealthState::Healthy,
            checked_at_ms: 1_000,
            message: None,
        }),
        outputs: Vec::new(),
        usage: None,
        evidence: Some(RuntimeEvidence {
            provider_build: provider.provider_build_id.clone(),
            spec_digest,
            semantics_profile_digest: Some(provider.semantics_profile_digest.clone()),
            claims,
        }),
        provider_attestation: None,
        failure: None,
    }
}

#[tokio::test]
async fn binding_store_round_trips_idempotently_and_removes_exact_ownership() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt(7);

    assert!(store.put(&receipt).await.unwrap());
    assert!(!store.put(&receipt).await.unwrap());
    assert_eq!(
        store
            .get(&workspace_scope(), receipt.surface())
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    assert!(store.remove(&receipt).await.unwrap());
    assert!(!store.remove(&receipt).await.unwrap());
}

#[tokio::test]
async fn provisioning_store_advances_and_commits_without_an_unowned_gap() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let (plan, provider) = service_plan(7);
    let mut pending = provisioning_receipt(7);

    assert!(store.put_provisioning(&pending).await.unwrap());
    assert!(!store.put_provisioning(&pending).await.unwrap());
    pending
        .record_runtime_observation(&plan, &provider, running_observation(&plan, &provider))
        .unwrap();
    assert!(store.put_provisioning(&pending).await.unwrap());
    pending
        .record_gateway_readiness(
            RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap(),
            RuntimeServiceReadinessEvidence::HttpHealthy,
        )
        .unwrap();
    assert!(store.put_provisioning(&pending).await.unwrap());
    let binding = RuntimeBindingReceipt::Service(pending.binding_receipt().unwrap());

    assert!(store.commit_provisioning(&pending, &binding).await.unwrap());
    assert!(store
        .get_provisioning(&workspace_scope(), &plan.surface(), 7)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get_generation(&workspace_scope(), &plan.surface(), 7)
            .await
            .unwrap(),
        Some(binding.clone())
    );
    assert!(!store.commit_provisioning(&pending, &binding).await.unwrap());
}

#[tokio::test]
async fn provisioning_store_rejects_conflicting_operation_identity() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let pending = provisioning_receipt(7);
    store.put_provisioning(&pending).await.unwrap();
    let mut conflict = pending.clone();
    conflict.lifecycle_idempotency_key = format!("sha256:{}", "7".repeat(64));

    let error = store.put_provisioning(&conflict).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.provisioning_conflict");
    assert_eq!(
        store
            .get_provisioning(&workspace_scope(), &pending.surface, 7)
            .await
            .unwrap(),
        Some(pending)
    );
}

#[tokio::test]
async fn identical_scope_ids_are_isolated_by_scope_kind() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let workspace = task_receipt(7);
    let user = task_receipt_for_scope(
        7,
        PlanScope {
            kind: PlanScopeKind::User,
            id: "workspace-01".to_string(),
        },
    );

    assert!(store.put(&workspace).await.unwrap());
    assert!(store.put(&user).await.unwrap());
    assert_ne!(
        binding_path(
            &store,
            workspace.scope(),
            workspace.surface(),
            workspace.generation(),
        ),
        binding_path(&store, user.scope(), user.surface(), user.generation(),)
    );
    assert_eq!(
        store
            .get_generation(
                workspace.scope(),
                workspace.surface(),
                workspace.generation(),
            )
            .await
            .unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store
            .get_generation(user.scope(), user.surface(), user.generation())
            .await
            .unwrap(),
        Some(user)
    );
}

#[tokio::test]
async fn binding_store_retains_exact_generations_and_rejects_conflicts() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let current = task_receipt(7);
    store.put(&current).await.unwrap();

    let prior = task_receipt(6);
    assert!(store.put(&prior).await.unwrap());
    let mut conflict = task_receipt(7);
    let RuntimeBindingReceipt::Task(conflict_receipt) = &mut conflict else {
        panic!("fixture should be a Task binding");
    };
    conflict_receipt.provider_build_id = "build-2".to_string();
    assert_eq!(
        store.put(&conflict).await.unwrap_err().code,
        "use.plugin.runtime.binding_conflict"
    );
    let next = task_receipt(8);
    assert!(store.put(&next).await.unwrap());
    assert_eq!(
        store
            .get_generation(&workspace_scope(), current.surface(), 6)
            .await
            .unwrap(),
        Some(prior.clone())
    );
    assert_eq!(
        store
            .get_generation(&workspace_scope(), current.surface(), 7)
            .await
            .unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store
            .get_generation(&workspace_scope(), current.surface(), 8)
            .await
            .unwrap(),
        Some(next.clone())
    );
    assert_eq!(
        store
            .get(&workspace_scope(), current.surface())
            .await
            .unwrap(),
        Some(next)
    );
    assert!(store.remove(&current).await.unwrap());
    assert_eq!(
        store
            .get_generation(&workspace_scope(), prior.surface(), 6)
            .await
            .unwrap(),
        Some(prior)
    );
}

#[tokio::test]
async fn service_observation_refresh_is_monotonic_within_one_generation() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let first = service_receipt(1_000);
    store.put(&first).await.unwrap();
    let mut refreshed = service_receipt(1_001);
    let RuntimeBindingReceipt::Service(refreshed_receipt) = &mut refreshed else {
        panic!("fixture should be a Service binding");
    };
    refreshed_receipt.endpoint_ref =
        RuntimeEndpointRef::parse("gateway:workspace-01/index-2").unwrap();

    assert!(store.put(&refreshed).await.unwrap());
    assert_eq!(
        store.remove(&first).await.unwrap_err().code,
        "use.plugin.runtime.binding_ownership_changed"
    );
    assert_eq!(
        store
            .get(&workspace_scope(), refreshed.surface())
            .await
            .unwrap(),
        Some(refreshed)
    );
}

#[tokio::test]
async fn binding_store_fails_closed_on_tampered_json() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt(7);
    store.put(&receipt).await.unwrap();
    let path = binding_path(
        &store,
        receipt.scope(),
        receipt.surface(),
        receipt.generation(),
    );
    fs::write(&path, b"{\"bindingKind\":\"task\",\"receipt\":{}}").unwrap();

    let error = store
        .get(receipt.scope(), receipt.surface())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_receipt_invalid");
}

#[tokio::test]
async fn binding_store_rejects_a_receipt_moved_to_another_generation() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt(7);
    store.put(&receipt).await.unwrap();
    let original = binding_path(&store, receipt.scope(), receipt.surface(), 7);
    let moved = binding_path(&store, receipt.scope(), receipt.surface(), 8);
    fs::rename(original, moved).unwrap();

    let error = store
        .get_generation(receipt.scope(), receipt.surface(), 8)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_ownership_mismatch");
}

#[cfg(unix)]
#[tokio::test]
async fn binding_store_rejects_symlinked_generation_receipts() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt(7);
    store.put(&receipt).await.unwrap();
    let path = binding_path(&store, receipt.scope(), receipt.surface(), 7);
    let owned = temporary.path().join("owned-runtime-receipt.json");
    fs::rename(&path, &owned).unwrap();
    std::os::unix::fs::symlink(&owned, &path).unwrap();

    let error = store
        .get(receipt.scope(), receipt.surface())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_path_invalid");
}

#[tokio::test]
async fn binding_store_enforces_the_retained_generation_limit() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    for generation in 1..=MAX_RUNTIME_BINDING_GENERATIONS as u64 {
        store.put(&task_receipt(generation)).await.unwrap();
    }

    let error = store
        .put(&task_receipt(MAX_RUNTIME_BINDING_GENERATIONS as u64 + 1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_limit_exceeded");
    let qualified = surface(PluginSurfaceKind::Tool, "convert");
    assert!(store
        .get_generation(&workspace_scope(), &qualified, 1)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_generation(
            &workspace_scope(),
            &qualified,
            MAX_RUNTIME_BINDING_GENERATIONS as u64,
        )
        .await
        .unwrap()
        .is_some());

    let first = binding_path(&store, &workspace_scope(), &qualified, 1);
    let injected = binding_path(
        &store,
        &workspace_scope(),
        &qualified,
        MAX_RUNTIME_BINDING_GENERATIONS as u64 + 1,
    );
    fs::copy(first, injected).unwrap();
    let error = store.get(&workspace_scope(), &qualified).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_limit_exceeded");
}

#[tokio::test]
async fn binding_store_rejects_okf_surfaces() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let okf = surface(PluginSurfaceKind::Okf, "domain-knowledge");

    let error = store.get(&workspace_scope(), &okf).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_path_invalid");
}

#[test]
fn binding_receipts_reject_cross_kind_readiness_claims() {
    let mut receipt = service_receipt(1_000);
    let RuntimeBindingReceipt::Service(receipt) = &mut receipt else {
        panic!("fixture should be a Service binding");
    };
    receipt.surface.surface.kind = PluginSurfaceKind::Mcp;
    assert!(RuntimeBindingReceipt::Service(receipt.clone())
        .validate()
        .is_err());
}

#[test]
fn binding_receipts_require_runtime_provider_id_syntax() {
    let mut receipt = task_receipt(7);
    let RuntimeBindingReceipt::Task(receipt) = &mut receipt else {
        panic!("fixture should be a Task binding");
    };
    receipt.provider_id = "runtime/provider".to_string();
    assert!(RuntimeBindingReceipt::Task(receipt.clone())
        .validate()
        .is_err());
}

#[test]
fn binding_store_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RuntimeBindingStore>();
    assert_send_sync::<RuntimeBindingReceipt>();
    assert_send_sync::<RuntimeServiceProvisioningReceipt>();
}

fn binding_path(
    store: &RuntimeBindingStore,
    scope: &PlanScope,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> std::path::PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(scope.id.as_bytes()));
    store
        .root()
        .join(scope.kind.as_str())
        .join(scope_digest)
        .join("acme")
        .join("research")
        .join(format!("tool-{}", surface.surface.id))
        .join(format!("{generation:020}.json"))
}

fn workspace_scope() -> PlanScope {
    PlanScope {
        kind: PlanScopeKind::Workspace,
        id: "workspace-01".to_owned(),
    }
}
