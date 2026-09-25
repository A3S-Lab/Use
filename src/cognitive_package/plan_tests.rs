use a3s_use_core::{
    PlanActor, PlanAuthority, PlanEnforcementProfile, PlanPolicyDecision, PlanScopeKind,
    PluginCatalogRecord, PluginPackageLockHost, PluginPackageResolver,
    VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
};

use super::*;
use crate::cognitive_package::ReviewedCognitivePackageAuthorizationProvider;

#[test]
fn install_plan_omits_an_unselected_independent_optional_surface() {
    let mut record = PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-v3.json"
    ))
    .unwrap();
    let optional = record
        .surfaces
        .iter_mut()
        .find(|surface| surface.kind == PluginSurfaceKind::Ui)
        .unwrap();
    optional.optional = true;
    let omitted = optional.reference();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: test_digest('f'),
        root_version: 1,
        timestamp_version: 4,
        snapshot_version: 3,
        targets_version: 2,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    let lock = PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap();
    let root = lock.package(&lock.root_package_id).unwrap();
    let selected = root
        .catalog
        .record
        .resolve_surfaces(&[])
        .unwrap()
        .into_iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    assert!(!selected.contains(&omitted));
    let dispositions =
        BTreeMap::from([(lock.root_package_id.clone(), InstallDisposition::Add)]);
    let selections = BTreeMap::from([(lock.root_package_id.clone(), selected.clone())]);

    let transitions = install_plan_packages(&lock, &dispositions, &selections).unwrap();

    let planned = &transitions[0].after.as_ref().unwrap().release.surfaces;
    assert_eq!(
        planned
            .iter()
            .map(a3s_use_core::CatalogSurface::reference)
            .collect::<Vec<_>>(),
        selected
    );
}

#[test]
fn reviewed_host_provider_evidence_preserves_managed_surfaces_and_locks_native_launchers() {
    let (lock, manifests, dispositions) = managed_package_graph();
    let surface_selections = all_surface_selections(&lock);
    let transitions = install_plan_packages(&lock, &dispositions, &surface_selections).unwrap();
    let providers = reviewed_providers(&transitions, &manifests);
    let reviewed = reviewed_authorization(&lock, transitions, providers.clone());

    let actual = operation_provider_evidence(&lock.packages, &manifests, &reviewed).unwrap();

    assert_eq!(actual, providers);
    assert_eq!(
        actual
            .iter()
            .filter(|provider| provider.provider_id == "managed-runtime")
            .count(),
        2
    );
    assert_eq!(
        actual
            .iter()
            .filter(|provider| provider.provider_id == "a3s-use-native-launcher")
            .count(),
        1
    );
}

#[test]
fn reviewed_host_cannot_replace_a_package_native_launcher() {
    let (lock, manifests, dispositions) = managed_package_graph();
    let surface_selections = all_surface_selections(&lock);
    let transitions = install_plan_packages(&lock, &dispositions, &surface_selections).unwrap();
    let mut providers = reviewed_providers(&transitions, &manifests);
    let native = providers
        .iter_mut()
        .find(|provider| provider.provider_id == "a3s-use-native-launcher")
        .unwrap();
    native.provider_id = "unreviewed-native".to_string();
    native.provider_build_id = "build-1".to_string();
    native.capability_digest = test_digest('c');
    native.semantics_profile_digest = test_digest('d');
    let reviewed = reviewed_authorization(&lock, transitions, providers);

    let error = operation_provider_evidence(&lock.packages, &manifests, &reviewed).unwrap_err();

    assert_eq!(error.code, "use.plugin.package_provider_invalid");
}

fn managed_package_graph() -> (
    PluginPackageLock,
    BTreeMap<String, ExtensionManifest>,
    BTreeMap<String, InstallDisposition>,
) {
    let record = PluginCatalogRecord::from_json(include_bytes!(
        "../../crates/core/fixtures/plugins/catalog-record-v3.json"
    ))
    .unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_string(),
        registry_url: "https://packages.example.test/a3s/".to_string(),
        root_sha256: test_digest('f'),
        root_version: 1,
        timestamp_version: 4,
        snapshot_version: 3,
        targets_version: 2,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    let lock = PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap();
    let package_id = lock.root_package_id.clone();
    let manifest = ExtensionManifest::parse_acl(include_str!(
        "../../crates/extension/fixtures/manifests/plugin-v3.acl"
    ))
    .unwrap();
    (
        lock,
        BTreeMap::from([(package_id.clone(), manifest)]),
        BTreeMap::from([(package_id, InstallDisposition::Add)]),
    )
}

fn reviewed_providers(
    transitions: &[PlannedPackageTransition],
    manifests: &BTreeMap<String, ExtensionManifest>,
) -> Vec<PlannedProviderEvidence> {
    let mut providers = Vec::new();
    for transition in transitions {
        let state = transition.after.as_ref().unwrap();
        let manifest = manifests.get(&transition.package_id).unwrap();
        for surface in &state.release.surfaces {
            if !matches!(
                surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            ) {
                continue;
            }
            let qualified = PlanQualifiedSurfaceRef {
                package_id: transition.package_id.clone(),
                surface: surface.reference(),
            };
            if is_static_surface(manifest, surface.kind, &surface.id) {
                providers.push(
                    native_provider_evidence(qualified, &state.release.package_sha256).unwrap(),
                );
            } else {
                providers.push(PlannedProviderEvidence {
                    surface: qualified,
                    provider_id: "managed-runtime".to_string(),
                    provider_build_id: "build-1".to_string(),
                    capability_digest: test_digest('a'),
                    semantics_profile_digest: test_digest('b'),
                    enforcement: PlanEnforcementProfile::Container,
                });
            }
        }
    }
    providers.sort_by(|left, right| left.surface.cmp(&right.surface));
    providers
}

fn reviewed_authorization(
    lock: &PluginPackageLock,
    transitions: Vec<PlannedPackageTransition>,
    providers: Vec<PlannedProviderEvidence>,
) -> ReviewedCognitivePackageAuthorizationProvider {
    let mut draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        lock.root_package_id.clone(),
        format!("use/{}", lock.root_package_id),
        transitions,
        providers,
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: lock.packages[0].catalog.record.archive.length,
            installed_bytes_after: lock.packages[0].catalog.record.package.expanded_bytes,
            reclaimed_bytes: 0,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 2,
            capability_generation: 1,
            receipt_digest: None,
        },
    )
    .unwrap();
    draft.package_lock_digest = Some(lock.descriptor_digest().unwrap());
    let plan = draft
        .bind(PluginOperationPlanBinding {
            operation_id: "install:managed-runtime".to_string(),
            created_at_ms: 100,
            expires_at_ms: 200,
            scope: PlanScope {
                kind: PlanScopeKind::User,
                id: "current".to_string(),
            },
            authority: PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Allow,
                policy_digest: test_digest('e'),
                confirmation_required: false,
            },
        })
        .unwrap();
    let envelope =
        PluginOperationPlanEnvelope::new_with_package_lock(plan, lock.clone()).unwrap();
    ReviewedCognitivePackageAuthorizationProvider::new(envelope, None).unwrap()
}

fn test_digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
