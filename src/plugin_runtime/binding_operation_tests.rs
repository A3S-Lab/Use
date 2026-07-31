use std::fs;
use std::sync::Arc;

use a3s_runtime::contract::{NetworkMode, RuntimeRemoval};
use a3s_use_core::{
    PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlannedProviderEvidence, PluginSurfaceKind,
    PluginSurfaceRef,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::test_support::{
    artifact, capabilities, evidence as provider_evidence, policy, task_descriptor, task_surface,
    FakeRuntime,
};
use super::*;

const PACKAGE_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PACKAGE_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DESCRIPTOR: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const CAPABILITIES: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const ARTIFACT: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const SPEC: &str = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
const SEMANTICS: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PLAN: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const GRANTS: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const SNAPSHOT: &str = "sha256:3333333333333333333333333333333333333333333333333333333333333333";
const SCOPE: &str = "workspace-01";

#[tokio::test]
async fn install_records_prepares_publishes_and_commits_idempotently() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt("convert", PACKAGE_A, 8);
    let intent = intent(vec![candidate(&receipt)], Vec::new());

    let begun = store.begin_binding_change(&intent).await.unwrap();
    assert_eq!(begun.phase, RuntimeBindingOperationPhase::IntentRecorded);
    assert_eq!(store.begin_binding_change(&intent).await.unwrap(), begun);

    let prepared = store
        .record_prepared_binding(SCOPE, "operation-01", &receipt)
        .await
        .unwrap();
    assert_eq!(prepared.phase, RuntimeBindingOperationPhase::Prepared);
    assert_eq!(prepared.prepared, vec![receipt.clone()]);

    let published = store
        .publish_prepared_bindings(SCOPE, "operation-01")
        .await
        .unwrap();
    assert_eq!(
        published.phase,
        RuntimeBindingOperationPhase::BindingsPublished
    );
    assert_eq!(
        store.get("workspace-01", receipt.surface()).await.unwrap(),
        Some(receipt.clone())
    );

    let completed = store
        .commit_binding_cutover(SCOPE, "operation-01", cutover(), 2_100)
        .await
        .unwrap();
    assert_eq!(completed.phase, RuntimeBindingOperationPhase::Completed);
    assert_eq!(
        store
            .commit_binding_cutover(SCOPE, "operation-01", cutover(), 2_100)
            .await
            .unwrap(),
        completed
    );
}

#[tokio::test]
async fn operation_intent_derives_exact_candidates_from_provider_selection() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        RuntimeSurfaceContext::new(
            "acme/research",
            PACKAGE_A,
            "workspace-01",
            GRANTS,
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            },
            8,
        )
        .unwrap(),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("planning-template", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = provider_evidence(&plan, &capabilities);
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(capabilities, true)));
    let receipt = RuntimeBindingReceipt::Task(client.prepare_task(&plan, &provider).await.unwrap());
    let selection =
        RuntimeProviderSelection::from_surfaces(vec![SelectedRuntimeSurface::from_parts(
            plan, provider, client,
        )]);

    let intent = RuntimeBindingOperationIntent::from_selection(
        "operation-01",
        PLAN,
        Some(GRANTS.to_string()),
        "workspace-01",
        4,
        7,
        2_000,
        &selection,
        Vec::new(),
    )
    .unwrap();

    assert_eq!(intent.candidates.len(), 1);
    assert!(intent.candidates[0].matches_receipt(&receipt).unwrap());
}

#[tokio::test]
async fn one_operation_keeps_scope_specific_runtime_journals_disjoint() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let first = intent(
        vec![candidate(&task_receipt("convert", PACKAGE_A, 8))],
        Vec::new(),
    );
    let mut second = first.clone();
    second.scope_id = "workspace-02".to_string();
    for candidate in &mut second.candidates {
        candidate.scope_id = second.scope_id.clone();
    }
    second.validate().unwrap();

    let first_journal = store.begin_binding_change(&first).await.unwrap();
    let second_journal = store.begin_binding_change(&second).await.unwrap();

    assert_eq!(
        first_journal.intent.operation_id,
        second_journal.intent.operation_id
    );
    assert_ne!(
        first_journal.intent.scope_id,
        second_journal.intent.scope_id
    );
    assert_eq!(
        store
            .observe_binding_change(&first.scope_id, &first.operation_id)
            .await
            .unwrap(),
        Some(first_journal)
    );
    assert_eq!(
        store
            .observe_binding_change(&second.scope_id, &second.operation_id)
            .await
            .unwrap(),
        Some(second_journal)
    );
}

