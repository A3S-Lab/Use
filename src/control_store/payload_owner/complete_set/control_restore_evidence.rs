use a3s_use_core::UseResult;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::ControlPayloadSnapshotBinding;
use super::restore::restore_staging_invalid;
use super::{canonical_json, ControlPayloadOwnerRegistry};
use crate::control_store::model::valid_sha256;

const EVIDENCE_SCHEMA: &str = "a3s.use.control-store-restore-candidate.v1";
const EVIDENCE_DOMAIN: &[u8] = b"a3s.use.control-store-restore-candidate.v1\0";
pub(super) const MAX_CONTROL_CANDIDATE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ControlCandidateEvidence {
    schema: &'static str,
    snapshot_descriptor_digest: String,
    binding: ControlPayloadSnapshotBinding,
    database_bytes: u64,
    database_sha256: String,
    descriptor_digest: String,
}

impl ControlCandidateEvidence {
    pub(super) fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot_descriptor_digest: &str,
        binding: &ControlPayloadSnapshotBinding,
        database_bytes: u64,
        database_sha256: String,
    ) -> UseResult<Self> {
        let mut evidence = Self {
            schema: EVIDENCE_SCHEMA,
            snapshot_descriptor_digest: snapshot_descriptor_digest.to_owned(),
            binding: binding.clone(),
            database_bytes,
            database_sha256,
            descriptor_digest: String::new(),
        };
        evidence.descriptor_digest = evidence.expected_digest()?;
        evidence.validate(registry)?;
        Ok(evidence)
    }

    fn validate(&self, registry: &ControlPayloadOwnerRegistry) -> UseResult<()> {
        self.binding.validate(registry).map_err(|error| {
            restore_staging_invalid(format!(
                "The Control candidate binding is invalid: {}",
                error.message
            ))
        })?;
        if self.schema != EVIDENCE_SCHEMA
            || !valid_sha256(&self.snapshot_descriptor_digest)
            || self.database_bytes == 0
            || self.database_bytes > MAX_CONTROL_CANDIDATE_BYTES
            || !valid_sha256(&self.database_sha256)
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_staging_invalid(
                "The staged Control candidate evidence is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            snapshot_descriptor_digest: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            database_bytes: u64,
            database_sha256: &'a str,
        }
        let bytes = canonical_json(&Descriptor {
            schema: self.schema,
            snapshot_descriptor_digest: &self.snapshot_descriptor_digest,
            binding: &self.binding,
            database_bytes: self.database_bytes,
            database_sha256: &self.database_sha256,
        })
        .map_err(|error| {
            restore_staging_invalid(format!(
                "Failed to encode Control candidate evidence: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(EVIDENCE_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }

    pub(super) fn canonical_bytes(&self, maximum: u64) -> UseResult<Vec<u8>> {
        let bytes = canonical_json(self).map_err(|error| {
            restore_staging_invalid(format!(
                "Failed to encode staged Control candidate: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() as u64 > maximum {
            return Err(restore_staging_invalid(
                "The staged Control candidate evidence exceeds its byte bound.",
            ));
        }
        Ok(bytes)
    }
}
