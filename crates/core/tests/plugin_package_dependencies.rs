use a3s_use_core::{
    CatalogAvailability, PlanActor, PlanAuthority, PlanPackageChangeKind, PlanPackageRole,
    PlanPolicyDecision, PlanScope, PlanScopeKind, PlannedOperationImpact, PlannedPackageTransition,
    PlannedStateEvidence, PluginCatalogRecord, PluginOperationAction, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginPackageDependency,
    PluginPackageLock, PluginPackageLockHost, PluginPackageResolver, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V2, PLUGIN_CATALOG_SCHEMA_V3,
    PLUGIN_OPERATION_PLAN_SCHEMA_V3, PLUGIN_PACKAGE_LOCK_SCHEMA,
};

const CATALOG: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-okf-v3.json");

fn dependency(package_id: &str, version_requirement: &str) -> PluginPackageDependency {
    PluginPackageDependency::new(package_id, version_requirement).unwrap()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

fn verified_record(
    package_id: &str,
    version: &str,
    dependencies: Vec<PluginPackageDependency>,
    registry_name: &str,
    registry_url: &str,
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    record.package_id = package_id.to_string();
    record.publisher = publisher.to_string();
    record.display_name = format!("{publisher} {name}");
    record.description = format!("Cognitive package fixture for {package_id}.");
    record.version = version.to_string();
    record.repository = format!("https://github.com/{publisher}/{name}");
    record.archive.target_name = format!(
        "extensions/{package_id}/{version}/stable/linux-x86_64/{publisher}-{name}-{version}.tar.gz"
    );
    record.archive.sha256 = digest(seed);
    record.package.sha256 = Some(digest(seed));
    record.package.manifest_sha256 = Some(digest(seed));
    record.dependencies = dependencies;
    record.availability = CatalogAvailability::Available;
    record.validate().unwrap();

    let provenance = VerifiedCatalogProvenance {
        registry_name: registry_name.to_string(),
        registry_url: registry_url.to_string(),
        root_sha256: digest('f'),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn host() -> PluginPackageLockHost {
    PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap()
}

fn install_plan(lock: &PluginPackageLock) -> a3s_use_core::PluginOperationPlan {
    let mut packages = lock
        .packages
        .iter()
        .map(|package| {
            package.catalog.install_transition(
                if package.package_id() == lock.root_package_id {
                    PlanPackageRole::Root
                } else {
                    PlanPackageRole::Dependency
                },
                &[],
            )
        })
        .collect::<a3s_use_core::UseResult<Vec<_>>>()
        .unwrap();
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let impact = PlannedOperationImpact {
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
    };
    PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        lock.root_package_id.clone(),
        "runtime:local",
        packages,
        Vec::new(),
        Vec::new(),
        impact,
        PlannedStateEvidence {
            state_revision: 1,
            capability_generation: 1,
            receipt_digest: None,
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: "install:acme-root:lock-1".to_string(),
        created_at_ms: 1,
        expires_at_ms: 2,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "current".to_string(),
        },
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('9'),
            confirmation_required: true,
        },
    })
    .unwrap()
}

#[test]
fn package_dependencies_require_canonical_semver_and_sorted_unique_ids() {
    let first = dependency("acme/base", ">=1.0.0, <2.0.0");
    let second = dependency("acme/knowledge", "^2.1.0");
    PluginPackageDependency::validate_set("acme/root", &[first.clone(), second.clone()]).unwrap();
    assert!(first.matches("1.9.9").unwrap());
    assert!(!first.matches("2.0.0").unwrap());

    assert_eq!(
        PluginPackageDependency::new("acme/base", "1.0.0")
            .unwrap_err()
            .code,
        "use.plugin.package_dependency_invalid"
    );
    assert_eq!(
        PluginPackageDependency::validate_set("acme/root", &[second, first])
            .unwrap_err()
            .code,
        "use.plugin.package_dependency_invalid"
    );
    assert_eq!(
        PluginPackageDependency::validate_set("acme/root", &[dependency("acme/root", "^1.0.0")],)
            .unwrap_err()
            .code,
        "use.plugin.package_dependency_invalid"
    );
}

#[test]
fn only_catalog_v3_may_publish_package_dependencies() {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    record.dependencies = vec![dependency("acme/base", "^1.0.0")];
    record.validate().unwrap();

    record.schema = PLUGIN_CATALOG_SCHEMA_V2.to_string();
    assert_eq!(
        record.validate().unwrap_err().code,
        "use.plugin.catalog_invalid"
    );
}

