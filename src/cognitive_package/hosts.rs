use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use a3s_runtime::contract::{RuntimeObservation, RuntimeServiceEndpoint};
use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{
    PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
};
use a3s_use_extension::{
    ExtensionLifecyclePackage, ExtensionManifest, ExtensionRegistry, PluginFlowSurface,
    PluginMcpLaunch, PluginMcpSurface, ToolSurface, ToolTaskSource, ToolWorkload,
};
use async_trait::async_trait;

use crate::flow_runtime::{A3sFlowLifecycleHost, FlowRuntimeBindingStore};
use crate::okf_knowledge::{
    OkfKnowledgeBindingStore, OkfKnowledgeClient, SqliteOkfKnowledgeAdapter,
};
use crate::plugin_lifecycle::{
    ExtensionCapabilityLifecycleHost, ExtensionPackageLifecycleHost, LifecycleProviderSet,
    OkfKnowledgeLifecycleHost, PluginFlowLifecycleHost, PluginLifecycleEvidence,
    PluginLifecycleHosts, PluginLifecycleIntent, PluginMcpServiceReadiness,
    PluginRuntimeServiceReadinessHost, PluginUiLifecycleHostFactory,
    RuntimePluginSurfaceLifecycleHost, StaticPluginSurfaceLifecycleHost,
    StaticPluginSurfaceLifecycleHostFactory,
};
use crate::plugin_runtime::{
    RuntimeBindingStore, RuntimeEndpointRef, RuntimeProviderSelection, RuntimeSurfacePlan,
};

use super::CognitivePackageLifecycleFactory;

/// Narrow lifecycle composition used by the standalone package engine.
///
/// Embedding hosts may wrap this factory for executable Tool Tasks, stdio MCP,
/// Skill, UI, OKF, and explicitly configured A3S Flow packages. It deliberately
/// rejects Runtime Service and HTTP MCP surfaces until the host supplies their
/// real lifecycle adapters. OKF uses the scope-isolated SQLite/FTS5 Knowledge
/// backend.
#[derive(Debug, Clone, Default)]
pub struct StandaloneCognitivePackageLifecycleFactory {
    flow_compiler_binary: Option<PathBuf>,
}

/// Host-composed lifecycle for release-backed Runtime Tasks and Services.
///
/// The selection contains exact process-local Runtime clients plus the
/// provider evidence already bound into the reviewed package plan. Gateway
/// readiness owns private endpoint publication, MCP initialization, route
/// drain, and route removal. Neither dependency can be selected by package
/// content.
#[derive(Clone)]
pub struct ManagedCognitivePackageLifecycleFactory {
    selection: RuntimeProviderSelection,
    runtime_registry: Arc<RuntimeClientRegistry>,
    readiness: Arc<dyn PluginRuntimeServiceReadinessHost>,
    control_runtime_readiness:
        Option<Arc<dyn super::ControlRuntimeServiceReadinessPort>>,
    ui_factory: Arc<dyn PluginUiLifecycleHostFactory>,
    flow_compiler_binary: Option<PathBuf>,
}

impl std::fmt::Debug for ManagedCognitivePackageLifecycleFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedCognitivePackageLifecycleFactory")
            .field("selection", &self.selection)
            .field("flow_compiler_binary", &self.flow_compiler_binary)
            .finish_non_exhaustive()
    }
}

/// Cross-host environment variable selecting the reviewed native TypeScript
/// compiler used by standalone A3S Flow package lifecycle operations.
pub const A3S_FLOW_NATIVE_TS_COMPILER_ENV: &str = "A3S_FLOW_NATIVE_TS_COMPILER";

#[derive(Clone)]
struct RuntimeLifecycleComposition {
    selection: RuntimeProviderSelection,
    registry: Arc<RuntimeClientRegistry>,
    readiness: Arc<dyn PluginRuntimeServiceReadinessHost>,
}

