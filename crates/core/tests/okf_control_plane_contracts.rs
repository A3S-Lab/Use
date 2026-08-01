use a3s_use_core::{
    OkfBundleContract, OkfCapabilityProjection, OkfKnowledgeObservation, OkfKnowledgeObservedState,
    OkfProjectionReceipt, OkfSelectedGeneration, PlanQualifiedSurfaceRef, PluginSurfaceKind,
    PluginSurfaceRef, OKF_CAPABILITY_PROJECTION_SCHEMA, OKF_KNOWLEDGE_OBSERVATION_SCHEMA,
    OKF_PROJECTION_RECEIPT_SCHEMA,
};

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const DIGEST_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const PROJECTION_RECEIPT: &[u8] = include_bytes!("../fixtures/okf/projection-receipt-v1.json");
const PROJECTION_RECEIPT_DIGEST: &str =
    include_str!("../fixtures/okf/projection-receipt-v1.sha256").trim_ascii_end();
const KNOWLEDGE_OBSERVATION: &[u8] =
    include_bytes!("../fixtures/okf/knowledge-observation-v1.json");
const KNOWLEDGE_OBSERVATION_DIGEST: &str =
    include_str!("../fixtures/okf/knowledge-observation-v1.sha256").trim_ascii_end();
const CAPABILITY_PROJECTION: &[u8] =
    include_bytes!("../fixtures/okf/capability-projection-v1.json");
const CAPABILITY_PROJECTION_DIGEST: &str =
    include_str!("../fixtures/okf/capability-projection-v1.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn bundle() -> OkfBundleContract {
    OkfBundleContract::from_json(include_bytes!("../fixtures/okf/bundle-contract-v1.json")).unwrap()
}

fn surface() -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_owned(),
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
        },
    }
}

fn receipt() -> OkfProjectionReceipt {
    OkfProjectionReceipt {
        schema: OKF_PROJECTION_RECEIPT_SCHEMA.to_owned(),
        operation_id: "install:acme-research:0002".to_owned(),
        scope_id: "workspace:research".to_owned(),
        surface: surface(),
        generation: 13,
        package_digest: DIGEST_A.to_owned(),
        manifest_digest: DIGEST_B.to_owned(),
        bundle: bundle(),
        projection_id: "knowledge:workspace:research:domain-knowledge:13".to_owned(),
        index_schema: "a3s.knowledge.okf-index.v1".to_owned(),
        index_build_id: "knowledge:0.4.0:linux-x86_64".to_owned(),
        staged_at_ms: 1_785_360_100_000,
    }
}

