use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    inspect_okf_bundle_files, InstallationId, OkfBundleContract, OkfBundleFile, OkfBundleLimits,
    OkfCapabilityProjection, OkfFormatVersion, PlanQualifiedSurfaceRef, PluginOperationAction,
    PluginSurfaceKind, PluginSurfaceRef, OKF_BUNDLE_CONTRACT_SCHEMA,
};
use a3s_use_extension::{load_okf_bundle_files, ExtensionManifest, ExtensionPaths};
use tempfile::TempDir;

use super::super::aggregate_tests::fixtures::{
    apply_all_effects, claim, control_installation as fixture_installation,
    observation as fixture_observation, operation, operation_at, projected_transition, transition,
};
use super::super::model::{
    ControlAppliedEffect, ControlAppliedEffectEvidence, ControlEffectKind,
    ControlEffectObservation, ControlEffectOutcome, ControlEffectOwner, ControlEffectSubject,
    ControlProjectionHistory, ControlSurfaceObservationState, ReviewedControlOperation,
};
use super::super::payload_owner::*;
use super::super::ControlStore;
use crate::okf_knowledge::{
    OkfKnowledgeBinding, OkfKnowledgeClient, OkfKnowledgeStageRequest, OkfKnowledgeStageSpec,
    SqliteOkfKnowledgeAdapter,
};

pub(super) fn control_installation() -> InstallationId {
    fixture_installation()
}

pub(super) async fn seed_control_knowledge(
    store: &ControlStore,
    paths: &ExtensionPaths,
) -> OkfKnowledgeBinding {
    seed_control_knowledge_with_evidence(store, paths, true).await
}

pub(super) async fn seed_control_knowledge_with_evidence(
    store: &ControlStore,
    paths: &ExtensionPaths,
    exact_evidence: bool,
) -> OkfKnowledgeBinding {
    let reviewed = operation("knowledge-snapshot-operation");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(fixture_installation(), &reviewed))
        .await
        .unwrap();

    let claimed_at_ms = wall_clock_ms();
    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:knowledge-snapshot",
            claimed_at_ms,
            claimed_at_ms + 300_000,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.intent.owner, ControlEffectOwner::KnowledgeHost);
    let ControlEffectSubject::Surface {
        package_id,
        lifecycle_generation,
        package_digest,
        manifest_digest,
        surface,
        ..
    } = &claimed.intent.subject
    else {
        panic!("the Knowledge preparation fixture must target one surface");
    };

    let package_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let manifest = ExtensionManifest::parse_acl(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/extension/fixtures/packages/plugin-v3-okf/package/a3s-use-extension.acl"
    )))
    .unwrap();
    let okf = manifest.okf.first().unwrap();
    assert_eq!(okf.id, surface.id);
    let files = load_okf_bundle_files(okf, &package_root).await.unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(paths));
    let client = OkfKnowledgeClient::new(adapter);
    let staged = client
        .stage(
            OkfKnowledgeStageRequest::new(
                OkfKnowledgeStageSpec {
                    operation_id: reviewed.operation_id().to_string(),
                    scope: fixture_installation(),
                    surface: PlanQualifiedSurfaceRef {
                        package_id: package_id.clone(),
                        surface: surface.clone(),
                    },
                    generation: *lifecycle_generation,
                    package_digest: package_digest.clone(),
                    manifest_digest: manifest_digest.clone(),
                    bundle: okf.bundle.clone(),
                },
                files,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let promoted = client.promote(&staged.receipt).await.unwrap();
    let projection =
        OkfCapabilityProjection::from_promoted(&promoted.receipt, &promoted.observation).unwrap();
    let application = ControlAppliedEffect::new(
        &claimed.intent,
        ControlAppliedEffectEvidence::KnowledgeHost {
            state: ControlSurfaceObservationState::Prepared,
            receipt_digest: if exact_evidence {
                promoted.observation.descriptor_digest().unwrap()
            } else {
                digest('e')
            },
            projection_digest: Some(if exact_evidence {
                projection.descriptor_digest().unwrap()
            } else {
                digest('f')
            }),
        },
    )
    .unwrap();
    store
        .record_effect_observation(ControlEffectObservation {
            operation_id: reviewed.operation_id().to_string(),
            idempotency_key: claimed.intent.idempotency_key.clone(),
            claim_token: claimed.claim_token,
            outcome: ControlEffectOutcome::Applied,
            application: Some(application),
            failure_evidence_digest: None,
            error_code: None,
            observed_at_ms: wall_clock_ms(),
        })
        .await
        .unwrap();
    apply_all_effects(store, &reviewed, wall_clock_ms() + 1).await;
    promoted
}

pub(super) async fn remove_control_knowledge(
    store: &ControlStore,
    paths: &ExtensionPaths,
    promoted: &OkfKnowledgeBinding,
) -> OkfKnowledgeBinding {
    let installed = store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&installed).unwrap();
    let reviewed = operation_at(
        "knowledge-snapshot-uninstall",
        PluginOperationAction::Uninstall,
        installed.snapshot.generation,
        installed.capability.generation,
    );
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(projected_transition(&reviewed, &installed, &history))
        .await
        .unwrap();

    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(paths));
    let client = OkfKnowledgeClient::new(adapter);
    let mut removed = None;
    let mut claim_sequence = 0_u32;
    let mut effect_clock_ms = wall_clock_ms() + 1_000;
    loop {
        let token = format!("claim:knowledge-remove:{claim_sequence}");
        let Some(claimed) = store
            .claim_next_effect(claim(
                reviewed.operation_id(),
                &token,
                effect_clock_ms,
                effect_clock_ms + 300_000,
                false,
            ))
            .await
            .unwrap()
        else {
            break;
        };
        let observation = if claimed.intent.owner == ControlEffectOwner::KnowledgeHost {
            assert_eq!(claimed.intent.kind, ControlEffectKind::SurfaceRemove);
            let binding = client.remove(&promoted.receipt).await.unwrap();
            let application = ControlAppliedEffect::new(
                &claimed.intent,
                ControlAppliedEffectEvidence::KnowledgeHost {
                    state: ControlSurfaceObservationState::Removed,
                    receipt_digest: binding.observation.descriptor_digest().unwrap(),
                    projection_digest: None,
                },
            )
            .unwrap();
            removed = Some(binding);
            ControlEffectObservation {
                operation_id: reviewed.operation_id().to_string(),
                idempotency_key: claimed.intent.idempotency_key.clone(),
                claim_token: claimed.claim_token,
                outcome: ControlEffectOutcome::Applied,
                application: Some(application),
                failure_evidence_digest: None,
                error_code: None,
                observed_at_ms: effect_clock_ms + 1,
            }
        } else {
            fixture_observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(claimed.intent.sequence % 16, 16).unwrap(),
                effect_clock_ms + 1,
            )
        };
        store.record_effect_observation(observation).await.unwrap();
        claim_sequence += 1;
        effect_clock_ms += 10;
    }
    store
        .complete_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('d'),
            effect_clock_ms,
        )
        .await
        .unwrap();
    removed.expect("the uninstall fixture must remove its Knowledge surface")
}

