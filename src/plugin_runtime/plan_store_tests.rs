use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};
use std::time::Duration;
use tokio::task::JoinSet;

use super::test_support::*;
use super::*;

#[tokio::test]
async fn plan_store_round_trips_after_reopen_and_exposes_only_key_inventory() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        plan.surface(),
        plan.context().generation(),
        Some(plan.context().grant_digest().to_owned()),
        provider.semantics_profile_digest.clone(),
        provider.provider_id.clone(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let store =
        RuntimeSurfacePlanStore::new(temporary.path(), plan.context().scope().clone()).unwrap();

    assert!(store.put(&key, &plan).await.unwrap());
    assert!(!store.put(&key, &plan).await.unwrap());

    let reopened =
        RuntimeSurfacePlanStore::new(temporary.path(), plan.context().scope().clone()).unwrap();
    let bytes = reopened.read_plan(&key).await.unwrap();
    assert_eq!(
        RuntimeSurfacePlan::from_canonical_bytes(&bytes).unwrap(),
        plan
    );
    assert_eq!(reopened.get(&key).await.unwrap(), Some(plan.clone()));
    assert_eq!(reopened.inspect_keys().await.unwrap(), vec![key.clone()]);
    assert!(reopened.root().ends_with("runtime-plans"));
    assert!(!reopened.root().to_string_lossy().contains("acme"));
}

#[tokio::test]
async fn batch_publication_is_bounded_deterministic_and_idempotent() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        plan.surface(),
        plan.context().generation(),
        Some(plan.context().grant_digest().to_owned()),
        provider.semantics_profile_digest.clone(),
        provider.provider_id.clone(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let publication = RuntimeSurfacePlanPublication::new(key.clone(), plan.clone()).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let store =
        RuntimeSurfacePlanStore::new(temporary.path(), plan.context().scope().clone()).unwrap();

    assert_eq!(
        store
            .publish(std::slice::from_ref(&publication))
            .await
            .unwrap(),
        RuntimeSurfacePlanPublishResult {
            published: 1,
            existing: 0,
        }
    );
    assert_eq!(
        store
            .publish(std::slice::from_ref(&publication))
            .await
            .unwrap(),
        RuntimeSurfacePlanPublishResult {
            published: 0,
            existing: 1,
        }
    );
    assert_eq!(
        store.publish(&[]).await.unwrap(),
        RuntimeSurfacePlanPublishResult {
            published: 0,
            existing: 0,
        }
    );
    let error = store
        .publish(&[publication.clone(), publication])
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.plan_store_invalid");
    assert_eq!(store.inspect_keys().await.unwrap(), vec![key]);
}

#[tokio::test]
async fn extension_path_publication_waits_for_global_collection_boundary() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::from_plan(&plan, &provider).unwrap();
    let publication = RuntimeSurfacePlanPublication::new(key, plan).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let paths = ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        publication.key.scope.clone(),
    )
    .unwrap();
    let store = RuntimeSurfacePlanStore::from_extension_paths(&paths);
    let collection = paths.artifact_store().acquire_collection().await.unwrap();

    // The publication task must remain pending while the collector owns the
    // exclusive side of the global reference boundary. This proves the
    // ExtensionPaths constructor cannot accidentally bypass the outer lock.
    let mut task = tokio::spawn({
        let store = store.clone();
        async move { store.publish(&[publication]).await }
    });
    tokio::task::yield_now().await;
    assert!(tokio::time::timeout(Duration::from_millis(100), &mut task)
        .await
        .is_err());

    drop(collection);
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.published, 1);
    assert_eq!(result.existing, 0);
}

#[tokio::test]
async fn concurrent_publishers_converge_without_replacing_immutable_content() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        plan.surface(),
        plan.context().generation(),
        Some(plan.context().grant_digest().to_owned()),
        provider.semantics_profile_digest.clone(),
        provider.provider_id.clone(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let store =
        RuntimeSurfacePlanStore::new(temporary.path(), plan.context().scope().clone()).unwrap();
    let mut tasks = JoinSet::new();
    for _ in 0..12 {
        let store = store.clone();
        let key = key.clone();
        let plan = plan.clone();
        tasks.spawn(async move { store.put(&key, &plan).await.unwrap() });
    }
    let mut published = 0;
    while let Some(result) = tasks.join_next().await {
        published += usize::from(result.unwrap());
    }
    assert_eq!(published, 1);
    assert_eq!(store.inspect_keys().await.unwrap(), vec![key]);
}

