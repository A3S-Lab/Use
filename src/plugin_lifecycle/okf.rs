use std::path::PathBuf;

use a3s_use_core::{
    OkfCapabilityProjection, OkfKnowledgeObservedState, PlanQualifiedSurfaceRef, PluginSurfaceKind,
    PluginSurfaceRef, UseError, UseResult,
};
use a3s_use_extension::{load_okf_bundle_files, PluginOkfSurface};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::okf_knowledge::{
    OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient, OkfKnowledgeStageRequest,
    OkfKnowledgeStageSpec,
};

use super::{PluginLifecycleEvidence, PluginLifecycleIntent, PluginOkfLifecycleHost};

/// Typed lifecycle adapter for receipt-owned OKF projections.
///
/// Preparation stages and promotes an exact immutable package generation while
/// persisting both observations. `stop_okf` deliberately does not delete an
/// index: the package capability projection was already hidden by the parent
/// saga. Removal delegates only the retained projection receipt to Knowledge;
/// the Knowledge contract forbids deleting personal or another package's data.
#[derive(Clone)]
pub struct OkfKnowledgeLifecycleHost {
    package_root: PathBuf,
    client: OkfKnowledgeClient,
    store: OkfKnowledgeBindingStore,
}

impl OkfKnowledgeLifecycleHost {
    pub fn new(
        package_root: impl Into<PathBuf>,
        client: OkfKnowledgeClient,
        store: OkfKnowledgeBindingStore,
    ) -> Self {
        Self {
            package_root: package_root.into(),
            client,
            store,
        }
    }

    pub fn package_root(&self) -> &std::path::Path {
        &self.package_root
    }

    pub fn store(&self) -> &OkfKnowledgeBindingStore {
        &self.store
    }

    async fn prepare(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let qualified = validate_call(intent, surface)?;
        if let Some(binding) = self
            .store
            .get(&intent.scope_id, &qualified, intent.generation)
            .await?
        {
            validate_binding(intent, surface, &binding)?;
            return match binding.observation.state {
                OkfKnowledgeObservedState::Promoted => {
                    promoted_evidence(idempotency_key, &binding)
                }
                OkfKnowledgeObservedState::Staged => {
                    self.promote(intent, surface, idempotency_key, binding)
                        .await
                }
                OkfKnowledgeObservedState::Failed => Err(okf_lifecycle_error(
                    "use.plugin.okf_generation_failed",
                    "The retained OKF candidate generation failed and cannot be promoted in place.",
                )),
                OkfKnowledgeObservedState::Removed => Err(okf_lifecycle_error(
                    "use.plugin.okf_generation_removed",
                    "A removed OKF generation cannot be prepared again under the same generation identity.",
                )),
            };
        }

        let files = load_okf_bundle_files(surface, &self.package_root).await?;
        let request = OkfKnowledgeStageRequest::new(
            OkfKnowledgeStageSpec {
                operation_id: intent.operation_id.clone(),
                scope_id: intent.scope_id.clone(),
                surface: qualified,
                generation: intent.generation,
                package_digest: intent.package_digest.clone(),
                manifest_digest: intent.manifest_digest.clone(),
                bundle: surface.bundle.clone(),
            },
            files,
        )?;
        let staged = self.client.stage(request).await?;
        self.store.put(&staged).await?;
        if staged.observation.state == OkfKnowledgeObservedState::Failed {
            return Err(okf_lifecycle_error(
                "use.plugin.okf_stage_failed",
                "A3S Knowledge failed to stage the exact OKF package generation.",
            ));
        }
        self.promote(intent, surface, idempotency_key, staged).await
    }

    async fn promote(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
        staged: OkfKnowledgeBinding,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_binding(intent, surface, &staged)?;
        let promoted = self.client.promote(&staged.receipt).await?;
        self.store.put(&promoted).await?;
        if promoted.observation.state == OkfKnowledgeObservedState::Failed {
            return Err(okf_lifecycle_error(
                "use.plugin.okf_promotion_failed",
                "A3S Knowledge failed to promote the exact OKF package generation.",
            ));
        }
        promoted_evidence(idempotency_key, &promoted)
    }

    async fn stop(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
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
                binding.observation.descriptor_digest()?
            }
            None => missing_subject_digest(intent, surface),
        };
        checkpoint_evidence("okf-hidden", idempotency_key, &subject)
    }

    async fn remove(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        let qualified = validate_call(intent, surface)?;
        let Some(binding) = self
            .store
            .get(&intent.scope_id, &qualified, intent.generation)
            .await?
        else {
            return checkpoint_evidence(
                "okf-removed",
                idempotency_key,
                &missing_subject_digest(intent, surface),
            );
        };
        validate_binding(intent, surface, &binding)?;
        let removed = if binding.observation.state == OkfKnowledgeObservedState::Removed {
            binding
        } else {
            let removed = self.client.remove(&binding.receipt).await?;
            self.store.put(&removed).await?;
            removed
        };
        checkpoint_evidence(
            "okf-removed",
            idempotency_key,
            &removed.observation.descriptor_digest()?,
        )
    }
}

#[async_trait]
impl PluginOkfLifecycleHost for OkfKnowledgeLifecycleHost {
    async fn prepare_okf(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.prepare(intent, surface, idempotency_key).await
    }

    async fn stop_okf(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.stop(intent, surface, idempotency_key).await
    }

    async fn remove_okf(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        self.remove(intent, surface, idempotency_key).await
    }
}

fn validate_call(
    intent: &PluginLifecycleIntent,
    surface: &PluginOkfSurface,
) -> UseResult<PlanQualifiedSurfaceRef> {
    intent.validate()?;
    let reference = PluginSurfaceRef {
        kind: PluginSurfaceKind::Okf,
        id: surface.id.clone(),
    };
    intent
        .surfaces
        .iter()
        .find(|candidate| candidate.surface == reference)
        .ok_or_else(|| {
            okf_lifecycle_error(
                "use.plugin.okf_surface_mismatch",
                "The OKF lifecycle call is absent from the admitted package surface inventory.",
            )
        })?;
    Ok(PlanQualifiedSurfaceRef {
        package_id: intent.package_id.clone(),
        surface: reference,
    })
}

fn validate_binding(
    intent: &PluginLifecycleIntent,
    surface: &PluginOkfSurface,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    binding.validate()?;
    if binding.receipt.scope_id != intent.scope_id
        || binding.receipt.surface.package_id != intent.package_id
        || binding.receipt.surface.surface.kind != PluginSurfaceKind::Okf
        || binding.receipt.surface.surface.id != surface.id
        || binding.receipt.generation != intent.generation
        || binding.receipt.package_digest != intent.package_digest
        || binding.receipt.manifest_digest != intent.manifest_digest
        || binding.receipt.bundle != surface.bundle
    {
        return Err(okf_lifecycle_error(
            "use.plugin.okf_binding_mismatch",
            "The retained OKF binding does not belong to the exact cognitive-package generation.",
        ));
    }
    Ok(())
}

fn promoted_evidence(
    idempotency_key: &str,
    binding: &OkfKnowledgeBinding,
) -> UseResult<PluginLifecycleEvidence> {
    let projection =
        OkfCapabilityProjection::from_promoted(&binding.receipt, &binding.observation)?;
    checkpoint_evidence(
        "okf-promoted",
        idempotency_key,
        &projection.descriptor_digest()?,
    )
}

fn missing_subject_digest(intent: &PluginLifecycleIntent, surface: &PluginOkfSurface) -> String {
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

fn okf_lifecycle_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests;
