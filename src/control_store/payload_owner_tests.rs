use super::payload_owner::*;
use crate::control_store::model::valid_sha256;

#[test]
fn registry_requires_the_exact_typed_external_owner_set() {
    let registry = registry();
    registry.validate().unwrap();
    assert_eq!(registry.registrations().len(), 5);
    assert_eq!(
        registry
            .registrations()
            .iter()
            .map(ControlPayloadOwnerRegistration::owner)
            .collect::<Vec<_>>(),
        ControlPayloadOwnerId::ALL
    );
    assert!(valid_sha256(registry.descriptor_digest()));
    let replay = ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(registration)
            .collect(),
    )
    .unwrap();
    assert_eq!(replay, registry);
    let decoded: ControlPayloadOwnerRegistry =
        serde_json::from_slice(&serde_json::to_vec(&registry).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, registry);

    let mut missing = registrations();
    missing.pop();
    assert_eq!(
        ControlPayloadOwnerRegistry::new(missing).unwrap_err().code,
        "use.control_store.payload_registry_invalid"
    );

    let mut duplicate = registrations();
    duplicate.push(registration(ControlPayloadOwnerId::KnowledgePayload));
    assert_eq!(
        ControlPayloadOwnerRegistry::new(duplicate)
            .unwrap_err()
            .code,
        "use.control_store.payload_registry_invalid"
    );

    assert_eq!(
        ControlPayloadOwnerRegistration::excluded_global(ControlPayloadOwnerId::KnowledgePayload)
            .unwrap_err()
            .code,
        "use.control_store.payload_registry_invalid"
    );
    assert_eq!(
        ControlPayloadOwnerRegistration::snapshotted(
            ControlPayloadOwnerId::ArtifactStore,
            "a3s.use.test.artifact-store-snapshot.v1",
            limits(),
        )
        .unwrap_err()
        .code,
        "use.control_store.payload_registry_invalid"
    );
}

