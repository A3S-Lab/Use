use a3s_use_core::{InstallationId, InstallationKind};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};
use tempfile::TempDir;

use super::model::valid_sha256;
use super::payload_owner::*;
use super::ControlStore;

#[tokio::test]
async fn snapshot_session_binds_one_verified_export_under_the_exclusive_fence() {
    let temporary = TempDir::new().unwrap();
    let installation =
        InstallationId::new(InstallationKind::Workspace, "snapshot-session").unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation.clone(),
    )
    .unwrap();
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();

    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    assert_eq!(session.binding().installation, installation);
    assert_eq!(session.binding().control_generation, 0);
    assert_eq!(
        session.binding().owner_registry_digest,
        registry.descriptor_digest()
    );
    assert!(valid_sha256(&session.binding().control_export_digest));
    session.binding().validate(&registry).unwrap();

    let verified = store
        .verify_export(session.control_export().to_vec())
        .await
        .unwrap();
    assert_eq!(
        verified.descriptor_digest,
        session.binding().control_export_digest
    );
    assert!(StateMaintenanceLock::new(paths.installation_state_root())
        .try_acquire_shared()
        .await
        .unwrap()
        .is_none());
    let receipts = ControlPayloadOwnerId::SNAPSHOTTED
        .into_iter()
        .map(|owner| {
            session
                .receipt(
                    owner,
                    ControlPayloadSnapshotEvidence::new(digest('a'), digest('b'), 64, 1, 1),
                )
                .unwrap()
        })
        .collect();
    let completed = session.complete(receipts).unwrap();
    assert_eq!(completed.binding, *session.binding());

    drop(session);
    StateMaintenanceLock::new(paths.installation_state_root())
        .try_acquire_shared()
        .await
        .unwrap()
        .expect("dropping the session must release its exclusive maintenance fence");
}

#[tokio::test]
async fn snapshot_session_rejects_a_registry_or_export_rebound() {
    let temporary = TempDir::new().unwrap();
    let installation = InstallationId::new(InstallationKind::User, "snapshot-rebind").unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation,
    )
    .unwrap();
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();

    let mut rebound = session.binding().clone();
    rebound.control_export_digest = digest('f');
    assert_eq!(
        rebound.validate(&registry).unwrap_err().code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut other_registry = registrations();
    if let ControlPayloadOwnerRegistration::Snapshotted { limits, .. } = &mut other_registry[2] {
        *limits = ControlPayloadOwnerLimits::new(15, 16 * 1024, 4 * 1024).unwrap();
    }
    let other_registry = ControlPayloadOwnerRegistry::new(other_registry).unwrap();
    assert_eq!(
        session
            .binding()
            .validate(&other_registry)
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );
}

fn registry() -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(registrations()).unwrap()
}

fn registrations() -> Vec<ControlPayloadOwnerRegistration> {
    ControlPayloadOwnerId::ALL
        .into_iter()
        .map(|owner| {
            if owner == ControlPayloadOwnerId::ArtifactStore {
                ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
            } else {
                ControlPayloadOwnerRegistration::snapshotted(
                    owner,
                    format!("a3s.use.test.{}-snapshot.v1", owner.as_str()),
                    ControlPayloadOwnerLimits::new(16, 16 * 1024, 4 * 1024).unwrap(),
                )
                .unwrap()
            }
        })
        .collect()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
