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

#[tokio::test]
async fn pending_store_advances_one_exact_reviewed_plan_to_admission() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = PendingPackageGraphStore::new(&state_root);
    let lock = package_lock("1.0.0", '1');
    let admitted_fixture = install_pending(&lock);
    let planned = PendingPackageGraphOperation::planned(
        admitted_fixture.envelope.clone(),
        9,
        admitted_fixture.generations.clone(),
        admitted_fixture.manifests.clone(),
    )
    .unwrap();

    assert_eq!(planned.phase(), PackageGraphOperationPhase::Planned);
    assert!(store.put(&planned).await.unwrap());
    let (admitted, changed) = store
        .admit(&planned, 10, PackageGraphAuthorization::default())
        .await
        .unwrap();
    assert!(changed);
    assert_eq!(admitted.phase(), PackageGraphOperationPhase::Admitted);
    assert_eq!(
        store
            .get(PluginOperationAction::Install, &lock.root_package_id)
            .await
            .unwrap(),
        Some(admitted.clone())
    );
    let (replayed, changed) = store
        .admit(&planned, 10, PackageGraphAuthorization::default())
        .await
        .unwrap();
    assert!(!changed);
    assert_eq!(replayed, admitted);

    let error = store
        .admit(&planned, 11, PackageGraphAuthorization::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
}

#[tokio::test]
async fn one_admitted_graph_durably_owns_the_global_installation_domain() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = PendingPackageGraphStore::new(&state_root);
    let first_lock = package_lock_for("acme/first", "1.0.0", '1');
    let second_lock = package_lock_for("acme/second", "1.0.0", '2');
    let first_fixture = install_pending(&first_lock);
    let second_fixture = install_pending(&second_lock);
    let first = PendingPackageGraphOperation::planned(
        first_fixture.envelope.clone(),
        10,
        first_fixture.generations.clone(),
        first_fixture.manifests.clone(),
    )
    .unwrap();
    let second = PendingPackageGraphOperation::planned(
        second_fixture.envelope.clone(),
        10,
        second_fixture.generations.clone(),
        second_fixture.manifests.clone(),
    )
    .unwrap();
    store.put(&first).await.unwrap();
    store.put(&second).await.unwrap();

    let (first_admitted, changed) = store
        .admit(&first, 11, PackageGraphAuthorization::default())
        .await
        .unwrap();
    assert!(changed);
    let preflight_error = store
        .require_admission_available(&second)
        .await
        .unwrap_err();
    assert_eq!(preflight_error.code, "use.plugin.package_graph_busy");
    let error = store
        .admit(&second, 11, PackageGraphAuthorization::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_busy");
    assert_eq!(
        error.details["activeOperationId"],
        serde_json::json!(first_admitted.envelope.plan.operation_id)
    );
    assert_eq!(
        store
            .get(PluginOperationAction::Install, second.root_package_id())
            .await
            .unwrap()
            .unwrap()
            .phase(),
        PackageGraphOperationPhase::Planned
    );

    store.remove(&first_admitted).await.unwrap();
    let (second_admitted, changed) = store
        .admit(&second, 11, PackageGraphAuthorization::default())
        .await
        .unwrap();
    assert!(changed);
    assert_eq!(
        second_admitted.phase(),
        PackageGraphOperationPhase::Admitted
    );
}

#[tokio::test]
async fn pending_store_persists_exact_cancellation_before_host_projection() {
    let temp = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let store = PendingPackageGraphStore::new(&state_root);
    let lock = package_lock("1.0.0", '1');
    let admitted_fixture = install_pending(&lock);
    let planned = PendingPackageGraphOperation::planned(
        admitted_fixture.envelope.clone(),
        9,
        admitted_fixture.generations.clone(),
        admitted_fixture.manifests.clone(),
    )
    .unwrap();

    assert!(store.put(&planned).await.unwrap());
    let (cancelled, changed) = store
        .cancel_planned(&planned, "cancel:test:0001", 10)
        .await
        .unwrap();
    assert!(changed);
    assert_eq!(cancelled.phase(), PackageGraphOperationPhase::Cancelled);
    assert_eq!(cancelled.cancelled_at_ms, 10);
    assert_eq!(
        cancelled.cancellation_request_id.as_deref(),
        Some("cancel:test:0001")
    );

    let restarted = PendingPackageGraphStore::new(&state_root);
    assert_eq!(
        restarted
            .get(PluginOperationAction::Install, &lock.root_package_id)
            .await
            .unwrap(),
        Some(cancelled.clone())
    );
    let (replayed, changed) = restarted
        .cancel_planned(&planned, "cancel:test:0001", 10)
        .await
        .unwrap();
    assert!(!changed);
    assert_eq!(replayed, cancelled);
    let error = restarted
        .admit(&planned, 11, PackageGraphAuthorization::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
}

#[tokio::test]
async fn pending_store_rejects_superseded_v2_and_v3_records() {
    let lock = package_lock("1.0.0", '1');
    let admitted = install_pending(&lock);
    for schema in [
        "a3s.use.pending-package-graph-operation.v2",
        "a3s.use.pending-package-graph-operation.v3",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("state");
        let store = PendingPackageGraphStore::new(&state_root);
        store.put(&admitted).await.unwrap();
        let path = pending_record_path(
            &state_root.join("operations").join("package-graphs"),
            admitted.action(),
            admitted.root_package_id(),
        )
        .unwrap();
        let mut superseded = serde_json::to_value(&admitted).unwrap();
        superseded["schema"] = serde_json::json!(schema);
        if schema == "a3s.use.pending-package-graph-operation.v2" {
            superseded.as_object_mut().unwrap().remove("phase");
            superseded.as_object_mut().unwrap().remove("plannedAtMs");
        }
        fs::write(&path, serde_json::to_vec_pretty(&superseded).unwrap())
            .await
            .unwrap();

        let error = store
            .get(admitted.action(), admitted.root_package_id())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
    }
}

#[tokio::test]
async fn pending_store_requires_every_current_v4_phase_field() {
    let lock = package_lock("1.0.0", '1');
    let admitted = install_pending(&lock);
    for field in ["phase", "plannedAtMs", "authorization"] {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("state");
        let store = PendingPackageGraphStore::new(&state_root);
        store.put(&admitted).await.unwrap();
        let path = pending_record_path(
            &state_root.join("operations").join("package-graphs"),
            admitted.action(),
            admitted.root_package_id(),
        )
        .unwrap();
        let mut incomplete = serde_json::to_value(&admitted).unwrap();
        incomplete.as_object_mut().unwrap().remove(field);
        fs::write(&path, serde_json::to_vec_pretty(&incomplete).unwrap())
            .await
            .unwrap();

        let error = store
            .get(admitted.action(), admitted.root_package_id())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
    }
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
