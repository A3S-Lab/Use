use std::sync::Arc;

use a3s_use_core::{
    inspect_okf_bundle_files, OkfBundleContract, OkfBundleFile, OkfKnowledgeObservation,
    OkfKnowledgeObservedState, OkfProjectionReceipt, PlanQualifiedSurfaceRef, PluginPackageId,
    PluginSurfaceKind, UseError, UseResult,
};
use async_trait::async_trait;

use super::OkfKnowledgeBinding;

/// Immutable identity reviewed before one OKF candidate is staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfKnowledgeStageSpec {
    pub operation_id: String,
    pub scope_id: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
    pub bundle: OkfBundleContract,
}

impl OkfKnowledgeStageSpec {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.operation_id)
            || !valid_machine_id(&self.scope_id)
            || PluginPackageId::parse(self.surface.package_id.clone()).is_err()
            || self.surface.surface.kind != PluginSurfaceKind::Okf
            || !valid_segment(&self.surface.surface.id)
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || self.bundle.validate().is_err()
        {
            return Err(stage_request_error(
                "The OKF Knowledge stage identity or immutable package evidence is invalid.",
            ));
        }
        Ok(())
    }
}

/// Exact, bounded OKF bytes handed to an injected A3S Knowledge adapter.
///
/// Construction re-runs OKF conformance against the reviewed bundle contract,
/// avoiding a path-based time-of-check/time-of-use gap at the adapter port.
#[derive(Debug, PartialEq, Eq)]
pub struct OkfKnowledgeStageRequest {
    spec: OkfKnowledgeStageSpec,
    files: Vec<OkfBundleFile>,
}

impl OkfKnowledgeStageRequest {
    pub fn new(spec: OkfKnowledgeStageSpec, files: Vec<OkfBundleFile>) -> UseResult<Self> {
        spec.validate()?;
        let inspection = inspect_okf_bundle_files(
            spec.bundle.format_version,
            spec.bundle.limits.clone(),
            &files,
        )?;
        spec.bundle.verify_inspection(&inspection)?;
        Ok(Self { spec, files })
    }

    pub fn spec(&self) -> &OkfKnowledgeStageSpec {
        &self.spec
    }

    pub fn files(&self) -> &[OkfBundleFile] {
        &self.files
    }

    pub fn validate_receipt(&self, receipt: &OkfProjectionReceipt) -> UseResult<()> {
        receipt.validate()?;
        if receipt.operation_id != self.spec.operation_id
            || receipt.scope_id != self.spec.scope_id
            || receipt.surface != self.spec.surface
            || receipt.generation != self.spec.generation
            || receipt.package_digest != self.spec.package_digest
            || receipt.manifest_digest != self.spec.manifest_digest
            || receipt.bundle != self.spec.bundle
        {
            return Err(adapter_evidence_error(
                "The A3S Knowledge stage receipt does not match the exact reviewed candidate.",
            ));
        }
        Ok(())
    }
}

/// Injected A3S Knowledge boundary for one exact OKF package generation.
///
/// Implementations own index staging and atomic promotion. They must remain
/// idempotent for the same receipt and must never mutate personal knowledge or
/// another package's projection while removing receipt-owned state.
#[async_trait]
pub trait OkfKnowledgeAdapter: Send + Sync {
    async fn stage(&self, request: &OkfKnowledgeStageRequest) -> UseResult<OkfKnowledgeBinding>;

    async fn promote(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation>;

    async fn observe(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation>;

    async fn remove(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation>;
}

/// Evidence-checking client around an injected Knowledge adapter.
#[derive(Clone)]
pub struct OkfKnowledgeClient {
    adapter: Arc<dyn OkfKnowledgeAdapter>,
}

impl OkfKnowledgeClient {
    pub fn new(adapter: Arc<dyn OkfKnowledgeAdapter>) -> Self {
        Self { adapter }
    }

    pub async fn stage(&self, request: OkfKnowledgeStageRequest) -> UseResult<OkfKnowledgeBinding> {
        let binding = self.adapter.stage(&request).await?;
        binding.validate()?;
        request.validate_receipt(&binding.receipt)?;
        if !matches!(
            binding.observation.state,
            OkfKnowledgeObservedState::Staged | OkfKnowledgeObservedState::Failed
        ) {
            return Err(adapter_evidence_error(
                "The A3S Knowledge stage operation returned a non-staged terminal state.",
            ));
        }
        Ok(binding)
    }

    pub async fn promote(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeBinding> {
        let observation = self.adapter.promote(receipt).await?;
        observation.validate_for_receipt(receipt)?;
        if !matches!(
            observation.state,
            OkfKnowledgeObservedState::Promoted | OkfKnowledgeObservedState::Failed
        ) {
            return Err(adapter_evidence_error(
                "The A3S Knowledge promote operation returned neither promoted nor failed evidence.",
            ));
        }
        OkfKnowledgeBinding::new(receipt.clone(), observation)
    }

    pub async fn observe(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeBinding> {
        let observation = self.adapter.observe(receipt).await?;
        OkfKnowledgeBinding::new(receipt.clone(), observation)
    }

    pub async fn remove(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeBinding> {
        let observation = self.adapter.remove(receipt).await?;
        observation.validate_for_receipt(receipt)?;
        if observation.state != OkfKnowledgeObservedState::Removed {
            return Err(adapter_evidence_error(
                "The A3S Knowledge remove operation did not return removed evidence.",
            ));
        }
        OkfKnowledgeBinding::new(receipt.clone(), observation)
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn stage_request_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_stage_request_invalid", message)
}

fn adapter_evidence_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_adapter_evidence_mismatch", message)
}
