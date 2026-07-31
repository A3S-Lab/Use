use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use a3s_runtime::RuntimeClientRegistry;

#[path = "authority_resolver_test_support.rs"]
mod support;

use super::test_support::{capabilities, FakeRuntime};
use super::tests::runtime_bundle_inputs;
use super::*;
use support::*;

#[tokio::test]
async fn resolver_binds_exact_identity_provider_and_provider_specific_references() {
    let (bundle, package) = authority_package();
    let assignments = assignments(&bundle, PROVIDER_ID);
    let resolver = Arc::new(ExactResolver::new(PROVIDER_ID));
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry.register(resolver.clone()).unwrap();

    let first = registry
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &assignments)
        .await
        .unwrap();
    let second = registry
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &assignments)
        .await
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(first.surfaces()[0].provider_id().as_str(), PROVIDER_ID);
    assert!(!format!("{first:?}").contains("018f47e8-34ce"));
    let requests = resolver.requests();
    assert_eq!(requests.len(), 2);
    let request = &requests[0];
    assert_eq!(request.scope_id(), "workspace-01");
    assert_eq!(request.package_id(), bundle.package_id);
    assert_eq!(request.package_digest(), bundle.package_sha256);
    assert_eq!(
        request.permission_ceiling_digest(),
        bundle.permission_ceiling_digest
    );
    assert_eq!(
        request.permissions_digest(),
        package.permissions.descriptor_digest().unwrap()
    );
    assert_eq!(request.generation(), 8);
    assert_eq!(request.provider_id().as_str(), PROVIDER_ID);
    assert_eq!(request.surface(), assignments[0].surface());
    assert_eq!(
        request.filesystem(),
        package.permissions.surfaces[0].filesystem
    );
    assert_eq!(
        request.secret_names(),
        package.permissions.surfaces[0].secrets
    );
    assert_eq!(
        request.ephemeral_storage_limit_bytes(),
        package.permissions.surfaces[0]
            .resources
            .as_ref()
            .unwrap()
            .ephemeral_storage_bytes
    );

    let proposal = authority_proposal(&package);
    let plans =
        plan_runtime_bundle_with_authority(&bundle, &package, &proposal, &first, &assignments, 8)
            .unwrap();
    assert_eq!(
        plans[0].spec().secrets[0].reference,
        "a3s-cloud-secret://018f47e8-34ce-7f2b-9460-71ad3fdbb546/018f47e8-34ce-7f2b-9460-71ad3fdbb547/7"
    );
}

#[tokio::test]
async fn missing_resolver_and_assignment_errors_fail_without_fallback_or_calls() {
    let (bundle, package) = authority_package();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry
        .register(Arc::new(CountingResolver::new(
            "other-runtime",
            calls.clone(),
        )))
        .unwrap();

    let missing = registry
        .resolve_bindings(
            &bundle,
            &package,
            "workspace-01",
            8,
            &assignments(&bundle, PROVIDER_ID),
        )
        .await
        .unwrap_err();
    let duplicate_assignments = vec![
        RuntimeProviderAssignment::new(qualified_surface(&bundle), "other-runtime").unwrap(),
        RuntimeProviderAssignment::new(qualified_surface(&bundle), "other-runtime").unwrap(),
    ];
    let duplicate = registry
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &duplicate_assignments)
        .await
        .unwrap_err();

    assert_eq!(
        missing.code,
        "use.plugin.runtime.authority_resolver_unavailable"
    );
    assert_eq!(
        duplicate.code,
        "use.plugin.runtime.provider_assignment_invalid"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn resolver_failure_and_timeout_are_bounded_and_redacted() {
    let (bundle, package) = authority_package();
    let assignments = assignments(&bundle, PROVIDER_ID);
    let mut failing = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    failing
        .register(Arc::new(FailingResolver::new(PROVIDER_ID)))
        .unwrap();
    let mut hanging = RuntimeAuthorityResolverRegistry::new(Duration::from_millis(10)).unwrap();
    hanging
        .register(Arc::new(HangingResolver::new(PROVIDER_ID)))
        .unwrap();

    let failure = failing
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &assignments)
        .await
        .unwrap_err();
    let timeout = hanging
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &assignments)
        .await
        .unwrap_err();

    let failure_debug = format!("{failure:?}");
    assert_eq!(
        failure.code,
        "use.plugin.runtime.authority_resolution_failed"
    );
    assert!(!failure.message.contains("never-print-this"));
    assert!(!failure_debug.contains("never-print-this"));
    assert_eq!(
        timeout.code,
        "use.plugin.runtime.authority_resolution_timeout"
    );
}

