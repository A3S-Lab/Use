use a3s_use_extension::{WorkspaceGrantStore, WORKSPACE_GRANT_OPERATION_SCHEMA};
use tempfile::TempDir;

use crate::plugin_runtime::{
    RuntimeBindingOperationPhase, RuntimeBindingStore, RUNTIME_BINDING_OPERATION_SCHEMA,
};

use super::test_support::{
    canonical_envelope, grant_fixture, multi_scope_runtime_envelope, prepared_receipt,
    runtime_intent, runtime_only_envelope, COMMITTED_AT_MS, SNAPSHOT_DIGEST, TRANSITIONED_AT_MS,
};
use super::*;

#[test]
fn parent_derives_runtime_cutover_when_the_scope_has_no_grant_child() {
    let envelope = runtime_only_envelope();
    let intent = runtime_intent(
        &envelope,
        &envelope.plan.workspace_impacts[0].scope_id,
        None,
    );
    let binding = PluginLifecycleOperationBinding::from_intents(
        &envelope,
        TRANSITIONED_AT_MS,
        &[],
        std::slice::from_ref(&intent),
    )
    .unwrap();

    assert!(binding.grant_operations().is_empty());
    assert_eq!(binding.runtime_operations().len(), 1);
    assert_eq!(
        binding.runtime_operations()[0].grant_change_set_digest(),
        None
    );
    let serialized = serde_json::to_value(&binding).unwrap();
    assert!(serialized["runtimeOperations"][0]
        .get("grantChangeSetDigest")
        .is_none());

    let cutover = PluginLifecycleCutoverEvidence::new(
        &binding,
        SNAPSHOT_DIGEST,
        COMMITTED_AT_MS,
        COMMITTED_AT_MS,
    )
    .unwrap();
    let runtime_cutover = cutover
        .runtime_cutover(&binding, &intent, COMMITTED_AT_MS)
        .unwrap();
    assert_eq!(runtime_cutover.capability_snapshot_digest, SNAPSHOT_DIGEST);
    assert_eq!(
        runtime_cutover.capability_generation_after,
        binding.capability_generation_after()
    );

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PluginLifecycleOperationBinding>();
    assert_send_sync::<PluginLifecycleCutoverEvidence>();
}

#[test]
fn parent_binding_sorts_and_keeps_same_operation_multi_scope_children_disjoint() {
    let envelope = multi_scope_runtime_envelope();
    let alpha = runtime_intent(&envelope, "workspace:alpha", None);
    let beta = runtime_intent(&envelope, "workspace:beta", None);
    let binding = PluginLifecycleOperationBinding::from_intents(
        &envelope,
        TRANSITIONED_AT_MS,
        &[],
        &[beta.clone(), alpha.clone()],
    )
    .unwrap();

    assert_eq!(
        binding
            .runtime_operations()
            .iter()
            .map(PluginLifecycleRuntimeIntentBinding::scope_id)
            .collect::<Vec<_>>(),
        vec!["workspace:alpha", "workspace:beta"]
    );
    assert_ne!(
        binding.runtime_operations()[0].intent_digest(),
        binding.runtime_operations()[1].intent_digest()
    );
    binding
        .validate_children(&envelope, &[], &[beta, alpha])
        .unwrap();
}

#[test]
fn parent_binding_rejects_provider_scope_generation_and_plan_drift() {
    let envelope = runtime_only_envelope();
    let intent = runtime_intent(
        &envelope,
        &envelope.plan.workspace_impacts[0].scope_id,
        None,
    );
    let binding = PluginLifecycleOperationBinding::from_intents(
        &envelope,
        TRANSITIONED_AT_MS,
        &[],
        std::slice::from_ref(&intent),
    )
    .unwrap();

    let mut provider_drift = intent.clone();
    provider_drift.candidates[0].provider.provider_build_id = "runtime:changed".to_string();
    provider_drift.validate().unwrap();
    assert_eq!(
        binding
            .validate_children(&envelope, &[], &[provider_drift])
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_binding_invalid"
    );

    let mut scope_drift = intent.clone();
    scope_drift.scope_id = "workspace:other".to_string();
    for candidate in &mut scope_drift.candidates {
        candidate.scope_id = scope_drift.scope_id.clone();
    }
    scope_drift.validate().unwrap();
    assert_eq!(
        binding
            .validate_children(&envelope, &[], &[scope_drift])
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_binding_invalid"
    );

    let mut generation_drift = intent.clone();
    generation_drift.capability_generation_before += 1;
    generation_drift.capability_generation_after += 1;
    for candidate in &mut generation_drift.candidates {
        candidate.generation += 1;
    }
    generation_drift.validate().unwrap();
    assert_eq!(
        binding
            .validate_children(&envelope, &[], &[generation_drift])
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_binding_invalid"
    );

    let mut changed_plan = envelope.plan.clone();
    changed_plan.state.state_revision += 1;
    let changed_plan = a3s_use_core::PluginOperationPlanEnvelope::new(changed_plan).unwrap();
    assert_eq!(
        binding
            .validate_against_plan(&changed_plan)
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_binding_invalid"
    );
}

