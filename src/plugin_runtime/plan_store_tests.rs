use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind};
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
