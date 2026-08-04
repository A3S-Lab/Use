use std::sync::Arc;

use a3s_runtime::contract::RuntimeObservation;
use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    ExtensionLifecyclePackage, ExtensionManifest, ExtensionRegistry, PluginFlowSurface,
    PluginMcpLaunch, PluginMcpSurface, PluginOkfSurface, ToolSurface, ToolTaskSource, ToolWorkload,
};
use async_trait::async_trait;

use crate::plugin_lifecycle::{
    ExtensionCapabilityLifecycleHost, ExtensionPackageLifecycleHost, PluginFlowLifecycleHost,
    PluginLifecycleCoordinator, PluginLifecycleEvidence, PluginLifecycleHosts,
    PluginLifecycleIntent, PluginMcpServiceReadiness, PluginOkfLifecycleHost,
    PluginRuntimeServiceReadinessHost, RuntimePluginSurfaceLifecycleHost,
    StaticPluginSurfaceLifecycleHost,
};
use crate::plugin_runtime::{
    RuntimeBindingStore, RuntimeEndpointRef, RuntimeProviderSelection, RuntimeSurfacePlan,
};

use super::CognitivePackageLifecycleFactory;

/// Narrow lifecycle composition used by the standalone package engine.
///
/// Embedding hosts may wrap this factory for executable Tool Tasks, stdio MCP,
/// Skill, and UI packages. It deliberately rejects Runtime Service, HTTP MCP,
/// A3S Flow, and OKF surfaces until the host supplies their real lifecycle
/// adapters.
#[derive(Debug, Default)]
pub struct StandaloneCognitivePackageLifecycleFactory;

impl CognitivePackageLifecycleFactory for StandaloneCognitivePackageLifecycleFactory {
    fn name(&self) -> &'static str {
        "standalone"
    }

    fn validate_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        validate_available_hosts(manifest)
    }

    fn install_coordinator(
        &self,
        registry: ExtensionRegistry,
        candidate: ExtensionLifecyclePackage,
        package_root: std::path::PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator> {
        Ok(install_coordinator(registry, candidate, package_root))
    }

    fn published_install_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator> {
        Ok(published_install_coordinator(registry, package_root))
    }

    fn uninstall_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator> {
        Ok(uninstall_coordinator(registry, package_root))
    }
}

pub(super) fn validate_available_hosts(manifest: &ExtensionManifest) -> UseResult<()> {
    if !manifest.flows.is_empty() {
        return Err(provider_error(
            "use.plugin.flow_provider_required",
            format!(
                "Cognitive package '{}' requires an injected a3s-flow lifecycle provider.",
                manifest.package_id
            ),
        )
        .with_detail(
            "surfaces",
            serde_json::json!(manifest.flows.iter().map(|value| &value.id).collect::<Vec<_>>()),
        )
        .with_suggestion(
            "Install through an A3S host with an explicit a3s-flow compiler/runtime adapter, then replay the exact package lock.",
        ));
    }

    if !manifest.okf.is_empty() {
        return Err(provider_error(
            "use.plugin.okf_provider_required",
            format!(
                "Cognitive package '{}' requires an injected A3S Knowledge provider for OKF surfaces.",
                manifest.package_id
            ),
        )
        .with_detail(
            "surfaces",
            serde_json::json!(manifest.okf.iter().map(|value| &value.id).collect::<Vec<_>>()),
        )
        .with_suggestion(
            "Install through an A3S host with an explicit Knowledge adapter, then replay the exact package lock.",
        ));
    }

    let runtime_tools = manifest
        .tools
        .iter()
        .filter(|surface| {
            !matches!(
                &surface.workload,
                ToolWorkload::Task(task)
                    if matches!(&task.source, ToolTaskSource::Executable { .. })
            )
        })
        .map(|surface| surface.id.as_str())
        .collect::<Vec<_>>();
    let runtime_mcp = manifest
        .mcp_servers
        .iter()
        .filter(|surface| matches!(surface.launch, PluginMcpLaunch::StreamableHttp { .. }))
        .map(|surface| surface.id.as_str())
        .collect::<Vec<_>>();
    if !runtime_tools.is_empty() || !runtime_mcp.is_empty() {
        return Err(provider_error(
            "use.plugin.runtime_provider_required",
            format!(
                "Cognitive package '{}' requires explicit Runtime and Gateway provider evidence.",
                manifest.package_id
            ),
        )
        .with_detail("toolSurfaces", serde_json::json!(runtime_tools))
        .with_detail("mcpSurfaces", serde_json::json!(runtime_mcp))
        .with_suggestion(
            "Install through an A3S host that injects exact Runtime provider selections and service readiness evidence.",
        ));
    }
    Ok(())
}

