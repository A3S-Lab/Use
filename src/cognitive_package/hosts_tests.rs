use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

struct InjectedLifecycleFactory;

struct RecordingUiFactory(Arc<AtomicBool>);

impl PluginUiLifecycleHostFactory for RecordingUiFactory {
    fn create(
        &self,
        package_root: PathBuf,
    ) -> Arc<dyn crate::plugin_lifecycle::PluginUiLifecycleHost> {
        self.0.store(true, Ordering::SeqCst);
        Arc::new(StaticPluginSurfaceLifecycleHost::new(package_root))
    }
}

impl CognitivePackageLifecycleFactory for InjectedLifecycleFactory {
    fn name(&self) -> &'static str {
        "test-injected"
    }

    fn supported_lifecycle(&self) -> crate::cognitive_package::CognitiveLifecycleSupport {
        // Explicit empty support: this factory exists only to prove
        // injection wiring, not to advertise surfaces.
        crate::cognitive_package::CognitiveLifecycleSupport {
            tool_task_executable: false,
            tool_task_runtime: false,
            tool_service_runtime: false,
            mcp_stdio: false,
            mcp_streamable_http: false,
            skill: false,
            okf: false,
            flow: false,
            ui: false,
        }
    }

    fn flow_compiler_binary(&self) -> Option<&Path> {
        None
    }

    fn validate_manifest(&self, _manifest: &ExtensionManifest) -> UseResult<()> {
        Ok(())
    }

    fn validate_manifest_for_planning(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        self.validate_manifest(manifest)
    }

    fn validate_manifest_for_retirement(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        self.validate_manifest(manifest)
    }

    fn install_providers(
        &self,
        _registry: ExtensionRegistry,
        _candidate: ExtensionLifecyclePackage,
        _package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        Err(provider_error(
            "use.plugin.test_factory_not_applied",
            "The test factory does not compose install providers.",
        ))
    }

    fn published_install_providers(
        &self,
        _registry: ExtensionRegistry,
        _package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        Err(provider_error(
            "use.plugin.test_factory_not_applied",
            "The test factory does not compose replay providers.",
        ))
    }

    fn uninstall_providers(
        &self,
        _registry: ExtensionRegistry,
        _package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        Err(provider_error(
            "use.plugin.test_factory_not_applied",
            "The test factory does not compose uninstall providers.",
        ))
    }

    fn enablement_providers(
        &self,
        _registry: ExtensionRegistry,
        _package_root: std::path::PathBuf,
    ) -> UseResult<LifecycleProviderSet> {
        Err(provider_error(
            "use.plugin.test_factory_not_applied",
            "The test factory does not compose enablement providers.",
        ))
    }
}

#[test]
fn runtime_services_fail_before_lifecycle_composition_without_an_injected_provider() {
    let manifest = ExtensionManifest::parse_acl(include_str!(
        "../../crates/extension/fixtures/manifests/plugin-v3.acl"
    ))
    .unwrap();
    let error = validate_available_hosts(&manifest, None).unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime_provider_required");
}

#[test]
fn standalone_accepts_okf_surfaces_with_the_local_knowledge_backend() {
    let manifest = ExtensionManifest::parse_acl(include_str!(
        "../../crates/extension/fixtures/manifests/plugin-v3-okf.acl"
    ))
    .unwrap();
    validate_available_hosts(&manifest, None).unwrap();
}

#[test]
fn lifecycle_factories_declare_supported_surfaces_without_default_trait_fiction() {
    let standalone = StandaloneCognitivePackageLifecycleFactory::default();
    let managed = ManagedCognitivePackageLifecycleFactory::new(
        RuntimeProviderSelection::default(),
        Arc::new(RuntimeClientRegistry::new()),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    );
    let standalone_support = standalone.supported_lifecycle();
    let managed_support = managed.supported_lifecycle();

    assert!(standalone_support.tool_task_executable);
    assert!(!standalone_support.tool_task_runtime);
    assert!(!standalone_support.tool_service_runtime);
    assert!(!standalone_support.mcp_streamable_http);
    assert!(standalone_support.skill);
    assert!(standalone_support.okf);
    assert!(!standalone_support.flow);

    assert!(managed_support.tool_task_runtime);
    assert!(managed_support.tool_service_runtime);
    assert!(managed_support.mcp_streamable_http);
    assert_ne!(
        standalone_support, managed_support,
        "standalone and managed hosts must negotiate distinct surface sets"
    );
}

#[test]
fn managed_factory_requires_an_exact_selection_for_each_runtime_surface() {
    let manifest = ExtensionManifest::parse_acl(include_str!(
        "../../crates/extension/fixtures/manifests/plugin-v3.acl"
    ))
    .unwrap();
    let factory = ManagedCognitivePackageLifecycleFactory::new(
        RuntimeProviderSelection::default(),
        Arc::new(RuntimeClientRegistry::new()),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    );

    let error = factory.validate_manifest(&manifest).unwrap_err();
    assert_eq!(factory.name(), "managed-runtime-gateway");
    assert_eq!(error.code, "use.plugin.runtime_provider_required");
}

#[test]
fn managed_factory_retires_runtime_surfaces_without_a_candidate_selection() {
    let manifest = ExtensionManifest::parse_acl(include_str!(
        "../../crates/extension/fixtures/manifests/plugin-v3.acl"
    ))
    .unwrap();
    let factory = ManagedCognitivePackageLifecycleFactory::new(
        RuntimeProviderSelection::default(),
        Arc::new(RuntimeClientRegistry::new()),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    );

    factory.validate_manifest_for_retirement(&manifest).unwrap();
}

