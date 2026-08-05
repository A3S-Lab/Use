//! Production composition for schema-v3 cognitive-package graphs.
//!
//! Registry endpoints and trust roots are supplied by the embedding host.
//! This module owns dependency-lock planning and package-level lifecycle
//! composition; Tool, MCP, Skill, UI, and OKF remain contributions inside one
//! immutable package generation.

mod grant;
mod hosts;
mod install;
mod plan;
mod store;
mod uninstall;
mod upgrade;

use a3s_use_core::{
    LockedPluginPackage, PluginOperationPlanEnvelope, PluginPackageLock, PluginReleaseChannel,
    UseError, UseResult, VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{
    inspect_remote_plugin, ExtensionLifecyclePackage, ExtensionManifest, ExtensionRegistry,
    InstalledExtension, PluginCatalogHost, TrustedRegistry,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

use crate::plugin_lifecycle::PluginLifecycleCoordinator;
use store::{InstalledPackageGraphStore, PendingPackageGraphStore};

pub use grant::{
    CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider,
    StandaloneCognitivePackageAuthorizationProvider,
};
pub use hosts::StandaloneCognitivePackageLifecycleFactory;

/// Stable user-level scope shared by the standalone facade and embedding A3S
/// hosts for globally installed cognitive packages.
pub const COGNITIVE_PACKAGE_DEFAULT_SCOPE: &str = "user/current";

/// Version of the A3S Use package engine enforcing cognitive-package host
/// compatibility.
pub const COGNITIVE_PACKAGE_HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Canonical target name used by signed cognitive-package catalogs and locks.
pub fn cognitive_package_host_target() -> UseResult<String> {
    current_host_target()
}

/// Shared package manager used by the standalone CLI and embedding A3S hosts.
/// Registry selection remains host configuration and is passed per operation.
#[derive(Clone)]
pub struct CognitivePackageManager {
    registry: ExtensionRegistry,
    scope_id: String,
    lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
    authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
}

impl std::fmt::Debug for CognitivePackageManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CognitivePackageManager")
            .field("registry", &self.registry)
            .field("scope_id", &self.scope_id)
            .field("lifecycle", &self.lifecycle.name())
            .field("authorization", &self.authorization.name())
            .finish()
    }
}

/// Host composition boundary for one package-owned lifecycle saga.
///
/// A3S Code, Web, or another embedding host injects exact Runtime, Gateway,
/// Knowledge, Skill, and UI adapters here. The standalone implementation is
/// deliberately narrower and fails closed for surfaces whose owner is absent.
pub trait CognitivePackageLifecycleFactory: Send + Sync {
    fn name(&self) -> &'static str;

    fn validate_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()>;

