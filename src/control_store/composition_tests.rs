use a3s_runtime::contract::NetworkMode;
use a3s_use_core::{
    PlanScope, PlanScopeKind, PluginOperationAction, PluginSurfaceKind, PluginSurfaceRef,
};
use a3s_use_extension::{ExtensionPaths, PluginMcpSurface, ToolSurface};
use async_trait::async_trait;

use super::aggregate_tests::fixtures::{control_installation, snapshot};
use super::composition::{validate_runtime_publication_authority, validate_runtime_publications};
use super::composition::{ControlEffectCompositionDependencies, ControlStoreRuntimeComposition};
use super::dispatcher::SystemControlEffectClock;
use super::effect_owner::runtime::ControlRuntimeServiceReadinessPort;
use super::effect_port::{
    ControlEffectFailure, ControlEffectPortOutcome, ControlFlowEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectRequest,
};
use super::model::{
    ControlCapabilitySelection, ControlEffectIntent, ControlEffectKind, ControlEffectOwner,
    ControlEffectSubject, ControlTransition,
};
use crate::plugin_runtime::test_support::{
    artifact, capabilities, context, evidence, policy, task_descriptor, task_surface,
};
use crate::plugin_runtime::{
    plan_tool_task_release, RuntimeSurfaceContext, RuntimeSurfacePlanKey,
    RuntimeSurfacePlanPublication, RuntimeTaskInvocation,
};

fn cognitive_authorization(
    reviewed: &super::model::ReviewedControlOperation,
) -> crate::cognitive_package::CognitivePackageAuthorizationEvidence {
    crate::cognitive_package::CognitivePackageAuthorizationEvidence {
        operation_confirmation: reviewed.authorization.operation_confirmation.clone(),
        grant_confirmations: reviewed.authorization.grant_confirmations.clone(),
    }
}

fn planned_grants(
    reviewed: &super::model::ReviewedControlOperation,
) -> crate::cognitive_package::PlannedWorkspaceGrantOperation {
    let transition = reviewed
        .authorization
        .grant_transition
        .as_ref()
        .expect("the fixture must carry reviewed Grant evidence");
    crate::cognitive_package::PlannedWorkspaceGrantOperation {
        snapshot: transition.snapshot.clone(),
        change_set: transition.change_set.clone(),
        ceilings: Vec::new(),
    }
}

struct RejectingFlow;

#[async_trait]
impl ControlFlowEffectPort for RejectingFlow {
    async fn apply_surface(
        &self,
        _request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        ControlEffectPortOutcome::rejected(
            ControlEffectFailure::new(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "provider.flow_unavailable",
            )
            .unwrap(),
        )
    }
}

struct RejectingReadiness;

fn readiness_error() -> a3s_use_core::UseError {
    a3s_use_core::UseError::new(
        "provider.gateway_unavailable",
        "Gateway readiness is not configured in this composition fixture.",
    )
}

