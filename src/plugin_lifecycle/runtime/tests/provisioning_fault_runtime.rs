use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::provisioning_fault_io::{
    append_durable_line, read_optional_json, replace_json, sync_test_parent, write_new_json,
};
use super::*;
use crate::plugin_runtime::provisioning_fault_matrix::{crash_after_checkpoint, RUNTIME_EFFECT};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeEffect {
    request_id: String,
    spec_digest: String,
    observation: RuntimeObservation,
}

pub(super) struct DurableRuntime {
    root: PathBuf,
    capabilities: RuntimeCapabilities,
}

impl DurableRuntime {
    pub(super) fn new(root: PathBuf, capabilities: RuntimeCapabilities) -> Self {
        Self { root, capabilities }
    }

    fn effect_path(&self) -> PathBuf {
        self.root.join("service.json")
    }
}

#[async_trait]
impl RuntimeClient for DurableRuntime {
    async fn capabilities(&self) -> RuntimeResult<RuntimeCapabilities> {
        Ok(self.capabilities.clone())
    }

    async fn apply(&self, request: &RuntimeApplyRequest) -> RuntimeResult<RuntimeObservation> {
        append_durable_line(
            &self.root.join("apply-attempts.log"),
            &format!("{}\n", request.request_id),
        )
        .await
        .map_err(runtime_io)?;
        let spec_digest = request.spec.digest().map_err(RuntimeError::Protocol)?;
        if let Some(effect) = read_optional_json::<RuntimeEffect>(&self.effect_path())
            .await
            .map_err(runtime_io)?
        {
            if effect.request_id != request.request_id || effect.spec_digest != spec_digest {
                return Err(RuntimeError::Protocol(
                    "durable test Runtime apply identity changed".to_string(),
                ));
            }
            return Ok(effect.observation);
        }
        let observation = service_observation(request, &self.capabilities, &spec_digest)?;
        write_new_json(
            &self.effect_path(),
            &RuntimeEffect {
                request_id: request.request_id.clone(),
                spec_digest,
                observation: observation.clone(),
            },
        )
        .await
        .map_err(runtime_io)?;
        crash_after_checkpoint(RUNTIME_EFFECT);
        Ok(observation)
    }

    async fn inspect(&self, unit_id: &str) -> RuntimeResult<RuntimeInspection> {
        let effect = read_optional_json::<RuntimeEffect>(&self.effect_path())
            .await
            .map_err(runtime_io)?;
        Ok(match effect {
            Some(effect) if effect.observation.unit_id == unit_id => RuntimeInspection::Found {
                schema: RuntimeInspection::SCHEMA.to_string(),
                observation: Box::new(effect.observation),
            },
            _ => RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: unit_id.to_string(),
                last_generation: None,
            },
        })
    }

    async fn stop(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeInspection> {
        let Some(mut effect) = read_optional_json::<RuntimeEffect>(&self.effect_path())
            .await
            .map_err(runtime_io)?
        else {
            return Ok(RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: request.unit_id.clone(),
                last_generation: None,
            });
        };
        validate_runtime_action(request, &effect.observation)?;
        effect.observation.state = RuntimeUnitState::Stopped;
        effect.observation.observed_at_ms = 1_100;
        effect.observation.finished_at_ms = Some(1_100);
        effect.observation.clear_service_endpoints();
        replace_json(&self.effect_path(), &effect)
            .await
            .map_err(runtime_io)?;
        Ok(RuntimeInspection::Found {
            schema: RuntimeInspection::SCHEMA.to_string(),
            observation: Box::new(effect.observation),
        })
    }

    async fn remove(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeRemoval> {
        let effect = read_optional_json::<RuntimeEffect>(&self.effect_path())
            .await
            .map_err(runtime_io)?;
        if let Some(effect) = &effect {
            validate_runtime_action(request, &effect.observation)?;
            tokio::fs::remove_file(self.effect_path())
                .await
                .map_err(runtime_io)?;
            sync_test_parent(&self.root).await.map_err(runtime_io)?;
        }
        Ok(RuntimeRemoval {
            schema: RuntimeRemoval::SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            unit_id: request.unit_id.clone(),
            generation: request.generation,
            removed_at_ms: 1_200,
            already_absent: effect.is_none(),
        })
    }

    async fn logs(&self, _query: &RuntimeLogQuery) -> RuntimeResult<Vec<RuntimeLogChunk>> {
        Ok(Vec::new())
    }

    async fn exec(&self, _request: &RuntimeExecRequest) -> RuntimeResult<RuntimeExecResult> {
        Err(RuntimeError::Protocol("unexpected exec".to_string()))
    }
}

fn service_observation(
    request: &RuntimeApplyRequest,
    capabilities: &RuntimeCapabilities,
    spec_digest: &str,
) -> RuntimeResult<RuntimeObservation> {
    let port = request.spec.network.ports.first().ok_or_else(|| {
        RuntimeError::Protocol("test Runtime Service omitted its declared port".to_string())
    })?;
    let mut claims = BTreeMap::new();
    RuntimeServiceEndpoint::node_local_tcp(&port.name, 31_337)
        .map_err(RuntimeError::Protocol)?
        .insert_claim(&mut claims)
        .map_err(RuntimeError::Protocol)?;
    Ok(RuntimeObservation {
        schema: RuntimeObservation::SCHEMA.to_string(),
        unit_id: request.spec.unit_id.clone(),
        generation: request.spec.generation,
        spec_digest: spec_digest.to_string(),
        class: request.spec.class,
        state: RuntimeUnitState::Running,
        provider_resource_id: Some("durable-resource-01".to_string()),
        provider_build: Some(capabilities.provider_build.clone()),
        observed_at_ms: 1_000,
        started_at_ms: Some(900),
        finished_at_ms: None,
        health: Some(RuntimeHealthObservation {
            state: RuntimeHealthState::Healthy,
            checked_at_ms: 1_000,
            message: None,
        }),
        outputs: Vec::new(),
        usage: None,
        evidence: Some(RuntimeEvidence {
            provider_build: capabilities.provider_build.clone(),
            spec_digest: spec_digest.to_string(),
            semantics_profile_digest: request.spec.semantics_profile_digest.clone(),
            claims,
        }),
        provider_attestation: None,
        failure: None,
    })
}

fn validate_runtime_action(
    request: &RuntimeActionRequest,
    observation: &RuntimeObservation,
) -> RuntimeResult<()> {
    if request.unit_id != observation.unit_id || request.generation != observation.generation {
        return Err(RuntimeError::Protocol(
            "durable test Runtime action identity changed".to_string(),
        ));
    }
    Ok(())
}

fn runtime_io(error: io::Error) -> RuntimeError {
    RuntimeError::Protocol(format!("durable test Runtime I/O failed: {error}"))
}
