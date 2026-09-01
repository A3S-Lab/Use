//! Durable, path-free state machine for complete restore activation.

use std::path::Path;

use a3s_use_core::UseResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::control_restore_result::ControlStoreRestoreResult;
use super::restore::{restore_activation_invalid, RestoreComponent};
use super::restore_activation_filesystem as filesystem;
use super::{canonical_json, ControlPayloadOwnerRegistry};
use crate::control_store::model::valid_sha256;

const ACTIVATION_SCHEMA: &str = "a3s.use.control-installation-restore-activation.v1";
const ACTIVATION_OPERATION_DOMAIN: &[u8] =
    b"a3s.use.control-installation-restore-activation-operation.v1\0";
const ACTIVATION_DESCRIPTOR_DOMAIN: &[u8] = b"a3s.use.control-installation-restore-activation.v1\0";
const CHECKPOINT_DOMAIN: &[u8] = b"a3s.use.control-installation-restore-checkpoint.v1\0";
const MARKER_SCHEMA: &str = "a3s.use.control-installation-restore-active.v1";
const MAX_RESULT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlInstallationRestoreActivation {
    schema: String,
    attempt_digest: String,
    operation_digest: String,
    checkpoints: Vec<RestoreActivationCheckpoint>,
    descriptor_digest: String,
}

impl ControlInstallationRestoreActivation {
    pub(super) fn new(attempt_digest: &str) -> UseResult<Self> {
        let mut activation = Self {
            schema: ACTIVATION_SCHEMA.to_owned(),
            attempt_digest: attempt_digest.to_owned(),
            operation_digest: operation_digest(attempt_digest),
            checkpoints: Vec::new(),
            descriptor_digest: String::new(),
        };
        activation.descriptor_digest = activation.expected_descriptor_digest()?;
        activation.validate(attempt_digest)?;
        Ok(activation)
    }

