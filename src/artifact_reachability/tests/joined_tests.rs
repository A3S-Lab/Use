use a3s_use_core::{InstallationId, InstallationKind};
use a3s_use_extension::{
    ArtifactInventoryEntry, ArtifactKind, ArtifactPhysicalState, ArtifactStoreInventory,
    ARTIFACT_STORE_INVENTORY_SCHEMA,
};

use super::*;

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn joined_inventory_preserves_orthogonal_logical_and_physical_evidence() {
    let installation = InstallationId::new(InstallationKind::User, "joined-inventory").unwrap();
    let expanded = digest('a');
    let missing_blob = digest('b');
    let unreferenced_blob = digest('c');
    let mismatched = digest('d');
    let references = ArtifactReferenceInventory {
        schema: ARTIFACT_REFERENCE_INVENTORY_SCHEMA.to_owned(),
        entries: vec![
            reference(
                ArtifactKind::ExpandedPackage,
                &expanded,
                ArtifactReferenceSource::InstallationSnapshot,
                Some(installation.clone()),
                Some(10),
                Some(2),
                2,
            ),
            reference(
                ArtifactKind::ExpandedPackage,
                &expanded,
                ArtifactReferenceSource::CurrentReceipt,
                Some(installation.clone()),
                Some(10),
                Some(2),
                1,
            ),
            reference(
                ArtifactKind::Blob,
                &missing_blob,
                ArtifactReferenceSource::RegistryObservation,
                None,
                Some(5),
                None,
                1,
            ),
            reference(
                ArtifactKind::ExpandedPackage,
                &mismatched,
                ArtifactReferenceSource::InstallationSnapshot,
                Some(installation),
                Some(9),
                Some(1),
                1,
            ),
        ],
    };
    let physical = ArtifactStoreInventory {
        schema: ARTIFACT_STORE_INVENTORY_SCHEMA.to_owned(),
        entries: vec![
            physical(
                ArtifactKind::ExpandedPackage,
                &expanded,
                ArtifactPhysicalState::Complete,
                10,
                2,
                1,
                3,
            ),
            physical(
                ArtifactKind::Blob,
                &unreferenced_blob,
                ArtifactPhysicalState::Incomplete,
                0,
                0,
                1,
                4,
            ),
            physical(
                ArtifactKind::ExpandedPackage,
                &mismatched,
                ArtifactPhysicalState::Complete,
                10,
                1,
                0,
                0,
            ),
        ],
    };

    let inventory = crate::artifact_reachability::joined::join(references, physical).unwrap();

    assert_eq!(inventory.schema, ARTIFACT_REACHABILITY_INVENTORY_SCHEMA);
    assert_eq!(inventory.artifacts.len(), 4);
    let expanded_entry = artifact(&inventory, ArtifactKind::ExpandedPackage, &expanded);
    assert_eq!(expanded_entry.references.len(), 2);
    assert_eq!(
        expanded_entry.references[0].source,
        ArtifactReferenceSource::InstallationSnapshot
    );
    assert_eq!(expanded_entry.references[0].reference_count, 2);
    assert_eq!(
        expanded_entry.references[1].source,
        ArtifactReferenceSource::CurrentReceipt
    );
    assert_eq!(expanded_entry.references[1].reference_count, 1);
    assert_eq!(
        expanded_entry.measurement_status,
        ArtifactMeasurementStatus::Matches
    );
    assert_eq!(
        artifact(&inventory, ArtifactKind::Blob, &missing_blob).measurement_status,
        ArtifactMeasurementStatus::Unavailable
    );
    assert_eq!(
        artifact(&inventory, ArtifactKind::Blob, &unreferenced_blob).measurement_status,
        ArtifactMeasurementStatus::Unspecified
    );
    assert_eq!(
        artifact(&inventory, ArtifactKind::ExpandedPackage, &mismatched).measurement_status,
        ArtifactMeasurementStatus::Mismatch
    );
    assert_eq!(
        inventory.usage,
        ArtifactStorageUsage {
            artifact_keys: 4,
            referenced_artifacts: 3,
            physical_artifacts: 3,
            unreferenced_artifacts: 1,
            missing_referenced_artifacts: 1,
            incomplete_physical_artifacts: 1,
            measurement_mismatches: 1,
            content_bytes: 20,
            content_files: 3,
            staging_entries: 2,
            staging_bytes: 7,
            physical_bytes: 27,
            referenced_content_bytes: 20,
            unreferenced_content_bytes: 0,
        }
    );
    assert_eq!(
        inventory.usage.physical_bytes,
        inventory.usage.referenced_content_bytes
            + inventory.usage.unreferenced_content_bytes
            + inventory.usage.staging_bytes
    );
    let json = serde_json::to_string(&inventory).unwrap();
    assert!(!json.contains("packageRoot"));
    assert!(!json.contains("contentPath"));
}