#[tokio::test]
async fn invalid_resolver_output_fails_exact_coverage_validation() {
    let (bundle, package) = authority_package();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry
        .register(Arc::new(CountingResolver::new(PROVIDER_ID, calls.clone())))
        .unwrap();

    let error = registry
        .resolve_bindings(
            &bundle,
            &package,
            "workspace-01",
            8,
            &assignments(&bundle, PROVIDER_ID),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.plugin.runtime.authority_resolution_invalid"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mismatched_package_surface_inventory_is_rejected_before_resolution() {
    let (bundle, mut package) = authority_package();
    package.release.surfaces[0].id = "different-tool".to_string();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry
        .register(Arc::new(CountingResolver::new(PROVIDER_ID, calls.clone())))
        .unwrap();

    let error = registry
        .resolve_bindings(
            &bundle,
            &package,
            "workspace-01",
            8,
            &assignments(&bundle, PROVIDER_ID),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.bundle_plan_invalid");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_bound_authority_cannot_be_reused_with_another_assignment() {
    let (bundle, package) = authority_package();
    let exact_assignments = assignments(&bundle, PROVIDER_ID);
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry
        .register(Arc::new(ExactResolver::new(PROVIDER_ID)))
        .unwrap();
    let bindings = registry
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &exact_assignments)
        .await
        .unwrap();
    let proposal = authority_proposal(&package);
    let wrong = assignments(&bundle, "other-runtime");

    let error =
        plan_runtime_bundle_with_authority(&bundle, &package, &proposal, &bindings, &wrong, 8)
            .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.authority_binding_invalid");
}

#[tokio::test]
async fn broker_resolves_and_retains_authority_across_both_provider_passes() {
    let (bundle, package) = authority_package();
    let proposal = authority_proposal(&package);
    let assignments = assignments(&bundle, PROVIDER_ID);
    let authority_resolver = Arc::new(ExactResolver::new(PROVIDER_ID));
    let mut authority_registry =
        RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    authority_registry
        .register(authority_resolver.clone())
        .unwrap();
    let bindings = authority_registry
        .resolve_bindings(&bundle, &package, "workspace-01", 8, &assignments)
        .await
        .unwrap();
    let plans = plan_runtime_bundle_with_authority(
        &bundle,
        &package,
        &proposal,
        &bindings,
        &assignments,
        8,
    )
    .unwrap();
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plans[0]), true));
    let mut runtime_registry = RuntimeClientRegistry::new();
    runtime_registry
        .register(Arc::new(StaticRuntimeFactory::new(PROVIDER_ID, runtime)))
        .unwrap();

    let selected = PluginRuntimeBroker::new(&runtime_registry)
        .preflight_bundle_with_resolvers(
            bundle,
            package,
            "workspace-01",
            8,
            &authority_registry,
            assignments,
        )
        .await
        .unwrap()
        .authorize(&proposal)
        .await
        .unwrap();

    assert_eq!(selected.surfaces()[0].plan().spec().mounts.len(), 3);
    assert_eq!(selected.surfaces()[0].plan().spec().secrets.len(), 1);
    assert_eq!(authority_resolver.requests().len(), 2);
}

#[tokio::test]
async fn authority_free_packages_do_not_require_or_invoke_a_resolver() {
    let (bundle, package, _) = runtime_bundle_inputs(false);
    let registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();

    let bindings = registry
        .resolve_bindings(
            &bundle,
            &package,
            "workspace-01",
            8,
            &assignments(&bundle, PROVIDER_ID),
        )
        .await
        .unwrap();

    assert!(bindings.surfaces().is_empty());
}

#[tokio::test]
async fn unselected_planning_bundle_surfaces_do_not_require_assignments_or_resolution() {
    let (mut bundle, package) = authority_package();
    let mut unselected = bundle.surfaces[0].clone();
    match &mut unselected {
        a3s_use_core::ExecutablePlanningSurface::ToolService { id, .. } => {
            *id = "optional-index".to_string();
        }
        _ => panic!("test fixture must remain a Tool Service"),
    }
    bundle.surfaces.push(unselected);
    bundle.validate().unwrap();
    let resolver = Arc::new(ExactResolver::new(PROVIDER_ID));
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry.register(resolver.clone()).unwrap();

    let bindings = registry
        .resolve_bindings(
            &bundle,
            &package,
            "workspace-01",
            8,
            &assignments(&bundle, PROVIDER_ID),
        )
        .await
        .unwrap();

    assert_eq!(bindings.surfaces().len(), 1);
    assert_eq!(resolver.requests().len(), 1);
}

#[test]
fn resolver_registry_rejects_unbounded_timeouts_and_duplicate_providers() {
    assert!(RuntimeAuthorityResolverRegistry::new(Duration::ZERO).is_err());
    assert!(RuntimeAuthorityResolverRegistry::new(Duration::from_secs(61)).is_err());
    let mut registry = RuntimeAuthorityResolverRegistry::new(Duration::from_secs(1)).unwrap();
    registry
        .register(Arc::new(ExactResolver::new(PROVIDER_ID)))
        .unwrap();

    let duplicate = registry
        .register(Arc::new(ExactResolver::new(PROVIDER_ID)))
        .unwrap_err();

    assert_eq!(
        duplicate.code,
        "use.plugin.runtime.authority_resolver_invalid"
    );
}

#[test]
fn authority_resolver_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeAuthorityResolutionRequest>();
    assert_send_sync::<ResolvedRuntimeSurfaceAuthority>();
    assert_send_sync::<RuntimeAuthorityResolverRegistry>();
}
