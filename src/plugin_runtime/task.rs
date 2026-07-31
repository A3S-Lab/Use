use a3s_runtime::contract::{
    RuntimeActionRequest, RuntimeApplyRequest, RuntimeInspection, RuntimeLogStream, RuntimeRemoval,
    RuntimeUnitState,
};
use a3s_use_core::{PlannedProviderEvidence, UseError, UseResult, MAX_TASK_CAPTURE_BYTES};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWrite;

use super::client::{runtime_error, PluginRuntimeClient};
use super::model::{
    runtime_contract_error, RuntimePreparedTaskBinding, RuntimeSurfaceContract, RuntimeSurfacePlan,
};
use super::receipt::RuntimeBindingReceipt;
use super::task_output::{
    flush_output, RuntimeTaskStreamingExecution, MAX_IN_MEMORY_TASK_OUTPUT_BYTES,
};

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
    /// Invoke a finite Runtime Task and collect UTF-8 output in memory.
    ///
    /// This compatibility path rejects capture contracts above
    /// [`MAX_IN_MEMORY_TASK_OUTPUT_BYTES`] before applying the Task. Use
    /// [`Self::invoke_task_streaming`] with host-owned sinks for larger output.
    pub async fn invoke_task(
        &self,
        plan: &RuntimeSurfacePlan,
        binding: &RuntimePreparedTaskBinding,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeTaskExecution> {
        validate_task_binding(plan, binding)?;
        let (max_stdout_bytes, max_stderr_bytes) = validate_task_capture_contract(plan.contract())?;
        validate_in_memory_capture(max_stdout_bytes, max_stderr_bytes)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let execution = self
            .invoke_task_streaming(
                plan,
                binding,
                request_id,
                deadline_at_ms,
                &mut stdout,
                &mut stderr,
            )
            .await?;
        let stdout = String::from_utf8(stdout).map_err(|_| {
            runtime_contract_error("Runtime stdout ceased to be valid UTF-8 during capture.")
        })?;
        let stderr = String::from_utf8(stderr).map_err(|_| {
            runtime_contract_error("Runtime stderr ceased to be valid UTF-8 during capture.")
        })?;
        Ok(RuntimeTaskExecution {
            observation: execution.observation,
            exit_code: execution.exit_code,
            stdout,
            stderr,
            truncated: execution.stdout.truncated || execution.stderr.truncated,
        })
    }

    /// Invoke a finite Runtime Task and stream its separately bounded UTF-8
    /// stdout and stderr into caller-owned sinks.
    ///
    /// The caller owns sink creation, path policy, persistence, and cleanup.
    /// A sink failure still triggers exact Runtime Task cleanup.
    pub async fn invoke_task_streaming<Stdout, Stderr>(
        &self,
        plan: &RuntimeSurfacePlan,
        binding: &RuntimePreparedTaskBinding,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
        stdout: &mut Stdout,
        stderr: &mut Stderr,
    ) -> UseResult<RuntimeTaskStreamingExecution>
    where
        Stdout: AsyncWrite + Unpin + Send + ?Sized,
        Stderr: AsyncWrite + Unpin + Send + ?Sized,
    {
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
            let stdout_summary = self
                .capture_log_stream(plan, RuntimeLogStream::Stdout, max_stdout_bytes, stdout)
                .await?;
            flush_output(stdout, RuntimeLogStream::Stdout).await?;
            let stderr_summary = self
                .capture_log_stream(plan, RuntimeLogStream::Stderr, max_stderr_bytes, stderr)
                .await?;
            flush_output(stderr, RuntimeLogStream::Stderr).await?;
            Ok::<_, UseError>((stdout_summary, stderr_summary))
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
        Ok(RuntimeTaskStreamingExecution {
            observation,
            exit_code: 0,
            stdout,
            stderr,
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
        || *max_stdout_bytes > MAX_TASK_CAPTURE_BYTES
        || *max_stderr_bytes > MAX_TASK_CAPTURE_BYTES
    {
        return Err(UseError::new(
            "use.plugin.runtime.capture_unsupported",
            format!(
                "Runtime Task output capture must be between 1 and {MAX_TASK_CAPTURE_BYTES} bytes per stream."
            ),
        ));
    }
    Ok((*max_stdout_bytes, *max_stderr_bytes))
}

fn validate_in_memory_capture(max_stdout_bytes: u64, max_stderr_bytes: u64) -> UseResult<()> {
    if max_stdout_bytes > MAX_IN_MEMORY_TASK_OUTPUT_BYTES
        || max_stderr_bytes > MAX_IN_MEMORY_TASK_OUTPUT_BYTES
    {
        return Err(UseError::new(
            "use.plugin.runtime.capture_unsupported",
            format!(
                "In-memory Runtime Task capture supports at most {MAX_IN_MEMORY_TASK_OUTPUT_BYTES} bytes per stream; use invoke_task_streaming for a caller-owned sink."
            ),
        ));
    }
    Ok(())
}
