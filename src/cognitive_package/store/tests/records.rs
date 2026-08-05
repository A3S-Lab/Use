use super::*;

#[tokio::test]
async fn installed_graph_replace_is_cas_idempotent_and_atomically_overwrites() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = InstalledPackageGraphStore::new(&state_root);
    let prior = package_lock("1.0.0", '1');
    let candidate = package_lock("2.0.0", '2');

    assert!(store.put(&prior, 1).await.unwrap());
    let error = store
        .replace(&prior.root_package_id, &digest('0'), &candidate, 2)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
    assert_eq!(
        store
            .get(&prior.root_package_id)
            .await
            .unwrap()
            .unwrap()
            .package_lock,
        prior
    );

    let prior_digest = prior.descriptor_digest().unwrap();
    assert!(store
        .replace(&prior.root_package_id, &prior_digest, &candidate, 2,)
        .await
        .unwrap());
    assert!(!store
        .replace(&prior.root_package_id, &prior_digest, &candidate, 3,)
        .await
        .unwrap());
    assert_eq!(
        store
            .get(&prior.root_package_id)
            .await
            .unwrap()
            .unwrap()
            .package_lock,
        candidate
    );

    let parent = package_record_path(&state_root.join("package-graphs"), &prior.root_package_id)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut entries = fs::read_dir(parent).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        assert!(!entry.file_name().to_string_lossy().contains(".tmp-"));
    }
}

#[tokio::test]
async fn installed_graph_read_rejects_digest_tampering() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = InstalledPackageGraphStore::new(&state_root);
    let lock = package_lock("1.0.0", '1');
    store.put(&lock, 1).await.unwrap();
    let path =
        package_record_path(&state_root.join("package-graphs"), &lock.root_package_id).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
    value["packageLockDigest"] = serde_json::json!(digest('0'));
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap())
        .await
        .unwrap();

    let error = store.get(&lock.root_package_id).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
}

#[tokio::test]
async fn pending_store_serializes_all_actions_for_one_root() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = PendingPackageGraphStore::new(&state_root);
    let lock = package_lock("1.0.0", '1');
    let install = install_pending(&lock);
    let uninstall = uninstall_pending(&lock);

    assert!(store.put(&install).await.unwrap());
    assert!(!store.put(&install).await.unwrap());
    let error = store.put(&uninstall).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_busy");
    assert!(store
        .get(PluginOperationAction::Uninstall, &lock.root_package_id)
        .await
        .unwrap()
        .is_none());
}

#[test]
fn package_graph_plans_derive_okf_impact_for_every_action() {
    let prior = package_lock("1.0.0", '1');
    let candidate = package_lock("2.0.0", '2');
    let install = install_pending(&prior);
    let upgrade = upgrade_pending(&prior, &candidate);
    let uninstall = uninstall_pending(&prior);

    assert_eq!(
        install.envelope.plan.impact.okf_changes[0].change,
        SurfaceChangeKind::Add
    );
    assert_eq!(
        upgrade.envelope.plan.impact.okf_changes[0].change,
        SurfaceChangeKind::Replace
    );
    assert_eq!(
        uninstall.envelope.plan.impact.okf_changes[0].change,
        SurfaceChangeKind::Remove
    );
}

#[test]
fn pending_upgrade_rejects_prior_lock_manifest_and_generation_tampering() {
    let prior = package_lock("1.0.0", '1');
    let candidate = package_lock("2.0.0", '2');
    let pending = upgrade_pending(&prior, &candidate);
    let package_id = candidate.root_package_id.as_str();

    let mut changed_lock = pending.clone();
    changed_lock
        .prior_package_lock
        .as_mut()
        .unwrap()
        .root_package_id = "acme/other".to_string();
    assert_eq!(
        changed_lock.validate().unwrap_err().code,
        "use.plugin.package_graph_store_invalid"
    );

    let mut changed_manifest = pending.clone();
    changed_manifest
        .prior_manifests
        .get_mut(package_id)
        .unwrap()
        .skills[0]
        .path = PathBuf::from("skills/changed/SKILL.md");
    assert_eq!(
        changed_manifest.validate().unwrap_err().code,
        "use.plugin.package_graph_store_invalid"
    );

    let mut changed_generation = pending;
    let candidate_generation = changed_generation.generations[package_id];
    changed_generation
        .prior_generations
        .insert(package_id.to_string(), candidate_generation);
    assert_eq!(
        changed_generation.validate().unwrap_err().code,
        "use.plugin.package_graph_store_invalid"
    );
}
