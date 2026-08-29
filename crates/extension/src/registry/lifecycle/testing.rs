use a3s_use_core::{PluginPackageLock, UseResult};

use super::{ExtensionLifecycleIdentity, ExtensionLifecycleResult};
use crate::registry::{ExtensionGenerationLease, ExtensionRegistry};

impl ExtensionRegistry {
    pub(crate) async fn publish_lifecycle_package_for_host_version(
        &self,
        identity: &ExtensionLifecycleIdentity,
        host_version: &str,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, true, host_version)
            .await
    }

    pub(crate) async fn publish_lifecycle_packages_for_test_host_version(
        &self,
        identities: &[ExtensionLifecycleIdentity],
        host_version: &str,
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_packages_for_host_version(identities, &[], host_version, None, None)
            .await?
            .packages)
    }

    pub(crate) async fn publish_lifecycle_package_graph_for_test_host_version(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        host_version: &str,
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_packages_for_host_version(
                identities,
                &[],
                host_version,
                Some(package_lock),
                None,
            )
            .await?
            .packages)
    }

    pub(crate) async fn acquire_lifecycle_alias_for_host_version(
        &self,
        alias: &str,
        host_version: &str,
    ) -> UseResult<Option<ExtensionGenerationLease>> {
        let Some(candidate) = self
            .resolve_alias_for_host_version(alias, host_version)
            .await?
        else {
            return Ok(None);
        };
        self.acquire_extension_generation_for_host_version(candidate, Some(alias), host_version)
            .await
    }
}
