use std::fs;

use a3s_use_core::{
    PlanEnforcementProfile, PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DESCRIPTOR_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CAPABILITY_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const ARTIFACT_DIGEST: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
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
    RuntimeBindingReceipt::Task(RuntimePreparedTaskBinding {
        schema: RUNTIME_TASK_BINDING_SCHEMA.to_string(),
        surface: surface(PluginSurfaceKind::Tool, "convert"),
        package_digest: PACKAGE_DIGEST.to_string(),
        scope_id: "workspace-01".to_string(),
        descriptor_digest: DESCRIPTOR_DIGEST.to_string(),
        provider_id: "test-runtime".to_string(),
        provider_build_id: "build-1".to_string(),
        capability_digest: CAPABILITY_DIGEST.to_string(),
        enforcement: PlanEnforcementProfile::Container,
        artifact_digest: ARTIFACT_DIGEST.to_string(),
        artifact_media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
        generation,
        semantics_profile_digest: SEMANTICS_DIGEST.to_string(),
    })
}

fn service_receipt(observation_revision: u64) -> RuntimeBindingReceipt {
    RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
        schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
        surface: surface(PluginSurfaceKind::Tool, "index"),
        package_digest: PACKAGE_DIGEST.to_string(),
        scope_id: "workspace-01".to_string(),
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

#[tokio::test]
async fn binding_store_round_trips_idempotently_and_removes_exact_ownership() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt(7);

    assert!(store.put(&receipt).await.unwrap());
    assert!(!store.put(&receipt).await.unwrap());
    assert_eq!(
        store.get("workspace-01", receipt.surface()).await.unwrap(),
        Some(receipt.clone())
    );
    assert!(store.remove(&receipt).await.unwrap());
    assert!(!store.remove(&receipt).await.unwrap());
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
            .get_generation("workspace-01", current.surface(), 6)
            .await
            .unwrap(),
        Some(prior.clone())
    );
    assert_eq!(
        store
            .get_generation("workspace-01", current.surface(), 7)
            .await
            .unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store
            .get_generation("workspace-01", current.surface(), 8)
            .await
            .unwrap(),
        Some(next.clone())
    );
    assert_eq!(
        store.get("workspace-01", current.surface()).await.unwrap(),
        Some(next)
    );
    assert!(store.remove(&current).await.unwrap());
    assert_eq!(
        store
            .get_generation("workspace-01", prior.surface(), 6)
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
            .get("workspace-01", refreshed.surface())
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
        receipt.scope_id(),
        receipt.surface(),
        receipt.generation(),
    );
    fs::write(&path, b"{\"bindingKind\":\"task\",\"receipt\":{}}").unwrap();

    let error = store
        .get(receipt.scope_id(), receipt.surface())
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
    let original = binding_path(&store, receipt.scope_id(), receipt.surface(), 7);
    let moved = binding_path(&store, receipt.scope_id(), receipt.surface(), 8);
    fs::rename(original, moved).unwrap();

    let error = store
        .get_generation(receipt.scope_id(), receipt.surface(), 8)
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
    let path = binding_path(&store, receipt.scope_id(), receipt.surface(), 7);
    let owned = temporary.path().join("owned-runtime-receipt.json");
    fs::rename(&path, &owned).unwrap();
    std::os::unix::fs::symlink(&owned, &path).unwrap();

    let error = store
        .get(receipt.scope_id(), receipt.surface())
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
        .get_generation("workspace-01", &qualified, 1)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_generation(
            "workspace-01",
            &qualified,
            MAX_RUNTIME_BINDING_GENERATIONS as u64,
        )
        .await
        .unwrap()
        .is_some());

    let first = binding_path(&store, "workspace-01", &qualified, 1);
    let injected = binding_path(
        &store,
        "workspace-01",
        &qualified,
        MAX_RUNTIME_BINDING_GENERATIONS as u64 + 1,
    );
    fs::copy(first, injected).unwrap();
    let error = store.get("workspace-01", &qualified).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_limit_exceeded");
}

#[tokio::test]
async fn binding_store_rejects_okf_surfaces() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let okf = surface(PluginSurfaceKind::Okf, "domain-knowledge");

    let error = store.get("workspace-01", &okf).await.unwrap_err();
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
}

fn binding_path(
    store: &RuntimeBindingStore,
    scope_id: &str,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> std::path::PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
    store
        .root()
        .join(scope_digest)
        .join("acme")
        .join("research")
        .join(format!("tool-{}", surface.surface.id))
        .join(format!("{generation:020}.json"))
}