impl StandaloneCognitivePackageLifecycleFactory {
    /// Construct the deterministic provider-free standalone lifecycle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct the standalone lifecycle with one explicit compiler identity.
    ///
    /// The path must be absolute and lexically stable. Binary availability is
    /// checked by the asynchronous `a3s-flow` preflight before publication, so
    /// a missing or failing compiler cannot publish a package generation.
    pub fn with_flow_compiler(compiler_binary: impl Into<PathBuf>) -> UseResult<Self> {
        let compiler_binary = compiler_binary.into();
        validate_flow_compiler_path(&compiler_binary)?;
        Ok(Self {
            flow_compiler_binary: Some(compiler_binary),
        })
    }

    /// Read the optional standalone Flow provider from the process environment.
    pub fn from_env() -> UseResult<Self> {
        match std::env::var_os(A3S_FLOW_NATIVE_TS_COMPILER_ENV) {
            Some(value) => Self::with_flow_compiler(PathBuf::from(value)),
            None => Ok(Self::default()),
        }
    }

    pub fn flow_compiler_binary(&self) -> Option<&Path> {
        self.flow_compiler_binary.as_deref()
    }
}

impl ManagedCognitivePackageLifecycleFactory {
    pub fn new(
        selection: RuntimeProviderSelection,
        runtime_registry: Arc<RuntimeClientRegistry>,
        readiness: Arc<dyn PluginRuntimeServiceReadinessHost>,
    ) -> Self {
        Self {
            selection,
            runtime_registry,
            readiness,
            control_runtime_readiness: None,
            ui_factory: Arc::new(StaticPluginSurfaceLifecycleHostFactory),
            flow_compiler_binary: None,
        }
    }

    /// Inject the Control-shaped Runtime Service readiness port used when
    /// opening production Control. Without this, Control mints opaque
    /// `gateway:` endpoint identities (standalone default).
    pub fn with_control_runtime_readiness(
        mut self,
        readiness: Arc<dyn super::ControlRuntimeServiceReadinessPort>,
    ) -> Self {
        self.control_runtime_readiness = Some(readiness);
        self
    }

    /// Replace the default static UI host with one trusted embedding-host
    /// composition. The injected host remains responsible for validating the
    /// immutable package assets before adding product-owned behavior.
    pub fn with_ui_lifecycle_factory(
        mut self,
        ui_factory: Arc<dyn PluginUiLifecycleHostFactory>,
    ) -> Self {
        self.ui_factory = ui_factory;
        self
    }

    pub fn with_flow_compiler(mut self, compiler_binary: impl Into<PathBuf>) -> UseResult<Self> {
        let compiler_binary = compiler_binary.into();
        validate_flow_compiler_path(&compiler_binary)?;
        self.flow_compiler_binary = Some(compiler_binary);
        Ok(self)
    }

    pub fn selection(&self) -> &RuntimeProviderSelection {
        &self.selection
    }

    fn runtime_composition(&self) -> RuntimeLifecycleComposition {
        RuntimeLifecycleComposition {
            selection: self.selection.clone(),
            registry: self.runtime_registry.clone(),
            readiness: self.readiness.clone(),
        }
    }
}

impl CognitivePackageLifecycleFactory for StandaloneCognitivePackageLifecycleFactory {
    fn name(&self) -> &'static str {
        "standalone"
    }

    fn supported_lifecycle(&self) -> super::CognitiveLifecycleSupport {
        super::CognitiveLifecycleSupport {
            tool_task_executable: true,
            tool_task_runtime: false,
            tool_service_runtime: false,
            mcp_stdio: true,
            mcp_streamable_http: false,
            skill: true,
            okf: true,
            flow: self.flow_compiler_binary.is_some(),
            ui: true,
        }
    }

    fn flow_compiler_binary(&self) -> Option<&Path> {
        self.flow_compiler_binary.as_deref()
    }

    fn validate_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        validate_available_hosts(manifest, self.flow_compiler_binary())
    }

    fn validate_manifest_for_planning(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        // Standalone has no deferred Runtime selection: planning uses the same
        // host availability check as install.
        self.validate_manifest(manifest)
    }

    fn validate_manifest_for_retirement(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        self.validate_manifest(manifest)
    }

    fn install_providers(
        &self,
        registry: ExtensionRegistry,
        candidate: ExtensionLifecyclePackage,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        install_providers(
            registry,
            candidate,
            package_root,
            self.flow_compiler_binary(),
        )
    }

    fn published_install_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        published_install_providers(registry, package_root, self.flow_compiler_binary())
    }

    fn uninstall_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        uninstall_providers(registry, package_root, self.flow_compiler_binary())
    }

    fn enablement_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        // Enablement reuses the published install host set for this factory;
        // it does not invent a second provider composition.
        self.published_install_providers(registry, package_root)
    }
}

