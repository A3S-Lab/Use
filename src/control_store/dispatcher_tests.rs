use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use a3s_use_core::{
    InstallationId, PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlannedProviderEvidence,
    PluginOperationAction, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
};
use async_trait::async_trait;

use super::aggregate_tests::fixtures::{
    control_installation, digest, initialized_store, operation, transition,
};
use super::dispatcher::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    ControlEffectDispatcher, ControlEffectPorts, RoutedControlEffectOutcome,
    SystemControlEffectClock,
};
use super::effect_port::{
    ControlCapabilityCutoverRequest, ControlCapabilityIndexEffectPort, ControlEffectFailure,
    ControlEffectPortOutcome, ControlFlowEffectPort, ControlInvocationDrainRequest,
    ControlInvocationLeaseEffectPort, ControlKnowledgeEffectPort, ControlReceiptApplication,
    ControlRuntimeApplication, ControlRuntimeEffectPort, ControlRuntimeEffectRequest,
    ControlSkillEffectPort, ControlSurfaceApplication, ControlSurfaceEffectAction,
    ControlSurfaceEffectRequest, ControlUiEffectPort,
};
use super::model::{
    ClaimedControlEffect, ControlCapabilityEffectAuthority, ControlCapabilityStatus,
    ControlEffectAuthority, ControlEffectIntent, ControlEffectKind, ControlEffectOutcome,
    ControlEffectOwner, ControlEffectStatus, ControlEffectSubject, ControlGeneration,
    ControlOperationStatus, ControlPackageEffectAuthority, ControlProviderSelection,
    ControlRuntimeBindingObservation, ControlRuntimeEffectAuthority,
};
use super::ControlStore;
use crate::plugin_lifecycle::PluginLifecycleAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Applied,
    Rejected,
    Unknown,
}

struct RecordingPorts {
    dispositions: Mutex<VecDeque<Disposition>>,
    calls: Mutex<Vec<&'static str>>,
    authority_generations: Mutex<Vec<u64>>,
    inspect_store: Option<ControlStore>,
    delay_ms: u64,
}

impl RecordingPorts {
    fn new(dispositions: impl IntoIterator<Item = Disposition>) -> Self {
        Self {
            dispositions: Mutex::new(dispositions.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
            authority_generations: Mutex::new(Vec::new()),
            inspect_store: None,
            delay_ms: 0,
        }
    }

    fn with_reentrant_inspection(mut self, store: ControlStore) -> Self {
        self.inspect_store = Some(store);
        self
    }

    fn with_delay_ms(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }

    async fn before_call(&self, owner: &'static str, authority_generation: u64) {
        if let Some(store) = &self.inspect_store {
            store.inspect().await.unwrap();
        }
        self.calls.lock().unwrap().push(owner);
        self.authority_generations
            .lock()
            .unwrap()
            .push(authority_generation);
        if self.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        }
    }

