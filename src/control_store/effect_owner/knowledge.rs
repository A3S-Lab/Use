use a3s_use_core::{
    OkfBundleContract, OkfCapabilityProjection, OkfKnowledgeObservedState, PlanQualifiedSurfaceRef,
    PluginSurfaceKind, UseError, UseResult,
};
use a3s_use_extension::ArtifactStore;
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::super::effect_port::{
    ControlEffectFailure, ControlEffectPortOutcome, ControlKnowledgeEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
};
use super::super::model::{valid_error_code, ControlEffectOwner};
use crate::okf_knowledge::{
    OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient, OkfKnowledgeStageRequest,
    OkfKnowledgeStageSpec,
};

const KNOWLEDGE_RECEIPT_DOMAIN: &[u8] = b"a3s.use.control-knowledge-receipt.v1\0";
const KNOWLEDGE_FAILURE_DOMAIN: &[u8] = b"a3s.use.control-knowledge-failure.v1\0";
const KNOWLEDGE_AUTHORITY_ERROR: &str = "use.control_store.knowledge_authority_invalid";
const KNOWLEDGE_OWNER_ERROR: &str = "use.control_store.knowledge_owner_failed";
const KNOWLEDGE_STAGE_FAILED: &str = "use.control_store.knowledge_stage_failed";
const KNOWLEDGE_PROMOTION_FAILED: &str = "use.control_store.knowledge_promotion_failed";
const KNOWLEDGE_GENERATION_REMOVED: &str = "use.control_store.knowledge_generation_removed";

/// Inactive Control owner for receipt-owned OKF Knowledge projections.
///
/// Desired state comes only from the committed Control request. First-time
/// preparation reads exact bytes through a verified Artifact Store lease;
/// replay proceeds from retained Knowledge receipt evidence. The binding store
/// records external payload evidence but never selects a package generation.
#[derive(Clone)]
pub(in crate::control_store) struct ControlOkfKnowledgeEffectPort {
    artifact_store: ArtifactStore,
    client: OkfKnowledgeClient,
    bindings: OkfKnowledgeBindingStore,
}

impl ControlOkfKnowledgeEffectPort {
    pub(in crate::control_store) fn new(
        artifact_store: ArtifactStore,
        client: OkfKnowledgeClient,
        bindings: OkfKnowledgeBindingStore,
    ) -> Self {
        Self {
            artifact_store,
            client,
            bindings,
        }
    }

    async fn apply(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let bundle = match self.validate_request(request) {
            Ok(bundle) => bundle,
            Err(error) => return rejected(request, "authority", error.code),
        };
        match request.action {
            ControlSurfaceEffectAction::Prepare => self.prepare(request, bundle).await,
            ControlSurfaceEffectAction::Stop => self.stop(request, bundle).await,
            ControlSurfaceEffectAction::Remove => self.remove(request, bundle).await,
        }
    }