impl CognitivePackageLifecycleFactory for ManagedCognitivePackageLifecycleFactory {
    fn name(&self) -> &'static str {
        "managed-runtime-gateway"
    }

    fn supported_lifecycle(&self) -> super::CognitiveLifecycleSupport {
        super::CognitiveLifecycleSupport {
            tool_task_executable: true,
            tool_task_runtime: true,
            tool_service_runtime: true,
            mcp_stdio: true,
            mcp_streamable_http: true,
            skill: true,
            okf: true,
            flow: self.flow_compiler_binary.is_some(),
            ui: true,
        }
    }

    fn flow_compiler_binary(&self) -> Option<&Path> {
        self.flow_compiler_binary.as_deref()
    }

    fn control_runtime_readiness(
        &self,
    ) -> Option<Arc<dyn super::ControlRuntimeServiceReadinessPort>> {
        self.control_runtime_readiness.clone()
    }

    fn runtime_client_registry(&self) -> Arc<RuntimeClientRegistry> {
        self.runtime_registry.clone()
    }

    fn runtime_plan_publications(
        &self,
    ) -> UseResult<Vec<crate::plugin_runtime::RuntimeSurfacePlanPublication>> {
        self.selection.plan_publications()
    }

    fn validate_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        validate_managed_hosts(
            manifest,
            &self.selection,
            self.flow_compiler_binary.as_deref(),
        )
    }

    fn validate_manifest_for_planning(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        validate_managed_host_availability(manifest, self.flow_compiler_binary.as_deref())
    }

    fn validate_manifest_for_retirement(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        validate_managed_host_availability(manifest, self.flow_compiler_binary.as_deref())
    }

    fn install_providers(
        &self,
        registry: ExtensionRegistry,
        candidate: ExtensionLifecyclePackage,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        managed_install_providers(
            registry,
            candidate,
            package_root,
            self.runtime_composition(),
            self.ui_factory.clone(),
            self.flow_compiler_binary.as_deref(),
        )
    }

    fn published_install_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        managed_published_install_providers(
            registry,
            package_root,
            self.runtime_composition(),
            self.ui_factory.clone(),
            self.flow_compiler_binary.as_deref(),
        )
    }

    fn uninstall_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        managed_uninstall_providers(
            registry,
            package_root,
            self.runtime_composition(),
            self.ui_factory.clone(),
            self.flow_compiler_binary.as_deref(),
        )
    }

    fn enablement_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        self.published_install_providers(registry, package_root)
    }
}

fn validate_flow_compiler_path(compiler_binary: &Path) -> UseResult<()> {
    if compiler_binary.as_os_str().is_empty()
        || !compiler_binary.is_absolute()
        || compiler_binary
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(provider_error(
            "use.plugin.flow_compiler_path_invalid",
            format!(
                "{A3S_FLOW_NATIVE_TS_COMPILER_ENV} must identify one absolute, lexically stable compiler path."
            ),
        )
        .with_detail(
            "compilerBinary",
            serde_json::json!(compiler_binary.to_string_lossy()),
        )
        .with_suggestion(
            "Set the variable to the reviewed absolute path of the a3s-flow native TypeScript compiler.",
        ));
    }
    Ok(())
}