#[test]
fn managed_factory_uses_the_embedding_hosts_ui_composition() {
    let temp = tempfile::tempdir().unwrap();
    let created = Arc::new(AtomicBool::new(false));
    let factory = ManagedCognitivePackageLifecycleFactory::new(
        RuntimeProviderSelection::default(),
        Arc::new(RuntimeClientRegistry::new()),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    )
    .with_ui_lifecycle_factory(Arc::new(RecordingUiFactory(created.clone())));
    let registry = ExtensionRegistry::new(crate::test_extension_paths(temp.path()));

    let providers = factory
        .published_install_providers(registry, temp.path().join("package"))
        .unwrap();
    // Engine owns coordinator construction from injected ports.
    let _coordinator = providers.into_coordinator();

    assert!(created.load(Ordering::SeqCst));
}

#[test]
fn factories_inject_provider_sets_without_constructing_coordinators() {
    let temp = tempfile::tempdir().unwrap();
    let factory = StandaloneCognitivePackageLifecycleFactory::default();
    let registry = ExtensionRegistry::new(crate::test_extension_paths(temp.path()));
    let providers = factory
        .published_install_providers(registry, temp.path().join("package"))
        .unwrap();
    // Prove the factory returned ports, not a pre-built coordinator type.
    let _hosts = providers.hosts();
    let _coordinator = providers.into_coordinator();
}

#[test]
fn flow_surfaces_fail_before_lifecycle_composition_without_a3s_flow() {
    let manifest = flow_manifest();
    let factory = StandaloneCognitivePackageLifecycleFactory::default();
    let error = factory.validate_manifest(&manifest).unwrap_err();
    assert_eq!(error.code, "use.plugin.flow_provider_required");
}

#[test]
fn explicit_absolute_a3s_flow_compiler_admits_flow_surfaces() {
    let temp = tempfile::tempdir().unwrap();
    let factory = StandaloneCognitivePackageLifecycleFactory::with_flow_compiler(
        temp.path().join("a3s-flow-native-compiler"),
    )
    .unwrap();

    factory.validate_manifest(&flow_manifest()).unwrap();
}

#[test]
fn relative_a3s_flow_compiler_is_rejected_before_composition() {
    let error = StandaloneCognitivePackageLifecycleFactory::with_flow_compiler(
        "bin/a3s-flow-native-compiler",
    )
    .unwrap_err();

    assert_eq!(error.code, "use.plugin.flow_compiler_path_invalid");
}

fn flow_manifest() -> ExtensionManifest {
    ExtensionManifest::parse_acl(
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
    .unwrap()
}

#[test]
fn embedding_hosts_can_replace_the_standalone_lifecycle_factory() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ExtensionRegistry::new(crate::test_extension_paths(temp.path()));
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

struct RejectingControlRuntimeReadiness;

#[async_trait::async_trait]
impl crate::cognitive_package::ControlRuntimeServiceReadinessPort for RejectingControlRuntimeReadiness {
    async fn bind_tool_service(
        &self,
        _surface: &a3s_use_extension::ToolSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<crate::plugin_runtime::RuntimeEndpointRef> {
        Err(provider_error(
            "use.plugin.test_control_readiness_injected",
            "The test Control readiness port was reached.",
        ))
    }

    async fn bind_mcp_service(
        &self,
        _surface: &a3s_use_extension::PluginMcpSurface,
        _plan: &crate::plugin_runtime::RuntimeSurfacePlan,
        _observation: &a3s_runtime::contract::RuntimeObservation,
        _runtime_endpoint: &a3s_runtime::contract::RuntimeServiceEndpoint,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<crate::control_store::ControlRuntimeMcpReadiness> {
        Err(provider_error(
            "use.plugin.test_control_readiness_injected",
            "The test Control readiness port was reached.",
        ))
    }

    async fn drain_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        Err(provider_error(
            "use.plugin.test_control_readiness_injected",
            "The test Control readiness port was reached.",
        ))
    }

    async fn remove_service(
        &self,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
        _deadline_at_ms: Option<u64>,
    ) -> UseResult<()> {
        Err(provider_error(
            "use.plugin.test_control_readiness_injected",
            "The test Control readiness port was reached.",
        ))
    }
}

#[test]
fn managed_factory_exposes_injected_control_runtime_readiness() {
    let factory = ManagedCognitivePackageLifecycleFactory::new(
        RuntimeProviderSelection::default(),
        Arc::new(RuntimeClientRegistry::new()),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    )
    .with_control_runtime_readiness(Arc::new(RejectingControlRuntimeReadiness));
    assert!(factory.control_runtime_readiness().is_some());
    assert!(factory.runtime_plan_publications().unwrap().is_empty());
}

#[test]
fn managed_factory_forwards_runtime_client_registry_to_control_open() {
    let registry = Arc::new(RuntimeClientRegistry::new());
    let factory = ManagedCognitivePackageLifecycleFactory::new(
        RuntimeProviderSelection::default(),
        registry.clone(),
        Arc::new(UnavailableRuntimeServiceReadinessHost),
    );
    assert!(
        Arc::ptr_eq(&factory.runtime_client_registry(), &registry),
        "Control open must reuse the managed host RuntimeClientRegistry, not an empty sibling"
    );
}

#[test]
fn public_control_runtime_readiness_port_is_nameable_for_embedding_hosts() {
    // Product hosts must name this trait from the public cognitive_package
    // face without reaching into the private control_store module.
    fn _accepts_port(_port: Arc<dyn crate::cognitive_package::ControlRuntimeServiceReadinessPort>) {}
    _accepts_port(Arc::new(RejectingControlRuntimeReadiness));
}

#[test]
fn standalone_factory_has_no_control_runtime_readiness() {
    let standalone = StandaloneCognitivePackageLifecycleFactory::default();
    assert!(standalone.control_runtime_readiness().is_none());
    assert!(standalone.runtime_plan_publications().unwrap().is_empty());
}
