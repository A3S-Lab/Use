use a3s_use_core::{InstallationId, UseResult};
use a3s_use_extension::{ExtensionRegistry, ExtensionRegistrySnapshot, InstalledExtension};
use std::time::Duration;

pub async fn list(installation: InstallationId) -> UseResult<Vec<InstalledExtension>> {
    ExtensionRegistry::from_env(installation)?.list().await
}

pub async fn get(
    installation: InstallationId,
    package_id: &str,
) -> UseResult<Option<InstalledExtension>> {
    ExtensionRegistry::from_env(installation)?
        .get(package_id)
        .await
}

pub async fn snapshot(installation: InstallationId) -> UseResult<ExtensionRegistrySnapshot> {
    ExtensionRegistry::from_env(installation)?.snapshot().await
}

pub async fn wait_for_change(
    installation: InstallationId,
    after_generation: u64,
    timeout: Duration,
) -> UseResult<Option<ExtensionRegistrySnapshot>> {
    ExtensionRegistry::from_env(installation)?
        .wait_for_change(after_generation, timeout)
        .await
}