#[test]
fn runtime_intent_without_grant_sub_saga_does_not_fabricate_a_change_set() {
    let runtime_only = RuntimeBindingOperationIntent::new(
        "operation-01",
        PLAN,
        None,
        SCOPE,
        4,
        7,
        2_000,
        vec![candidate(&task_receipt("convert", PACKAGE_A, 8))],
        Vec::new(),
    )
    .unwrap();

    assert_eq!(runtime_only.grant_change_set_digest, None);
    let value = serde_json::to_value(&runtime_only).unwrap();
    assert!(value.get("grantChangeSetDigest").is_none());
}

#[tokio::test]
async fn partial_candidate_preparation_recovers_from_the_durable_journal() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let first = task_receipt("convert", PACKAGE_A, 8);
    let second = task_receipt("summarize", PACKAGE_A, 8);
    let intent = intent(vec![candidate(&first), candidate(&second)], Vec::new());
    store.begin_binding_change(&intent).await.unwrap();

    let partial = store
        .record_prepared_binding(SCOPE, "operation-01", &second)
        .await
        .unwrap();
    assert_eq!(partial.phase, RuntimeBindingOperationPhase::Preparing);
    assert_eq!(partial.prepared, vec![second.clone()]);
    assert_eq!(
        store
            .observe_binding_change(SCOPE, "operation-01")
            .await
            .unwrap()
            .unwrap(),
        partial
    );

    let prepared = store
        .record_prepared_binding(SCOPE, "operation-01", &first)
        .await
        .unwrap();
    assert_eq!(prepared.phase, RuntimeBindingOperationPhase::Prepared);
    assert_eq!(prepared.prepared, vec![first, second]);
}

#[tokio::test]
async fn upgrade_preserves_candidate_while_checkpointing_exact_prior_removal() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let prior = service_receipt(PACKAGE_A, 7, 1_000);
    let next = service_receipt(PACKAGE_B, 8, 2_000);
    store.put(&prior).await.unwrap();
    let intent = intent(vec![candidate(&next)], vec![prior.clone()]);
    store.begin_binding_change(&intent).await.unwrap();
    store
        .record_prepared_binding(SCOPE, "operation-01", &next)
        .await
        .unwrap();
    store
        .publish_prepared_bindings(SCOPE, "operation-01")
        .await
        .unwrap();
    store
        .commit_binding_cutover(SCOPE, "operation-01", cutover(), 2_100)
        .await
        .unwrap();

    let evidence = RuntimeBindingRetirementEvidence::service(
        service(&prior).clone(),
        removal_for(&prior, 2_200),
    )
    .unwrap();
    let completed = store
        .record_retired_binding(SCOPE, "operation-01", &evidence, 2_300)
        .await
        .unwrap();

    assert_eq!(completed.phase, RuntimeBindingOperationPhase::Completed);
    assert_eq!(completed.retired, vec![evidence]);
    assert_eq!(
        store.get("workspace-01", next.surface()).await.unwrap(),
        Some(next)
    );
}

#[tokio::test]
async fn uninstall_requires_cutover_then_removes_only_the_exact_prior_binding() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let prior = service_receipt(PACKAGE_A, 7, 1_000);
    store.put(&prior).await.unwrap();
    let intent = intent(Vec::new(), vec![prior.clone()]);
    store.begin_binding_change(&intent).await.unwrap();
    store
        .publish_prepared_bindings(SCOPE, "operation-01")
        .await
        .unwrap();
    let evidence = RuntimeBindingRetirementEvidence::service(
        service(&prior).clone(),
        removal_for(&prior, 2_200),
    )
    .unwrap();

    let early = store
        .record_retired_binding(SCOPE, "operation-01", &evidence, 2_300)
        .await
        .unwrap_err();
    assert_eq!(
        early.code,
        "use.plugin.runtime.binding_operation_cutover_required"
    );
    store
        .commit_binding_cutover(SCOPE, "operation-01", cutover(), 2_100)
        .await
        .unwrap();
    let completed = store
        .record_retired_binding(SCOPE, "operation-01", &evidence, 2_300)
        .await
        .unwrap();

    assert_eq!(completed.phase, RuntimeBindingOperationPhase::Completed);
    assert_eq!(
        store.get("workspace-01", prior.surface()).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn task_launcher_retirement_uses_trusted_time_and_exact_ownership() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let prior = task_receipt("convert", PACKAGE_A, 7);
    store.put(&prior).await.unwrap();
    store
        .begin_binding_change(&intent(Vec::new(), vec![prior.clone()]))
        .await
        .unwrap();
    store
        .publish_prepared_bindings(SCOPE, "operation-01")
        .await
        .unwrap();
    store
        .commit_binding_cutover(SCOPE, "operation-01", cutover(), 2_100)
        .await
        .unwrap();
    let RuntimeBindingReceipt::Task(prior_receipt) = &prior else {
        panic!("expected Task receipt");
    };
    let evidence = RuntimeBindingRetirementEvidence::task(prior_receipt.clone(), 2_200).unwrap();

    let completed = store
        .record_retired_binding(SCOPE, "operation-01", &evidence, 2_300)
        .await
        .unwrap();

    assert_eq!(completed.phase, RuntimeBindingOperationPhase::Completed);
    assert_eq!(
        store.get("workspace-01", prior.surface()).await.unwrap(),
        None
    );
}

