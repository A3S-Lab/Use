use a3s_use_core::{InstallationId, UseResult};
use a3s_use_extension::{ExtensionPackageBinding, ExtensionRegistrySnapshot, InstalledExtension};
use std::collections::BTreeSet;
use std::time::Duration;

use crate::cognitive_package::CognitivePackageManager;

const REGISTRY_SCHEMA_VERSION: u32 = 3;

pub async fn list(installation: InstallationId) -> UseResult<Vec<InstalledExtension>> {
    let manager = CognitivePackageManager::from_env(installation)?;
    let locks = manager.installed_package_locks().await?;
    let mut seen = BTreeSet::new();
    let mut installed = Vec::new();
    for lock in locks {
        for package in &lock.packages {
            let package_id = package.catalog.record.package_id.as_str();
            if !seen.insert(package_id.to_owned()) {
                continue;
            }
            if let Some(extension) = manager.installed_extension(package_id).await? {
                installed.push(extension);
            }
        }
    }
    installed.sort_by(|left, right| left.receipt.package_id.cmp(&right.receipt.package_id));
    Ok(installed)
}

pub async fn get(
    installation: InstallationId,
    package_id: &str,
) -> UseResult<Option<InstalledExtension>> {
    CognitivePackageManager::from_env(installation)?
        .installed_extension(package_id)
        .await
}

pub async fn snapshot(installation: InstallationId) -> UseResult<ExtensionRegistrySnapshot> {
    let manager = CognitivePackageManager::from_env(installation.clone())?;
    let installed = list(installation.clone()).await?;
    let generation = manager.current_capability_generation().await?;
    let packages = installed
        .iter()
        .map(|extension| ExtensionPackageBinding {
            package_id: extension.receipt.package_id.clone(),
            component_id: extension.receipt.component_id.clone(),
            route_alias: extension.receipt.route_alias.clone(),
            version: extension.receipt.version.clone(),
            package_root: extension.receipt.package_root.clone(),
            manifest_sha256: extension.receipt.manifest_sha256.clone(),
            package_sha256: extension.receipt.package_sha256.clone(),
            lifecycle_generation: extension.receipt.lifecycle_generation,
            enabled: extension.receipt.enabled,
            surfaces: extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
        .collect();
    Ok(ExtensionRegistrySnapshot {
        schema_version: REGISTRY_SCHEMA_VERSION,
        installation,
        generation,
        packages,
        pending_cutovers: Vec::new(),
    })
}

pub async fn wait_for_change(
    installation: InstallationId,
    after_generation: u64,
    timeout: Duration,
) -> UseResult<Option<ExtensionRegistrySnapshot>> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let current = snapshot(installation.clone()).await?;
        if current.generation > after_generation {
            return Ok(Some(current));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
