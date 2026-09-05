use super::*;

fn installation(label: &str) -> InstallationId {
    InstallationId::new(
        a3s_use_core::InstallationKind::User,
        format!("user/{label}"),
    )
    .unwrap()
}

fn catalog(installation: &InstallationId, generation: u64) -> CapabilityGatewayCatalog {
    CapabilityGatewayCatalog::new(installation.clone(), generation, Vec::new()).unwrap()
}

#[tokio::test]
async fn clean_restore_publishes_the_exact_set_and_replays_without_clobbering() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("catalog-clean-restore");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let catalogs = vec![catalog(&installation, 1), catalog(&installation, 2)];
    let plan = store.plan_clean_restore(&catalogs).unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let result = store
        .apply_clean_restore(&plan, &catalogs, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.restored_record_count, 2);
    assert_eq!(result.restored_byte_count, plan.byte_count);
    assert_eq!(result.inventory_digest, plan.inventory_digest);

    let mut publications = store.list().await.unwrap();
    publications.sort_by(|left, right| left.digest.cmp(&right.digest));
    let mut expected = catalogs
        .iter()
        .map(|catalog| catalog.descriptor_digest().unwrap())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        publications
            .iter()
            .map(|publication| publication.digest.clone())
            .collect::<Vec<_>>(),
        expected
    );

    let replay = store
        .apply_clean_restore(&plan, &catalogs, &plan_digest)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.plan_digest, result.plan_digest);
}

#[tokio::test]
async fn clean_restore_refuses_an_existing_different_owner_inventory() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("catalog-clean-restore-conflict");
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let existing = catalog(&installation, 1);
    store.publish(&existing).await.unwrap();
    let requested = vec![catalog(&installation, 2)];
    let plan = store.plan_clean_restore(&requested).unwrap();
    let error = store
        .apply_clean_restore(&plan, &requested, &plan.descriptor_digest().unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_TARGET_NOT_EMPTY);
    assert!(store
        .get(&existing.descriptor_digest().unwrap())
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn clean_restore_rejects_a_foreign_or_rebound_source_set() {
    let temporary = tempfile::tempdir().unwrap();
    let owner_installation = installation("catalog-clean-restore-source");
    let foreign = installation("catalog-clean-restore-foreign");
    let store = CapabilityGatewayCatalogStore::new(
        temporary.path().join("state"),
        owner_installation.clone(),
    )
    .unwrap();
    let requested = vec![catalog(&owner_installation, 1)];
    let plan = store.plan_clean_restore(&requested).unwrap();
    let error = store
        .apply_clean_restore(
            &plan,
            &[catalog(&foreign, 1)],
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_INVALID);

    let mut tampered = plan.clone();
    tampered.byte_count += 1;
    assert_eq!(
        tampered.descriptor_digest().unwrap_err().code,
        ERROR_INVALID
    );
}

#[tokio::test]
async fn clean_restore_replays_a_durable_candidate_and_activation_marker() {
    let temporary = tempfile::tempdir().unwrap();
    let owner_installation = installation("catalog-clean-restore-replay");
    let store = CapabilityGatewayCatalogStore::new(
        temporary.path().join("state"),
        owner_installation.clone(),
    )
    .unwrap();
    let catalogs = vec![catalog(&owner_installation, 7)];
    let plan = store.plan_clean_restore(&catalogs).unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    ensure_directory_exists(store.state_root()).await.unwrap();
    let (state_root, root) = store.physical_paths().await.unwrap();
    let parent = root.parent().unwrap();
    ensure_owned_directory_chain(&state_root, parent)
        .await
        .unwrap();
    let staging = staging_directory(parent, &plan_digest).unwrap();
    let prepared = prepare_catalogs(&store, &catalogs).unwrap();
    prepare_staging(
        &store,
        &state_root,
        &staging,
        &prepared,
        &plan,
        &plan_digest,
    )
    .await
    .unwrap();
    create_activation_marker(&staging, &plan, &plan_digest)
        .await
        .unwrap();

    let result = store
        .apply_clean_restore(&plan, &catalogs, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert!(store.root().is_absolute());
    assert_eq!(store.list().await.unwrap().len(), 1);
    assert!(!staging.exists());
}

#[tokio::test]
async fn clean_restore_rejects_a_foreign_durable_staging_attempt() {
    let temporary = tempfile::tempdir().unwrap();
    let owner_installation = installation("catalog-clean-restore-foreign-stage");
    let store = CapabilityGatewayCatalogStore::new(
        temporary.path().join("state"),
        owner_installation.clone(),
    )
    .unwrap();
    let catalogs = vec![catalog(&owner_installation, 11)];
    let plan = store.plan_clean_restore(&catalogs).unwrap();
    let parent = store.root().parent().unwrap();
    tokio::fs::create_dir_all(parent).await.unwrap();
    tokio::fs::create_dir(parent.join(format!("{STAGING_PREFIX}{}", "f".repeat(64))))
        .await
        .unwrap();

    let error = store
        .apply_clean_restore(&plan, &catalogs, &plan.descriptor_digest().unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_INVALID);
    assert!(!store.root().exists());
}