pub(super) async fn disable_and_reenable_control_knowledge(
    store: &ControlStore,
    promoted: &OkfKnowledgeBinding,
) {
    let installed = store.current_generation().await.unwrap().unwrap();
    let mut history = ControlProjectionHistory::default();
    history.observe(&installed).unwrap();
    let disable = operation_at(
        "knowledge-snapshot-disable",
        PluginOperationAction::Disable,
        installed.snapshot.generation,
        installed.capability.generation,
    );
    store.register_operation(disable.clone()).await.unwrap();
    let disabled = store
        .commit_transition(projected_transition(&disable, &installed, &history))
        .await
        .unwrap();
    history.observe(&disabled).unwrap();
    apply_all_effects(store, &disable, wall_clock_ms() + 1_000).await;

    let enable = operation_at(
        "knowledge-snapshot-enable",
        PluginOperationAction::Enable,
        disabled.snapshot.generation,
        disabled.capability.generation,
    );
    store.register_operation(enable.clone()).await.unwrap();
    store
        .commit_transition(projected_transition(&enable, &disabled, &history))
        .await
        .unwrap();
    apply_operation_with_retained_knowledge(store, &enable, promoted).await;
}

async fn apply_operation_with_retained_knowledge(
    store: &ControlStore,
    reviewed: &ReviewedControlOperation,
    promoted: &OkfKnowledgeBinding,
) {
    let projection =
        OkfCapabilityProjection::from_promoted(&promoted.receipt, &promoted.observation).unwrap();
    let mut claim_sequence = 0_u32;
    let mut effect_clock_ms = wall_clock_ms() + 1_000;
    loop {
        let token = format!("claim:knowledge-reenable:{claim_sequence}");
        let Some(claimed) = store
            .claim_next_effect(claim(
                reviewed.operation_id(),
                &token,
                effect_clock_ms,
                effect_clock_ms + 300_000,
                false,
            ))
            .await
            .unwrap()
        else {
            break;
        };
        let observation = if claimed.intent.owner == ControlEffectOwner::KnowledgeHost {
            assert_eq!(claimed.intent.kind, ControlEffectKind::SurfacePrepare);
            let application = ControlAppliedEffect::new(
                &claimed.intent,
                ControlAppliedEffectEvidence::KnowledgeHost {
                    state: ControlSurfaceObservationState::Prepared,
                    receipt_digest: promoted.observation.descriptor_digest().unwrap(),
                    projection_digest: Some(projection.descriptor_digest().unwrap()),
                },
            )
            .unwrap();
            ControlEffectObservation {
                operation_id: reviewed.operation_id().to_string(),
                idempotency_key: claimed.intent.idempotency_key.clone(),
                claim_token: claimed.claim_token,
                outcome: ControlEffectOutcome::Applied,
                application: Some(application),
                failure_evidence_digest: None,
                error_code: None,
                observed_at_ms: effect_clock_ms + 1,
            }
        } else {
            fixture_observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(claimed.intent.sequence % 16, 16).unwrap(),
                effect_clock_ms + 1,
            )
        };
        store.record_effect_observation(observation).await.unwrap();
        claim_sequence += 1;
        effect_clock_ms += 10;
    }
    store
        .complete_operation(
            reviewed.operation_id(),
            reviewed.plan_digest(),
            &digest('c'),
            effect_clock_ms,
        )
        .await
        .unwrap();
}

