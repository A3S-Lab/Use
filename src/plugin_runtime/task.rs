use a3s_runtime::contract::{
    RuntimeActionRequest, RuntimeApplyRequest, RuntimeInspection, RuntimeLogQuery,
    RuntimeLogStream, RuntimeRemoval, RuntimeUnitState,
};
use a3s_use_core::{PlannedProviderEvidence, UseError, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::client::{runtime_error, PluginRuntimeClient};
use super::model::{
    runtime_contract_error, RuntimePreparedTaskBinding, RuntimeSurfaceContract, RuntimeSurfacePlan,
};
use super::receipt::RuntimeBindingReceipt;

const MAX_IN_MEMORY_TASK_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
const LOG_QUERY_CHUNKS: u32 = 64;
const MAX_LOG_QUERY_ROUNDS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTaskExecution {
    pub observation: a3s_runtime::contract::RuntimeObservation,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

impl PluginRuntimeClient {
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
        let request_id = request_id.into();
        let request = RuntimeApplyRequest {
            schema: RuntimeApplyRequest::SCHEMA.to_string(),
            request_id: request_id.clone(),
            deadline_at_ms,
            spec: plan.spec().clone(),
        };
        request.validate().map_err(runtime_contract_error)?;
        let observation = match self.client.apply(&request).await {
            Ok(observation) => observation,
            Err(error) => {
                let primary = runtime_error("invoke Runtime Task", error);
                let cleanup = self
                    .cleanup_task_unit(plan, &request_id, deadline_at_ms, true)
                    .await;
                return Err(attach_cleanup_error(primary, cleanup));
            }
        };
        if let Err(error) = observation.validate_against(plan.spec()) {
            let primary = runtime_contract_error(error);
            let cleanup = self
                .cleanup_task_unit(plan, &request_id, deadline_at_ms, true)
                .await;
            return Err(attach_cleanup_error(primary, cleanup));
        }
        if observation.provider_build.as_deref() != Some(binding.provider_build_id.as_str()) {
            let primary = UseError::new(
                "use.plugin.runtime.observation_evidence_mismatch",
                "The Runtime Task observation was produced by an unreviewed provider build.",
            );
            let cleanup = self
                .cleanup_task_unit(plan, &request_id, deadline_at_ms, true)
                .await;
            return Err(attach_cleanup_error(primary, cleanup));
        }
        if observation.state == RuntimeUnitState::Failed {
            let failure = observation.failure.as_ref();
            let primary = UseError::new(
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
            );
            let cleanup = self
                .cleanup_task_unit(plan, &request_id, deadline_at_ms, false)
                .await;
            return Err(attach_cleanup_error(primary, cleanup));
        }
        if !observation.converges(plan.spec()) {
            let primary = UseError::new(
                "use.plugin.runtime.not_converged",
                "The Runtime Task did not reach its reviewed terminal success state.",
            )
            .with_detail("unitId", observation.unit_id.clone())
            .with_detail(
                "state",
                serde_json::to_value(observation.state).unwrap_or_default(),
            );
            let cleanup = self
                .cleanup_task_unit(plan, &request_id, deadline_at_ms, true)
                .await;
            return Err(attach_cleanup_error(primary, cleanup));
        }
        let captured = async {
            let stdout = self
                .capture_log_stream(plan, RuntimeLogStream::Stdout, max_stdout_bytes)
                .await?;
            let stderr = self
                .capture_log_stream(plan, RuntimeLogStream::Stderr, max_stderr_bytes)
                .await?;
            Ok::<_, UseError>((stdout, stderr))
        }
        .await;
        let cleanup = self
            .cleanup_task_unit(plan, &request_id, deadline_at_ms, false)
            .await;
        let (stdout, stderr) = match captured {
            Ok(output) => {
                cleanup?;
                output
            }
            Err(error) => return Err(attach_cleanup_error(error, cleanup)),
        };
        Ok(RuntimeTaskExecution {
            observation,
            exit_code: 0,
            stdout: stdout.data,
            stderr: stderr.data,
            truncated: stdout.truncated || stderr.truncated,
        })
    }

    async fn cleanup_task_unit(
        &self,
        plan: &RuntimeSurfacePlan,
        request_id: &str,
        deadline_at_ms: Option<u64>,
        stop_first: bool,
    ) -> UseResult<RuntimeRemoval> {
        if stop_first {
            let stop = RuntimeActionRequest {
                schema: RuntimeActionRequest::SCHEMA.to_string(),
                request_id: derived_request_id("task-stop", request_id),
                unit_id: plan.spec().unit_id.clone(),
                generation: plan.spec().generation,
                deadline_at_ms,
            };
            stop.validate().map_err(runtime_contract_error)?;
            let inspection = self
                .client
                .stop(&stop)
                .await
                .map_err(|error| runtime_error("stop incomplete Runtime Task", error))?;
            inspection.validate().map_err(runtime_contract_error)?;
            match inspection {
                RuntimeInspection::Found { observation, .. } => {
                    observation
                        .validate_against(plan.spec())
                        .map_err(runtime_contract_error)?;
                    if !observation.state.is_terminal() {
                        return Err(runtime_contract_error(
                            "Runtime Task stop did not reach a terminal state.",
                        ));
                    }
                }
                RuntimeInspection::NotFound { unit_id, .. } if unit_id == plan.spec().unit_id => {}
                _ => {
                    return Err(runtime_contract_error(
                        "Runtime Task stop did not converge on the requested unit identity.",
                    ))
                }
            }
        }
        let remove = RuntimeActionRequest {
            schema: RuntimeActionRequest::SCHEMA.to_string(),
            request_id: derived_request_id("task-remove", request_id),
            unit_id: plan.spec().unit_id.clone(),
            generation: plan.spec().generation,
            deadline_at_ms,
        };
        remove.validate().map_err(runtime_contract_error)?;
        let removal = self
            .client
            .remove(&remove)
            .await
            .map_err(|error| runtime_error("remove completed Runtime Task", error))?;
        removal.validate().map_err(runtime_contract_error)?;
        if removal.request_id != remove.request_id
            || removal.unit_id != plan.spec().unit_id
            || removal.generation != plan.spec().generation
        {
            return Err(runtime_contract_error(
                "Runtime Task removal does not match the invoked unit identity.",
            ));
        }
        Ok(removal)
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

fn derived_request_id(kind: &str, request_id: &str) -> String {
    format!("use:{kind}:{:x}", Sha256::digest(request_id.as_bytes()))
}

fn attach_cleanup_error(primary: UseError, cleanup: UseResult<RuntimeRemoval>) -> UseError {
    match cleanup {
        Ok(_) => primary,
        Err(cleanup) => primary
            .with_detail("cleanupCode", cleanup.code)
            .with_detail("cleanupMessage", cleanup.message),
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

pub(super) fn validate_task_capture_contract(
    contract: &RuntimeSurfaceContract,
) -> UseResult<(u64, u64)> {
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