#[test]
fn parent_contracts_reject_unknown_fields_and_digest_tampering() {
    let envelope = runtime_only_envelope();
    let intent = runtime_intent(
        &envelope,
        &envelope.plan.workspace_impacts[0].scope_id,
        None,
    );
    let binding = PluginLifecycleOperationBinding::from_intents(
        &envelope,
        TRANSITIONED_AT_MS,
        &[],
        &[intent],
    )
    .unwrap();
    let mut binding_value = serde_json::to_value(&binding).unwrap();
    binding_value["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PluginLifecycleOperationBinding>(binding_value).is_err());

    let cutover = PluginLifecycleCutoverEvidence::new(
        &binding,
        SNAPSHOT_DIGEST,
        COMMITTED_AT_MS,
        COMMITTED_AT_MS,
    )
    .unwrap();
    let mut cutover_value = serde_json::to_value(&cutover).unwrap();
    cutover_value["capabilitySnapshotDigest"] = serde_json::json!(
        "sha256:9999999999999999999999999999999999999999999999999999999999999999"
    );
    let tampered: PluginLifecycleCutoverEvidence = serde_json::from_value(cutover_value).unwrap();
    assert_eq!(
        tampered
            .validate_against(&binding, COMMITTED_AT_MS)
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_binding_invalid"
    );
}

#[test]
fn parent_binding_requires_complete_runtime_provider_coverage() {
    let envelope = runtime_only_envelope();
    let mut incomplete = runtime_intent(
        &envelope,
        &envelope.plan.workspace_impacts[0].scope_id,
        None,
    );
    incomplete.candidates.pop();
    incomplete.validate().unwrap();

    assert_eq!(
        PluginLifecycleOperationBinding::from_intents(
            &envelope,
            TRANSITIONED_AT_MS,
            &[],
            &[incomplete],
        )
        .unwrap_err()
        .code,
        "use.plugin.lifecycle_binding_invalid"
    );
}

#[tokio::test]
async fn parent_gate_recovers_partial_child_cutover_after_store_reopen() {
    let temporary = TempDir::new().unwrap();
    let envelope = canonical_envelope();
    let scope_id = envelope.plan.workspace_impacts[0].scope_id.clone();
    let operation_id = envelope.plan.operation_id.clone();
    let (resolved, ceilings) = grant_fixture(&envelope);
    let runtime_intent = runtime_intent(
        &envelope,
        &scope_id,
        Some(resolved.change_set_digest.clone()),
    );
    let grant_store = WorkspaceGrantStore::new(temporary.path());
    let runtime_store = RuntimeBindingStore::new(temporary.path());

    let grant_started = grant_store
        .begin_change_set(&resolved, &ceilings)
        .await
        .unwrap();
    let runtime_started = runtime_store
        .begin_binding_change(&runtime_intent)
        .await
        .unwrap();
    let binding = PluginLifecycleOperationBinding::from_intents(
        &envelope,
        TRANSITIONED_AT_MS,
        std::slice::from_ref(&grant_started.intent),
        std::slice::from_ref(&runtime_started.intent),
    )
    .unwrap();
    assert!(binding
        .verify_ready_for_cutover(&[grant_started], &[runtime_started])
        .is_err());

    let grant_prepared = grant_store
        .prepare_change_set(&scope_id, &operation_id, TRANSITIONED_AT_MS)
        .await
        .unwrap();
    for candidate in &runtime_intent.candidates {
        runtime_store
            .record_prepared_binding(&scope_id, &operation_id, &prepared_receipt(candidate))
            .await
            .unwrap();
    }
    let runtime_published = runtime_store
        .publish_prepared_bindings(&scope_id, &operation_id)
        .await
        .unwrap();
    assert_eq!(
        runtime_published.phase,
        RuntimeBindingOperationPhase::BindingsPublished
    );
    let mut tampered_binding = serde_json::to_value(&binding).unwrap();
    tampered_binding["bindingDigest"] = serde_json::json!(
        "sha256:9999999999999999999999999999999999999999999999999999999999999999"
    );
    let tampered_binding: PluginLifecycleOperationBinding =
        serde_json::from_value(tampered_binding).unwrap();
    assert_eq!(
        tampered_binding
            .verify_ready_for_cutover(
                std::slice::from_ref(&grant_prepared),
                std::slice::from_ref(&runtime_published),
            )
            .unwrap_err()
            .code,
        "use.plugin.lifecycle_binding_invalid"
    );
    binding
        .verify_ready_for_cutover(
            std::slice::from_ref(&grant_prepared),
            std::slice::from_ref(&runtime_published),
        )
        .unwrap();

    let parent_cutover = PluginLifecycleCutoverEvidence::new(
        &binding,
        SNAPSHOT_DIGEST,
        COMMITTED_AT_MS,
        COMMITTED_AT_MS,
    )
    .unwrap();
    let grant_cutover = parent_cutover
        .grant_cutover(&binding, &grant_prepared.intent, COMMITTED_AT_MS)
        .unwrap();
    let runtime_cutover = parent_cutover
        .runtime_cutover(&binding, &runtime_published.intent, COMMITTED_AT_MS)
        .unwrap();
    assert_eq!(
        grant_cutover.capability_snapshot_digest,
        runtime_cutover.capability_snapshot_digest
    );
    assert_eq!(
        grant_cutover.capability_generation_after,
        runtime_cutover.capability_generation_after
    );

    grant_store
        .commit_change_set_cutover(
            &scope_id,
            &operation_id,
            grant_cutover.clone(),
            COMMITTED_AT_MS,
        )
        .await
        .unwrap();
    drop(grant_store);
    drop(runtime_store);

    let grant_store = WorkspaceGrantStore::new(temporary.path());
    let runtime_store = RuntimeBindingStore::new(temporary.path());
    let grant_partial = grant_store
        .observe_change_set(&scope_id, &operation_id)
        .await
        .unwrap()
        .unwrap();
    let runtime_partial = runtime_store
        .observe_binding_change(&scope_id, &operation_id)
        .await
        .unwrap()
        .unwrap();
    assert!(binding
        .verify_completed(
            &parent_cutover,
            std::slice::from_ref(&grant_partial),
            std::slice::from_ref(&runtime_partial),
            COMMITTED_AT_MS,
        )
        .is_err());
    assert!(binding
        .verify_ready_for_cutover(&[grant_partial], &[runtime_partial])
        .is_err());

    let runtime_completed = runtime_store
        .commit_binding_cutover(
            &scope_id,
            &operation_id,
            runtime_cutover.clone(),
            COMMITTED_AT_MS,
        )
        .await
        .unwrap();
    let grant_completed = grant_store
        .retire_change_set(&scope_id, &operation_id)
        .await
        .unwrap();
    binding
        .verify_completed(
            &parent_cutover,
            std::slice::from_ref(&grant_completed),
            std::slice::from_ref(&runtime_completed),
            COMMITTED_AT_MS,
        )
        .unwrap();

    assert_eq!(grant_completed.schema, WORKSPACE_GRANT_OPERATION_SCHEMA);
    assert_eq!(runtime_completed.schema, RUNTIME_BINDING_OPERATION_SCHEMA);
    assert_eq!(
        grant_store
            .commit_change_set_cutover(&scope_id, &operation_id, grant_cutover, COMMITTED_AT_MS,)
            .await
            .unwrap(),
        grant_completed
    );
    assert_eq!(
        runtime_store
            .commit_binding_cutover(&scope_id, &operation_id, runtime_cutover, COMMITTED_AT_MS,)
            .await
            .unwrap(),
        runtime_completed
    );
    assert_eq!(
        grant_store
            .retire_change_set(&scope_id, &operation_id)
            .await
            .unwrap(),
        grant_completed
    );
}
