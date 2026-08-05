use std::collections::BTreeMap;

use a3s_runtime::{ProviderId, RuntimeClientRegistry};
use a3s_use_core::{
    PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
};
use a3s_use_extension::{ExtensionManifest, PluginMcpLaunch, ToolTaskSource, ToolWorkload};
use serde::Serialize;

use super::client::{runtime_error, PluginRuntimeClient};
use super::lifecycle::RuntimeBindingObservedState;
use super::model::{
    runtime_contract_error, runtime_input_error, valid_machine_id, valid_sha256,
    valid_surface_segment, RuntimeServiceBindingReceipt, RuntimeSurfaceContract,
};
use super::receipt::RuntimeBindingReceipt;
use super::store::RuntimeBindingStore;

pub const RUNTIME_SURFACE_OBSERVATION_SCHEMA_VERSION: u32 = 1;
const MAX_RUNTIME_SURFACES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeSurfaceObservedState {
    Unbound,
    Prepared,
    Starting,
    Healthy,
    Failed,
    Draining,
    Stopped,
    Missing,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSurfaceObservation {
    surface: PluginSurfaceRef,
    state: RuntimeSurfaceObservedState,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSurfaceObservationSnapshot {
    schema_version: u32,
    scope_id: String,
    package_id: String,
    package_digest: String,
    surfaces: Vec<RuntimeSurfaceObservation>,
}

pub struct RuntimeSurfaceObserver<'a> {
    store: &'a RuntimeBindingStore,
    providers: &'a RuntimeClientRegistry,
}

impl<'a> RuntimeSurfaceObserver<'a> {
    pub fn new(store: &'a RuntimeBindingStore, providers: &'a RuntimeClientRegistry) -> Self {
        Self { store, providers }
    }

    pub async fn observe_manifest(
        &self,
        scope_id: &str,
        package_digest: &str,
        generation: u64,
        manifest: &ExtensionManifest,
    ) -> UseResult<RuntimeSurfaceObservationSnapshot> {
        validate_scope_and_digest(scope_id, package_digest)?;
        if generation == 0 {
            return Err(runtime_input_error(
                "Runtime surface observation requires an exact positive package generation.",
            ));
        }
        let expectations = runtime_surface_expectations(manifest)?;
        let mut clients = BTreeMap::<String, PluginRuntimeClient>::new();
        let mut surfaces = Vec::with_capacity(expectations.len());

        for (surface, expected) in expectations {
            let qualified = PlanQualifiedSurfaceRef {
                package_id: manifest.package_id.clone(),
                surface: surface.clone(),
            };
            let Some(receipt) = self
                .store
                .get_generation(scope_id, &qualified, generation)
                .await?
            else {
                surfaces.push(RuntimeSurfaceObservation {
                    surface,
                    state: RuntimeSurfaceObservedState::Unbound,
                    provider_id: None,
                    generation: None,
                });
                continue;
            };
            validate_binding(&receipt, package_digest, expected)?;
            let provider_id = receipt.provider_id().to_string();
            if !clients.contains_key(&provider_id) {
                let parsed = ProviderId::parse(provider_id.clone())
                    .map_err(|error| runtime_contract_error(error.to_string()))?;
                let client = self.providers.connect(&parsed).await.map_err(|error| {
                    runtime_error("connect the Runtime binding's explicit provider", error)
                })?;
                clients.insert(provider_id.clone(), PluginRuntimeClient::new(client));
            }
            let client = clients.get(&provider_id).ok_or_else(|| {
                runtime_contract_error("The explicit Runtime provider connection disappeared.")
            })?;
            let observation = client.observe_binding(&receipt).await?;
            surfaces.push(RuntimeSurfaceObservation {
                surface,
                state: observation.state.into(),
                provider_id: Some(provider_id),
                generation: Some(receipt.generation()),
            });
        }

        Ok(RuntimeSurfaceObservationSnapshot {
            schema_version: RUNTIME_SURFACE_OBSERVATION_SCHEMA_VERSION,
            scope_id: scope_id.to_string(),
            package_id: manifest.package_id.clone(),
            package_digest: package_digest.to_string(),
            surfaces,
        })
    }
}

impl RuntimeSurfaceObservationSnapshot {
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn surfaces(&self) -> &[RuntimeSurfaceObservation] {
        &self.surfaces
    }

    pub(crate) fn validate_for_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        validate_scope_and_digest(&self.scope_id, &self.package_digest)?;
        let expectations = runtime_surface_expectations(manifest)?;
        if self.schema_version != RUNTIME_SURFACE_OBSERVATION_SCHEMA_VERSION
            || self.package_id != manifest.package_id
            || self.surfaces.len() != expectations.len()
        {
            return Err(runtime_input_error(
                "The Runtime surface observation snapshot does not match its manifest.",
            ));
        }
        for (observation, expected_surface) in self.surfaces.iter().zip(expectations.keys()) {
            let unbound = observation.state == RuntimeSurfaceObservedState::Unbound;
            if &observation.surface != expected_surface
                || unbound != observation.provider_id.is_none()
                || unbound != observation.generation.is_none()
                || observation
                    .provider_id
                    .as_deref()
                    .is_some_and(|provider| ProviderId::parse(provider).is_err())
                || observation.generation == Some(0)
            {
                return Err(runtime_input_error(
                    "A Runtime surface observation contains invalid binding evidence.",
                ));
            }
        }
        Ok(())
    }
}