#[test]
fn service_retirement_requires_exact_runtime_removal_identity() {
    let prior = service_receipt(PACKAGE_A, 7, 1_000);
    let mut removal = removal_for(&prior, 2_200);
    removal.unit_id = "use:service:substituted".to_string();

    let error =
        RuntimeBindingRetirementEvidence::service(service(&prior).clone(), removal).unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.binding_operation_invalid");
}

#[tokio::test]
async fn candidate_or_operation_identity_drift_fails_closed() {
    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt("convert", PACKAGE_A, 8);
    let intent = intent(vec![candidate(&receipt)], Vec::new());
    store.begin_binding_change(&intent).await.unwrap();
    store
        .record_prepared_binding(SCOPE, "operation-01", &receipt)
        .await
        .unwrap();
    store
        .publish_prepared_bindings(SCOPE, "operation-01")
        .await
        .unwrap();
    store
        .put(&task_receipt("convert", PACKAGE_B, 9))
        .await
        .unwrap();

    let changed = store
        .commit_binding_cutover(SCOPE, "operation-01", cutover(), 2_100)
        .await
        .unwrap_err();
    assert_eq!(
        changed.code,
        "use.plugin.runtime.binding_operation_candidate_changed"
    );

    let mut conflict = intent;
    conflict.plan_digest = SNAPSHOT.to_string();
    let conflict = store.begin_binding_change(&conflict).await.unwrap_err();
    assert_eq!(
        conflict.code,
        "use.plugin.runtime.binding_operation_conflict"
    );
}

#[tokio::test]
async fn operation_journal_rejects_unknown_fields_and_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RuntimeBindingOperationIntent>();
    assert_send_sync::<RuntimeBindingOperationJournal>();
    assert_send_sync::<RuntimeBindingRetirementEvidence>();

    let temporary = TempDir::new().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let receipt = task_receipt("convert", PACKAGE_A, 8);
    store
        .begin_binding_change(&intent(vec![candidate(&receipt)], Vec::new()))
        .await
        .unwrap();
    let path = operation_path(&store, SCOPE, "operation-01");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["unexpected"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let error = store
        .observe_binding_change(SCOPE, "operation-01")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.binding_operation_invalid");
}

fn intent(
    candidates: Vec<RuntimeBindingCandidatePlan>,
    retirements: Vec<RuntimeBindingReceipt>,
) -> RuntimeBindingOperationIntent {
    RuntimeBindingOperationIntent::new(
        "operation-01",
        PLAN,
        Some(GRANTS.to_string()),
        "workspace-01",
        4,
        7,
        2_000,
        candidates,
        retirements,
    )
    .unwrap()
}

fn cutover() -> RuntimeBindingCutoverEvidence {
    let intent = intent(
        vec![candidate(&task_receipt("cutover", PACKAGE_A, 8))],
        Vec::new(),
    );
    RuntimeBindingCutoverEvidence::from_grant_cutover(
        &intent,
        &a3s_use_extension::WorkspaceGrantCutoverEvidence {
            schema: a3s_use_extension::WORKSPACE_GRANT_CUTOVER_SCHEMA.to_string(),
            capability_generation_before: 7,
            capability_generation_after: 8,
            capability_snapshot_digest: SNAPSHOT.to_string(),
            committed_at_ms: 2_100,
        },
    )
    .unwrap()
}

#[test]
fn runtime_cutover_rejects_a_different_grant_capability_generation() {
    let intent = intent(
        vec![candidate(&task_receipt("cutover", PACKAGE_A, 8))],
        Vec::new(),
    );
    let grant = a3s_use_extension::WorkspaceGrantCutoverEvidence {
        schema: a3s_use_extension::WORKSPACE_GRANT_CUTOVER_SCHEMA.to_string(),
        capability_generation_before: 7,
        capability_generation_after: 9,
        capability_snapshot_digest: SNAPSHOT.to_string(),
        committed_at_ms: 2_100,
    };

    let error = RuntimeBindingCutoverEvidence::from_grant_cutover(&intent, &grant).unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.binding_operation_invalid");
}