    pub(super) fn validate(&self, attempt_digest: &str) -> UseResult<()> {
        if self.schema != ACTIVATION_SCHEMA
            || self.attempt_digest != attempt_digest
            || !valid_sha256(&self.attempt_digest)
            || self.operation_digest != operation_digest(attempt_digest)
            || !valid_sha256(&self.operation_digest)
            || !valid_sha256(&self.descriptor_digest)
            || self.checkpoints.len() > RestoreComponent::ALL.len()
            || self
                .checkpoints
                .iter()
                .zip(RestoreComponent::ALL)
                .any(|(checkpoint, expected)| {
                    checkpoint.component != expected || checkpoint.validate().is_err()
                })
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(restore_activation_invalid(
                "The complete restore activation journal is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(super) fn checkpoint_control(
        &self,
        attempt_digest: &str,
        result: &ControlStoreRestoreResult,
    ) -> UseResult<Self> {
        self.validate(attempt_digest)?;
        let checkpoint = RestoreActivationCheckpoint::new(RestoreComponent::ControlStore, result)?;
        if let Some(existing) = self.checkpoints.first() {
            if existing == &checkpoint {
                return Ok(self.clone());
            }
            return Err(restore_activation_invalid(
                "The durable Control restore checkpoint conflicts with its replayed result.",
            ));
        }
        let mut next = self.clone();
        next.checkpoints.push(checkpoint);
        next.descriptor_digest = next.expected_descriptor_digest()?;
        next.validate(attempt_digest)?;
        Ok(next)
    }

    pub(super) fn verify_control_result(
        &self,
        attempt_digest: &str,
        result: &ControlStoreRestoreResult,
    ) -> UseResult<()> {
        self.validate(attempt_digest)?;
        let expected = RestoreActivationCheckpoint::new(RestoreComponent::ControlStore, result)?;
        match self.checkpoints.first() {
            Some(checkpoint) if checkpoint == &expected => Ok(()),
            Some(_) => Err(restore_activation_invalid(
                "The replayed Control restore result differs from its durable checkpoint.",
            )),
            None => Err(restore_activation_invalid(
                "The Control restore result has not been durably checkpointed.",
            )),
        }
    }

    pub(super) fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    pub(super) fn operation_digest(&self) -> &str {
        &self.operation_digest
    }

    pub(super) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        let bytes = canonical_json(self).map_err(|error| {
            restore_activation_invalid(format!(
                "Failed to encode the complete restore activation journal: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() as u64 > filesystem::MAX_ACTIVATION_BYTES {
            return Err(restore_activation_invalid(
                "The complete restore activation journal exceeds its byte bound.",
            ));
        }
        Ok(bytes)
    }

    fn expected_descriptor_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            attempt_digest: &'a str,
            operation_digest: &'a str,
            checkpoints: &'a [RestoreActivationCheckpoint],
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            attempt_digest: &self.attempt_digest,
            operation_digest: &self.operation_digest,
            checkpoints: &self.checkpoints,
        })
        .map_err(|error| {
            restore_activation_invalid(format!(
                "Failed to encode the complete restore activation descriptor: {error}"
            ))
        })?;
        Ok(domain_digest(ACTIVATION_DESCRIPTOR_DOMAIN, &bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreActivationCheckpoint {
    component: RestoreComponent,
    result_bytes: u64,
    result_sha256: String,
}

impl RestoreActivationCheckpoint {
    fn new<T: Serialize>(component: RestoreComponent, result: &T) -> UseResult<Self> {
        let bytes = canonical_json(result).map_err(|error| {
            restore_activation_invalid(format!(
                "Failed to encode a complete restore component result: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_RESULT_BYTES {
            return Err(restore_activation_invalid(
                "A complete restore component result exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(CHECKPOINT_DOMAIN);
        digest.update(component.label().as_bytes());
        digest.update([0]);
        digest.update(&bytes);
        Ok(Self {
            component,
            result_bytes: bytes.len() as u64,
            result_sha256: format!("sha256:{:x}", digest.finalize()),
        })
    }

    fn validate(&self) -> UseResult<()> {
        if self.result_bytes == 0
            || self.result_bytes > MAX_RESULT_BYTES as u64
            || !valid_sha256(&self.result_sha256)
        {
            return Err(restore_activation_invalid(
                "A complete restore activation checkpoint is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlInstallationRestoreActiveMarker {
    schema: String,
    plan_digest: String,
    operation_digest: String,
}

impl ControlInstallationRestoreActiveMarker {
    pub(super) fn new(activation: &ControlInstallationRestoreActivation) -> UseResult<Self> {
        activation.validate(&activation.attempt_digest)?;
        let marker = Self {
            schema: MARKER_SCHEMA.to_owned(),
            plan_digest: activation.attempt_digest.clone(),
            operation_digest: activation.operation_digest.clone(),
        };
        marker.validate(activation)?;
        Ok(marker)
    }

    pub(super) fn validate(
        &self,
        activation: &ControlInstallationRestoreActivation,
    ) -> UseResult<()> {
        if self.schema != MARKER_SCHEMA
            || self.plan_digest != activation.attempt_digest
            || self.operation_digest != activation.operation_digest
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.operation_digest)
        {
            return Err(restore_activation_invalid(
                "The active complete restore marker is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(super) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        let bytes = canonical_json(self).map_err(|error| {
            restore_activation_invalid(format!(
                "Failed to encode the active complete restore marker: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() as u64 > filesystem::MAX_MARKER_BYTES {
            return Err(restore_activation_invalid(
                "The active complete restore marker exceeds its byte bound.",
            ));
        }
        Ok(bytes)
    }
}

pub(super) async fn begin(
    state_root: &Path,
    attempt: &Path,
    attempt_bytes: &[u8],
    attempt_digest: &str,
    control_candidate: &Path,
) -> UseResult<ControlInstallationRestoreActivation> {
    filesystem::load_or_begin(
        state_root,
        attempt,
        attempt_bytes,
        attempt_digest,
        control_candidate,
    )
    .await
}

pub(super) async fn checkpoint_control(
    state_root: &Path,
    attempt: &Path,
    attempt_digest: &str,
    result: &ControlStoreRestoreResult,
) -> UseResult<ControlInstallationRestoreActivation> {
    let current = filesystem::load_active(state_root, attempt, attempt_digest).await?;
    let next = current.checkpoint_control(attempt_digest, result)?;
    if next == current {
        current.verify_control_result(attempt_digest, result)?;
        return Ok(current);
    }
    filesystem::replace_journal(attempt, &current, &next).await?;
    let durable = filesystem::load_active(state_root, attempt, attempt_digest).await?;
    durable.verify_control_result(attempt_digest, result)?;
    Ok(durable)
}

pub(super) async fn load(
    state_root: &Path,
    attempt: &Path,
    attempt_digest: &str,
) -> UseResult<ControlInstallationRestoreActivation> {
    filesystem::load_active(state_root, attempt, attempt_digest).await
}

pub(super) fn journal_path(attempt: &Path) -> std::path::PathBuf {
    filesystem::journal_path(attempt)
}

fn operation_digest(attempt_digest: &str) -> String {
    domain_digest(ACTIVATION_OPERATION_DOMAIN, attempt_digest.as_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    format!("sha256:{:x}", digest.finalize())
}

pub(super) fn validate_result_registry(
    registry: &ControlPayloadOwnerRegistry,
    result: &ControlStoreRestoreResult,
) -> UseResult<()> {
    result.validate(registry).map_err(|error| {
        restore_activation_invalid(format!(
            "The Control restore result failed activation validation: {}",
            error.message
        ))
    })
}
