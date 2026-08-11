use std::collections::BTreeSet;

use a3s_use_core::{
    PluginPackageLock, PluginPackageLockHost, PluginReleaseChannel, PluginSurfaceRef, UseResult,
};
use a3s_use_extension::{
    download_selected_locked_cached_remote_packages, download_selected_locked_remote_packages,
    resolve_cached_remote_package_lock, resolve_remote_package_lock, DownloadedRemotePackage,
    TrustedRegistry,
};

use super::{
    current_host_target, CognitivePackageInstallResult, CognitivePackageManager,
    CognitivePackageUpgradeResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegistryAccess {
    Refreshed,
    Cached,
}

impl CognitivePackageManager {
    /// Resolve, revalidate, download, prepare, and atomically publish one
    /// complete schema-v3 dependency closure. The root Registry and enabled
    /// dependency Registries are supplied by the host and remain replaceable.
    #[allow(clippy::too_many_arguments)]
    pub async fn install_remote(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageInstallResult> {
        self.install_remote_with_access(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            None,
        )
        .await
    }

    /// Install the exact mandatory closure plus the explicitly selected root
    /// surfaces. This is the managed-host path; ordinary CLI installs retain
    /// their complete-surface behavior through [`Self::install_remote`].
    #[allow(clippy::too_many_arguments)]
    pub async fn install_remote_selected(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        selected_surfaces: &[PluginSurfaceRef],
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageInstallResult> {
        self.install_remote_with_access(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            Some(selected_surfaces),
        )
        .await
    }

    /// Install a complete dependency closure from only the last verified,
    /// unexpired Registry metadata and content-addressed target caches.
    #[allow(clippy::too_many_arguments)]
    pub async fn install_cached(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageInstallResult> {
        self.install_remote_with_access(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Cached,
            None,
        )
        .await
    }

    /// Resolve and atomically upgrade one installed cognitive-package graph.
    /// Candidate generations are prepared dependency-first, published once,
    /// and exact prior generations retire only after the snapshot cutover.
    #[allow(clippy::too_many_arguments)]
    pub async fn upgrade_remote(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageUpgradeResult> {
        self.upgrade_remote_with_access(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            None,
        )
        .await
    }

    /// Upgrade one exact graph while selecting only the mandatory closure and
    /// explicitly requested root surfaces for changed generations.
    #[allow(clippy::too_many_arguments)]
    pub async fn upgrade_remote_selected(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        selected_surfaces: &[PluginSurfaceRef],
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageUpgradeResult> {
        self.upgrade_remote_with_access(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            Some(selected_surfaces),
        )
        .await
    }

    /// Upgrade an installed graph using only the last verified, unexpired
    /// Registry metadata and content-addressed target caches.
    #[allow(clippy::too_many_arguments)]
    pub async fn upgrade_cached(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageUpgradeResult> {
        self.upgrade_remote_with_access(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Cached,
            None,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_package_lock(
    access: RegistryAccess,
    root_registry: &TrustedRegistry,
    dependency_registries: &[TrustedRegistry],
    package_id: &str,
    requested_version: Option<&str>,
    channel: PluginReleaseChannel,
) -> UseResult<PluginPackageLock> {
    let host = PluginPackageLockHost::new(current_host_target()?, env!("CARGO_PKG_VERSION"))?;
    match access {
        RegistryAccess::Refreshed => {
            resolve_remote_package_lock(
                root_registry,
                dependency_registries,
                package_id,
                requested_version,
                channel,
                host,
            )
            .await
        }
        RegistryAccess::Cached => {
            resolve_cached_remote_package_lock(
                root_registry,
                dependency_registries,
                package_id,
                requested_version,
                channel,
                host,
            )
            .await
        }
    }
}

pub(super) async fn download_selected_packages(
    access: RegistryAccess,
    package_lock: &PluginPackageLock,
    registries: &[TrustedRegistry],
    selected_package_ids: &BTreeSet<String>,
) -> UseResult<Vec<DownloadedRemotePackage>> {
    match access {
        RegistryAccess::Refreshed => {
            download_selected_locked_remote_packages(package_lock, registries, selected_package_ids)
                .await
        }
        RegistryAccess::Cached => {
            download_selected_locked_cached_remote_packages(
                package_lock,
                registries,
                selected_package_ids,
            )
            .await
        }
    }
}
