use std::sync::Arc;

use a3s_runtime::contract::{
    IsolationLevel, RuntimeApplyRequest, RuntimeCapabilities, RuntimeFeature, RuntimeLogQuery,
    RuntimeLogStream, RuntimeUnitClass, RuntimeUnitState,
};
use a3s_runtime::{RuntimeClient, RuntimeError};
use a3s_use_core::{PlanEnforcementProfile, PlannedProviderEvidence, UseError, UseResult};
use sha2::{Digest, Sha256};

use super::model::{
    runtime_contract_error, RuntimePreparedTaskBinding, RuntimeServiceActivation,
    RuntimeSurfaceContract, RuntimeSurfacePlan, RuntimeTaskExecution,
};
use super::receipt::RuntimeBindingReceipt;

const MAX_IN_MEMORY_TASK_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
const LOG_QUERY_CHUNKS: u32 = 64;
const MAX_LOG_QUERY_ROUNDS: usize = 1_024;

#[derive(Clone)]
pub struct PluginRuntimeClient {
    client: Arc<dyn RuntimeClient>,
}

impl PluginRuntimeClient {
    pub fn new(client: Arc<dyn RuntimeClient>) -> Self {
        Self { client }
    }

    pub async fn verify_plan(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
    ) -> UseResult<RuntimeCapabilities> {
        validate_plan_evidence(plan, provider)?;
        let capabilities = self
            .client
            .capabilities()
            .await
            .map_err(|error| runtime_error("read Runtime capabilities", error))?;
        capabilities.validate().map_err(runtime_contract_error)?;

        let capability_digest = runtime_capabilities_digest(&capabilities)?;
        if capabilities.provider_id.as_str() != provider.provider_id
            || capabilities.provider_build != provider.provider_build_id
            || capability_digest != provider.capability_digest
        {
            return Err(UseError::new(
                "use.plugin.runtime.provider_evidence_changed",
                "The selected Runtime provider no longer matches the reviewed plan evidence.",
            )
            .with_detail("plannedProviderId", provider.provider_id.clone())
            .with_detail("observedProviderId", capabilities.provider_id.to_string())
            .with_detail("plannedProviderBuild", provider.provider_build_id.clone())
            .with_detail("observedProviderBuild", capabilities.provider_build.clone())
            .with_detail(
                "plannedCapabilityDigest",
                provider.capability_digest.clone(),
            )
            .with_detail("observedCapabilityDigest", capability_digest));
        }

        let mut missing = capabilities
            .missing_for(plan.spec())
            .map_err(runtime_contract_error)?;
        for feature in required_lifecycle_features(plan.contract()) {
            if !capabilities.supports_feature(feature) {
                missing.push(format!("feature:{feature:?}"));
            }
        }
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            return Err(UseError::new(
                "use.plugin.runtime.capability_missing",
                "The selected Runtime provider cannot satisfy the reviewed surface plan.",
            )
            .with_detail("providerId", capabilities.provider_id.as_str().to_string())
            .with_detail(
                "missing",
                serde_json::to_value(&missing).unwrap_or_default(),
            ));
        }
        Ok(capabilities)
    }

    pub async fn prepare_task(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
    ) -> UseResult<RuntimePreparedTaskBinding> {
        if !matches!(plan.contract(), RuntimeSurfaceContract::ToolTask { .. })
            || plan.spec().class != RuntimeUnitClass::Task
        {
            return Err(UseError::new(
                "use.plugin.runtime.class_mismatch",
                "Only Runtime Task plans can produce prepared Task bindings.",
            ));
        }
        validate_task_capture_contract(plan.contract())?;
        self.verify_plan(plan, provider).await?;
        let semantics_profile_digest =
            plan.spec()
                .semantics_profile_digest
                .clone()
                .ok_or_else(|| {
                    runtime_contract_error(
                        "Runtime Task plan omitted its semantics-profile digest.",
                    )
                })?;
        Ok(RuntimePreparedTaskBinding {
            schema: super::model::RUNTIME_TASK_BINDING_SCHEMA.to_string(),
            surface: plan.surface(),
            package_digest: plan.context().package_digest().to_string(),
            scope_id: plan.context().scope_id().to_string(),
            descriptor_digest: plan.descriptor_digest().to_string(),
            provider_id: provider.provider_id.clone(),
            provider_build_id: provider.provider_build_id.clone(),
            capability_digest: provider.capability_digest.clone(),
            enforcement: provider.enforcement,
            artifact_digest: plan.spec().artifact.digest.clone(),
            artifact_media_type: plan.spec().artifact.media_type.clone(),
            generation: plan.spec().generation,
            semantics_profile_digest,
        })
    }

    pub async fn invoke_task(
        &self,
        plan: &RuntimeSurfacePlan,
        binding: &RuntimePreparedTaskBinding,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeTaskExecution> {
        validate_task_binding(plan, binding)?;
        let (max_stdout_bytes, max_stderr_bytes) = validate_task_capture_contract(plan.contract())?;
        let provider = PlannedProviderEvidence {
            surface: binding.surface.clone(),
            provider_id: binding.provider_id.clone(),
            provider_build_id: binding.provider_build_id.clone(),
            capability_digest: binding.capability_digest.clone(),
            semantics_profile_digest: binding.semantics_profile_digest.clone(),
            enforcement: binding.enforcement,
        };
        self.verify_plan(plan, &provider).await?;
        let request = RuntimeApplyRequest {
            schema: RuntimeApplyRequest::SCHEMA.to_string(),
            request_id: request_id.into(),
            deadline_at_ms,
            spec: plan.spec().clone(),
        };
        request.validate().map_err(runtime_contract_error)?;
        let observation = self
            .client
            .apply(&request)
            .await
            .map_err(|error| runtime_error("invoke Runtime Task", error))?;
        observation
            .validate_against(plan.spec())
            .map_err(runtime_contract_error)?;
        if observation.provider_build.as_deref() != Some(binding.provider_build_id.as_str()) {
            return Err(UseError::new(
                "use.plugin.runtime.observation_evidence_mismatch",
                "The Runtime Task observation was produced by an unreviewed provider build.",
            ));
        }
        if observation.state == RuntimeUnitState::Failed {
            let failure = observation.failure.as_ref();
            return Err(UseError::new(
                "use.plugin.runtime.task_failed",
                "The Runtime Task reported a failed native invocation.",
            )
            .with_detail(
                "failureCode",
                failure.map_or("unknown", |failure| failure.code.as_str()),
            )
            .with_detail(
                "retryable",
                failure.is_some_and(|failure| failure.retryable),
            ));
        }
        if !observation.converges(plan.spec()) {
            return Err(UseError::new(
                "use.plugin.runtime.not_converged",
                "The Runtime Task did not reach its reviewed terminal success state.",
            )
            .with_detail("unitId", observation.unit_id.clone())
            .with_detail(
                "state",
                serde_json::to_value(observation.state).unwrap_or_default(),
            ));
        }
        let stdout = self
            .capture_log_stream(plan, RuntimeLogStream::Stdout, max_stdout_bytes)
            .await?;
        let stderr = self
            .capture_log_stream(plan, RuntimeLogStream::Stderr, max_stderr_bytes)
            .await?;
        Ok(RuntimeTaskExecution {
            observation,
            exit_code: 0,
            stdout: stdout.data,
            stderr: stderr.data,
            truncated: stdout.truncated || stderr.truncated,
        })
    }

    pub async fn apply_service(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeServiceActivation> {
        if matches!(plan.contract(), RuntimeSurfaceContract::ToolTask { .. })
            || plan.spec().class != RuntimeUnitClass::Service
        {
            return Err(UseError::new(
                "use.plugin.runtime.class_mismatch",
                "Only Runtime Service plans can be applied as persistent plugin bindings.",
            ));
        }
        self.verify_plan(plan, provider).await?;
        let request = RuntimeApplyRequest {
            schema: RuntimeApplyRequest::SCHEMA.to_string(),
            request_id: request_id.into(),
            deadline_at_ms,
            spec: plan.spec().clone(),
        };
        request.validate().map_err(runtime_contract_error)?;
        let observation = self
            .client
            .apply(&request)
            .await
            .map_err(|error| runtime_error("apply Runtime Service", error))?;
        observation
            .validate_against(plan.spec())
            .map_err(runtime_contract_error)?;
        if observation.provider_build.as_deref() != Some(provider.provider_build_id.as_str()) {
            return Err(UseError::new(
                "use.plugin.runtime.observation_evidence_mismatch",
                "The Runtime Service observation was produced by an unreviewed provider build.",
            ));
        }
        if !observation.converges(plan.spec()) {
            return Err(UseError::new(
                "use.plugin.runtime.not_converged",
                "The Runtime Service did not reach its reviewed running and healthy state.",
            )
            .with_detail("unitId", observation.unit_id.clone())
            .with_detail(
                "state",
                serde_json::to_value(observation.state).unwrap_or_default(),
            ));
        }
        Ok(RuntimeServiceActivation {
            plan: plan.clone(),
            provider: provider.clone(),
            observation,
        })
    }

    async fn capture_log_stream(
        &self,
        plan: &RuntimeSurfacePlan,
        stream: RuntimeLogStream,
        max_bytes: u64,
    ) -> UseResult<CapturedLog> {
        if max_bytes == 0 || max_bytes > MAX_IN_MEMORY_TASK_OUTPUT_BYTES {
            return Err(UseError::new(
                "use.plugin.runtime.capture_unsupported",
                format!(
                    "In-memory Runtime Task capture must be between 1 and {MAX_IN_MEMORY_TASK_OUTPUT_BYTES} bytes per stream."
                ),
            ));
        }
        let max_bytes = usize::try_from(max_bytes).map_err(|_| {
            runtime_contract_error("Runtime Task capture bound does not fit this host.")
        })?;
        let mut cursor = None;
        let mut last_sequence = None;
        let mut data = String::new();
        for _ in 0..MAX_LOG_QUERY_ROUNDS {
            let query = RuntimeLogQuery {
                schema: RuntimeLogQuery::SCHEMA.to_string(),
                unit_id: plan.spec().unit_id.clone(),
                generation: plan.spec().generation,
                cursor: cursor.clone(),
                limit: LOG_QUERY_CHUNKS,
                stream: Some(stream),
            };
            query.validate().map_err(runtime_contract_error)?;
            let chunks = self
                .client
                .logs(&query)
                .await
                .map_err(|error| runtime_error("read Runtime Task output", error))?;
            if chunks.is_empty() {
                return Ok(CapturedLog {
                    data,
                    truncated: false,
                });
            }
            let previous_cursor = cursor.clone();
            for chunk in chunks {
                chunk.validate().map_err(runtime_contract_error)?;
                if chunk.stream != stream
                    || last_sequence.is_some_and(|sequence| chunk.sequence <= sequence)
                {
                    return Err(runtime_contract_error(
                        "Runtime Task log chunks are out of order or crossed streams.",
                    ));
                }
                last_sequence = Some(chunk.sequence);
                cursor = Some(chunk.cursor);
                let remaining = max_bytes.saturating_sub(data.len());
                if chunk.data.len() > remaining {
                    append_utf8_prefix(&mut data, &chunk.data, remaining);
                    return Ok(CapturedLog {
                        data,
                        truncated: true,
                    });
                }
                data.push_str(&chunk.data);
            }
            if cursor == previous_cursor {
                return Err(runtime_contract_error(
                    "Runtime Task log cursor did not advance.",
                ));
            }
        }
        Err(runtime_contract_error(
            "Runtime Task log pagination exceeded its bounded round count.",
        ))
    }
}

struct CapturedLog {
    data: String,
    truncated: bool,
}

fn append_utf8_prefix(target: &mut String, value: &str, max_bytes: usize) {
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    target.push_str(&value[..end]);
}

fn validate_task_binding(
    plan: &RuntimeSurfacePlan,
    binding: &RuntimePreparedTaskBinding,
) -> UseResult<()> {
    RuntimeBindingReceipt::Task(binding.clone()).validate()?;
    if !matches!(plan.contract(), RuntimeSurfaceContract::ToolTask { .. })
        || binding.surface != plan.surface()
        || binding.package_digest != plan.context().package_digest()
        || binding.scope_id != plan.context().scope_id()
        || binding.descriptor_digest != plan.descriptor_digest()
        || binding.artifact_digest != plan.spec().artifact.digest
        || binding.artifact_media_type != plan.spec().artifact.media_type
        || binding.generation != plan.spec().generation
        || binding.semantics_profile_digest
            != plan
                .spec()
                .semantics_profile_digest
                .as_deref()
                .unwrap_or_default()
    {
        return Err(UseError::new(
            "use.plugin.runtime.binding_mismatch",
            "The Runtime Task invocation does not match its installed launcher binding.",
        ));
    }
    Ok(())
}

fn validate_task_capture_contract(contract: &RuntimeSurfaceContract) -> UseResult<(u64, u64)> {
    let RuntimeSurfaceContract::ToolTask {
        max_stdout_bytes,
        max_stderr_bytes,
        ..
    } = contract
    else {
        return Err(UseError::new(
            "use.plugin.runtime.class_mismatch",
            "Only Runtime Task plans can be prepared or invoked as CLI Tool bindings.",
        ));
    };
    if *max_stdout_bytes == 0
        || *max_stderr_bytes == 0
        || *max_stdout_bytes > MAX_IN_MEMORY_TASK_OUTPUT_BYTES
        || *max_stderr_bytes > MAX_IN_MEMORY_TASK_OUTPUT_BYTES
    {
        return Err(UseError::new(
            "use.plugin.runtime.capture_unsupported",
            format!(
                "This host supports at most {MAX_IN_MEMORY_TASK_OUTPUT_BYTES} captured bytes per Runtime Task output stream."
            ),
        ));
    }
    Ok((*max_stdout_bytes, *max_stderr_bytes))
}

pub fn runtime_capabilities_digest(capabilities: &RuntimeCapabilities) -> UseResult<String> {
    capabilities.validate().map_err(runtime_contract_error)?;
    let mut canonical = capabilities.clone();
    canonical.unit_classes.sort();
    canonical.artifact_media_types.sort();
    canonical.isolation_levels.sort();
    canonical.network_modes.sort();
    canonical.mount_kinds.sort();
    canonical.health_check_kinds.sort();
    canonical.resource_controls.sort();
    canonical.features.sort();
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        runtime_contract_error(format!(
            "Failed to encode canonical Runtime capabilities: {error}"
        ))
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn validate_plan_evidence(
    plan: &RuntimeSurfacePlan,
    provider: &PlannedProviderEvidence,
) -> UseResult<()> {
    let semantics_profile_digest = plan
        .spec()
        .semantics_profile_digest
        .as_deref()
        .ok_or_else(|| runtime_contract_error("Runtime plan omitted its semantics profile."))?;
    let expected_isolation = match provider.enforcement {
        PlanEnforcementProfile::Container => IsolationLevel::Container,
        PlanEnforcementProfile::Sandbox => IsolationLevel::Sandbox,
        PlanEnforcementProfile::NativeUnconfined => IsolationLevel::Process,
    };
    if provider.surface != plan.surface()
        || provider.semantics_profile_digest != semantics_profile_digest
        || plan.spec().isolation != expected_isolation
    {
        return Err(UseError::new(
            "use.plugin.runtime.plan_evidence_mismatch",
            "The Runtime surface spec does not match its reviewed provider evidence.",
        ));
    }
    Ok(())
}

fn required_lifecycle_features(contract: &RuntimeSurfaceContract) -> Vec<RuntimeFeature> {
    match contract {
        RuntimeSurfaceContract::ToolTask { .. } => {
            vec![RuntimeFeature::Logs, RuntimeFeature::Remove]
        }
        RuntimeSurfaceContract::ToolService { .. } | RuntimeSurfaceContract::McpService { .. } => {
            vec![RuntimeFeature::Stop, RuntimeFeature::Remove]
        }
    }
}

fn runtime_error(action: &str, error: RuntimeError) -> UseError {
    let code = match error {
        RuntimeError::ProviderUnavailable(_) => "use.plugin.runtime.provider_unavailable",
        RuntimeError::UnsupportedCapabilities(_) => "use.plugin.runtime.capability_missing",
        RuntimeError::DeadlineExceeded(_) => "use.plugin.runtime.deadline_exceeded",
        _ => "use.plugin.runtime.operation_failed",
    };
    UseError::new(code, format!("Failed to {action}: {error}"))
}
