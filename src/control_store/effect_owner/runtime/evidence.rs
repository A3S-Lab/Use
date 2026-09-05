use super::*;
use sha2::{Digest, Sha256};

use super::super::super::effect_port::ControlEffectFailure;
use super::super::super::model::valid_error_code;

pub(super) fn prepare_application(
    request: &ControlRuntimeEffectRequest,
    receipt: &RuntimeBindingReceipt,
    phase: &str,
) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
    let binding = match binding_observation(receipt) {
        Ok(binding) => binding,
        Err(error) => return unknown(request, phase, error),
    };
    let digest = match receipt_digest(request, receipt) {
        Ok(digest) => digest,
        Err(error) => return unknown(request, phase, error),
    };
    let schema_attestation = match schema_attestation(receipt) {
        Ok(attestation) => attestation,
        Err(error) => return unknown(request, phase, error),
    };
    match ControlRuntimeApplication::new_with_schema_attestation(
        request,
        digest,
        Some(binding),
        schema_attestation,
    ) {
        Ok(application) => ControlEffectPortOutcome::applied(application),
        Err(error) => unknown(request, phase, error),
    }
}

pub(super) fn checkpoint_application(
    request: &ControlRuntimeEffectRequest,
    state: &str,
    receipt: Option<&RuntimeBindingReceipt>,
) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
    let digest = match checkpoint_digest(request, state, receipt) {
        Ok(digest) => digest,
        Err(error) => return unknown(request, "checkpoint", error),
    };
    match ControlRuntimeApplication::new(request, digest, None) {
        Ok(application) => ControlEffectPortOutcome::applied(application),
        Err(error) => unknown(request, "checkpoint", error),
    }
}

