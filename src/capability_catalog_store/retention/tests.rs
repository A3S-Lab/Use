use std::path::PathBuf;

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, InstallationKind};
use tokio::io::AsyncWriteExt;

use super::{journal::RetentionJournal, CapabilityGatewayCatalogStore};

fn installation(label: &str) -> InstallationId {
    InstallationId::new(InstallationKind::User, format!("user/{label}")).unwrap()
}

fn catalog(installation: &InstallationId, generation: u64) -> CapabilityGatewayCatalog {
    CapabilityGatewayCatalog::new(installation.clone(), generation, Vec::new()).unwrap()
}

async fn journal_path(store: &CapabilityGatewayCatalogStore) -> PathBuf {
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    root.join(super::CATALOG_RETENTION_JOURNAL)
}

#[tokio::test]
async fn retention_resumes_after_a_durable_in_flight_checkpoint() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-recovery");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();

    let mut journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    journal.begin_removal(0).await.unwrap();
    drop(journal);

    // The process is assumed to have exited after persisting Removing but
    // before unlinking. The next apply must safely continue the same plan.
    let result = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(result.changed);
    assert_eq!(result.removed, plan.remove);
    assert!(store.get(&first.digest).await.unwrap().is_none());
    assert!(store.get(&second.digest).await.unwrap().is_some());
    assert!(!journal_path(&store).await.exists());
}

#[tokio::test]
async fn retention_reconciles_an_unlinked_in_flight_record_after_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-recovery-unlink");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    let mut journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    journal.begin_removal(0).await.unwrap();
    let target = super::super::path_for_digest(&root, &first.digest).unwrap();
    tokio::fs::remove_file(&target).await.unwrap();
    super::super::sync_directory(target.parent().unwrap())
        .await
        .unwrap();
    drop(journal);

    let result = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(result.changed);
    assert_eq!(result.removed, plan.remove);
    assert!(!journal_path(&store).await.exists());
}

#[tokio::test]
async fn retention_can_recover_without_reconstructing_the_reviewed_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-recovery-api");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    let mut journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    journal.begin_removal(0).await.unwrap();
    drop(journal);

    let result = store.recover_retention().await.unwrap().unwrap();
    assert!(result.changed);
    assert_eq!(result.removed, plan.remove);
    assert!(store.get(&first.digest).await.unwrap().is_none());
    assert!(store.get(&second.digest).await.unwrap().is_some());
    assert!(!journal_path(&store).await.exists());
}

#[tokio::test]
async fn retention_repairs_a_torn_journal_tail_before_resuming() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-recovery-tail");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    let mut journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    journal.begin_removal(0).await.unwrap();
    drop(journal);
    let path = journal_path(&store).await;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    file.write_all(br#"{"schema":"a3s.use.capability-gateway-catalog-retention-journal.v1"}"#)
        .await
        .unwrap();
    file.sync_all().await.unwrap();
    drop(file);

    let result = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(result.changed);
    assert_eq!(result.removed, plan.remove);
    assert!(store.get(&first.digest).await.unwrap().is_none());
    assert!(!journal_path(&store).await.exists());
}

#[tokio::test]
async fn pending_retention_blocks_a_new_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-recovery-gate");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    let journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    drop(journal);

    let error = store.publish(&catalog(&installation, 2)).await.unwrap_err();
    assert_eq!(
        error.code,
        "use.plugin.capability_gateway_catalog_retention_stale"
    );
    assert_eq!(
        store.get(&first.digest).await.unwrap_err().code,
        "use.plugin.capability_gateway_catalog_retention_stale"
    );
    assert_eq!(
        store.get(&second.digest).await.unwrap_err().code,
        "use.plugin.capability_gateway_catalog_retention_stale"
    );
}

#[tokio::test]
async fn pending_retention_blocks_catalog_reads_and_inventory_listing() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-read-gate");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    let journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    drop(journal);

    let list_error = store.list().await.unwrap_err();
    assert_eq!(
        list_error.code,
        "use.plugin.capability_gateway_catalog_retention_stale"
    );
    let get_error = store.get(&first.digest).await.unwrap_err();
    assert_eq!(
        get_error.code,
        "use.plugin.capability_gateway_catalog_retention_stale"
    );
}

#[tokio::test]
async fn retention_rejects_a_noncanonical_recovery_journal() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("retention-recovery-tamper");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = store.publish(&catalog(&installation, 0)).await.unwrap();
    let second = store.publish(&catalog(&installation, 1)).await.unwrap();
    let plan = store
        .plan_retention(std::slice::from_ref(&second.digest))
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let (_, root) = store.existing_physical_paths().await.unwrap().unwrap();
    let journal = RetentionJournal::create(&root, &plan, &plan_digest)
        .await
        .unwrap();
    drop(journal);
    let path = journal_path(&store).await;
    let mut bytes = tokio::fs::read(&path).await.unwrap();
    let position = bytes
        .iter()
        .position(|byte| *byte == b'0')
        .expect("prepared journal contains a numeric byte");
    bytes[position] = b'1';
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .await
        .unwrap();
    file.write_all(&bytes).await.unwrap();
    file.sync_all().await.unwrap();
    drop(file);

    let error = store
        .apply_retention(&plan, &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.plugin.capability_gateway_catalog_retention_invalid"
    );
    assert_eq!(
        store.get(&first.digest).await.unwrap_err().code,
        "use.plugin.capability_gateway_catalog_retention_invalid"
    );
    assert_eq!(
        store.get(&second.digest).await.unwrap_err().code,
        "use.plugin.capability_gateway_catalog_retention_invalid"
    );
}