    fn outcome<T>(&self, applied: T) -> ControlEffectPortOutcome<T> {
        match self
            .dispositions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Disposition::Applied)
        {
            Disposition::Applied => ControlEffectPortOutcome::applied(applied),
            Disposition::Rejected => ControlEffectPortOutcome::rejected(
                ControlEffectFailure::new(digest('e'), "provider.rejected").unwrap(),
            ),
            Disposition::Unknown => ControlEffectPortOutcome::unknown(
                ControlEffectFailure::new(digest('f'), "provider.ambiguous").unwrap(),
            ),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn authority_generations(&self) -> Vec<u64> {
        self.authority_generations.lock().unwrap().clone()
    }
}

#[async_trait]
impl ControlCapabilityIndexEffectPort for RecordingPorts {
    async fn cutover(
        &self,
        request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        self.before_call(
            "capability-index",
            request.authority.generation.snapshot.generation,
        )
        .await;
        self.outcome(ControlReceiptApplication::new(digest('1')).unwrap())
    }
}

#[async_trait]
impl ControlInvocationLeaseEffectPort for RecordingPorts {
    async fn drain(
        &self,
        request: &ControlInvocationDrainRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        self.before_call(
            "invocation-leases",
            request.authority.installation_generation,
        )
        .await;
        self.outcome(ControlReceiptApplication::new(digest('2')).unwrap())
    }
}

#[async_trait]
impl ControlRuntimeEffectPort for RecordingPorts {
    async fn apply_surface(
        &self,
        request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
        self.before_call(
            "runtime-provider",
            request.authority.package.installation_generation,
        )
        .await;
        let binding = (request.surface.action == ControlSurfaceEffectAction::Prepare)
            .then_some(ControlRuntimeBindingObservation::Task);
        self.outcome(ControlRuntimeApplication::new(request, digest('3'), binding).unwrap())
    }
}

macro_rules! surface_port {
    ($trait_name:ident, $owner:literal) => {
        #[async_trait]
        impl $trait_name for RecordingPorts {
            async fn apply_surface(
                &self,
                request: &ControlSurfaceEffectRequest,
            ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
                self.before_call($owner, request.authority.installation_generation)
                    .await;
                let materialization =
                    (request.action == ControlSurfaceEffectAction::Prepare).then(|| digest('4'));
                self.outcome(
                    ControlSurfaceApplication::new(request, digest('5'), materialization).unwrap(),
                )
            }
        }
    };
}

surface_port!(ControlFlowEffectPort, "flow-host");
surface_port!(ControlKnowledgeEffectPort, "knowledge-host");
surface_port!(ControlSkillEffectPort, "skill-host");
surface_port!(ControlUiEffectPort, "ui-host");

struct TestClock {
    times: Mutex<VecDeque<u64>>,
}

impl TestClock {
    fn new(times: impl IntoIterator<Item = u64>) -> Self {
        Self {
            times: Mutex::new(times.into_iter().collect()),
        }
    }
}

impl ControlEffectClock for TestClock {
    fn now_ms(&self) -> UseResult<u64> {
        self.times.lock().unwrap().pop_front().ok_or_else(|| {
            UseError::new(
                "use.control_store.test_clock_exhausted",
                "The deterministic Control effect test clock is exhausted.",
            )
        })
    }
}

fn ports(recording: Arc<RecordingPorts>) -> ControlEffectPorts {
    ControlEffectPorts::new(
        recording.clone(),
        recording.clone(),
        recording.clone(),
        recording.clone(),
        recording.clone(),
        recording.clone(),
        recording,
    )
}

fn dispatch_request(token: &str, explicit_reconciliation: bool) -> ControlEffectDispatchRequest {
    ControlEffectDispatchRequest {
        operation_id: "operation:dispatcher".to_string(),
        worker_id: "worker:dispatcher".to_string(),
        claim_token: token.to_string(),
        lease_duration_ms: 10_000,
        provider_timeout_ms: 5_000,
        explicit_reconciliation,
    }
}

#[tokio::test]
async fn dispatcher_enters_provider_only_after_commit_and_releases_store_resources() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:dispatcher");
    store.register_operation(reviewed.clone()).await.unwrap();
    let recording = Arc::new(
        RecordingPorts::new([Disposition::Applied]).with_reentrant_inspection(store.clone()),
    );
    let dispatcher = ControlEffectDispatcher::new(
        store.clone(),
        ports(recording.clone()),
        Arc::new(TestClock::new([20, 30, 35])),
    );

    assert_eq!(
        dispatcher
            .dispatch_next(dispatch_request("claim:before-commit", false))
            .await
            .unwrap_err()
            .code,
        "use.control_store.conflict"
    );
    assert!(recording.calls().is_empty());

    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let observed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        dispatcher.dispatch_next(dispatch_request("claim:after-commit", false)),
    )
    .await
    .expect("provider re-entry must not deadlock the bounded store")
    .unwrap();
    assert!(matches!(
        observed,
        ControlEffectDispatchResult::Observed {
            sequence: 0,
            attempt: 1,
            outcome: ControlEffectOutcome::Applied,
            observation_changed: true,
            ..
        }
    ));
    assert_eq!(recording.calls(), vec!["knowledge-host"]);
    assert_eq!(recording.authority_generations(), vec![1]);
    assert_eq!(
        store.effects(reviewed.operation_id()).await.unwrap()[0].status,
        ControlEffectStatus::Applied
    );
}

