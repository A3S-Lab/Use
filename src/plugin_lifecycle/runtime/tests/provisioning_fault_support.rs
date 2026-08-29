use std::path::{Path, PathBuf};

use super::provisioning_fault_gateway::DurableReadiness;
use super::provisioning_fault_runtime::DurableRuntime;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SurfaceCase {
    Tool,
    Mcp,
}

impl SurfaceCase {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Mcp => "mcp",
        }
    }

    fn provider(self) -> &'static str {
        match self {
            Self::Tool => "tool-runtime",
            Self::Mcp => "mcp-runtime",
        }
    }
}

pub(super) struct FaultFixture {
    host: RuntimePluginSurfaceLifecycleHost,
    manifest: ExtensionManifest,
    intent: PluginLifecycleIntent,
    plan: RuntimeSurfacePlan,
    case: SurfaceCase,
    root: PathBuf,
}

impl FaultFixture {
    pub(super) async fn new(root: &Path, case: SurfaceCase) -> Self {
        let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
        let intent = intent_generation(&manifest, 41, PluginLifecycleAction::Install);
        let plan = match case {
            SurfaceCase::Tool => tool_plan(
                &intent,
                manifest
                    .tools
                    .iter()
                    .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
                    .unwrap(),
            ),
            SurfaceCase::Mcp => mcp_plan(
                &intent,
                manifest
                    .mcp_servers
                    .iter()
                    .find(|surface| {
                        matches!(&surface.launch, PluginMcpLaunch::StreamableHttp { .. })
                    })
                    .unwrap(),
            ),
        };
        let runtime = Arc::new(DurableRuntime::new(
            root.join("runtime"),
            capabilities(&plan, case.provider()),
        ));
        let mut registry = RuntimeClientRegistry::new();
        registry
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse(case.provider()).unwrap(),
                client: runtime,
            }))
            .unwrap();
        let registry = Arc::new(registry);
        let selection = RuntimeProviderSelector::new(&registry)
            .select(
                vec![plan.clone()],
                vec![RuntimeProviderAssignment::new(plan.surface(), case.provider()).unwrap()],
            )
            .await
            .unwrap();
        let host = RuntimePluginSurfaceLifecycleHost::new(
            package_root(),
            selection,
            registry,
            RuntimeBindingStore::new(root.join("state"), runtime_installation()).unwrap(),
            Arc::new(DurableReadiness::new(root.join("gateway"))),
        );
        Self {
            host,
            manifest,
            intent,
            plan,
            case,
            root: root.to_path_buf(),
        }
    }

    pub(super) async fn prepare(&self) -> UseResult<PluginLifecycleEvidence> {
        match self.case {
            SurfaceCase::Tool => {
                let surface = self
                    .manifest
                    .tools
                    .iter()
                    .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
                    .unwrap();
                self.host
                    .prepare_tool(
                        &self.intent,
                        surface,
                        key(&self.intent, PluginSurfaceKind::Tool, &surface.id),
                    )
                    .await
            }
            SurfaceCase::Mcp => {
                let surface = self
                    .manifest
                    .mcp_servers
                    .iter()
                    .find(|surface| {
                        matches!(&surface.launch, PluginMcpLaunch::StreamableHttp { .. })
                    })
                    .unwrap();
                self.host
                    .prepare_mcp(
                        &self.intent,
                        surface,
                        key(&self.intent, PluginSurfaceKind::Mcp, &surface.id),
                    )
                    .await
            }
        }
    }

    pub(super) async fn remove(&self) -> UseResult<PluginLifecycleEvidence> {
        match self.case {
            SurfaceCase::Tool => {
                let surface = self
                    .manifest
                    .tools
                    .iter()
                    .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
                    .unwrap();
                self.host
                    .remove_tool(&self.intent, surface, "fault-matrix-remove-tool")
                    .await
            }
            SurfaceCase::Mcp => {
                let surface = self
                    .manifest
                    .mcp_servers
                    .iter()
                    .find(|surface| {
                        matches!(&surface.launch, PluginMcpLaunch::StreamableHttp { .. })
                    })
                    .unwrap();
                self.host
                    .remove_mcp(&self.intent, surface, "fault-matrix-remove-mcp")
                    .await
            }
        }
    }

    pub(super) async fn binding(&self) -> Option<RuntimeBindingReceipt> {
        self.host
            .store()
            .get_generation(
                &self.intent.scope,
                &self.plan.surface(),
                self.intent.generation,
            )
            .await
            .unwrap()
    }

    pub(super) async fn provisioning(&self) -> Option<RuntimeServiceProvisioningReceipt> {
        self.host
            .store()
            .get_provisioning(
                &self.intent.scope,
                &self.plan.surface(),
                self.intent.generation,
            )
            .await
            .unwrap()
    }

    pub(super) fn runtime_effect_path(&self) -> PathBuf {
        self.root.join("runtime/service.json")
    }

    pub(super) fn gateway_effect_path(&self) -> PathBuf {
        self.root.join("gateway/route.json")
    }

    pub(super) fn runtime_attempt_path(&self) -> PathBuf {
        self.root.join("runtime/apply-attempts.log")
    }

    pub(super) fn gateway_attempt_path(&self) -> PathBuf {
        self.root.join("gateway/bind-attempts.log")
    }
}