    fn validate_request<'a>(
        &self,
        request: &'a ControlSurfaceEffectRequest,
    ) -> UseResult<&'a OkfBundleContract> {
        self.bindings
            .installation()
            .ensure_same(&request.identity.installation)
            .map_err(|_| authority_error())?;
        request
            .validate_for_owner(PluginSurfaceKind::Okf, ControlEffectOwner::KnowledgeHost)
            .map_err(|_| authority_error())?;
        request
            .authority
            .package
            .package
            .catalog
            .record
            .surfaces
            .iter()
            .find(|surface| {
                surface.kind == PluginSurfaceKind::Okf && surface.id == request.surface.id
            })
            .and_then(|surface| surface.okf_bundle.as_ref())
            .ok_or_else(authority_error)
    }

    async fn prepare(
        &self,
        request: &ControlSurfaceEffectRequest,
        bundle: &OkfBundleContract,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let qualified = qualified_surface(request);
        let existing = match self
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
        {
            Ok(binding) => binding,
            Err(error) => return before_effect_failure(request, "binding-read", error),
        };
        if let Some(binding) = existing {
            if let Err(error) = validate_binding(request, bundle, &binding) {
                return rejected(request, "binding", error.code);
            }
            return match binding.observation.state {
                OkfKnowledgeObservedState::Promoted => promoted_application(request, &binding),
                OkfKnowledgeObservedState::Staged => self.promote(request, bundle, binding).await,
                OkfKnowledgeObservedState::Failed => {
                    rejected(request, "stage", KNOWLEDGE_STAGE_FAILED)
                }
                OkfKnowledgeObservedState::Removed => {
                    rejected(request, "stage", KNOWLEDGE_GENERATION_REMOVED)
                }
            };
        }

        let package = match self
            .artifact_store
            .acquire_verified_package(&request.authority.package.package.catalog)
            .await
        {
            Ok(package) => package,
            Err(error) => return before_effect_failure(request, "artifact-acquire", error),
        };
        let payload = match package.read_okf_surface(&request.surface.id).await {
            Ok(payload) => payload,
            Err(error) => return before_effect_failure(request, "artifact-read", error),
        };
        if payload.bundle() != bundle {
            return rejected(request, "artifact-binding", KNOWLEDGE_AUTHORITY_ERROR);
        }
        let (payload_bundle, files) = payload.into_parts();
        let stage = match OkfKnowledgeStageRequest::new(
            OkfKnowledgeStageSpec {
                operation_id: request.identity.operation_id.clone(),
                scope: request.identity.installation.clone(),
                surface: qualified,
                generation: request.lifecycle_generation,
                package_digest: request.package_digest.clone(),
                manifest_digest: request.manifest_digest.clone(),
                bundle: payload_bundle,
            },
            files,
        ) {
            Ok(stage) => stage,
            Err(error) => return rejected(request, "stage-request", error.code),
        };
        let staged = match self.client.stage(stage).await {
            Ok(binding) => binding,
            Err(error) => return unknown(request, "stage-call", error),
        };
        if let Err(error) = validate_binding(request, bundle, &staged) {
            return unknown(request, "stage-evidence", error);
        }
        if let Err(error) = self.bindings.put(&staged).await {
            return unknown(request, "stage-receipt", error);
        }
        if staged.observation.state == OkfKnowledgeObservedState::Failed {
            return rejected(request, "stage", KNOWLEDGE_STAGE_FAILED);
        }
        self.promote(request, bundle, staged).await
    }

    async fn promote(
        &self,
        request: &ControlSurfaceEffectRequest,
        bundle: &OkfBundleContract,
        staged: OkfKnowledgeBinding,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let promoted = match self.client.promote(&staged.receipt).await {
            Ok(binding) => binding,
            Err(error) => return unknown(request, "promote-call", error),
        };
        if let Err(error) = validate_binding(request, bundle, &promoted) {
            return unknown(request, "promote-evidence", error);
        }
        if let Err(error) = self.bindings.put(&promoted).await {
            return unknown(request, "promote-receipt", error);
        }
        if promoted.observation.state == OkfKnowledgeObservedState::Failed {
            return rejected(request, "promote", KNOWLEDGE_PROMOTION_FAILED);
        }
        promoted_application(request, &promoted)
    }

    async fn stop(
        &self,
        request: &ControlSurfaceEffectRequest,
        bundle: &OkfBundleContract,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let qualified = qualified_surface(request);
        let subject_digest = match self
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
        {
            Ok(Some(binding)) => {
                if let Err(error) = validate_binding(request, bundle, &binding) {
                    return rejected(request, "binding", error.code);
                }
                match binding.observation.descriptor_digest() {
                    Ok(digest) => digest,
                    Err(error) => return rejected(request, "binding", error.code),
                }
            }
            Ok(None) => missing_subject_digest(request, bundle),
            Err(error) => return before_effect_failure(request, "binding-read", error),
        };
        let receipt = checkpoint_digest(request, "stopped", &subject_digest);
        application(request, receipt, None, "stop-evidence")
    }

    async fn remove(
        &self,
        request: &ControlSurfaceEffectRequest,
        bundle: &OkfBundleContract,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let qualified = qualified_surface(request);
        let binding = match self
            .bindings
            .get(
                &request.identity.installation,
                &qualified,
                request.lifecycle_generation,
            )
            .await
        {
            Ok(Some(binding)) => binding,
            Ok(None) => {
                let subject = missing_subject_digest(request, bundle);
                let receipt = checkpoint_digest(request, "removed", &subject);
                return application(request, receipt, None, "remove-evidence");
            }
            Err(error) => return before_effect_failure(request, "binding-read", error),
        };
        if let Err(error) = validate_binding(request, bundle, &binding) {
            return rejected(request, "binding", error.code);
        }
        let removed = if binding.observation.state == OkfKnowledgeObservedState::Removed {
            binding
        } else {
            let removed = match self.client.remove(&binding.receipt).await {
                Ok(binding) => binding,
                Err(error) => return unknown(request, "remove-call", error),
            };
            if let Err(error) = validate_binding(request, bundle, &removed) {
                return unknown(request, "remove-evidence", error);
            }
            if let Err(error) = self.bindings.put(&removed).await {
                return unknown(request, "remove-receipt", error);
            }
            removed
        };
        let receipt = match removed.observation.descriptor_digest() {
            Ok(digest) => digest,
            Err(error) => return unknown(request, "remove-digest", error),
        };
        application(request, receipt, None, "remove-application")
    }
}

#[async_trait]
impl ControlKnowledgeEffectPort for ControlOkfKnowledgeEffectPort {
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        self.apply(request).await
    }
}

fn qualified_surface(request: &ControlSurfaceEffectRequest) -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: request.package_id.clone(),
        surface: request.surface.clone(),
    }
}