#[tokio::test]
async fn explicit_provider_rejection_rejects_a_required_pre_cutover_operation() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:dispatcher");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let recording = Arc::new(RecordingPorts::new([Disposition::Rejected]));
    let dispatcher = ControlEffectDispatcher::new(
        store.clone(),
        ports(recording),
        Arc::new(TestClock::new([30, 35])),
    );

    let result = dispatcher
        .dispatch_next(dispatch_request("claim:rejected", false))
        .await
        .unwrap();
    assert!(matches!(
        result,
        ControlEffectDispatchResult::Observed {
            outcome: ControlEffectOutcome::Rejected,
            ..
        }
    ));
    assert_eq!(
        store
            .operation(reviewed.operation_id())
            .await
            .unwrap()
            .unwrap()
            .status,
        ControlOperationStatus::Rejected
    );
}

#[tokio::test]
async fn unknown_acceptance_requires_explicit_same_key_reconciliation() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:dispatcher");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let recording = Arc::new(RecordingPorts::new([
        Disposition::Unknown,
        Disposition::Applied,
    ]));
    let dispatcher = ControlEffectDispatcher::new(
        store,
        ports(recording.clone()),
        Arc::new(TestClock::new([30, 35, 50, 70, 75])),
    );

    let first = dispatcher
        .dispatch_next(dispatch_request("claim:unknown", false))
        .await
        .unwrap();
    let ControlEffectDispatchResult::Observed {
        idempotency_key: first_key,
        attempt: 1,
        outcome: ControlEffectOutcome::Unknown,
        ..
    } = first
    else {
        panic!("the first provider observation must be unknown");
    };

    assert_eq!(
        dispatcher
            .dispatch_next(dispatch_request("claim:implicit-retry", false))
            .await
            .unwrap_err()
            .code,
        "use.control_store.reconciliation_required"
    );
    assert_eq!(recording.calls(), vec!["knowledge-host"]);

    let reconciled = dispatcher
        .dispatch_next(dispatch_request("claim:explicit-retry", true))
        .await
        .unwrap();
    let ControlEffectDispatchResult::Observed {
        idempotency_key,
        attempt: 2,
        outcome: ControlEffectOutcome::Applied,
        ..
    } = reconciled
    else {
        panic!("explicit reconciliation must apply the same effect");
    };
    assert_eq!(idempotency_key, first_key);
    assert_eq!(recording.calls(), vec!["knowledge-host", "knowledge-host"]);
}

#[tokio::test]
async fn expired_claim_after_process_exit_replays_only_with_the_committed_key() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:dispatcher");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let abandoned = store
        .claim_next_effect(super::model::ControlEffectClaim {
            operation_id: reviewed.operation_id().to_string(),
            worker_id: "worker:exited".to_string(),
            claim_token: "claim:exited".to_string(),
            now_ms: 30,
            lease_until_ms: 40,
            explicit_reconciliation: false,
        })
        .await
        .unwrap()
        .unwrap();

    let recording = Arc::new(RecordingPorts::new([
        Disposition::Applied,
        Disposition::Applied,
    ]));
    let effect_only = ControlEffectDispatcher::new(
        store.clone(),
        ports(recording.clone()),
        Arc::new(TestClock::new(std::iter::empty())),
    );
    assert!(matches!(
        effect_only
            .apply_claimed(reviewed.operation_id(), &abandoned)
            .await
            .unwrap(),
        RoutedControlEffectOutcome::Applied(_)
    ));
    assert_eq!(recording.calls(), vec!["knowledge-host"]);

    // Simulate process exit after the owner accepted the effect but before the
    // later observation transaction. Recovery may call the owner only with the
    // exact committed identity again.
    let dispatcher = ControlEffectDispatcher::new(
        store,
        ports(recording.clone()),
        Arc::new(TestClock::new([41, 50, 55])),
    );
    assert_eq!(
        dispatcher
            .dispatch_next(dispatch_request("claim:implicit-exit-retry", false))
            .await
            .unwrap_err()
            .code,
        "use.control_store.reconciliation_required"
    );
    assert_eq!(recording.calls(), vec!["knowledge-host"]);

    let replayed = dispatcher
        .dispatch_next(dispatch_request("claim:explicit-exit-retry", true))
        .await
        .unwrap();
    let ControlEffectDispatchResult::Observed {
        idempotency_key,
        attempt: 2,
        outcome: ControlEffectOutcome::Applied,
        ..
    } = replayed
    else {
        panic!("the expired claim must reconcile as the second attempt");
    };
    assert_eq!(idempotency_key, abandoned.intent.idempotency_key);
    assert_eq!(recording.calls(), vec!["knowledge-host", "knowledge-host"]);
}