pub(super) fn install_coordinator(
    registry: ExtensionRegistry,
    candidate: ExtensionLifecyclePackage,
    package_root: impl Into<std::path::PathBuf>,
) -> PluginLifecycleCoordinator {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::new(
        registry.clone(),
        candidate,
    ));
    coordinator(registry, package, package_root, &paths)
}

pub(super) fn uninstall_coordinator(
    registry: ExtensionRegistry,
    package_root: impl Into<std::path::PathBuf>,
) -> PluginLifecycleCoordinator {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::for_installed(
        registry.clone(),
    ));
    coordinator(registry, package, package_root, &paths)
}

/// Resume an install whose exact generation is already committed and visible.
/// The installed package host deliberately carries no candidate: a replay may
/// finish publication journals, but it cannot recommit missing package bytes.
pub(super) fn published_install_coordinator(
    registry: ExtensionRegistry,
    package_root: impl Into<std::path::PathBuf>,
) -> PluginLifecycleCoordinator {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::for_installed(
        registry.clone(),
    ));
    coordinator(registry, package, package_root, &paths)
}

fn coordinator(
    registry: ExtensionRegistry,
    package: Arc<dyn crate::plugin_lifecycle::PluginPackageLifecycleHost>,
    package_root: impl Into<std::path::PathBuf>,
    paths: &a3s_use_extension::ExtensionPaths,
) -> PluginLifecycleCoordinator {
    let package_root = package_root.into();
    let capability = Arc::new(ExtensionCapabilityLifecycleHost::new(registry));
    let runtime = Arc::new(RuntimePluginSurfaceLifecycleHost::new(
        &package_root,
        RuntimeProviderSelection::default(),
        RuntimeBindingStore::from_extension_paths(paths),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    ));
    let static_surfaces = Arc::new(StaticPluginSurfaceLifecycleHost::new(package_root));
    let okf = Arc::new(UnavailableOkfLifecycleHost);
    let flow = Arc::new(UnavailableFlowLifecycleHost);
    let hosts = PluginLifecycleHosts::new(
        package,
        capability,
        runtime.clone(),
        runtime,
        okf,
        flow,
        static_surfaces.clone(),
        static_surfaces,
    );
    PluginLifecycleCoordinator::new(
        crate::plugin_lifecycle::PluginLifecycleJournalStore::from_extension_paths(paths),
        hosts,
    )
}

struct UnavailableRuntimeServiceReadinessHost;

#[async_trait]
impl PluginRuntimeServiceReadinessHost for UnavailableRuntimeServiceReadinessHost {
    async fn bind_tool_service(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &ToolSurface,
        _plan: &RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        _idempotency_key: &str,
    ) -> UseResult<RuntimeEndpointRef> {
        Err(provider_error(
            "use.plugin.runtime_provider_required",
            "No Runtime Service readiness host was injected for this cognitive-package operation.",
        ))
    }

    async fn bind_mcp_service(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginMcpSurface,
        _plan: &RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        _idempotency_key: &str,
    ) -> UseResult<PluginMcpServiceReadiness> {
        Err(provider_error(
            "use.plugin.runtime_provider_required",
            "No MCP Gateway readiness host was injected for this cognitive-package operation.",
        ))
    }
}

struct UnavailableOkfLifecycleHost;

struct UnavailableFlowLifecycleHost;

#[async_trait]
impl PluginFlowLifecycleHost for UnavailableFlowLifecycleHost {
    async fn prepare_flow(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginFlowSurface,
        _idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        Err(flow_unavailable())
    }

    async fn stop_flow(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginFlowSurface,
        _idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        Err(flow_unavailable())
    }

    async fn remove_flow(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginFlowSurface,
        _idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        Err(flow_unavailable())
    }
}

fn flow_unavailable() -> UseError {
    provider_error(
        "use.plugin.flow_provider_required",
        "No a3s-flow compiler/runtime lifecycle adapter was injected for this cognitive-package operation.",
    )
}