fn validate_binding(
    request: &ControlSurfaceEffectRequest,
    bundle: &OkfBundleContract,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    binding.validate()?;
    let receipt = &binding.receipt;
    if receipt.scope != request.identity.installation
        || receipt.surface != qualified_surface(request)
        || receipt.generation != request.lifecycle_generation
        || receipt.package_digest != request.package_digest
        || receipt.manifest_digest != request.manifest_digest
        || &receipt.bundle != bundle
        || (receipt.operation_id == request.identity.operation_id
            && receipt.staged_at_ms < request.authority.committed_at_ms)
    {
        return Err(authority_error());
    }
    Ok(())
}

fn promoted_application(
    request: &ControlSurfaceEffectRequest,
    binding: &OkfKnowledgeBinding,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let receipt = match binding.observation.descriptor_digest() {
        Ok(digest) => digest,
        Err(error) => return unknown(request, "projection-receipt", error),
    };
    let projection =
        match OkfCapabilityProjection::from_promoted(&binding.receipt, &binding.observation) {
            Ok(projection) => projection,
            Err(error) => return unknown(request, "projection", error),
        };
    let projection_digest = match projection.descriptor_digest() {
        Ok(digest) => digest,
        Err(error) => return unknown(request, "projection-digest", error),
    };
    application(
        request,
        receipt,
        Some(projection_digest),
        "prepare-application",
    )
}

fn application(
    request: &ControlSurfaceEffectRequest,
    receipt_digest: String,
    materialization_digest: Option<String>,
    phase: &str,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    match ControlSurfaceApplication::new(request, receipt_digest, materialization_digest) {
        Ok(application) => ControlEffectPortOutcome::applied(application),
        Err(error) => unknown(request, phase, error),
    }
}

fn before_effect_failure(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error: UseError,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let code = normalized_error_code(error);
    let failure = failure(request, phase, &code);
    if matches!(
        code.as_str(),
        "use.artifact_store.busy"
            | "use.artifact_store.io"
            | "use.extension.io"
            | "use.okf.knowledge_binding_io"
            | "use.state.maintenance_io"
            | "use.state.maintenance_lock_failed"
    ) {
        ControlEffectPortOutcome::deferred(failure)
    } else {
        ControlEffectPortOutcome::rejected(failure)
    }
}

fn rejected(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error_code: impl Into<String>,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let error_code = error_code.into();
    let error_code = if valid_error_code(&error_code) {
        error_code
    } else {
        KNOWLEDGE_OWNER_ERROR.to_string()
    };
    ControlEffectPortOutcome::rejected(failure(request, phase, &error_code))
}

fn unknown(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error: UseError,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let code = normalized_error_code(error);
    ControlEffectPortOutcome::unknown(failure(request, phase, &code))
}

fn normalized_error_code(error: UseError) -> String {
    if valid_error_code(&error.code) {
        error.code
    } else {
        KNOWLEDGE_OWNER_ERROR.to_string()
    }
}

fn failure(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error_code: &str,
) -> ControlEffectFailure {
    let mut hasher = Sha256::new();
    hasher.update(KNOWLEDGE_FAILURE_DOMAIN);
    hash_field(&mut hasher, &request.identity.idempotency_key);
    hash_field(&mut hasher, phase);
    hash_field(&mut hasher, error_code);
    ControlEffectFailure {
        evidence_digest: format!("sha256:{:x}", hasher.finalize()),
        error_code: error_code.to_string(),
    }
}

fn checkpoint_digest(
    request: &ControlSurfaceEffectRequest,
    state: &str,
    subject_digest: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(KNOWLEDGE_RECEIPT_DOMAIN);
    hash_field(&mut hasher, &request.identity.idempotency_key);
    hash_field(&mut hasher, state);
    hash_field(&mut hasher, subject_digest);
    format!("sha256:{:x}", hasher.finalize())
}

fn missing_subject_digest(
    request: &ControlSurfaceEffectRequest,
    bundle: &OkfBundleContract,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.use.control-knowledge-missing-subject.v1\0");
    hash_field(&mut hasher, request.identity.installation.kind.as_str());
    hash_field(&mut hasher, &request.identity.installation.id);
    hash_field(&mut hasher, &request.package_id);
    hash_field(&mut hasher, &request.surface.id);
    hasher.update(request.lifecycle_generation.to_be_bytes());
    hash_field(&mut hasher, &request.package_digest);
    hash_field(&mut hasher, &request.manifest_digest);
    hash_field(&mut hasher, &bundle.content_digest);
    format!("sha256:{:x}", hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn authority_error() -> UseError {
    UseError::new(
        KNOWLEDGE_AUTHORITY_ERROR,
        "Knowledge execution requires one exact committed OKF package authority.",
    )
}