#[tokio::test]
async fn dispatcher_bounds_a_hung_provider_and_records_unknown_acceptance() {
    let (_temporary, store) = initialized_store().await;
    let reviewed = operation("operation:dispatcher");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    let recording = Arc::new(RecordingPorts::new([Disposition::Applied]).with_delay_ms(100));
    let dispatcher = ControlEffectDispatcher::new(
        store.clone(),
        ports(recording.clone()),
        Arc::new(TestClock::new([30, 40])),
    );
    let mut request = dispatch_request("claim:provider-timeout", false);
    request.lease_duration_ms = 2_000;
    request.provider_timeout_ms = 5;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        dispatcher.dispatch_next(request),
    )
    .await
    .expect("the dispatcher must bound a hung provider")
    .unwrap();
    assert!(matches!(
        result,
        ControlEffectDispatchResult::Observed {
            outcome: ControlEffectOutcome::Unknown,
            ..
        }
    ));
    let effect = &store.effects(reviewed.operation_id()).await.unwrap()[0];
    assert_eq!(effect.status, ControlEffectStatus::Unknown);
    assert_eq!(
        effect.error_code.as_deref(),
        Some("provider.deadline_exceeded")
    );
    assert_eq!(recording.calls(), vec!["knowledge-host"]);
}

#[tokio::test]
async fn dispatcher_rejects_a_timeout_that_cannot_leave_observation_budget() {
    let (_temporary, store) = initialized_store().await;
    let recording = Arc::new(RecordingPorts::new(std::iter::empty()));
    let dispatcher = ControlEffectDispatcher::new(
        store,
        ports(recording.clone()),
        Arc::new(TestClock::new(std::iter::empty())),
    );
    let mut request = dispatch_request("claim:invalid-timeout", false);
    request.provider_timeout_ms = request.lease_duration_ms - 999;

    assert_eq!(
        dispatcher.dispatch_next(request).await.unwrap_err().code,
        "use.control_store.dispatch_invalid"
    );
    assert!(recording.calls().is_empty());
}

#[tokio::test]
async fn dispatcher_routes_every_owner_through_its_typed_port() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = control_installation();
    let store = ControlStore::new(temporary.path().join("state"), installation.clone()).unwrap();
    let recording = Arc::new(RecordingPorts::new(std::iter::empty()));
    let dispatcher = ControlEffectDispatcher::new(
        store,
        ports(recording.clone()),
        Arc::new(TestClock::new(std::iter::empty())),
    );
    let effects = routed_effects(installation);

    for effect in effects {
        let claimed = ClaimedControlEffect {
            authority: routed_authority(&effect),
            intent: effect,
            attempt: 1,
            claim_token: "claim:routing".to_string(),
            lease_until_ms: 100,
        };
        assert!(matches!(
            dispatcher
                .apply_claimed("operation:routing", &claimed)
                .await
                .unwrap(),
            RoutedControlEffectOutcome::Applied(_)
        ));
    }
    assert_eq!(
        recording.calls(),
        vec![
            "capability-index",
            "invocation-leases",
            "runtime-provider",
            "flow-host",
            "knowledge-host",
            "skill-host",
            "ui-host",
        ]
    );
}