#[tokio::test]
async fn plan_store_rejects_noncanonical_or_tampered_records() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        plan.surface(),
        plan.context().generation(),
        Some(plan.context().grant_digest().to_owned()),
        provider.semantics_profile_digest.clone(),
        provider.provider_id.clone(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let store =
        RuntimeSurfacePlanStore::new(temporary.path(), plan.context().scope().clone()).unwrap();
    store.put(&key, &plan).await.unwrap();
    let digest = key.descriptor_digest().unwrap();
    let path = store
        .root()
        .join(format!("{}.json", digest.strip_prefix("sha256:").unwrap()));
    let value: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
    tokio::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.get(&key).await.unwrap_err().code,
        "use.plugin.runtime.plan_store_invalid"
    );

    tokio::fs::write(&path, b"{\"schema\":\"tampered\"}")
        .await
        .unwrap();
    assert_eq!(
        store.read_plan(&key).await.unwrap_err().code,
        "use.plugin.runtime.plan_store_invalid"
    );
}

#[tokio::test]
async fn plan_store_rejects_cross_installation_keys_before_path_access() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        PlanQualifiedSurfaceRef {
            package_id: plan.context().package_id().to_owned(),
            surface: plan.context().surface().clone(),
        },
        plan.context().generation(),
        Some(plan.context().grant_digest().to_owned()),
        provider.semantics_profile_digest.clone(),
        provider.provider_id.clone(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let other = RuntimeSurfacePlanStore::new(
        temporary.path(),
        a3s_use_core::InstallationId::new(a3s_use_core::InstallationKind::User, "workspace-01")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        other.put(&key, &plan).await.unwrap_err().code,
        "use.installation.identity_mismatch"
    );
    assert!(!other.root().exists());
}

#[tokio::test]
async fn unscoped_inventory_is_read_only_when_the_plan_root_or_lock_is_missing() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::Workspace,
        "workspace-01",
    )
    .unwrap();
    let state_root = temporary.path().join("state");
    let root = state_root.join("runtime-plans");
    let maintenance = StateMaintenanceLock::new(&state_root)
        .acquire_shared()
        .await
        .unwrap();

    let records =
        RuntimeSurfacePlanStore::inspect_records_unscoped_under_maintenance(&root, &maintenance)
            .await
            .unwrap();
    assert!(records.is_empty());
    assert!(!root.exists());
    assert!(!root.join(".runtime-plans.lock").exists());
    drop(maintenance);

    // A restored owner root intentionally has no operational lock. Inventory
    // must inspect it without manufacturing one as a side effect.
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::from_plan(&plan, &provider).unwrap();
    let store = RuntimeSurfacePlanStore::new(&state_root, installation.clone()).unwrap();
    let publication = RuntimeSurfacePlanPublication::new(key, plan).unwrap();
    store.publish(&[publication]).await.unwrap();
    let lock_path = store.root().join(".runtime-plans.lock");
    tokio::fs::remove_file(&lock_path).await.unwrap();
    assert!(!lock_path.exists());

    let maintenance = StateMaintenanceLock::new(&state_root)
        .acquire_shared()
        .await
        .unwrap();

    let records =
        RuntimeSurfacePlanStore::inspect_records_unscoped_under_maintenance(&root, &maintenance)
            .await
            .unwrap();
    assert_eq!(records.len(), 1);
    assert!(!lock_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn plan_store_rejects_a_symlinked_root() {
    use std::os::unix::fs::symlink;

    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        plan.surface(),
        plan.context().generation(),
        Some(plan.context().grant_digest().to_owned()),
        provider.semantics_profile_digest.clone(),
        provider.provider_id.clone(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let root = temporary.path().join("runtime-plans");
    symlink(target.path(), &root).unwrap();
    let store =
        RuntimeSurfacePlanStore::new(temporary.path(), plan.context().scope().clone()).unwrap();
    assert_eq!(
        store.put(&key, &plan).await.unwrap_err().code,
        "use.plugin.runtime.plan_store_invalid"
    );
}

#[test]
fn omitted_grant_digest_is_not_a_runtime_plan_wildcard() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let provider = evidence(&plan, &capabilities(&plan));
    let key = RuntimeSurfacePlanKey::new(
        plan.context().package_id(),
        plan.context().package_digest(),
        plan.context().scope().clone(),
        plan.surface(),
        plan.context().generation(),
        None,
        provider.semantics_profile_digest,
        provider.provider_id,
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    assert!(!key.matches_plan(&plan));
}