fn selected(receipt: &OkfProjectionReceipt) -> OkfSelectedGeneration {
    OkfSelectedGeneration {
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest().unwrap(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        index_digest: DIGEST_C.to_owned(),
    }
}

fn promoted(receipt: &OkfProjectionReceipt) -> OkfKnowledgeObservation {
    OkfKnowledgeObservation {
        schema: OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_owned(),
        scope_id: receipt.scope_id.clone(),
        surface: receipt.surface.clone(),
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest().unwrap(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        state: OkfKnowledgeObservedState::Promoted,
        observed_at_ms: receipt.staged_at_ms + 1,
        index_digest: Some(DIGEST_C.to_owned()),
        selected: Some(selected(receipt)),
    }
}

#[test]
fn projection_receipt_binds_one_exact_non_executable_package_generation() {
    let receipt = receipt();
    receipt.validate().unwrap();

    let decoded = OkfProjectionReceipt::from_json(&receipt.canonical_bytes().unwrap()).unwrap();
    assert_eq!(decoded, receipt);
    assert!(receipt.descriptor_digest().unwrap().starts_with("sha256:"));

    let mut runtime = receipt.clone();
    runtime.surface.surface.kind = PluginSurfaceKind::Tool;
    assert!(runtime.validate().is_err());
}

#[test]
fn knowledge_observation_selects_only_the_exact_promoted_generation() {
    let receipt = receipt();
    let observation = promoted(&receipt);
    observation.validate_for_receipt(&receipt).unwrap();

    let decoded =
        OkfKnowledgeObservation::from_json(&observation.canonical_bytes().unwrap()).unwrap();
    assert_eq!(decoded, observation);
    assert!(observation
        .descriptor_digest()
        .unwrap()
        .starts_with("sha256:"));

    let mut substituted = observation.clone();
    substituted.bundle_digest = DIGEST_D.to_owned();
    assert!(substituted.validate_for_receipt(&receipt).is_err());

    let mut staged = observation;
    staged.state = OkfKnowledgeObservedState::Staged;
    assert!(staged.validate().is_err());
}

#[test]
fn failed_candidate_can_preserve_only_a_distinct_last_good_generation() {
    let receipt = receipt();
    let previous = OkfSelectedGeneration {
        generation: 12,
        package_digest: DIGEST_D.to_owned(),
        bundle_digest: DIGEST_B.to_owned(),
        projection_receipt_digest: DIGEST_A.to_owned(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: "knowledge:0.3.9:linux-x86_64".to_owned(),
        index_digest: DIGEST_C.to_owned(),
    };
    let failed = OkfKnowledgeObservation {
        schema: OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_owned(),
        scope_id: receipt.scope_id.clone(),
        surface: receipt.surface.clone(),
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest().unwrap(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        state: OkfKnowledgeObservedState::Failed,
        observed_at_ms: receipt.staged_at_ms + 1,
        index_digest: None,
        selected: Some(previous),
    };

    failed.validate_for_receipt(&receipt).unwrap();
    let mut unsafe_selection = failed;
    unsafe_selection.selected = Some(selected(&receipt));
    assert!(unsafe_selection.validate().is_err());
}

#[test]
fn capability_projection_requires_matching_promoted_evidence() {
    let receipt = receipt();
    let observation = promoted(&receipt);
    let projection = OkfCapabilityProjection::from_promoted(&receipt, &observation).unwrap();

    assert_eq!(projection.schema, OKF_CAPABILITY_PROJECTION_SCHEMA);
    assert_eq!(projection.generation, receipt.generation);
    assert_eq!(projection.bundle, receipt.bundle);
    projection.validate().unwrap();
    assert_eq!(
        OkfCapabilityProjection::from_json(&projection.canonical_bytes().unwrap()).unwrap(),
        projection
    );

    let mut failed = observation;
    failed.state = OkfKnowledgeObservedState::Failed;
    failed.index_digest = None;
    failed.selected = None;
    assert!(OkfCapabilityProjection::from_promoted(&receipt, &failed).is_err());
}

#[test]
fn okf_control_plane_fixtures_are_canonical_and_frozen() {
    let parsed_receipt = OkfProjectionReceipt::from_json(PROJECTION_RECEIPT).unwrap();
    assert_eq!(parsed_receipt, receipt());
    assert_eq!(
        parsed_receipt.canonical_bytes().unwrap(),
        canonical_fixture(PROJECTION_RECEIPT)
    );
    assert_eq!(
        parsed_receipt.descriptor_digest().unwrap(),
        PROJECTION_RECEIPT_DIGEST
    );

    let observation = OkfKnowledgeObservation::from_json(KNOWLEDGE_OBSERVATION).unwrap();
    assert_eq!(observation, promoted(&parsed_receipt));
    assert_eq!(
        observation.canonical_bytes().unwrap(),
        canonical_fixture(KNOWLEDGE_OBSERVATION)
    );
    assert_eq!(
        observation.descriptor_digest().unwrap(),
        KNOWLEDGE_OBSERVATION_DIGEST
    );

    let projection = OkfCapabilityProjection::from_json(CAPABILITY_PROJECTION).unwrap();
    assert_eq!(
        projection,
        OkfCapabilityProjection::from_promoted(&parsed_receipt, &observation).unwrap()
    );
    assert_eq!(
        projection.canonical_bytes().unwrap(),
        canonical_fixture(CAPABILITY_PROJECTION)
    );
    assert_eq!(
        projection.descriptor_digest().unwrap(),
        CAPABILITY_PROJECTION_DIGEST
    );
}

#[test]
fn okf_surface_kind_has_a_canonical_policy_value() {
    assert_eq!(
        serde_json::to_string(&PluginSurfaceKind::Okf).unwrap(),
        "\"okf\""
    );
    assert_eq!(
        serde_json::from_str::<PluginSurfaceKind>("\"okf\"").unwrap(),
        PluginSurfaceKind::Okf
    );
}