fn routed_authority(intent: &ControlEffectIntent) -> ControlEffectAuthority {
    let reviewed = operation("operation:routing-authority");
    let candidate = transition(control_installation(), &reviewed);
    let generation = ControlGeneration {
        operation_id: reviewed.operation_id().to_string(),
        snapshot_digest: candidate.snapshot.descriptor_digest().unwrap(),
        snapshot: candidate.snapshot,
        package_lifecycles: candidate.package_lifecycles,
        grants: candidate.grants,
        provider_selections: candidate.provider_selections,
        capability: candidate.capability,
        capability_status: ControlCapabilityStatus::Candidate,
        capability_published_at_ms: None,
        committed_at_ms: candidate.committed_at_ms,
    };
    if matches!(intent.owner, ControlEffectOwner::CapabilityIndex) {
        return ControlEffectAuthority::CapabilityIndex(ControlCapabilityEffectAuthority {
            generation,
            materializations: Vec::new(),
        });
    }
    let package = generation.snapshot.packages[0].clone();
    let package_authority = ControlPackageEffectAuthority {
        generation_operation_id: generation.operation_id,
        installation_generation: intent.installation_generation,
        snapshot_digest: generation.snapshot_digest,
        committed_at_ms: generation.committed_at_ms,
        host: generation.snapshot.host,
        package,
        lifecycle_generation: intent
            .subject
            .package_identity()
            .map(|(_, generation)| generation)
            .unwrap(),
        grant: None,
    };
    match &intent.owner {
        ControlEffectOwner::CapabilityIndex => unreachable!(),
        ControlEffectOwner::InvocationLeases => {
            ControlEffectAuthority::InvocationLeases(package_authority)
        }
        ControlEffectOwner::RuntimeProvider {
            provider_id,
            selection_digest,
        } => ControlEffectAuthority::RuntimeProvider(ControlRuntimeEffectAuthority {
            package: package_authority,
            provider_selection: ControlProviderSelection {
                evidence: PlannedProviderEvidence {
                    surface: PlanQualifiedSurfaceRef {
                        package_id: "acme/package".to_string(),
                        surface: intent.subject.surface().unwrap().clone(),
                    },
                    provider_id: provider_id.clone(),
                    provider_build_id: "runtime-build:test".to_string(),
                    capability_digest: digest('b'),
                    semantics_profile_digest: digest('c'),
                    enforcement: PlanEnforcementProfile::Sandbox,
                },
                selection_digest: selection_digest.clone(),
            },
        }),
        ControlEffectOwner::FlowHost => ControlEffectAuthority::FlowHost(package_authority),
        ControlEffectOwner::KnowledgeHost => {
            ControlEffectAuthority::KnowledgeHost(package_authority)
        }
        ControlEffectOwner::SkillHost => ControlEffectAuthority::SkillHost(package_authority),
        ControlEffectOwner::UiHost => ControlEffectAuthority::UiHost(package_authority),
    }
}

#[test]
fn owner_application_types_reject_action_incompatible_evidence() {
    let request = surface_request(ControlSurfaceEffectAction::Stop, PluginSurfaceKind::Tool);
    let runtime_intent = routed_effects(control_installation())
        .into_iter()
        .find(|intent| matches!(intent.owner, ControlEffectOwner::RuntimeProvider { .. }))
        .unwrap();
    let ControlEffectAuthority::RuntimeProvider(authority) = routed_authority(&runtime_intent)
    else {
        panic!("the Runtime route must carry Runtime authority");
    };
    let runtime = ControlRuntimeEffectRequest {
        surface: request.clone(),
        authority,
        provider_id: "runtime:test".to_string(),
        selection_digest: digest('d'),
    };
    assert!(ControlRuntimeApplication::new(
        &runtime,
        digest('1'),
        Some(ControlRuntimeBindingObservation::Task),
    )
    .is_err());
    assert!(ControlSurfaceApplication::new(&request, digest('2'), Some(digest('3'))).is_err());
}

