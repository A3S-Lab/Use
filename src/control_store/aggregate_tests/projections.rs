use super::*;
use a3s_use_core::{
    CatalogAvailability, PlanScope, PluginPackageDependency, PLUGIN_CATALOG_SCHEMA_V3,
};
use std::collections::BTreeMap;

const OKF_CATALOG: &[u8] =
    include_bytes!("../../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");

#[test]
fn deterministic_projection_covers_all_actions_and_never_reuses_generations() {
    let history = ControlProjectionHistory::default();
    let install = operation_at(
        "operation:projection:install",
        PluginOperationAction::Install,
        0,
        0,
    );
    let installed = install
        .project_generation(None, &history, install.reviewed_at_ms + 10)
        .unwrap();
    assert_eq!(installed.snapshot.generation, 1);
    assert_eq!(installed.snapshot.packages[0].state_generation, 1);
    assert_eq!(installed.package_lifecycles[0].lifecycle_generation, 1);
    assert_eq!(installed.history_after.last_lifecycle_generation(), 1);
    let installed_generation = generation(&install, &installed);

    let disable = operation_at(
        "operation:projection:disable",
        PluginOperationAction::Disable,
        1,
        1,
    );
    let disabled = disable
        .project_generation(
            Some(&installed_generation),
            &installed.history_after,
            disable.reviewed_at_ms + 10,
        )
        .unwrap();
    assert!(!disabled.snapshot.packages[0].enabled);
    assert_eq!(disabled.snapshot.packages[0].state_generation, 2);
    assert_eq!(disabled.package_lifecycles[0].lifecycle_generation, 1);
    let disabled_generation = generation(&disable, &disabled);

    let enable = operation_at(
        "operation:projection:enable",
        PluginOperationAction::Enable,
        2,
        2,
    );
    let enabled = enable
        .project_generation(
            Some(&disabled_generation),
            &disabled.history_after,
            enable.reviewed_at_ms + 10,
        )
        .unwrap();
    assert!(enabled.snapshot.packages[0].enabled);
    assert_eq!(enabled.snapshot.packages[0].state_generation, 3);
    assert_eq!(enabled.package_lifecycles[0].lifecycle_generation, 1);
    let enabled_generation = generation(&enable, &enabled);

    let uninstall = operation_at(
        "operation:projection:uninstall",
        PluginOperationAction::Uninstall,
        3,
        3,
    );
    let removed = uninstall
        .project_generation(
            Some(&enabled_generation),
            &enabled.history_after,
            uninstall.reviewed_at_ms + 10,
        )
        .unwrap();
    assert!(removed.snapshot.roots.is_empty());
    assert!(removed.snapshot.packages.is_empty());
    assert!(removed.package_lifecycles.is_empty());
    assert_eq!(removed.history_after.last_lifecycle_generation(), 1);
    let removed_generation = generation(&uninstall, &removed);

    let reinstall = operation_at(
        "operation:projection:reinstall",
        PluginOperationAction::Install,
        4,
        4,
    );
    let reinstalled = reinstall
        .project_generation(
            Some(&removed_generation),
            &removed.history_after,
            reinstall.reviewed_at_ms + 10,
        )
        .unwrap();
    assert_eq!(reinstalled.snapshot.packages[0].state_generation, 4);
    assert_eq!(reinstalled.package_lifecycles[0].lifecycle_generation, 2);
    assert_eq!(reinstalled.history_after.last_lifecycle_generation(), 2);
}

#[test]
fn upgrade_projection_replaces_exact_bytes_and_advances_both_package_axes() {
    let install = operation_at(
        "operation:projection:upgrade-base",
        PluginOperationAction::Install,
        0,
        0,
    );
    let installed = install
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            install.reviewed_at_ms + 10,
        )
        .unwrap();
    let installed_generation = generation(&install, &installed);
    let upgrade = operation_at(
        "operation:projection:upgrade",
        PluginOperationAction::Upgrade,
        1,
        1,
    );
    let upgraded = upgrade
        .project_generation(
            Some(&installed_generation),
            &installed.history_after,
            upgrade.reviewed_at_ms + 10,
        )
        .unwrap();

    assert_eq!(upgraded.snapshot.generation, 2);
    assert_eq!(upgraded.snapshot.packages[0].package.version(), "2.1.0");
    assert_eq!(upgraded.snapshot.packages[0].state_generation, 2);
    assert_eq!(upgraded.package_lifecycles[0].lifecycle_generation, 2);
    assert_ne!(
        upgraded.snapshot.packages[0].package,
        installed.snapshot.packages[0].package
    );
}