#[test]
fn snapshot_set_is_generation_bound_path_free_and_deterministic() {
    let registry = registry();
    let installation = installation();
    let binding = binding(&registry, &installation, 7);
    let mut receipts = ControlPayloadOwnerId::SNAPSHOTTED
        .into_iter()
        .rev()
        .enumerate()
        .map(|(index, owner)| {
            receipt(
                &registry,
                owner,
                &installation,
                7,
                u64::try_from(index + 1).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let snapshot =
        ControlPayloadSnapshotSet::new(&registry, binding.clone(), receipts.clone()).unwrap();
    snapshot.validate(&registry).unwrap();
    assert_eq!(snapshot.receipts.len(), 4);
    assert_eq!(
        snapshot.receipts[0].owner,
        ControlPayloadOwnerId::HostProtocolProjection
    );
    assert_eq!(snapshot.file_count, 10);
    assert_eq!(snapshot.byte_count, 10 * 1024);
    assert_eq!(snapshot.manifest_bytes, 10 * 128);
    let first_digest = snapshot.descriptor_digest(&registry).unwrap();
    assert!(valid_sha256(&first_digest));

    receipts.reverse();
    let replay = ControlPayloadSnapshotSet::new(&registry, binding, receipts).unwrap();
    assert_eq!(replay, snapshot);
    assert_eq!(replay.descriptor_digest(&registry).unwrap(), first_digest);

    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("absolutePath"));
    assert!(!json.contains("sourcePath"));
    assert!(!json.contains("stateRoot"));
    assert!(!json.contains("dataRoot"));
    let decoded: ControlPayloadSnapshotSet = serde_json::from_str(&json).unwrap();
    decoded.validate(&registry).unwrap();
    assert_eq!(decoded.descriptor_digest(&registry).unwrap(), first_digest);
}

#[test]
fn snapshot_set_rejects_missing_rebound_or_excluded_owner_receipts() {
    let registry = registry();
    let installation = installation();
    let binding = binding(&registry, &installation, 3);
    let receipts = ControlPayloadOwnerId::SNAPSHOTTED
        .into_iter()
        .map(|owner| receipt(&registry, owner, &installation, 3, 1))
        .collect::<Vec<_>>();

    assert_eq!(
        ControlPayloadSnapshotSet::new(&registry, binding.clone(), receipts[..3].to_vec())
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut rebound = receipts.clone();
    rebound[0].control_generation = 4;
    assert_eq!(
        ControlPayloadSnapshotSet::new(&registry, binding.clone(), rebound)
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut wrong_installation = receipts.clone();
    wrong_installation[0].installation =
        a3s_use_core::InstallationId::new(a3s_use_core::InstallationKind::User, "other-user")
            .unwrap();
    assert_eq!(
        ControlPayloadSnapshotSet::new(&registry, binding.clone(), wrong_installation)
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut excluded = receipts;
    excluded.push(ControlPayloadSnapshotReceipt {
        schema: CONTROL_PAYLOAD_SNAPSHOT_RECEIPT_SCHEMA.to_string(),
        owner: ControlPayloadOwnerId::ArtifactStore,
        installation: installation.clone(),
        control_generation: 3,
        owner_snapshot_schema: "a3s.use.test.artifact-store-snapshot.v1".to_string(),
        owner_manifest_digest: digest('a'),
        inventory_digest: digest('b'),
        manifest_bytes: 1,
        file_count: 1,
        byte_count: 1,
    });
    assert_eq!(
        ControlPayloadSnapshotSet::new(&registry, binding, excluded)
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );
}

#[test]
fn owner_receipts_enforce_registered_schema_and_bounds() {
    let registry = registry();
    let installation = installation();
    let binding = binding(&registry, &installation, 9);
    let mut schema_receipt = receipt(
        &registry,
        ControlPayloadOwnerId::KnowledgePayload,
        &installation,
        9,
        1,
    );
    schema_receipt.owner_snapshot_schema = "a3s.use.test.wrong.v1".to_string();
    assert_eq!(
        schema_receipt
            .validate(&registry, &binding)
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut oversized_receipt = receipt(
        &registry,
        ControlPayloadOwnerId::KnowledgePayload,
        &installation,
        9,
        1,
    );
    oversized_receipt.byte_count = limits().max_payload_bytes + 1;
    assert_eq!(
        oversized_receipt
            .validate(&registry, &binding)
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let zero = ControlPayloadSnapshotReceipt::new(
        &registry,
        &binding,
        ControlPayloadOwnerId::PlanningAndDiagnosticObservations,
        ControlPayloadSnapshotEvidence::new(digest('c'), digest('d'), 64, 0, 0),
    )
    .unwrap();
    zero.validate(&registry, &binding).unwrap();
}

#[test]
fn decoded_registry_and_snapshot_evidence_must_revalidate() {
    let registry = registry();
    let mut registry_json = serde_json::to_value(&registry).unwrap();
    registry_json["registrations"][1]["backupPolicy"] =
        serde_json::Value::String("owner-snapshot".to_string());
    let decoded: ControlPayloadOwnerRegistry = serde_json::from_value(registry_json).unwrap();
    assert_eq!(
        decoded.validate().unwrap_err().code,
        "use.control_store.payload_registry_invalid"
    );

    let installation = installation();
    let binding = binding(&registry, &installation, 5);
    let receipts = ControlPayloadOwnerId::SNAPSHOTTED
        .into_iter()
        .map(|owner| receipt(&registry, owner, &installation, 5, 1))
        .collect::<Vec<_>>();
    let snapshot = ControlPayloadSnapshotSet::new(&registry, binding, receipts).unwrap();
    let mut snapshot_json = serde_json::to_value(&snapshot).unwrap();
    snapshot_json["receipts"][0]["controlGeneration"] = serde_json::Value::from(6_u64);
    let decoded: ControlPayloadSnapshotSet = serde_json::from_value(snapshot_json).unwrap();
    assert_eq!(
        decoded.descriptor_digest(&registry).unwrap_err().code,
        "use.control_store.payload_snapshot_invalid"
    );
}

#[test]
fn payload_registry_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ControlPayloadOwnerRegistry>();
    assert_send_sync::<ControlPayloadOwnerRegistration>();
    assert_send_sync::<ControlPayloadSnapshotEvidence>();
    assert_send_sync::<ControlPayloadSnapshotBinding>();
    assert_send_sync::<ControlPayloadSnapshotReceipt>();
    assert_send_sync::<ControlPayloadSnapshotSet>();
}

fn registry() -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(registrations()).unwrap()
}

fn registrations() -> Vec<ControlPayloadOwnerRegistration> {
    ControlPayloadOwnerId::ALL
        .into_iter()
        .rev()
        .map(registration)
        .collect()
}

fn registration(owner: ControlPayloadOwnerId) -> ControlPayloadOwnerRegistration {
    if owner == ControlPayloadOwnerId::ArtifactStore {
        ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
    } else {
        ControlPayloadOwnerRegistration::snapshotted(
            owner,
            format!("a3s.use.test.{}-snapshot.v1", owner.as_str()),
            limits(),
        )
        .unwrap()
    }
}

fn limits() -> ControlPayloadOwnerLimits {
    ControlPayloadOwnerLimits::new(16, 16 * 1024, 4 * 1024).unwrap()
}

fn receipt(
    registry: &ControlPayloadOwnerRegistry,
    owner: ControlPayloadOwnerId,
    installation: &a3s_use_core::InstallationId,
    control_generation: u64,
    count: u64,
) -> ControlPayloadSnapshotReceipt {
    let binding = binding(registry, installation, control_generation);
    ControlPayloadSnapshotReceipt::new(
        registry,
        &binding,
        owner,
        ControlPayloadSnapshotEvidence::new(
            digest('a'),
            digest('b'),
            count * 128,
            count,
            count * 1024,
        ),
    )
    .unwrap()
}

fn binding(
    registry: &ControlPayloadOwnerRegistry,
    installation: &a3s_use_core::InstallationId,
    control_generation: u64,
) -> ControlPayloadSnapshotBinding {
    ControlPayloadSnapshotBinding::new(
        registry,
        installation.clone(),
        control_generation,
        digest('e'),
    )
    .unwrap()
}

fn installation() -> a3s_use_core::InstallationId {
    a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::Workspace,
        "workspace-payloads",
    )
    .unwrap()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