pub(super) fn validate_available_hosts(
    manifest: &ExtensionManifest,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<()> {
    if !manifest.flows.is_empty() && flow_compiler_binary.is_none() {
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
            format!(
                "Set {A3S_FLOW_NATIVE_TS_COMPILER_ENV} to the reviewed absolute compiler path or install through an A3S host with an explicit a3s-flow adapter."
            ),
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

fn validate_managed_hosts(
    manifest: &ExtensionManifest,
    selection: &RuntimeProviderSelection,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<()> {
    validate_managed_host_availability(manifest, flow_compiler_binary)?;
    let mut required = manifest
        .tools
        .iter()
        .filter(|surface| {
            !matches!(
                &surface.workload,
                ToolWorkload::Task(task)
                    if matches!(&task.source, ToolTaskSource::Executable { .. })
            )
        })
        .map(|surface| PlanQualifiedSurfaceRef {
            package_id: manifest.package_id.clone(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: surface.id.clone(),
            },
        })
        .chain(
            manifest
                .mcp_servers
                .iter()
                .filter(|surface| matches!(surface.launch, PluginMcpLaunch::StreamableHttp { .. }))
                .map(|surface| PlanQualifiedSurfaceRef {
                    package_id: manifest.package_id.clone(),
                    surface: PluginSurfaceRef {
                        kind: PluginSurfaceKind::Mcp,
                        id: surface.id.clone(),
                    },
                }),
        )
        .collect::<Vec<_>>();
    required.sort();
    let missing = required
        .iter()
        .filter(|required| {
            !selection
                .surfaces()
                .iter()
                .any(|selected| selected.plan().surface() == **required)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(provider_error(
            "use.plugin.runtime_provider_required",
            format!(
                "Cognitive package '{}' lacks exact managed Runtime selections.",
                manifest.package_id
            ),
        )
        .with_detail(
            "surfaces",
            serde_json::to_value(missing).unwrap_or_default(),
        ));
    }
    Ok(())
}

fn validate_managed_host_availability(
    manifest: &ExtensionManifest,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<()> {
    if !manifest.flows.is_empty() && flow_compiler_binary.is_none() {
        return Err(provider_error(
            "use.plugin.flow_provider_required",
            format!(
                "Cognitive package '{}' requires an injected a3s-flow lifecycle provider.",
                manifest.package_id
            ),
        ));
    }
    Ok(())
}

pub(super) fn install_providers(
    registry: ExtensionRegistry,
    candidate: ExtensionLifecyclePackage,
    package_root: impl Into<std::path::PathBuf>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::new(
        registry.clone(),
        candidate,
    ));
    provider_set(
        registry,
        package,
        package_root,
        &paths,
        RuntimeLifecycleComposition {
            selection: RuntimeProviderSelection::default(),
            registry: Arc::new(RuntimeClientRegistry::new()),
            readiness: Arc::new(UnavailableRuntimeServiceReadinessHost),
        },
        Arc::new(StaticPluginSurfaceLifecycleHostFactory),
        flow_compiler_binary,
    )
}

pub(super) fn uninstall_providers(
    registry: ExtensionRegistry,
    package_root: impl Into<std::path::PathBuf>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::for_installed(
        registry.clone(),
    ));
    provider_set(
        registry,
        package,
        package_root,
        &paths,
        RuntimeLifecycleComposition {
            selection: RuntimeProviderSelection::default(),
            registry: Arc::new(RuntimeClientRegistry::new()),
            readiness: Arc::new(UnavailableRuntimeServiceReadinessHost),
        },
        Arc::new(StaticPluginSurfaceLifecycleHostFactory),
        flow_compiler_binary,
    )
}

/// Resume an install whose exact generation is already committed and visible.
/// The installed package host deliberately carries no candidate: a replay may
/// finish publication journals, but it cannot recommit missing package bytes.
pub(super) fn published_install_providers(
    registry: ExtensionRegistry,
    package_root: impl Into<std::path::PathBuf>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::for_installed(
        registry.clone(),
    ));
    provider_set(
        registry,
        package,
        package_root,
        &paths,
        RuntimeLifecycleComposition {
            selection: RuntimeProviderSelection::default(),
            registry: Arc::new(RuntimeClientRegistry::new()),
            readiness: Arc::new(UnavailableRuntimeServiceReadinessHost),
        },
        Arc::new(StaticPluginSurfaceLifecycleHostFactory),
        flow_compiler_binary,
    )
}

fn managed_install_providers(
    registry: ExtensionRegistry,
    candidate: ExtensionLifecyclePackage,
    package_root: impl Into<std::path::PathBuf>,
    runtime: RuntimeLifecycleComposition,
    ui_factory: Arc<dyn PluginUiLifecycleHostFactory>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::new(
        registry.clone(),
        candidate,
    ));
    provider_set(
        registry,
        package,
        package_root,
        &paths,
        runtime,
        ui_factory,
        flow_compiler_binary,
    )
}

fn managed_uninstall_providers(
    registry: ExtensionRegistry,
    package_root: impl Into<std::path::PathBuf>,
    runtime: RuntimeLifecycleComposition,
    ui_factory: Arc<dyn PluginUiLifecycleHostFactory>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    let paths = registry.paths().clone();
    let package = Arc::new(ExtensionPackageLifecycleHost::for_installed(
        registry.clone(),
    ));
    provider_set(
        registry,
        package,
        package_root,
        &paths,
        runtime,
        ui_factory,
        flow_compiler_binary,
    )
}

fn managed_published_install_providers(
    registry: ExtensionRegistry,
    package_root: impl Into<std::path::PathBuf>,
    runtime: RuntimeLifecycleComposition,
    ui_factory: Arc<dyn PluginUiLifecycleHostFactory>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    managed_uninstall_providers(
        registry,
        package_root,
        runtime,
        ui_factory,
        flow_compiler_binary,
    )
}

fn provider_set(
    registry: ExtensionRegistry,
    package: Arc<dyn crate::plugin_lifecycle::PluginPackageLifecycleHost>,
    package_root: impl Into<std::path::PathBuf>,
    paths: &a3s_use_extension::ExtensionPaths,
    runtime: RuntimeLifecycleComposition,
    ui_factory: Arc<dyn PluginUiLifecycleHostFactory>,
    flow_compiler_binary: Option<&Path>,
) -> UseResult<LifecycleProviderSet> {
    let package_root = package_root.into();
    let capability = Arc::new(ExtensionCapabilityLifecycleHost::new(registry));
    let runtime = Arc::new(RuntimePluginSurfaceLifecycleHost::new(
        &package_root,
        runtime.selection,
        runtime.registry,
        RuntimeBindingStore::for_control_authority(paths),
        runtime.readiness,
    ));
    let static_surfaces = Arc::new(StaticPluginSurfaceLifecycleHost::new(package_root.clone()));
    let ui = ui_factory.create(package_root.clone());
    let okf = Arc::new(OkfKnowledgeLifecycleHost::new(
        package_root.clone(),
        OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(
            paths,
        ))),
        OkfKnowledgeBindingStore::for_control_authority(paths),
    ));
    let flow: Arc<dyn PluginFlowLifecycleHost> = match flow_compiler_binary {
        Some(compiler_binary) => Arc::new(A3sFlowLifecycleHost::new(
            package_root.clone(),
            compiler_binary,
            paths
                .use_paths()
                .data_root()
                .join("artifacts")
                .join("flow-native-ts"),
            FlowRuntimeBindingStore::for_control_authority(paths),
        )?),
        None => Arc::new(UnavailableFlowLifecycleHost),
    };
    let hosts = PluginLifecycleHosts::new(
        package,
        capability,
        runtime.clone(),
        runtime,
        okf,
        flow,
        static_surfaces.clone(),
        ui,
    );
    Ok(LifecycleProviderSet::new(
        crate::plugin_lifecycle::PluginLifecycleJournalStore::for_control_authority(paths),
        hosts,
    ))
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
        _runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
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
        _runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<PluginMcpServiceReadiness> {
        Err(provider_error(
            "use.plugin.runtime_provider_required",
            "No MCP Gateway readiness host was injected for this cognitive-package operation.",
        ))
    }

    async fn drain_service(
        &self,
        _intent: &PluginLifecycleIntent,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        Err(provider_error(
            "use.plugin.runtime_provider_required",
            "No Gateway lifecycle host was injected to drain this cognitive-package Service.",
        ))
    }

    async fn remove_service(
        &self,
        _intent: &PluginLifecycleIntent,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        Err(provider_error(
            "use.plugin.runtime_provider_required",
            "No Gateway lifecycle host was injected to remove this cognitive-package Service binding.",
        ))
    }
}

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

fn provider_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