#[test]
fn projection_rejects_history_that_does_not_describe_the_prior_generation() {
    let install = operation_at(
        "operation:projection:history-base",
        PluginOperationAction::Install,
        0,
        0,
    );
    let installed = install
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            install.reviewed_at_ms + 10,
        )
        .unwrap();
    let installed_generation = generation(&install, &installed);
    let disable = operation_at(
        "operation:projection:history-drift",
        PluginOperationAction::Disable,
        1,
        1,
    );

    assert_eq!(
        disable
            .project_generation(
                Some(&installed_generation),
                &ControlProjectionHistory::default(),
                disable.reviewed_at_ms + 10,
            )
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );
}

#[test]
fn projection_history_rejects_inconsistent_and_over_capacity_state() {
    let inconsistent = BTreeMap::from([("acme/history".to_string(), 1)]);
    assert_eq!(
        ControlProjectionHistory::new(0, inconsistent)
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );

    let maximum = super::super::model::MAX_CONTROL_HISTORY_PACKAGES;
    let package_state_generations = (0..maximum)
        .map(|index| (format!("history/package-{index}"), 1))
        .collect();
    let history = ControlProjectionHistory::new(maximum as u64, package_state_generations).unwrap();
    let lock = PluginPackageResolver::new(test_host())
        .resolve(
            verified_record("acme/capacity", Vec::new(), 'c'),
            Vec::new(),
        )
        .unwrap();
    let installation = InstallationId::new(InstallationKind::User, "current").unwrap();
    let prior_snapshot = InstallationSnapshot::from_root_locks(
        installation,
        1,
        lock.host.clone(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let prior = ControlGeneration {
        operation_id: "operation:projection:prior-empty".to_string(),
        snapshot_digest: prior_snapshot.descriptor_digest().unwrap(),
        snapshot: prior_snapshot,
        package_lifecycles: Vec::new(),
        grants: Vec::new(),
        bindings: Vec::new(),
        capability: ControlCapabilitySelection {
            generation: 1,
            descriptor_digest: digest('5'),
        },
        capability_status: ControlCapabilityStatus::Published,
        capability_published_at_ms: Some(150),
        committed_at_ms: 100,
    };
    let install = reviewed_install(
        "operation:projection:history-capacity",
        lock,
        Some(&prior.snapshot),
        1,
        1,
    );

    assert_eq!(
        install
            .project_generation(Some(&prior), &history, install.reviewed_at_ms + 10)
            .unwrap_err()
            .code,
        "use.control_store.input_invalid"
    );
}

#[test]
fn projection_merges_two_roots_without_reallocating_the_shared_dependency() {
    let shared = verified_record("acme/shared", Vec::new(), 'c');
    let first_lock = PluginPackageResolver::new(test_host())
        .resolve(
            verified_record(
                "acme/root-a",
                vec![PluginPackageDependency::new("acme/shared", "^1.0.0").unwrap()],
                'a',
            ),
            vec![shared.clone()],
        )
        .unwrap();
    let second_lock = PluginPackageResolver::new(test_host())
        .resolve(
            verified_record(
                "acme/root-b",
                vec![PluginPackageDependency::new("acme/shared", "^1.0.0").unwrap()],
                'b',
            ),
            vec![shared],
        )
        .unwrap();

    let first = reviewed_install("operation:multi-root:first", first_lock, None, 0, 0);
    let first_projection = first
        .project_generation(
            None,
            &ControlProjectionHistory::default(),
            first.reviewed_at_ms + 10,
        )
        .unwrap();
    let first_generation = generation(&first, &first_projection);
    let shared_before = first_projection
        .snapshot
        .package_selection("acme/shared")
        .unwrap();
    let shared_lifecycle_before = first_projection
        .package_lifecycles
        .iter()
        .find(|value| value.package_id == "acme/shared")
        .unwrap()
        .lifecycle_generation;

    let second = reviewed_install(
        "operation:multi-root:second",
        second_lock.clone(),
        Some(&first_projection.snapshot),
        1,
        1,
    );
    let second_projection = second
        .project_generation(
            Some(&first_generation),
            &first_projection.history_after,
            second.reviewed_at_ms + 10,
        )
        .unwrap();

    assert_eq!(
        second_projection
            .snapshot
            .roots
            .iter()
            .map(|root| root.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/root-a", "acme/root-b"]
    );
    assert_eq!(second_projection.snapshot.packages.len(), 3);
    let shared_after = second_projection
        .snapshot
        .package_selection("acme/shared")
        .unwrap();
    assert_eq!(shared_after, shared_before);
    assert_eq!(
        second_projection
            .package_lifecycles
            .iter()
            .find(|value| value.package_id == "acme/shared")
            .unwrap()
            .lifecycle_generation,
        shared_lifecycle_before
    );
    assert_eq!(
        second_projection.history_after.last_lifecycle_generation(),
        3
    );

    let second_generation = generation(&second, &second_projection);
    let uninstall = reviewed_uninstall(
        "operation:multi-root:uninstall",
        second_lock.clone(),
        &second_projection.snapshot,
        2,
        2,
    );
    let removed = uninstall
        .project_generation(
            Some(&second_generation),
            &second_projection.history_after,
            uninstall.reviewed_at_ms + 10,
        )
        .unwrap();
    assert_eq!(
        removed
            .snapshot
            .roots
            .iter()
            .map(|root| root.package_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/root-a"]
    );
    assert_eq!(removed.snapshot.packages.len(), 2);
    assert_eq!(
        removed.snapshot.package_selection("acme/shared").unwrap(),
        shared_before
    );
    assert_eq!(
        removed
            .package_lifecycles
            .iter()
            .find(|value| value.package_id == "acme/shared")
            .unwrap()
            .lifecycle_generation,
        shared_lifecycle_before
    );
    assert_eq!(removed.history_after.last_lifecycle_generation(), 3);

    let removed_generation = generation(&uninstall, &removed);
    let reinstall = reviewed_install(
        "operation:multi-root:reinstall",
        second_lock,
        Some(&removed.snapshot),
        3,
        3,
    );
    let reinstalled = reinstall
        .project_generation(
            Some(&removed_generation),
            &removed.history_after,
            reinstall.reviewed_at_ms + 10,
        )
        .unwrap();
    let root_b = reinstalled
        .snapshot
        .package_selection("acme/root-b")
        .unwrap();
    assert_eq!(root_b.state_generation, 2);
    assert_eq!(
        reinstalled
            .package_lifecycles
            .iter()
            .find(|value| value.package_id == "acme/root-b")
            .unwrap()
            .lifecycle_generation,
        4
    );
    assert_eq!(
        reinstalled
            .snapshot
            .package_selection("acme/shared")
            .unwrap(),
        shared_before
    );
}

fn generation(
    operation: &ReviewedControlOperation,
    projected: &ProjectedControlGeneration,
) -> ControlGeneration {
    ControlGeneration {
        operation_id: operation.operation_id().to_string(),
        snapshot: projected.snapshot.clone(),
        snapshot_digest: projected.snapshot.descriptor_digest().unwrap(),
        package_lifecycles: projected.package_lifecycles.clone(),
        grants: Vec::new(),
        bindings: Vec::new(),
        capability: ControlCapabilitySelection {
            generation: operation.target_capability_generation().unwrap(),
            descriptor_digest: digest('5'),
        },
        capability_status: ControlCapabilityStatus::Candidate,
        capability_published_at_ms: None,
        committed_at_ms: operation.reviewed_at_ms + 10,
    }
}

fn reviewed_install(
    operation_id: &str,
    lock: PluginPackageLock,
    prior: Option<&InstallationSnapshot>,
    expected_generation: u64,
    expected_capability_generation: u64,
) -> ReviewedControlOperation {
    let mut packages = lock
        .packages
        .iter()
        .map(|package| {
            let role = if package.package_id() == lock.root_package_id {
                PlanPackageRole::Root
            } else {
                PlanPackageRole::Dependency
            };
            match prior.and_then(|snapshot| snapshot.package_selection(package.package_id())) {
                Some(selection) => {
                    let state = selection
                        .package
                        .catalog
                        .selected_state(&selection.selected_surfaces)
                        .unwrap();
                    PlannedPackageTransition::resolved(
                        package.package_id(),
                        role,
                        PlanPackageChangeKind::Retain,
                        Some(state.clone()),
                        Some(state),
                        None,
                    )
                }
                None => package.catalog.install_transition(role, &[]),
            }
        })
        .collect::<a3s_use_core::UseResult<Vec<_>>>()
        .unwrap();
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let reviewed_at_ms = 100 + expected_generation * 100;
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        lock.root_package_id.clone(),
        "runtime:local",
        packages,
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: lock
                .packages
                .iter()
                .map(|package| package.catalog.record.archive.length)
                .sum(),
            installed_bytes_after: lock
                .packages
                .iter()
                .map(|package| package.catalog.record.package.expanded_bytes)
                .sum(),
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: expected_generation + 1,
            capability_generation: expected_capability_generation,
            receipt_digest: None,
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
        created_at_ms: reviewed_at_ms - 1,
        expires_at_ms: reviewed_at_ms + 100,
        scope: PlanScope::new(InstallationKind::User, "current").unwrap(),
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Allow,
            policy_digest: digest('9'),
            confirmation_required: false,
        },
    })
    .unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(plan, lock).unwrap();
    ReviewedControlOperation::new(
        envelope,
        None,
        None,
        Vec::new(),
        expected_generation,
        expected_capability_generation,
        reviewed_at_ms,
    )
    .unwrap()
}

fn reviewed_uninstall(
    operation_id: &str,
    lock: PluginPackageLock,
    prior: &InstallationSnapshot,
    expected_generation: u64,
    expected_capability_generation: u64,
) -> ReviewedControlOperation {
    let mut packages = lock
        .packages
        .iter()
        .map(|package| {
            let selection = prior.package_selection(package.package_id()).unwrap();
            let role = if package.package_id() == lock.root_package_id {
                PlanPackageRole::Root
            } else {
                PlanPackageRole::Dependency
            };
            if role == PlanPackageRole::Root {
                package
                    .catalog
                    .remove_transition(role, &selection.selected_surfaces)
            } else {
                let state = package
                    .catalog
                    .selected_state(&selection.selected_surfaces)
                    .unwrap();
                PlannedPackageTransition::resolved(
                    package.package_id(),
                    role,
                    PlanPackageChangeKind::Retain,
                    Some(state.clone()),
                    Some(state),
                    None,
                )
            }
        })
        .collect::<a3s_use_core::UseResult<Vec<_>>>()
        .unwrap();
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let reviewed_at_ms = 100 + expected_generation * 100;
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Uninstall,
        lock.root_package_id.clone(),
        "runtime:local",
        packages,
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: 0,
            installed_bytes_after: 0,
            reclaimed_bytes: lock
                .package(&lock.root_package_id)
                .unwrap()
                .catalog
                .record
                .package
                .expanded_bytes,
            drain_required: false,
            retained_data: true,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: expected_generation + 1,
            capability_generation: expected_capability_generation,
            receipt_digest: Some(digest('9')),
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: operation_id.to_string(),
        created_at_ms: reviewed_at_ms - 1,
        expires_at_ms: reviewed_at_ms + 100,
        scope: PlanScope::new(InstallationKind::User, "current").unwrap(),
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Allow,
            policy_digest: digest('9'),
            confirmation_required: false,
        },
    })
    .unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(plan, lock).unwrap();
    ReviewedControlOperation::new(
        envelope,
        None,
        None,
        Vec::new(),
        expected_generation,
        expected_capability_generation,
        reviewed_at_ms,
    )
    .unwrap()
}

fn verified_record(
    package_id: &str,
    dependencies: Vec<PluginPackageDependency>,
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(OKF_CATALOG).unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    record.package_id = package_id.to_string();
    record.publisher = publisher.to_string();
    record.display_name = format!("{publisher} {name}");
    record.description = format!("Projection fixture for {package_id}.");
    record.repository = format!("https://github.com/{publisher}/{name}");
    record.archive.target_name = format!(
        "extensions/{package_id}/1.0.0/stable/linux-x86_64/{publisher}-{name}-1.0.0-linux-x86_64.tar.gz"
    );
    record.archive.sha256 = digest(seed);
    record.package.sha256 = Some(digest(seed));
    record.package.manifest_sha256 = Some(digest(seed));
    record.dependencies = dependencies;
    record.availability = CatalogAvailability::Available;
    record.validate().unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: digest('f'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn test_host() -> PluginPackageLockHost {
    PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap()
}
