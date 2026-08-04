use std::path::PathBuf;

use a3s_flow::{NativeTsRuntime, NativeTsRuntimeConfig, WorkflowSpec};
use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef, UseResult};
use a3s_use_extension::{inspect_flow_surface_file, PluginFlowSurface};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::{
    PluginFlowLifecycleHost, PluginLifecycleEvidence, PluginLifecycleIntent,
};

use super::model::{digest_artifact, flow_error, FlowRuntimeBindingSpec};
use super::{FlowRuntimeBinding, FlowRuntimeBindingStore};

/// Lifecycle adapter that delegates compilation exclusively to `a3s-flow`
/// and retains exact-generation preflight evidence for capability projection.
#[derive(Debug, Clone)]
pub struct A3sFlowLifecycleHost {
    package_root: PathBuf,
    runtime: NativeTsRuntime,
    store: FlowRuntimeBindingStore,
}

impl A3sFlowLifecycleHost {
    pub fn new(
        package_root: impl Into<PathBuf>,
        compiler_binary: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
        store: FlowRuntimeBindingStore,
    ) -> Self {
        let package_root = package_root.into();
        let runtime = NativeTsRuntime::new(NativeTsRuntimeConfig::new(
            compiler_binary,
            cache_dir,
            package_root.clone(),
        ));
        Self {
            package_root,
            runtime,
            store,
        }
    }

    pub fn package_root(&self) -> &std::path::Path {
        &self.package_root
    }

    pub fn store(&self) -> &FlowRuntimeBindingStore {
        &self.store
    }

    async fn prepare(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let qualified = validate_call(intent, surface)?;
        if let Some(binding) = self
            .store
            .get(&intent.scope_id, &qualified, intent.generation)
            .await?
        {
            validate_binding(intent, surface, &binding)?;
            binding.inspect(surface, &self.package_root).await?;
            return checkpoint_evidence(
                "flow-prepared",
                idempotency_key,
                &binding.descriptor_digest()?,
            );
        }

        let source = inspect_flow_surface_file(surface, &self.package_root).await?;
        let spec = WorkflowSpec::native_ts(
            format!("{}:{}", intent.package_id, surface.id),
            format!("generation-{}", intent.generation),
            surface.source.to_string_lossy(),
            surface.export_name.clone(),
        );
        let preflight = self.runtime.preflight(&spec).await.map_err(|error| {
            flow_error(
                "use.plugin.flow_preflight_failed",
                format!(
                    "a3s-flow failed to preflight '{}:{}': {error}",
                    intent.package_id, surface.id
                ),
            )
        })?;
        let artifact_sha256 = digest_artifact(&preflight.artifact).await?;
        let binding = FlowRuntimeBinding::new(FlowRuntimeBindingSpec {
            scope_id: intent.scope_id.clone(),
            surface: qualified,
            generation: intent.generation,
            package_digest: intent.package_digest.clone(),
            manifest_digest: intent.manifest_digest.clone(),
            engine: surface.engine,
            runtime: surface.runtime,
            source_digest: source.digest().to_string(),
            export_name: surface.export_name.clone(),
            entrypoint: preflight.entrypoint,
            artifact: preflight.artifact,
            artifact_sha256,
            source_hash: preflight.source_hash,
        })?;
        validate_binding(intent, surface, &binding)?;
        self.store.put(&binding).await?;
        checkpoint_evidence(
            "flow-prepared",
            idempotency_key,
            &binding.descriptor_digest()?,
        )
    }

    async fn stop(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let qualified = validate_call(intent, surface)?;
        let subject = match self
            .store
            .get(&intent.scope_id, &qualified, intent.generation)
            .await?
        {
            Some(binding) => {
                validate_binding(intent, surface, &binding)?;
                binding.descriptor_digest()?
            }
            None => missing_subject_digest(intent, surface),
        };
        checkpoint_evidence("flow-stopped", idempotency_key, &subject)
    }

    async fn remove(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let qualified = validate_call(intent, surface)?;
        let Some(binding) = self
            .store
            .get(&intent.scope_id, &qualified, intent.generation)
            .await?
        else {
            return checkpoint_evidence(
                "flow-removed",
                idempotency_key,
                &missing_subject_digest(intent, surface),
            );
        };
        validate_binding(intent, surface, &binding)?;
        let subject = binding.descriptor_digest()?;
        self.store.remove(&binding).await?;
        checkpoint_evidence("flow-removed", idempotency_key, &subject)
    }
}

#[async_trait]
impl PluginFlowLifecycleHost for A3sFlowLifecycleHost {
    async fn prepare_flow(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.prepare(intent, surface, idempotency_key).await
    }

    async fn stop_flow(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.stop(intent, surface, idempotency_key).await
    }

    async fn remove_flow(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.remove(intent, surface, idempotency_key).await
    }
}

fn validate_call(
    intent: &PluginLifecycleIntent,
    surface: &PluginFlowSurface,
) -> UseResult<PlanQualifiedSurfaceRef> {
    intent.validate()?;
    let reference = PluginSurfaceRef {
        kind: PluginSurfaceKind::Flow,
        id: surface.id.clone(),
    };
    if !intent
        .surfaces
        .iter()
        .any(|candidate| candidate.surface == reference)
    {
        return Err(flow_error(
            "use.plugin.flow_surface_mismatch",
            "The A3S Flow lifecycle call is absent from the admitted package surface inventory.",
        ));
    }
    Ok(PlanQualifiedSurfaceRef {
        package_id: intent.package_id.clone(),
        surface: reference,
    })
}

fn validate_binding(
    intent: &PluginLifecycleIntent,
    surface: &PluginFlowSurface,
    binding: &FlowRuntimeBinding,
) -> UseResult<()> {
    binding.validate()?;
    if binding.scope_id() != intent.scope_id
        || binding.surface().package_id != intent.package_id
        || binding.surface().surface.kind != PluginSurfaceKind::Flow
        || binding.surface().surface.id != surface.id
        || binding.generation() != intent.generation
        || binding.package_digest() != intent.package_digest
        || binding.manifest_digest() != intent.manifest_digest
    {
        return Err(flow_error(
            "use.plugin.flow_binding_mismatch",
            "The retained A3S Flow binding does not belong to the exact cognitive-package generation.",
        ));
    }
    Ok(())
}

fn missing_subject_digest(intent: &PluginLifecycleIntent, surface: &PluginFlowSurface) -> String {
    let identity = format!(
        "{}\n{}\n{}\n{}\n{}",
        intent.scope_id, intent.package_id, surface.id, intent.generation, intent.package_digest
    );
    format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
}

fn checkpoint_evidence(
    label: &str,
    idempotency_key: &str,
    subject_digest: &str,
) -> UseResult<PluginLifecycleEvidence> {
    let identity = format!("{label}\n{idempotency_key}\n{subject_digest}");
    PluginLifecycleEvidence::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}
