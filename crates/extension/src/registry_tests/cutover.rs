use super::*;

#[tokio::test]
async fn lifecycle_cutover_replays_original_evidence_after_unrelated_registry_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let primary_source = temp.path().join("primary");
    let unrelated_source = temp.path().join("unrelated");
    cognitive_package_with_dependencies(&primary_source, "acme/primary", "primary", &[]).await;
    cognitive_package_with_dependencies(&unrelated_source, "acme/unrelated", "unrelated", &[])
        .await;
    let primary = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/primary",
        &primary_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let unrelated = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/unrelated",
        &unrelated_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let primary_identity = lifecycle_identity(&primary, 70);
    let unrelated_identity = lifecycle_identity(&unrelated, 71);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&primary_identity, &primary)
        .await
        .unwrap();

    let cutover_key = format!("sha256:{}", "7".repeat(64));
    let publication = registry
        .publish_lifecycle_package_with_durable_cutover(&primary_identity, &cutover_key)
        .await
        .unwrap();
    let cutover_generation = publication.registry_generation;
    let cutover_digest = publication.registry_snapshot_digest.clone();
    let cutover_snapshot = registry.snapshot().await.unwrap();
    assert_eq!(cutover_snapshot.pending_cutovers.len(), 1);

    registry
        .commit_lifecycle_package(&unrelated_identity, &unrelated)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package(&unrelated_identity)
        .await
        .unwrap();
    assert!(registry.snapshot().await.unwrap().generation > cutover_generation);

    let replay = registry
        .publish_lifecycle_package_with_durable_cutover(&primary_identity, &cutover_key)
        .await
        .unwrap();
    assert_eq!(replay.registry_generation, cutover_generation);
    assert_eq!(replay.registry_snapshot_digest, cutover_digest);
    assert!(!replay.packages[0].changed);

    let conflict = registry
        .publish_lifecycle_package_with_durable_cutover(&unrelated_identity, &cutover_key)
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "use.extension.registry_cutover_conflict");

    let before_completion = registry.snapshot().await.unwrap();
    let generation_before_completion = before_completion.generation;
    let digest_before_completion = before_completion.descriptor_digest().unwrap();
    registry
        .complete_lifecycle_cutover(&cutover_key)
        .await
        .unwrap();
    registry
        .complete_lifecycle_cutover(&cutover_key)
        .await
        .unwrap();
    let completed = registry.snapshot().await.unwrap();
    assert_eq!(completed.generation, generation_before_completion);
    assert_eq!(
        completed.descriptor_digest().unwrap(),
        digest_before_completion
    );
    assert!(completed.pending_cutovers.is_empty());
}

#[tokio::test]
async fn lifecycle_cutover_capacity_fails_before_receipt_or_generation_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let seed_source = temp.path().join("seed");
    let source = temp.path().join("capacity");
    cognitive_package_with_dependencies(&seed_source, "acme/seed", "seed", &[]).await;
    cognitive_package_with_dependencies(&source, "acme/capacity", "capacity", &[]).await;
    let seed = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/seed",
        &seed_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/capacity",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let seed_identity = lifecycle_identity(&seed, 71);
    let identity = lifecycle_identity(&candidate, 72);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&seed_identity, &seed)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package(&seed_identity)
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    let mut snapshot = registry.snapshot().await.unwrap();
    assert!(!registry
        .get("acme/capacity")
        .await
        .unwrap()
        .unwrap()
        .enabled());

    let receipt_path = registry.paths().receipt_path(identity.package_id());
    let receipt_identity_probe = temp.path().join("capacity-receipt-identity-probe.json");
    std::fs::hard_link(&receipt_path, &receipt_identity_probe).unwrap();
    for index in 0..MAX_PENDING_REGISTRY_CUTOVERS {
        snapshot.pending_cutovers.push(
            ExtensionRegistryCutoverRecord::new(
                format!(
                    "sha256:{:x}",
                    Sha256::digest(format!("capacity-key-{index}").as_bytes())
                ),
                format!(
                    "sha256:{:x}",
                    Sha256::digest(format!("capacity-request-{index}").as_bytes())
                ),
                0,
                1,
                format!("sha256:{}", "a".repeat(64)),
            )
            .unwrap(),
        );
    }
    write_registry_snapshot(registry.paths(), &snapshot)
        .await
        .unwrap();

    let key = format!("sha256:{}", "8".repeat(64));
    let error = registry
        .publish_lifecycle_package_with_durable_cutover(&identity, &key)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_cutover_capacity");
    let after = registry.snapshot().await.unwrap();
    assert_eq!(after.generation, snapshot.generation);
    assert_eq!(after.pending_cutovers, snapshot.pending_cutovers);
    assert!(!registry
        .get("acme/capacity")
        .await
        .unwrap()
        .unwrap()
        .enabled());

    let catalog = verified_knowledge_catalog(&source, "acme/capacity", &[], 'c').await;
    let package_lock = a3s_use_core::PluginPackageResolver::new(
        a3s_use_core::PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap(),
    )
    .resolve(catalog, Vec::new())
    .unwrap();
    let graph_key = format!("sha256:{}", "9".repeat(64));
    let graph_error = registry
        .publish_lifecycle_package_graph_with_durable_cutover(
            &package_lock,
            std::slice::from_ref(&identity),
            snapshot.generation,
            &graph_key,
        )
        .await
        .unwrap_err();
    assert_eq!(graph_error.code, "use.extension.registry_cutover_capacity");
    let graph_after = registry.snapshot().await.unwrap();
    assert_eq!(graph_after.generation, snapshot.generation);
    assert_eq!(graph_after.pending_cutovers, snapshot.pending_cutovers);
    assert!(!registry
        .get("acme/capacity")
        .await
        .unwrap()
        .unwrap()
        .enabled());

    std::fs::write(&receipt_identity_probe, b"receipt identity probe").unwrap();
    assert_eq!(
        std::fs::read(&receipt_path).unwrap(),
        b"receipt identity probe",
        "capacity rejection must happen before any receipt replacement"
    );
}