impl RuntimeSurfaceObservation {
    pub fn surface(&self) -> &PluginSurfaceRef {
        &self.surface
    }

    pub fn state(&self) -> RuntimeSurfaceObservedState {
        self.state
    }

    pub fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }

    pub fn generation(&self) -> Option<u64> {
        self.generation
    }
}

impl From<RuntimeBindingObservedState> for RuntimeSurfaceObservedState {
    fn from(state: RuntimeBindingObservedState) -> Self {
        match state {
            RuntimeBindingObservedState::Prepared => Self::Prepared,
            RuntimeBindingObservedState::Starting => Self::Starting,
            RuntimeBindingObservedState::Healthy => Self::Healthy,
            RuntimeBindingObservedState::Failed => Self::Failed,
            RuntimeBindingObservedState::Draining => Self::Draining,
            RuntimeBindingObservedState::Stopped => Self::Stopped,
            RuntimeBindingObservedState::Missing => Self::Missing,
            RuntimeBindingObservedState::Stale => Self::Stale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpectedRuntimeBinding {
    Task,
    ToolService { base_path: String },
    McpService,
}

fn runtime_surface_expectations(
    manifest: &ExtensionManifest,
) -> UseResult<BTreeMap<PluginSurfaceRef, ExpectedRuntimeBinding>> {
    validate_manifest_identity(manifest)?;
    let mut expectations = BTreeMap::new();
    for tool in &manifest.tools {
        let expected = match &tool.workload {
            ToolWorkload::Task(task) if matches!(&task.source, ToolTaskSource::Release { .. }) => {
                Some(ExpectedRuntimeBinding::Task)
            }
            ToolWorkload::Task(_) => None,
            ToolWorkload::Service(service) => Some(ExpectedRuntimeBinding::ToolService {
                base_path: service.base_path.clone(),
            }),
        };
        if let Some(expected) = expected {
            insert_expectation(
                &mut expectations,
                PluginSurfaceRef {
                    kind: PluginSurfaceKind::Tool,
                    id: tool.id.clone(),
                },
                expected,
            )?;
        }
    }
    for mcp in &manifest.mcp_servers {
        if matches!(&mcp.launch, PluginMcpLaunch::StreamableHttp { .. }) {
            insert_expectation(
                &mut expectations,
                PluginSurfaceRef {
                    kind: PluginSurfaceKind::Mcp,
                    id: mcp.id.clone(),
                },
                ExpectedRuntimeBinding::McpService,
            )?;
        }
    }
    if expectations.len() > MAX_RUNTIME_SURFACES {
        return Err(runtime_input_error(format!(
            "A plugin may observe at most {MAX_RUNTIME_SURFACES} Runtime surfaces."
        )));
    }
    Ok(expectations)
}

fn insert_expectation(
    expectations: &mut BTreeMap<PluginSurfaceRef, ExpectedRuntimeBinding>,
    surface: PluginSurfaceRef,
    expected: ExpectedRuntimeBinding,
) -> UseResult<()> {
    if !valid_surface_segment(&surface.id) || expectations.insert(surface, expected).is_some() {
        return Err(runtime_input_error(
            "The Runtime surface manifest contains an invalid or duplicate identity.",
        ));
    }
    Ok(())
}

fn validate_manifest_identity(manifest: &ExtensionManifest) -> UseResult<()> {
    let package = manifest.package_id.split('/').collect::<Vec<_>>();
    if manifest.schema_version != 3
        || manifest.package_id.len() > 128
        || package.len() != 2
        || package
            .iter()
            .any(|segment| !valid_surface_segment(segment))
    {
        return Err(runtime_input_error(
            "Runtime surface observation requires a schema v3 portable package manifest.",
        ));
    }
    Ok(())
}

fn validate_scope_and_digest(scope_id: &str, package_digest: &str) -> UseResult<()> {
    if !valid_machine_id(scope_id) || !valid_sha256(package_digest) {
        return Err(runtime_input_error(
            "Runtime surface observation requires an explicit scope and canonical package digest.",
        ));
    }
    Ok(())
}

fn validate_binding(
    receipt: &RuntimeBindingReceipt,
    package_digest: &str,
    expected: ExpectedRuntimeBinding,
) -> UseResult<()> {
    if receipt.package_digest() != package_digest {
        return Err(UseError::new(
            "use.plugin.runtime.binding_package_mismatch",
            "The Runtime binding belongs to a different immutable package generation.",
        )
        .with_detail("expectedPackageDigest", package_digest)
        .with_detail("bindingPackageDigest", receipt.package_digest()));
    }
    let matches = match (expected, receipt) {
        (ExpectedRuntimeBinding::Task, RuntimeBindingReceipt::Task(_)) => true,
        (
            ExpectedRuntimeBinding::ToolService { base_path },
            RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
                contract:
                    RuntimeSurfaceContract::ToolService {
                        base_path: bound_path,
                        ..
                    },
                ..
            }),
        ) => &base_path == bound_path,
        (
            ExpectedRuntimeBinding::McpService,
            RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
                contract: RuntimeSurfaceContract::McpService { .. },
                ..
            }),
        ) => true,
        _ => false,
    };
    if !matches {
        return Err(UseError::new(
            "use.plugin.runtime.binding_contract_mismatch",
            "The Runtime binding class does not match the installed surface workload.",
        ));
    }
    Ok(())
}