fn candidate(receipt: &RuntimeBindingReceipt) -> RuntimeBindingCandidatePlan {
    match receipt {
        RuntimeBindingReceipt::Task(receipt) => RuntimeBindingCandidatePlan {
            surface: receipt.surface.clone(),
            package_digest: receipt.package_digest.clone(),
            scope_id: receipt.scope_id.clone(),
            descriptor_digest: receipt.descriptor_digest.clone(),
            provider: provider(
                receipt.surface.clone(),
                &receipt.provider_id,
                &receipt.provider_build_id,
                &receipt.capability_digest,
                &receipt.semantics_profile_digest,
            ),
            generation: receipt.generation,
            kind: RuntimeBindingCandidateKind::Task {
                artifact_digest: receipt.artifact_digest.clone(),
                artifact_media_type: receipt.artifact_media_type.clone(),
            },
        },
        RuntimeBindingReceipt::Service(receipt) => RuntimeBindingCandidatePlan {
            surface: receipt.surface.clone(),
            package_digest: receipt.package_digest.clone(),
            scope_id: receipt.scope_id.clone(),
            descriptor_digest: receipt.descriptor_digest.clone(),
            provider: provider(
                receipt.surface.clone(),
                &receipt.provider_id,
                &receipt.provider_build_id,
                &receipt.capability_digest,
                &receipt.semantics_profile_digest,
            ),
            generation: receipt.generation,
            kind: RuntimeBindingCandidateKind::Service {
                unit_id: receipt.unit_id.clone(),
                spec_digest: receipt.spec_digest.clone(),
                contract: receipt.contract.clone(),
            },
        },
    }
}

fn provider(
    surface: PlanQualifiedSurfaceRef,
    provider_id: &str,
    provider_build_id: &str,
    capability_digest: &str,
    semantics_profile_digest: &str,
) -> PlannedProviderEvidence {
    PlannedProviderEvidence {
        surface,
        provider_id: provider_id.to_string(),
        provider_build_id: provider_build_id.to_string(),
        capability_digest: capability_digest.to_string(),
        semantics_profile_digest: semantics_profile_digest.to_string(),
        enforcement: PlanEnforcementProfile::Container,
    }
}

fn task_receipt(id: &str, package_digest: &str, generation: u64) -> RuntimeBindingReceipt {
    RuntimeBindingReceipt::Task(RuntimePreparedTaskBinding {
        schema: RUNTIME_TASK_BINDING_SCHEMA.to_string(),
        surface: surface(id),
        package_digest: package_digest.to_string(),
        scope_id: "workspace-01".to_string(),
        descriptor_digest: DESCRIPTOR.to_string(),
        provider_id: "test-runtime".to_string(),
        provider_build_id: "build-1".to_string(),
        capability_digest: CAPABILITIES.to_string(),
        enforcement: PlanEnforcementProfile::Container,
        artifact_digest: ARTIFACT.to_string(),
        artifact_media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
        generation,
        semantics_profile_digest: SEMANTICS.to_string(),
    })
}

fn service_receipt(
    package_digest: &str,
    generation: u64,
    observation_revision: u64,
) -> RuntimeBindingReceipt {
    RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
        schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
        surface: surface("index"),
        package_digest: package_digest.to_string(),
        scope_id: "workspace-01".to_string(),
        descriptor_digest: DESCRIPTOR.to_string(),
        provider_id: "test-runtime".to_string(),
        provider_build_id: "build-1".to_string(),
        capability_digest: CAPABILITIES.to_string(),
        enforcement: PlanEnforcementProfile::Container,
        unit_id: format!("use:service:generation-{generation}"),
        generation,
        spec_digest: SPEC.to_string(),
        semantics_profile_digest: SEMANTICS.to_string(),
        endpoint_ref: RuntimeEndpointRef::parse(format!("gateway:workspace-01/index-{generation}"))
            .unwrap(),
        runtime_started_at_ms: observation_revision - 100,
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

fn surface(id: &str) -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_string(),
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: id.to_string(),
        },
    }
}

fn service(receipt: &RuntimeBindingReceipt) -> &RuntimeServiceBindingReceipt {
    let RuntimeBindingReceipt::Service(receipt) = receipt else {
        panic!("expected Service receipt");
    };
    receipt
}

fn removal_for(receipt: &RuntimeBindingReceipt, removed_at_ms: u64) -> RuntimeRemoval {
    let receipt = service(receipt);
    RuntimeRemoval {
        schema: RuntimeRemoval::SCHEMA.to_string(),
        request_id: format!("remove-{}", receipt.generation),
        unit_id: receipt.unit_id.clone(),
        generation: receipt.generation,
        removed_at_ms,
        already_absent: false,
    }
}

fn operation_path(
    store: &RuntimeBindingStore,
    scope_id: &str,
    operation_id: &str,
) -> std::path::PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
    let operation_digest = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
    store
        .root()
        .join(".operations")
        .join(scope_digest)
        .join(format!("{operation_digest}.json"))
}