#[tokio::test]
async fn graph_cutover_rejects_a_stale_expected_generation_before_receipt_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let candidate_source = temp.path().join("candidate");
    let unrelated_source = temp.path().join("unrelated-generation");
    cognitive_package_with_dependencies(&candidate_source, "acme/candidate", "candidate", &[])
        .await;
    cognitive_package_with_dependencies(
        &unrelated_source,
        "acme/unrelated-generation",
        "unrelated-generation",
        &[],
    )
    .await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/candidate",
        &candidate_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let unrelated = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/unrelated-generation",
        &unrelated_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let candidate_identity = lifecycle_identity(&candidate, 80);
    let unrelated_identity = lifecycle_identity(&unrelated, 81);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&candidate_identity, &candidate)
        .await
        .unwrap();
    let expected_generation = registry.snapshot().await.unwrap().generation;

    registry
        .commit_lifecycle_package(&unrelated_identity, &unrelated)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package(&unrelated_identity)
        .await
        .unwrap();
    let snapshot_before = registry.snapshot().await.unwrap();
    assert!(snapshot_before.generation > expected_generation);
    let receipt_path = registry
        .paths()
        .receipt_path(candidate_identity.package_id());
    let receipt_before = std::fs::read(&receipt_path).unwrap();

    let catalog = verified_knowledge_catalog(&candidate_source, "acme/candidate", &[], 'c').await;
    let package_lock = a3s_use_core::PluginPackageResolver::new(
        a3s_use_core::PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap(),
    )
    .resolve(catalog, Vec::new())
    .unwrap();
    let key = format!("sha256:{}", "a".repeat(64));
    let error = registry
        .publish_lifecycle_package_graph_with_durable_cutover(
            &package_lock,
            std::slice::from_ref(&candidate_identity),
            expected_generation,
            &key,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.extension.registry_cutover_conflict");
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before);
    assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt_before);
    assert!(!registry
        .get(candidate_identity.package_id())
        .await
        .unwrap()
        .unwrap()
        .enabled());
}

#[tokio::test]
async fn single_package_cutovers_reject_stale_publish_and_hide_generations() {
    let temp = tempfile::tempdir().unwrap();
    let candidate_source = temp.path().join("single-candidate");
    let first_drift_source = temp.path().join("first-drift");
    let second_drift_source = temp.path().join("second-drift");
    for (source, package_id, route) in [
        (
            &candidate_source,
            "acme/single-candidate",
            "single-candidate",
        ),
        (&first_drift_source, "acme/first-drift", "first-drift"),
        (&second_drift_source, "acme/second-drift", "second-drift"),
    ] {
        cognitive_package_with_dependencies(source, package_id, route, &[]).await;
    }
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/single-candidate",
        &candidate_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let first_drift = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/first-drift",
        &first_drift_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let second_drift = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/second-drift",
        &second_drift_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let candidate_identity = lifecycle_identity(&candidate, 90);
    let first_drift_identity = lifecycle_identity(&first_drift, 91);
    let second_drift_identity = lifecycle_identity(&second_drift, 92);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&candidate_identity, &candidate)
        .await
        .unwrap();

    let expected_publish_generation = registry.snapshot().await.unwrap().generation;
    registry
        .commit_lifecycle_package(&first_drift_identity, &first_drift)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package(&first_drift_identity)
        .await
        .unwrap();
    let snapshot_before_stale_publish = registry.snapshot().await.unwrap();
    let candidate_receipt_path = registry
        .paths()
        .receipt_path(candidate_identity.package_id());
    let receipt_before_stale_publish = std::fs::read(&candidate_receipt_path).unwrap();
    let publish_key = format!("sha256:{}", "b".repeat(64));
    let publish_error = registry
        .publish_lifecycle_package_at_generation_with_durable_cutover(
            &candidate_identity,
            expected_publish_generation,
            &publish_key,
        )
        .await
        .unwrap_err();
    assert_eq!(
        publish_error.code,
        "use.extension.registry_cutover_conflict"
    );
    assert_eq!(
        registry.snapshot().await.unwrap(),
        snapshot_before_stale_publish
    );
    assert_eq!(
        std::fs::read(&candidate_receipt_path).unwrap(),
        receipt_before_stale_publish
    );

    registry
        .publish_lifecycle_package(&candidate_identity)
        .await
        .unwrap();
    let expected_hide_generation = registry.snapshot().await.unwrap().generation;
    registry
        .commit_lifecycle_package(&second_drift_identity, &second_drift)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package(&second_drift_identity)
        .await
        .unwrap();
    let snapshot_before_stale_hide = registry.snapshot().await.unwrap();
    let receipt_before_stale_hide = std::fs::read(&candidate_receipt_path).unwrap();
    let hide_key = format!("sha256:{}", "c".repeat(64));
    let hide_error = registry
        .hide_lifecycle_package_at_generation_with_durable_cutover(
            &candidate_identity,
            expected_hide_generation,
            &hide_key,
        )
        .await
        .unwrap_err();
    assert_eq!(hide_error.code, "use.extension.registry_cutover_conflict");
    assert_eq!(
        registry.snapshot().await.unwrap(),
        snapshot_before_stale_hide
    );
    assert_eq!(
        std::fs::read(&candidate_receipt_path).unwrap(),
        receipt_before_stale_hide
    );
    assert!(registry
        .get(candidate_identity.package_id())
        .await
        .unwrap()
        .unwrap()
        .enabled());
}