    fn install_coordinator(
        &self,
        registry: ExtensionRegistry,
        candidate: ExtensionLifecyclePackage,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator>;

    fn published_install_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator>;

    fn uninstall_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator>;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitivePackageInstallResult {
    pub changed: bool,
    pub root: InstalledExtension,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PluginOperationPlanEnvelope>,
    pub installed_packages: Vec<String>,
    pub retained_packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitivePackageUninstallResult {
    pub changed: bool,
    pub root_package_id: String,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    pub plan: PluginOperationPlanEnvelope,
    pub removed_packages: Vec<String>,
    pub retained_packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitivePackageUpgradeResult {
    pub changed: bool,
    pub root: InstalledExtension,
    pub prior_package_lock: PluginPackageLock,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PluginOperationPlanEnvelope>,
    pub added_packages: Vec<String>,
    pub replaced_packages: Vec<String>,
    pub removed_packages: Vec<String>,
    pub retained_packages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InstallDisposition {
    Add,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UninstallDisposition {
    Remove,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpgradeDisposition {
    Add,
    Replace,
    Remove,
    Retain,
}

impl CognitivePackageManager {
    pub fn from_env() -> UseResult<Self> {
        Self::new(ExtensionRegistry::from_env()?)
    }

    pub fn new(registry: ExtensionRegistry) -> UseResult<Self> {
        Self::with_scope(registry, COGNITIVE_PACKAGE_DEFAULT_SCOPE)
    }

    pub fn with_scope(registry: ExtensionRegistry, scope_id: impl Into<String>) -> UseResult<Self> {
        Self::with_scope_and_lifecycle(
            registry,
            scope_id,
            Arc::new(hosts::StandaloneCognitivePackageLifecycleFactory),
        )
    }

    pub fn with_lifecycle(
        registry: ExtensionRegistry,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
    ) -> UseResult<Self> {
        Self::with_scope_and_lifecycle(registry, COGNITIVE_PACKAGE_DEFAULT_SCOPE, lifecycle)
    }

    pub fn with_scope_and_lifecycle(
        registry: ExtensionRegistry,
        scope_id: impl Into<String>,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
    ) -> UseResult<Self> {
        Self::with_scope_lifecycle_and_authorization(
            registry,
            scope_id,
            lifecycle,
            Arc::new(StandaloneCognitivePackageAuthorizationProvider),
        )
    }

    pub fn with_authorization(
        registry: ExtensionRegistry,
        authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    ) -> UseResult<Self> {
        Self::with_scope_lifecycle_and_authorization(
            registry,
            COGNITIVE_PACKAGE_DEFAULT_SCOPE,
            Arc::new(hosts::StandaloneCognitivePackageLifecycleFactory),
            authorization,
        )
    }

    pub fn with_scope_lifecycle_and_authorization(
        registry: ExtensionRegistry,
        scope_id: impl Into<String>,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
        authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    ) -> UseResult<Self> {
        let scope_id = scope_id.into();
        if scope_id.is_empty()
            || scope_id.len() > 256
            || !scope_id
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !scope_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
            })
        {
            return Err(package_manager_error(
                "use.plugin.package_scope_invalid",
                "The cognitive-package manager scope identity is invalid.",
            ));
        }
        Ok(Self {
            registry,
            scope_id,
            lifecycle,
            authorization,
        })
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn lifecycle(&self) -> &dyn CognitivePackageLifecycleFactory {
        self.lifecycle.as_ref()
    }

    pub fn authorization(&self) -> &dyn CognitivePackageAuthorizationProvider {
        self.authorization.as_ref()
    }

    /// Return true only for a complete signed schema-v3 catalog record.
    /// Missing catalog metadata means the caller may use the compatible
    /// schema-v1/v2 installer; malformed or incompatible catalog state fails.
    pub async fn is_remote_cognitive_package(
        &self,
        registry: &TrustedRegistry,
        package_id: &str,
        version: Option<&str>,
        channel: PluginReleaseChannel,
    ) -> UseResult<bool> {
        let host = PluginCatalogHost::new(current_host_target()?, env!("CARGO_PKG_VERSION"))?;
        match inspect_remote_plugin(registry, &host, package_id, version, Some(channel)).await {
            Ok(inspection) => {
                Ok(inspection.plugin.record.schema == a3s_use_core::PLUGIN_CATALOG_SCHEMA_V3)
            }
            Err(error) if error.code == "use.extension.catalog_package_missing" => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn graph_store(&self) -> InstalledPackageGraphStore {
        InstalledPackageGraphStore::new(self.registry.paths().state_root())
    }

    fn pending_store(&self) -> PendingPackageGraphStore {
        PendingPackageGraphStore::new(self.registry.paths().state_root())
    }

    fn grant_store(&self) -> a3s_use_extension::WorkspaceGrantStore {
        a3s_use_extension::WorkspaceGrantStore::from_extension_paths(self.registry.paths())
    }
}

pub(super) fn current_host_target() -> UseResult<String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64".to_string()),
        ("macos", "x86_64") => Ok("darwin-x86_64".to_string()),
        ("linux", "aarch64") => Ok("linux-arm64".to_string()),
        ("linux", "x86_64") => Ok("linux-x86_64".to_string()),
        ("windows", "x86_64") => Ok("windows-x86_64".to_string()),
        (os, arch) => Err(package_manager_error(
            "use.plugin.package_host_unsupported",
            format!("Cognitive packages do not support host target '{os}-{arch}'."),
        )),
    }
}

pub(super) fn all_catalog_surfaces(
    package: &LockedPluginPackage,
) -> Vec<a3s_use_core::PluginSurfaceRef> {
    package
        .catalog
        .record
        .surfaces
        .iter()
        .map(a3s_use_core::CatalogSurface::reference)
        .collect()
}

pub(super) fn installed_matches_lock(
    installed: &InstalledExtension,
    catalog: &VerifiedPluginCatalogRecord,
) -> UseResult<bool> {
    if installed.receipt.lifecycle_generation.is_none() {
        return Ok(false);
    }
    Ok(installed.plan_ready_catalog()? == catalog)
}

pub(super) fn package_manager_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