pub(super) async fn remove_knowledge_without_control(
    paths: &ExtensionPaths,
    promoted: &OkfKnowledgeBinding,
) -> OkfKnowledgeBinding {
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(paths));
    OkfKnowledgeClient::new(adapter)
        .remove(&promoted.receipt)
        .await
        .unwrap()
}

pub(super) async fn seed_knowledge(paths: &ExtensionPaths, installation: InstallationId) {
    let files = vec![OkfBundleFile::new(
        "concept.md",
        b"---\ntype: Metric\n---\n\n# Throughput\n",
    )];
    let limits = OkfBundleLimits::default();
    let inspection =
        inspect_okf_bundle_files(OkfFormatVersion::V0_2, limits.clone(), &files).unwrap();
    let bundle = OkfBundleContract {
        schema: OKF_BUNDLE_CONTRACT_SCHEMA.to_string(),
        format_version: inspection.format_version,
        root: "knowledge".to_string(),
        content_digest: inspection.content_digest,
        concept_count: inspection.concept_count,
        file_count: inspection.file_count,
        expanded_bytes: inspection.expanded_bytes,
        limits,
    };
    let spec = OkfKnowledgeStageSpec {
        operation_id: "knowledge-snapshot-operation".to_string(),
        scope: installation,
        surface: PlanQualifiedSurfaceRef {
            package_id: "acme/research".to_string(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_string(),
            },
        },
        generation: 1,
        package_digest: digest('a'),
        manifest_digest: digest('b'),
        bundle,
    };
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(paths));
    let client = OkfKnowledgeClient::new(adapter);
    let staged = client
        .stage(OkfKnowledgeStageRequest::new(spec, files).unwrap())
        .await
        .unwrap();
    client.promote(&staged.receipt).await.unwrap();
}

pub(super) fn paths(temporary: &TempDir, installation: InstallationId) -> ExtensionPaths {
    ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation,
    )
    .unwrap()
}

pub(super) fn registry() -> ControlPayloadOwnerRegistry {
    registry_with_payload_limit(16 * 1024 * 1024)
}

pub(super) fn registry_with_payload_limit(max_payload_bytes: u64) -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(|owner| {
                if owner == ControlPayloadOwnerId::ArtifactStore {
                    ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
                } else {
                    let schema = if owner == ControlPayloadOwnerId::KnowledgePayload {
                        CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA.to_string()
                    } else {
                        format!("a3s.use.test.{}-snapshot.v1", owner.as_str())
                    };
                    ControlPayloadOwnerRegistration::snapshotted(
                        owner,
                        schema,
                        ControlPayloadOwnerLimits::new(16, max_payload_bytes, 256 * 1024).unwrap(),
                    )
                    .unwrap()
                }
            })
            .collect(),
    )
    .unwrap()
}

pub(super) fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

fn wall_clock_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}