#[async_trait]
impl PluginOkfLifecycleHost for UnavailableOkfLifecycleHost {
    async fn prepare_okf(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginOkfSurface,
        _idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        Err(okf_unavailable())
    }

    async fn stop_okf(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginOkfSurface,
        _idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        Err(okf_unavailable())
    }

    async fn remove_okf(
        &self,
        _intent: &PluginLifecycleIntent,
        _surface: &PluginOkfSurface,
        _idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        Err(okf_unavailable())
    }
}

fn okf_unavailable() -> UseError {
    provider_error(
        "use.plugin.okf_provider_required",
        "No A3S Knowledge lifecycle adapter was injected for this cognitive-package operation.",
    )
}

fn provider_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InjectedLifecycleFactory;

    impl CognitivePackageLifecycleFactory for InjectedLifecycleFactory {
        fn name(&self) -> &'static str {
            "test-injected"
        }

        fn validate_manifest(&self, _manifest: &ExtensionManifest) -> UseResult<()> {
            Ok(())
        }

        fn install_coordinator(
            &self,
            _registry: ExtensionRegistry,
            _candidate: ExtensionLifecyclePackage,
            _package_root: std::path::PathBuf,
        ) -> UseResult<PluginLifecycleCoordinator> {
            Err(provider_error(
                "use.plugin.test_factory_not_applied",
                "The test factory does not compose an install coordinator.",
            ))
        }

        fn published_install_coordinator(
            &self,
            _registry: ExtensionRegistry,
            _package_root: std::path::PathBuf,
        ) -> UseResult<PluginLifecycleCoordinator> {
            Err(provider_error(
                "use.plugin.test_factory_not_applied",
                "The test factory does not compose a replay coordinator.",
            ))
        }

        fn uninstall_coordinator(
            &self,
            _registry: ExtensionRegistry,
            _package_root: std::path::PathBuf,
        ) -> UseResult<PluginLifecycleCoordinator> {
            Err(provider_error(
                "use.plugin.test_factory_not_applied",
                "The test factory does not compose an uninstall coordinator.",
            ))
        }
    }

    #[test]
    fn runtime_services_fail_before_lifecycle_composition_without_an_injected_provider() {
        let manifest = ExtensionManifest::parse_acl(include_str!(
            "../../crates/extension/fixtures/manifests/plugin-v3.acl"
        ))
        .unwrap();
        let error = validate_available_hosts(&manifest).unwrap_err();
        assert_eq!(error.code, "use.plugin.runtime_provider_required");
    }

    #[test]
    fn okf_surfaces_fail_before_lifecycle_composition_without_knowledge() {
        let manifest = ExtensionManifest::parse_acl(include_str!(
            "../../crates/extension/fixtures/manifests/plugin-v3-okf.acl"
        ))
        .unwrap();
        let error = validate_available_hosts(&manifest).unwrap_err();
        assert_eq!(error.code, "use.plugin.okf_provider_required");
    }

    #[test]
    fn flow_surfaces_fail_before_lifecycle_composition_without_a3s_flow() {
        let manifest = ExtensionManifest::parse_acl(
            r#"
extension "acme/flow" {
  schema_version = 3
  version        = "1.0.0"
  route          = "flow"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/flow"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  flow "review" {
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/review.ts"
    export        = "run"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }
}
"#,
        )
        .unwrap();
        let error = validate_available_hosts(&manifest).unwrap_err();
        assert_eq!(error.code, "use.plugin.flow_provider_required");
    }

    #[test]
    fn embedding_hosts_can_replace_the_standalone_lifecycle_factory() {
        let temp = tempfile::tempdir().unwrap();
        let registry = ExtensionRegistry::new(a3s_use_extension::ExtensionPaths::new(
            temp.path().join("data"),
            temp.path().join("state"),
        ));
        let manager = super::super::CognitivePackageManager::with_lifecycle(
            registry,
            Arc::new(InjectedLifecycleFactory),
        )
        .unwrap();
        let manifest = ExtensionManifest::parse_acl(include_str!(
            "../../crates/extension/fixtures/manifests/plugin-v3-okf.acl"
        ))
        .unwrap();

        assert_eq!(manager.lifecycle().name(), "test-injected");
        manager.lifecycle().validate_manifest(&manifest).unwrap();
    }
}
