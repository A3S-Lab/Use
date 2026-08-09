use super::*;

#[tokio::test]
async fn dependency_closure_prepares_forward_then_publishes_once() {
    let fixture = install_graph_fixture(false);
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(0);
    let records = graph
        .apply_install(&fixture.envelope, &fixture.units, || {
            time.fetch_add(1, Ordering::Relaxed) + 1
        })
        .await
        .unwrap();
    assert!(records
        .iter()
        .all(|record| record.status == PluginLifecycleOperationStatus::Completed));
    let calls = fixture.host.calls.lock().await;
    assert_eq!(
        &calls[..calls.len() - 1],
        [
            "acme/base:commit",
            "acme/base:okf-prepare",
            "acme/base:skill-prepare",
            "acme/root:commit",
            "acme/root:okf-prepare",
            "acme/root:skill-prepare",
            "batch:acme/base,acme/root",
        ]
    );
    assert_eq!(
        calls.last(),
        Some(&format!(
            "cutover-complete:{}",
            publication_key(&fixture.envelope).unwrap()
        ))
    );
}

#[tokio::test]
async fn dependency_closure_reuses_a_reviewed_retained_dependency() {
    let fixture = install_graph_fixture(true);
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let records = graph
        .apply_install(&fixture.envelope, &fixture.units, || 1)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let calls = fixture.host.calls.lock().await;
    assert_eq!(
        &calls[..calls.len() - 1],
        [
            "acme/root:commit",
            "acme/root:okf-prepare",
            "acme/root:skill-prepare",
            "batch:acme/root",
        ]
    );
    assert_eq!(
        calls.last(),
        Some(&format!(
            "cutover-complete:{}",
            publication_key(&fixture.envelope).unwrap()
        ))
    );
}

#[tokio::test]
async fn publication_identity_mismatch_stays_replayable_without_repreparing_packages() {
    let fixture = install_graph_fixture(false);
    *fixture.host.publication_fault.lock().await = Some(PublicationFault::ReverseEvidence);
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());

    let error = graph
        .apply_install(&fixture.envelope, &fixture.units, || 1)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_invalid");
    assert!(error.message.contains("order or identity"));

    let records = graph
        .apply_install(&fixture.envelope, &fixture.units, || 2)
        .await
        .unwrap();
    assert!(records
        .iter()
        .all(|record| record.status == PluginLifecycleOperationStatus::Completed));
    let calls = fixture.host.calls.lock().await;
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "batch:acme/base,acme/root")
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.ends_with(":commit"))
            .count(),
        2
    );
}