#[test]
fn quota_assessment_is_bounded_and_never_claims_deletion_authority() {
    assert_send_sync::<ArtifactReachabilityEntry>();
    assert_send_sync::<ArtifactReachabilityInventory>();
    assert_send_sync::<ArtifactStorageUsage>();
    assert_send_sync::<ArtifactStorageQuotaPolicy>();
    assert_send_sync::<ArtifactStorageQuotaAssessment>();

    let physical = ArtifactStoreInventory {
        schema: ARTIFACT_STORE_INVENTORY_SCHEMA.to_owned(),
        entries: vec![physical(
            ArtifactKind::Blob,
            &digest('e'),
            ArtifactPhysicalState::Complete,
            30,
            1,
            0,
            0,
        )],
    };
    let inventory = crate::artifact_reachability::joined::join(
        ArtifactReferenceInventory {
            schema: ARTIFACT_REFERENCE_INVENTORY_SCHEMA.to_owned(),
            entries: Vec::new(),
        },
        physical,
    )
    .unwrap();
    let policy = ArtifactStorageQuotaPolicy::new(20, 1).unwrap();
    let assessment = inventory.assess_quota(policy).unwrap();

    assert!(!assessment.within_quota);
    assert_eq!(assessment.excess_bytes, 10);
    assert_eq!(assessment.excess_artifacts, 0);
    assert_eq!(assessment.usage.unreferenced_content_bytes, 30);
    let value = serde_json::to_value(assessment).unwrap();
    assert!(value.get("deletionAuthorized").is_none());
    assert_eq!(
        ArtifactStorageQuotaPolicy::new(0, 1).unwrap_err().code,
        "use.artifact_store.quota_policy_invalid"
    );
    assert_eq!(
        ArtifactStorageQuotaPolicy::new(1, MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS + 1)
            .unwrap_err()
            .code,
        "use.artifact_store.quota_policy_invalid"
    );
}

#[test]
fn quota_assessment_rejects_a_tampered_usage_projection() {
    let mut inventory = crate::artifact_reachability::joined::join(
        ArtifactReferenceInventory {
            schema: ARTIFACT_REFERENCE_INVENTORY_SCHEMA.to_owned(),
            entries: Vec::new(),
        },
        ArtifactStoreInventory {
            schema: ARTIFACT_STORE_INVENTORY_SCHEMA.to_owned(),
            entries: Vec::new(),
        },
    )
    .unwrap();
    inventory.usage.physical_bytes = 1;

    let error = inventory
        .assess_quota(ArtifactStorageQuotaPolicy::new(1, 1).unwrap())
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_reachability.join_invalid");
}

#[allow(clippy::too_many_arguments)]
fn reference(
    kind: ArtifactKind,
    digest: &str,
    source: ArtifactReferenceSource,
    installation: Option<InstallationId>,
    expected_bytes: Option<u64>,
    expected_files: Option<u64>,
    reference_count: u64,
) -> ArtifactReferenceEntry {
    ArtifactReferenceEntry {
        kind,
        digest: digest.to_owned(),
        source,
        installation,
        expected_bytes,
        expected_files,
        reference_count,
    }
}

#[allow(clippy::too_many_arguments)]
fn physical(
    kind: ArtifactKind,
    digest: &str,
    state: ArtifactPhysicalState,
    content_bytes: u64,
    content_files: u64,
    staging_entries: u64,
    staging_bytes: u64,
) -> ArtifactInventoryEntry {
    ArtifactInventoryEntry {
        kind,
        digest: digest.to_owned(),
        state,
        content_bytes,
        content_files,
        staging_entries,
        staging_bytes,
    }
}

fn artifact<'a>(
    inventory: &'a ArtifactReachabilityInventory,
    kind: ArtifactKind,
    digest: &str,
) -> &'a ArtifactReachabilityEntry {
    inventory
        .artifacts
        .iter()
        .find(|entry| entry.kind == kind && entry.digest == digest)
        .unwrap()
}
