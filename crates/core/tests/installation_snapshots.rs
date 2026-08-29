use a3s_use_core::{
    CatalogAvailability, InstallationId, InstallationKind, InstallationRootSelection,
    InstallationSnapshot, PluginCatalogRecord, PluginPackageDependency, PluginPackageLock,
    PluginPackageLockHost, PluginPackageResolver, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, INSTALLATION_SNAPSHOT_SCHEMA, MAX_INSTALLATION_ROOTS,
    PLUGIN_CATALOG_SCHEMA_V3,
};

const CATALOG: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-okf-v3.json");

#[test]
fn one_snapshot_merges_shared_packages_and_reconstructs_each_root_lock() {
    let shared = verified_record("acme/shared", "1.0.0", Vec::new(), 'c');
    let first = resolved_lock(
        verified_record(
            "acme/first",
            "1.0.0",
            vec![dependency("acme/shared", "^1.0.0")],
            '1',
        ),
        vec![shared.clone()],
    );
    let second = resolved_lock(
        verified_record(
            "acme/second",
            "2.0.0",
            vec![dependency("acme/shared", "^1.0.0")],
            '2',
        ),
        vec![shared],
    );
    let installation = installation();

    let snapshot = InstallationSnapshot::from_root_locks(
        installation.clone(),
        7,
        host(),
        vec![
            (
                InstallationRootSelection::new("acme/second", 20).unwrap(),
                second.clone(),
            ),
            (
                InstallationRootSelection::new("acme/first", 10).unwrap(),
                first.clone(),
            ),
        ],
    )
    .unwrap();

    assert_eq!(snapshot.schema, INSTALLATION_SNAPSHOT_SCHEMA);
    assert_eq!(snapshot.installation, installation);
    assert_eq!(snapshot.generation, 7);
    assert_eq!(
        snapshot
            .roots
            .iter()
            .map(|root| root.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["acme/first", "acme/second"]
    );
    assert_eq!(
        snapshot
            .packages
            .iter()
            .map(|package| package.package_id())
            .collect::<Vec<_>>(),
        vec!["acme/first", "acme/second", "acme/shared"]
    );
    assert_eq!(snapshot.package_lock("acme/first").unwrap(), Some(first));
    assert_eq!(snapshot.package_lock("acme/second").unwrap(), Some(second));
    assert_eq!(snapshot.package_lock("acme/missing").unwrap(), None);
}

#[test]
fn snapshot_rejects_conflicting_shared_package_selections() {
    let first = resolved_lock(
        verified_record(
            "acme/first",
            "1.0.0",
            vec![dependency("acme/shared", "^1.0.0")],
            '1',
        ),
        vec![verified_record("acme/shared", "1.0.0", Vec::new(), '3')],
    );
    let second = resolved_lock(
        verified_record(
            "acme/second",
            "1.0.0",
            vec![dependency("acme/shared", "^2.0.0")],
            '2',
        ),
        vec![verified_record("acme/shared", "2.0.0", Vec::new(), '4')],
    );

    let error = InstallationSnapshot::from_root_locks(
        installation(),
        1,
        host(),
        vec![
            (
                InstallationRootSelection::new("acme/first", 10).unwrap(),
                first,
            ),
            (
                InstallationRootSelection::new("acme/second", 11).unwrap(),
                second,
            ),
        ],
    )
    .unwrap_err();

    assert_eq!(error.code, "use.installation.snapshot_invalid");
}

#[test]
fn snapshot_rejects_orphaned_packages_and_non_monotonic_identity_fields() {
    let lock = resolved_lock(
        verified_record("acme/root", "1.0.0", Vec::new(), 'a'),
        Vec::new(),
    );
    let mut snapshot = InstallationSnapshot::from_root_locks(
        installation(),
        1,
        host(),
        vec![(
            InstallationRootSelection::new("acme/root", 10).unwrap(),
            lock,
        )],
    )
    .unwrap();

    snapshot.packages.push(
        resolved_lock(
            verified_record("acme/orphan", "1.0.0", Vec::new(), 'e'),
            Vec::new(),
        )
        .packages
        .remove(0),
    );
    snapshot
        .packages
        .sort_by(|left, right| left.package_id().cmp(right.package_id()));
    assert_eq!(
        snapshot.validate().unwrap_err().code,
        "use.installation.snapshot_invalid"
    );

    snapshot
        .packages
        .retain(|package| package.package_id() != "acme/orphan");
    snapshot.generation = 0;
    assert_eq!(
        snapshot.validate().unwrap_err().code,
        "use.installation.snapshot_invalid"
    );
}

#[test]
fn empty_snapshot_retains_host_and_round_trips_canonically() {
    let snapshot =
        InstallationSnapshot::from_root_locks(installation(), 9, host(), Vec::new()).unwrap();
    let bytes = snapshot.canonical_bytes().unwrap();
    let parsed = InstallationSnapshot::from_json(&bytes).unwrap();

    assert_eq!(parsed, snapshot);
    assert!(parsed.roots.is_empty());
    assert!(parsed.packages.is_empty());
    assert_eq!(
        parsed.descriptor_digest().unwrap(),
        snapshot.descriptor_digest().unwrap()
    );
    assert_send_sync::<InstallationSnapshot>();
}

#[test]
fn snapshot_constructor_rejects_the_root_bound_before_merging() {
    let lock = resolved_lock(
        verified_record("acme/root", "1.0.0", Vec::new(), 'a'),
        Vec::new(),
    );
    let roots = (0..=MAX_INSTALLATION_ROOTS)
        .map(|index| {
            (
                InstallationRootSelection::new("acme/root", index as u64 + 1).unwrap(),
                lock.clone(),
            )
        })
        .collect();

    let error =
        InstallationSnapshot::from_root_locks(installation(), 1, host(), roots).unwrap_err();
    assert_eq!(error.code, "use.installation.snapshot_invalid");
}

fn assert_send_sync<T: Send + Sync>() {}

fn installation() -> InstallationId {
    InstallationId::new(InstallationKind::Workspace, "workspace-01").unwrap()
}

fn host() -> PluginPackageLockHost {
    PluginPackageLockHost::new("linux-x86_64", "0.3.4").unwrap()
}

fn dependency(package_id: &str, version_requirement: &str) -> PluginPackageDependency {
    PluginPackageDependency::new(package_id, version_requirement).unwrap()
}

fn resolved_lock(
    root: VerifiedPluginCatalogRecord,
    candidates: Vec<VerifiedPluginCatalogRecord>,
) -> PluginPackageLock {
    PluginPackageResolver::new(host())
        .resolve(root, candidates)
        .unwrap()
}

fn verified_record(
    package_id: &str,
    version: &str,
    dependencies: Vec<PluginPackageDependency>,
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    record.package_id = package_id.to_string();
    record.publisher = publisher.to_string();
    record.display_name = format!("{publisher} {name}");
    record.description = format!("Installation snapshot fixture for {package_id}.");
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
        registry_name: "official".to_string(),
        registry_url: "https://packages.example.test/catalog/".to_string(),
        root_sha256: digest('f'),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