fn checkpoint_digest(
    request: &ControlRuntimeEffectRequest,
    state: &str,
    receipt: Option<&RuntimeBindingReceipt>,
) -> UseResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(RUNTIME_RECEIPT_DOMAIN);
    hash_field(&mut hasher, "checkpoint");
    hash_field(&mut hasher, state);
    hash_field(&mut hasher, &request.surface.identity.idempotency_key);
    hash_field(&mut hasher, &request.provider_id);
    hash_field(&mut hasher, &request.selection_digest);
    if let Some(receipt) = receipt {
        let bytes = serde_json::to_vec(receipt).map_err(|error| {
            runtime_error(
                RUNTIME_OWNER_ERROR,
                format!("Failed to encode Runtime checkpoint receipt: {error}"),
            )
        })?;
        hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(bytes);
    } else {
        hash_field(&mut hasher, "none");
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn binding_observation(
    receipt: &RuntimeBindingReceipt,
) -> UseResult<super::super::super::model::ControlRuntimeBindingObservation> {
    match receipt {
        RuntimeBindingReceipt::Task(_) => {
            Ok(super::super::super::model::ControlRuntimeBindingObservation::Task)
        }
        RuntimeBindingReceipt::Service(service) => Ok(
            super::super::super::model::ControlRuntimeBindingObservation::Service {
                endpoint_ref: service.endpoint_ref.as_str().to_string(),
                readiness_digest: readiness_digest(service)?,
            },
        ),
    }
}

fn schema_attestation(
    receipt: &RuntimeBindingReceipt,
) -> UseResult<Option<super::super::super::model::ControlRuntimeSchemaAttestation>> {
    receipt
        .tool_schema_attestation()
        .map(|attestation| {
            super::super::super::model::ControlRuntimeSchemaAttestation::new(
                attestation.descriptor_digest.clone(),
                attestation.input_schema_digest.clone(),
                attestation.output_schema_digest.clone(),
            )
        })
        .transpose()
}

fn receipt_digest(
    request: &ControlRuntimeEffectRequest,
    receipt: &RuntimeBindingReceipt,
) -> UseResult<String> {
    let bytes = serde_json::to_vec(receipt).map_err(|error| {
        runtime_error(
            RUNTIME_OWNER_ERROR,
            format!("Failed to encode Runtime receipt evidence: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(RUNTIME_RECEIPT_DOMAIN);
    hash_field(&mut hasher, &request.surface.identity.idempotency_key);
    hash_field(&mut hasher, &request.provider_id);
    hash_field(&mut hasher, &request.selection_digest);
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn readiness_digest(receipt: &RuntimeServiceBindingReceipt) -> UseResult<String> {
    let bytes = serde_json::to_vec(receipt).map_err(|error| {
        runtime_error(
            RUNTIME_OWNER_ERROR,
            format!("Failed to encode Runtime readiness evidence: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.use.control-runtime-readiness.v1\0");
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub(super) fn service_endpoint(
    plan: &RuntimeSurfacePlan,
    observation: &RuntimeObservation,
) -> UseResult<RuntimeServiceEndpoint> {
    let port_name = match plan.contract() {
        RuntimeSurfaceContract::ToolService { port_name, .. }
        | RuntimeSurfaceContract::McpService { port_name, .. } => port_name,
        RuntimeSurfaceContract::ToolTask { .. } => {
            return Err(runtime_error(
                RUNTIME_PLAN_ERROR,
                "A Runtime Task cannot expose a Service endpoint.",
            ));
        }
    };
    RuntimeServiceEndpoint::from_observation(observation, port_name)
        .map_err(|message| runtime_error(RUNTIME_PLAN_ERROR, message))
}

pub(super) fn runtime_request_id(operation: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.use.control-runtime-request.v1\0");
    hash_field(&mut hasher, operation);
    hash_field(&mut hasher, idempotency_key);
    format!("control-runtime-{operation}-{:x}", hasher.finalize())
}

pub(super) fn before_effect_failure(
    request: &ControlRuntimeEffectRequest,
    phase: &str,
    error: UseError,
) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
    let code = normalized_error_code(error);
    let failure = failure(request, phase, &code);
    if matches!(
        code.as_str(),
        "use.artifact_store.busy"
            | "use.artifact_store.io"
            | "use.extension.io"
            | "use.plugin.runtime.provider_unavailable"
            | "use.plugin.runtime.plan_source_unavailable"
            | "use.plugin.runtime.deadline_exceeded"
            | "use.plugin.runtime.operation_failed"
            | "use.plugin.runtime.binding_io"
    ) {
        ControlEffectPortOutcome::deferred(failure)
    } else {
        ControlEffectPortOutcome::rejected(failure)
    }
}

pub(super) fn rejected(
    request: &ControlRuntimeEffectRequest,
    error_code: impl Into<String>,
) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
    let error_code = error_code.into();
    let error_code = if valid_error_code(&error_code) {
        error_code
    } else {
        RUNTIME_OWNER_ERROR.to_string()
    };
    ControlEffectPortOutcome::rejected(failure(request, "rejected", &error_code))
}

pub(super) fn unknown(
    request: &ControlRuntimeEffectRequest,
    phase: &str,
    error: UseError,
) -> ControlEffectPortOutcome<ControlRuntimeApplication> {
    let code = normalized_error_code(error);
    ControlEffectPortOutcome::unknown(failure(request, phase, &code))
}

fn normalized_error_code(error: UseError) -> String {
    if valid_error_code(&error.code) {
        error.code
    } else {
        RUNTIME_OWNER_ERROR.to_string()
    }
}

fn failure(
    request: &ControlRuntimeEffectRequest,
    phase: &str,
    error_code: &str,
) -> ControlEffectFailure {
    let mut hasher = Sha256::new();
    hasher.update(RUNTIME_FAILURE_DOMAIN);
    hash_field(&mut hasher, &request.surface.identity.idempotency_key);
    hash_field(&mut hasher, phase);
    hash_field(&mut hasher, error_code);
    ControlEffectFailure {
        evidence_digest: format!("sha256:{:x}", hasher.finalize()),
        error_code: error_code.to_string(),
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub(super) fn authority_error() -> UseError {
    runtime_error(
        RUNTIME_AUTHORITY_ERROR,
        "Runtime execution requires one exact committed provider and package authority.",
    )
}

pub(super) fn runtime_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