#[test]
fn resolver_selects_the_highest_compatible_transitive_closure() {
    let root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/base", ">=1.0.0, <2.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let base_old = verified_record(
        "acme/base",
        "1.0.0",
        vec![dependency("acme/leaf", "^2.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    let base_new = verified_record(
        "acme/base",
        "1.5.0",
        vec![dependency("acme/leaf", "^2.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'c',
    );
    let leaf_old = verified_record(
        "acme/leaf",
        "2.0.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'd',
    );
    let leaf_new = verified_record(
        "acme/leaf",
        "2.3.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'e',
    );

    let lock = PluginPackageResolver::new(host())
        .resolve(root, vec![leaf_old, base_old, leaf_new, base_new])
        .unwrap();
    assert_eq!(lock.schema, PLUGIN_PACKAGE_LOCK_SCHEMA);
    assert_eq!(lock.root_package_id, "acme/root");
    assert_eq!(lock.packages.len(), 3);
    assert_eq!(lock.package("acme/base").unwrap().version(), "1.5.0");
    assert_eq!(lock.package("acme/leaf").unwrap().version(), "2.3.0");
    assert_eq!(
        lock.install_order()
            .unwrap()
            .into_iter()
            .map(|package| package.package_id())
            .collect::<Vec<_>>(),
        ["acme/leaf", "acme/base", "acme/root"]
    );
    assert_eq!(
        lock.removal_order()
            .unwrap()
            .into_iter()
            .map(|package| package.package_id())
            .collect::<Vec<_>>(),
        ["acme/root", "acme/base", "acme/leaf"]
    );

    let canonical = lock.canonical_bytes().unwrap();
    let reparsed = PluginPackageLock::from_json(&canonical).unwrap();
    assert_eq!(reparsed, lock);
    assert_eq!(
        reparsed.descriptor_digest().unwrap(),
        lock.descriptor_digest().unwrap()
    );
}

#[test]
fn operation_envelope_binds_the_complete_lock_and_rejects_drift() {
    let root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/base", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let base = verified_record(
        "acme/base",
        "1.2.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    let lock = PluginPackageResolver::new(host())
        .resolve(root, vec![base])
        .unwrap();
    let envelope =
        PluginOperationPlanEnvelope::new_with_package_lock(install_plan(&lock), lock.clone())
            .unwrap();
    envelope.validate().unwrap();
    assert_eq!(
        envelope.plan.package_lock_digest.as_deref(),
        Some(lock.descriptor_digest().unwrap().as_str())
    );

    let mut missing_lock = envelope.clone();
    missing_lock.package_lock = None;
    assert_eq!(
        missing_lock.validate().unwrap_err().code,
        "use.plugin.plan_invalid"
    );

    let mut drifted = envelope;
    drifted.package_lock.as_mut().unwrap().root_package_id = "acme/base".to_string();
    assert_eq!(
        drifted.validate().unwrap_err().code,
        "use.plugin.package_lock_invalid"
    );
}

#[test]
fn upgrade_envelope_binds_the_prior_candidate_union_and_removed_nodes() {
    let base = verified_record(
        "acme/base",
        "1.0.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    let obsolete = verified_record(
        "acme/obsolete",
        "1.0.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'd',
    );
    let prior_root = verified_record(
        "acme/root",
        "1.0.0",
        vec![
            dependency("acme/base", "^1.0.0"),
            dependency("acme/obsolete", "^1.0.0"),
        ],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let candidate_root = verified_record(
        "acme/root",
        "1.1.0",
        vec![dependency("acme/base", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'c',
    );
    let prior = PluginPackageResolver::new(host())
        .resolve(prior_root, vec![base.clone(), obsolete.clone()])
        .unwrap();
    let candidate = PluginPackageResolver::new(host())
        .resolve(candidate_root, vec![base])
        .unwrap();

    let retained = prior.package("acme/base").unwrap();
    let retained_state = retained.catalog.selected_state(&[]).unwrap();
    let mut packages = vec![
        PlannedPackageTransition::resolved(
            "acme/base",
            PlanPackageRole::Dependency,
            PlanPackageChangeKind::Retain,
            Some(retained_state.clone()),
            Some(retained_state),
            None,
        )
        .unwrap(),
        prior
            .package("acme/obsolete")
            .unwrap()
            .catalog
            .remove_transition(PlanPackageRole::Dependency, &[])
            .unwrap(),
        candidate
            .package("acme/root")
            .unwrap()
            .catalog
            .replace_transition(
                &prior.package("acme/root").unwrap().catalog,
                PlanPackageRole::Root,
                &[],
                &[],
            )
            .unwrap(),
    ];
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Upgrade,
        "acme/root",
        "runtime:local",
        packages,
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: candidate
                .package("acme/root")
                .unwrap()
                .catalog
                .record
                .archive
                .length,
            installed_bytes_after: candidate
                .packages
                .iter()
                .map(|package| package.catalog.record.package.expanded_bytes)
                .sum(),
            reclaimed_bytes: prior
                .packages
                .iter()
                .filter(|package| package.package_id() != "acme/base")
                .map(|package| package.catalog.record.package.expanded_bytes)
                .sum(),
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 2,
            capability_generation: 2,
            receipt_digest: Some(digest('9')),
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: "upgrade:acme-root:graph-gc-1".to_string(),
        created_at_ms: 1,
        expires_at_ms: 2,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "current".to_string(),
        },
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('8'),
            confirmation_required: true,
        },
    })
    .unwrap();
    let envelope = PluginOperationPlanEnvelope::new_with_upgrade_package_locks(
        plan,
        prior.clone(),
        candidate.clone(),
    )
    .unwrap();

    assert_eq!(envelope.plan.schema, PLUGIN_OPERATION_PLAN_SCHEMA_V3);
    assert_eq!(
        envelope.plan.prior_package_lock_digest.as_deref(),
        Some(prior.descriptor_digest().unwrap().as_str())
    );
    assert_eq!(
        envelope.plan.package_lock_digest.as_deref(),
        Some(candidate.descriptor_digest().unwrap().as_str())
    );
    assert_eq!(
        envelope
            .plan
            .packages
            .iter()
            .find(|package| package.package_id == "acme/obsolete")
            .unwrap()
            .change,
        PlanPackageChangeKind::Remove
    );
    envelope.validate().unwrap();

    let mut missing_prior = envelope.clone();
    missing_prior.prior_package_lock = None;
    assert_eq!(
        missing_prior.validate().unwrap_err().code,
        "use.plugin.plan_invalid"
    );

    let mut drifted_prior = envelope;
    drifted_prior
        .prior_package_lock
        .as_mut()
        .unwrap()
        .packages
        .pop();
    assert!(drifted_prior.validate().is_err());
}

#[test]
fn resolver_backtracks_when_the_highest_version_conflicts_later() {
    let root = verified_record(
        "acme/root",
        "1.0.0",
        vec![
            dependency("acme/alpha", ">=1.0.0, <3.0.0"),
            dependency("acme/beta", "^1.0.0"),
        ],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let alpha_v2 = verified_record(
        "acme/alpha",
        "2.0.0",
        vec![dependency("acme/shared", "^2.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    let alpha_v1 = verified_record(
        "acme/alpha",
        "1.5.0",
        vec![dependency("acme/shared", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'c',
    );
    let beta = verified_record(
        "acme/beta",
        "1.0.0",
        vec![dependency("acme/shared", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'd',
    );
    let shared_v2 = verified_record(
        "acme/shared",
        "2.0.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'e',
    );
    let shared_v1 = verified_record(
        "acme/shared",
        "1.9.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        '6',
    );

    let lock = PluginPackageResolver::new(host())
        .resolve(root, vec![alpha_v2, shared_v2, beta, shared_v1, alpha_v1])
        .unwrap();
    assert_eq!(lock.package("acme/alpha").unwrap().version(), "1.5.0");
    assert_eq!(lock.package("acme/shared").unwrap().version(), "1.9.0");
}

#[test]
fn resolver_rejects_missing_conflicting_cyclic_and_ambiguous_graphs() {
    let missing_root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/missing", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    assert_eq!(
        PluginPackageResolver::new(host())
            .resolve(missing_root, vec![])
            .unwrap_err()
            .code,
        "use.plugin.package_dependency_missing"
    );

    let conflict_root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/base", "^2.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let base_v1 = verified_record(
        "acme/base",
        "1.0.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    assert_eq!(
        PluginPackageResolver::new(host())
            .resolve(conflict_root, vec![base_v1])
            .unwrap_err()
            .code,
        "use.plugin.package_dependency_conflict"
    );

    let cycle_root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/base", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let cycle_base = verified_record(
        "acme/base",
        "1.0.0",
        vec![dependency("acme/root", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    assert_eq!(
        PluginPackageResolver::new(host())
            .resolve(cycle_root, vec![cycle_base])
            .unwrap_err()
            .code,
        "use.plugin.package_dependency_cycle"
    );

    let ambiguous_root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/base", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let official = verified_record(
        "acme/base",
        "1.0.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    let private = verified_record(
        "acme/base",
        "1.1.0",
        vec![],
        "private",
        "https://private.example.test/catalog/",
        'c',
    );
    assert_eq!(
        PluginPackageResolver::new(host())
            .resolve(ambiguous_root, vec![official, private])
            .unwrap_err()
            .code,
        "use.plugin.package_registry_ambiguous"
    );
}

#[test]
fn lock_validation_rejects_edges_that_do_not_match_signed_dependencies() {
    let root = verified_record(
        "acme/root",
        "1.0.0",
        vec![dependency("acme/base", "^1.0.0")],
        "official",
        "https://packages.example.test/catalog/",
        'a',
    );
    let base = verified_record(
        "acme/base",
        "1.1.0",
        vec![],
        "official",
        "https://packages.example.test/catalog/",
        'b',
    );
    let lock = PluginPackageResolver::new(host())
        .resolve(root, vec![base])
        .unwrap();
    let mut value = serde_json::to_value(lock).unwrap();
    value["packages"][1]["dependencies"][0]["version"] = serde_json::json!("1.0.0");
    assert_eq!(
        PluginPackageLock::from_json(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.package_lock_invalid"
    );
}
