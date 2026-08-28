use a3s_use_core::{PluginPackageLock, UseResult};

use super::{ExtensionLifecycleIdentity, ExtensionLifecycleResult};
use crate::registry::{ExtensionRegistry, ExtensionRouteLease};

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

    pub(crate) async fn acquire_lifecycle_route_for_host_version(
        &self,
        route: &str,
        host_version: &str,
    ) -> UseResult<Option<ExtensionRouteLease>> {
        let Some(candidate) = self
            .find_route_for_host_version(route, host_version)
            .await?
        else {
            return Ok(None);
        };
        self.acquire_extension_lease_for_host_version(candidate, Some(route), host_version)
            .await
    }
}
