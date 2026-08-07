use a3s_use_core::UseResult;
use a3s_use_extension::{
    ExtensionRegistry, ExtensionRegistrySnapshot, ExtensionRouteBinding, InstalledExtension,
};
use std::time::Duration;

pub async fn list() -> UseResult<Vec<InstalledExtension>> {
    ExtensionRegistry::from_env()?.list().await
}

pub async fn get(package_id: &str) -> UseResult<Option<InstalledExtension>> {
    ExtensionRegistry::from_env()?.get(package_id).await
}

pub async fn get_snapshot_binding(
    binding: &ExtensionRouteBinding,
) -> UseResult<Option<InstalledExtension>> {
    ExtensionRegistry::from_env()?
        .get_snapshot_binding(binding)
        .await
}

pub async fn snapshot() -> UseResult<ExtensionRegistrySnapshot> {
    ExtensionRegistry::from_env()?.snapshot().await
}

pub async fn wait_for_change(
    after_generation: u64,
    timeout: Duration,
) -> UseResult<Option<ExtensionRegistrySnapshot>> {
    ExtensionRegistry::from_env()?
        .wait_for_change(after_generation, timeout)
        .await
}
