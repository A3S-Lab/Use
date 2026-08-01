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
async fn binding_store_rejects_stale_and_conflicting_generations() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let current = task_receipt(7);
    store.put(&current).await.unwrap();

    let stale = task_receipt(6);
    assert_eq!(
        store.put(&stale).await.unwrap_err().code,
        "use.plugin.runtime.binding_stale"
    );
    let mut conflict = task_receipt(7);
    let RuntimeBindingReceipt::Task(conflict_receipt) = &mut conflict else {
        panic!("fixture should be a Task binding");
    };
    conflict_receipt.provider_build_id = "build-2".to_string();
    assert_eq!(
        store.put(&conflict).await.unwrap_err().code,
        "use.plugin.runtime.binding_conflict"
    );
    assert!(store.put(&task_receipt(8)).await.unwrap());
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
    let path = binding_path(&store, receipt.scope_id(), receipt.surface());
    fs::write(&path, b"{\"bindingKind\":\"task\",\"receipt\":{}}").unwrap();

    let error = store
        .get(receipt.scope_id(), receipt.surface())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_receipt_invalid");
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
) -> std::path::PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
    store
        .root()
        .join(scope_digest)
        .join("acme")
        .join("research")
        .join(format!("tool-{}.json", surface.surface.id))
}