#[test]
fn system_dispatch_clock_produces_a_positive_bounded_timestamp() {
    assert!(SystemControlEffectClock.now_ms().unwrap() > 0);
}

fn routed_effects(installation: InstallationId) -> Vec<ControlEffectIntent> {
    let mut effects = Vec::new();
    effects.push(
        ControlEffectIntent::new(
            0,
            installation.clone(),
            digest('a'),
            PluginOperationAction::Install,
            1,
            ControlEffectSubject::Installation {
                expected_capability_generation: 0,
                capability_generation: 1,
                descriptor_digest: digest('b'),
            },
            ControlEffectOwner::CapabilityIndex,
            ControlEffectKind::CapabilityCutover,
            true,
        )
        .unwrap(),
    );
    effects.push(
        ControlEffectIntent::new(
            1,
            installation.clone(),
            digest('a'),
            PluginOperationAction::Uninstall,
            1,
            ControlEffectSubject::Package {
                package_id: "acme/package".to_string(),
                lifecycle_generation: 1,
                package_digest: digest('b'),
                manifest_digest: digest('c'),
                action: PluginLifecycleAction::Uninstall,
            },
            ControlEffectOwner::InvocationLeases,
            ControlEffectKind::CallsDrain,
            true,
        )
        .unwrap(),
    );
    for (kind, owner, surface_id) in [
        (
            PluginSurfaceKind::Tool,
            ControlEffectOwner::RuntimeProvider {
                provider_id: "runtime:test".to_string(),
                selection_digest: digest('d'),
            },
            "tool",
        ),
        (
            PluginSurfaceKind::Flow,
            ControlEffectOwner::FlowHost,
            "flow",
        ),
        (
            PluginSurfaceKind::Okf,
            ControlEffectOwner::KnowledgeHost,
            "knowledge",
        ),
        (
            PluginSurfaceKind::Skill,
            ControlEffectOwner::SkillHost,
            "skill",
        ),
        (PluginSurfaceKind::Ui, ControlEffectOwner::UiHost, "ui"),
    ] {
        effects.push(
            ControlEffectIntent::new(
                u32::try_from(effects.len()).unwrap(),
                installation.clone(),
                digest('a'),
                PluginOperationAction::Install,
                1,
                ControlEffectSubject::Surface {
                    package_id: "acme/package".to_string(),
                    lifecycle_generation: 1,
                    package_digest: digest('b'),
                    manifest_digest: digest('c'),
                    action: PluginLifecycleAction::Install,
                    surface: PluginSurfaceRef {
                        kind,
                        id: surface_id.to_string(),
                    },
                },
                owner,
                ControlEffectKind::SurfacePrepare,
                true,
            )
            .unwrap(),
        );
    }
    effects
}

fn surface_request(
    action: ControlSurfaceEffectAction,
    kind: PluginSurfaceKind,
) -> ControlSurfaceEffectRequest {
    use super::effect_port::ControlEffectRequestIdentity;

    let invocation = routed_effects(control_installation())
        .into_iter()
        .find(|intent| matches!(intent.owner, ControlEffectOwner::InvocationLeases))
        .unwrap();
    let ControlEffectAuthority::InvocationLeases(authority) = routed_authority(&invocation) else {
        panic!("the invocation route must carry package authority");
    };
    ControlSurfaceEffectRequest {
        identity: ControlEffectRequestIdentity {
            operation_id: "operation:application-shape".to_string(),
            installation: control_installation(),
            plan_digest: digest('a'),
            operation_action: PluginOperationAction::Disable,
            installation_generation: 1,
            sequence: 0,
            idempotency_key: digest('b'),
            required: true,
            attempt: 1,
            deadline_at_ms: 100,
        },
        authority,
        package_id: "acme/package".to_string(),
        lifecycle_generation: 1,
        package_digest: digest('c'),
        manifest_digest: digest('d'),
        lifecycle_action: PluginLifecycleAction::Disable,
        surface: PluginSurfaceRef {
            kind,
            id: "surface".to_string(),
        },
        action,
    }
}