#[async_trait]
impl ControlRuntimeServiceReadinessPort for RejectingReadiness {
    async fn bind_tool_service(
        &self,
        _surface: &ToolSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<crate::plugin_runtime::RuntimeEndpointRef> {
        Err(readiness_error())
    }

    async fn bind_mcp_service(
        &self,
        _surface: &PluginMcpSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<super::effect_owner::runtime::ControlRuntimeMcpReadiness> {
        Err(readiness_error())
    }

    async fn drain_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<()> {
        Err(readiness_error())
    }

    async fn remove_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> a3s_use_core::UseResult<()> {
        Err(readiness_error())
    }
}

fn runtime_transition(
    plan: &crate::plugin_runtime::RuntimeSurfacePlan,
    provider: &a3s_use_core::PlannedProviderEvidence,
) -> (ControlTransition, RuntimeSurfacePlanPublication) {
    let installation = control_installation();
    let key = RuntimeSurfacePlanKey::from_plan(plan, provider).unwrap();
    let subject = ControlEffectSubject::Surface {
        package_id: plan.context().package_id().to_string(),
        lifecycle_generation: plan.context().generation(),
        package_digest: plan.context().package_digest().to_string(),
        manifest_digest: plan.context().package_digest().to_string(),
        action: crate::plugin_lifecycle::PluginLifecycleAction::Install,
        surface: PluginSurfaceRef {
            kind: plan.surface().surface.kind,
            id: plan.surface().surface.id.clone(),
        },
    };
    let effect = ControlEffectIntent::new(
        0,
        installation.clone(),
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string(),
        PluginOperationAction::Install,
        plan.context().generation(),
        subject,
        ControlEffectOwner::RuntimeProvider {
            provider_id: provider.provider_id.clone(),
            selection_digest: key.selection_digest.clone(),
        },
        ControlEffectKind::SurfacePrepare,
        true,
    )
    .unwrap();
    let transition = ControlTransition {
        operation_id: "operation:composition".to_string(),
        plan_digest: effect.plan_digest.clone(),
        snapshot: snapshot(installation, plan.context().generation()),
        package_lifecycles: Vec::new(),
        grants: Vec::new(),
        provider_selections: Vec::new(),
        capability: ControlCapabilitySelection {
            generation: 1,
            descriptor_digest:
                "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    .to_string(),
        },
        effects: vec![effect],
        committed_at_ms: 1,
    };
    (
        transition,
        RuntimeSurfacePlanPublication::new(key, plan.clone()).unwrap(),
    )
}

#[test]
fn runtime_publications_cover_exact_target_prepare_effects() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let (transition, publication) = runtime_transition(&plan, &provider);

    validate_runtime_publications(&transition, std::slice::from_ref(&publication)).unwrap();

    let error = validate_runtime_publications(&transition, &[]).unwrap_err();
    assert_eq!(
        error.code,
        "use.control_store.runtime_plan_publication_invalid"
    );

    let duplicate = vec![publication.clone(), publication];
    let error = validate_runtime_publications(&transition, &duplicate).unwrap_err();
    assert_eq!(
        error.code,
        "use.control_store.runtime_plan_publication_invalid"
    );
}

#[test]
fn transitions_without_runtime_prepares_reject_plan_payloads() {
    let installation = control_installation();
    let reviewed = super::aggregate_tests::fixtures::operation("operation:composition");
    let transition = super::aggregate_tests::fixtures::transition(installation, &reviewed);
    assert!(transition
        .effects
        .iter()
        .all(|effect| effect.kind != ControlEffectKind::SurfacePrepare
            || !matches!(effect.owner, ControlEffectOwner::RuntimeProvider { .. })));

    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::from_plan(&plan, &provider).unwrap();
    let publication = RuntimeSurfacePlanPublication::new(key, plan).unwrap();
    let error = validate_runtime_publications(&transition, &[publication]).unwrap_err();
    assert_eq!(
        error.code,
        "use.control_store.runtime_plan_publication_invalid"
    );
}

#[test]
fn runtime_plan_publication_requires_the_reviewed_grant_proposal() {
    let reviewed = super::aggregate_tests::grant_fixtures::reviewed_grant_operation(
        "operation:composition:grant",
        PluginOperationAction::Install,
        None,
        None,
    );
    let proposal = reviewed
        .authorization
        .grant_transition
        .as_ref()
        .and_then(|transition| transition.change_set.changes[0].after.as_ref())
        .unwrap();
    let descriptor = task_descriptor();
    let context = RuntimeSurfaceContext::new(
        proposal.package_id.clone(),
        proposal.package_digest.clone(),
        PlanScope {
            kind: PlanScopeKind::Workspace,
            id: proposal.scope_id.clone(),
        },
        proposal.descriptor_digest().unwrap(),
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_string(),
        },
        1,
    )
    .unwrap();
    let plan = plan_tool_task_release(
        context,
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::from_plan(&plan, &provider).unwrap();
    let publication = RuntimeSurfacePlanPublication::new(key, plan).unwrap();
    validate_runtime_publication_authority(&reviewed, std::slice::from_ref(&publication)).unwrap();

    let wrong_context = RuntimeSurfaceContext::new(
        proposal.package_id.clone(),
        proposal.package_digest.clone(),
        PlanScope {
            kind: PlanScopeKind::Workspace,
            id: proposal.scope_id.clone(),
        },
        super::aggregate_tests::fixtures::digest('f'),
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_string(),
        },
        1,
    )
    .unwrap();
    let wrong_plan = plan_tool_task_release(
        wrong_context,
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let wrong_provider = evidence(&wrong_plan, &capabilities(&wrong_plan));
    let wrong = RuntimeSurfacePlanPublication::new(
        RuntimeSurfacePlanKey::from_plan(&wrong_plan, &wrong_provider).unwrap(),
        wrong_plan,
    )
    .unwrap();
    let error = validate_runtime_publication_authority(&reviewed, &[wrong]).unwrap_err();
    assert_eq!(
        error.code,
        "use.control_store.runtime_plan_publication_invalid"
    );
}

#[tokio::test]
async fn composition_initializes_one_root_and_commits_without_runtime_payloads() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = control_installation();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation.clone(),
    )
    .unwrap();
    let composition = ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        ControlEffectCompositionDependencies {
            runtime_registry: std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: std::sync::Arc::new(RejectingReadiness),
            flow: std::sync::Arc::new(RejectingFlow),
            clock: std::sync::Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    assert_eq!(composition.store().installation, installation);
    assert_eq!(
        composition.plan_store().installation(),
        &composition.store().installation
    );

    let reviewed = super::aggregate_tests::fixtures::operation("operation:composition");
    let generation = composition
        .admit_and_commit_cognitive_package_operation_with_runtime_plans(
            &reviewed.envelope,
            &cognitive_authorization(&reviewed),
            None,
            reviewed.reviewed_at_ms,
            reviewed.reviewed_at_ms + 10,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(generation.snapshot.generation, 1);
    assert!(composition
        .plan_store()
        .inspect_keys()
        .await
        .unwrap()
        .is_empty());

    let stale = super::aggregate_tests::fixtures::operation("operation:composition-stale");
    let error = composition
        .register_cognitive_package_operation(
            &stale.envelope,
            &cognitive_authorization(&stale),
            None,
            stale.reviewed_at_ms,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.generation_changed");
}

#[tokio::test]
async fn lifecycle_admission_checks_global_reference_fence_before_registering() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = control_installation();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation,
    )
    .unwrap();
    let composition = ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        ControlEffectCompositionDependencies {
            runtime_registry: std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: std::sync::Arc::new(RejectingReadiness),
            flow: std::sync::Arc::new(RejectingFlow),
            clock: std::sync::Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let reviewed = super::aggregate_tests::fixtures::operation("operation:admission-order");
    let collection = paths.artifact_store().acquire_collection().await.unwrap();

    let error = composition
        .admit_and_commit_cognitive_package_operation_with_runtime_plans(
            &reviewed.envelope,
            &cognitive_authorization(&reviewed),
            None,
            reviewed.reviewed_at_ms,
            reviewed.reviewed_at_ms + 10,
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.busy");
    assert!(composition
        .store()
        .operation(reviewed.operation_id())
        .await
        .unwrap()
        .is_none());
    drop(collection);
}

#[tokio::test]
async fn lifecycle_admission_retains_reviewed_operation_when_runtime_payloads_are_missing() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap();
    let composition = ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        ControlEffectCompositionDependencies {
            runtime_registry: std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: std::sync::Arc::new(RejectingReadiness),
            flow: std::sync::Arc::new(RejectingFlow),
            clock: std::sync::Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let reviewed = super::aggregate_tests::grant_fixtures::reviewed_grant_operation(
        "operation:admission-runtime-payload-required",
        PluginOperationAction::Install,
        None,
        None,
    );
    let grants = planned_grants(&reviewed);

    let error = composition
        .admit_and_commit_cognitive_package_operation_with_runtime_plans(
            &reviewed.envelope,
            &cognitive_authorization(&reviewed),
            Some(&grants),
            reviewed.reviewed_at_ms,
            reviewed.reviewed_at_ms + 10,
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control_store.runtime_plan_publication_invalid"
    );
    let operation = composition
        .store()
        .operation(reviewed.operation_id())
        .await
        .unwrap()
        .expect("failed publication must leave a reviewed operation for retry");
    assert_eq!(
        operation.status,
        super::model::ControlOperationStatus::Reviewed
    );
    assert!(composition
        .store()
        .current_generation()
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn composition_is_fenced_by_global_artifact_reference_admission() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = control_installation();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation,
    )
    .unwrap();
    let composition = ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        ControlEffectCompositionDependencies {
            runtime_registry: std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: std::sync::Arc::new(RejectingReadiness),
            flow: std::sync::Arc::new(RejectingFlow),
            clock: std::sync::Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let reviewed = super::aggregate_tests::fixtures::operation("operation:admission-fence");
    composition
        .store()
        .register_operation(reviewed.clone())
        .await
        .unwrap();
    let before = composition.store().current_generation().await.unwrap();

    // A collector owns the exclusive side of the global reference boundary.
    // The composition must fail before taking the installation fence or
    // projecting/committing any Control generation.
    let collection = paths.artifact_store().acquire_collection().await.unwrap();
    let error = composition
        .commit_reviewed_operation_with_runtime_plans(
            reviewed.operation_id(),
            reviewed.reviewed_at_ms + 10,
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.busy");
    assert_eq!(
        composition.store().current_generation().await.unwrap(),
        before
    );
    drop(collection);
}

#[tokio::test]
async fn composition_projects_a_reviewed_operation_without_caller_graph_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = control_installation();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation.clone(),
    )
    .unwrap();
    let composition = ControlStoreRuntimeComposition::from_extension_paths(
        &paths,
        ControlEffectCompositionDependencies {
            runtime_registry: std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            runtime_readiness: std::sync::Arc::new(RejectingReadiness),
            flow: std::sync::Arc::new(RejectingFlow),
            clock: std::sync::Arc::new(SystemControlEffectClock),
        },
    )
    .unwrap();
    composition.initialize().await.unwrap();
    let reviewed = super::aggregate_tests::fixtures::operation("operation:projected");
    composition
        .store()
        .register_operation(reviewed.clone())
        .await
        .unwrap();
    let generation = composition
        .commit_reviewed_operation_with_runtime_plans(
            reviewed.operation_id(),
            reviewed.reviewed_at_ms + 10,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(generation.snapshot.generation, 1);
    assert_eq!(generation.operation_id, reviewed.operation_id());
}
