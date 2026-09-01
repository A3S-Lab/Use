use a3s_use_core::UseResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::ControlPayloadSnapshotBinding;
use super::control_restore_evidence::ControlCandidateEvidence;
use super::restore::restore_staging_invalid;
use super::{canonical_json, ControlPayloadOwnerRegistry};
use crate::control_store::model::valid_sha256;

const RESULT_SCHEMA: &str = "a3s.use.control-store-restore-result.v1";
const RESULT_DOMAIN: &[u8] = b"a3s.use.control-store-restore-result.v1\0";
const MAX_RESULT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlStoreRestoreResult {
    schema: String,
    snapshot_descriptor_digest: String,
    binding: ControlPayloadSnapshotBinding,
    database_bytes: u64,
    database_sha256: String,
    descriptor_digest: String,
}

impl ControlStoreRestoreResult {
    pub(super) fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot_descriptor_digest: &str,
        binding: &ControlPayloadSnapshotBinding,
        evidence: &ControlCandidateEvidence,
    ) -> UseResult<Self> {
        evidence.validate_exact(
            registry,
            snapshot_descriptor_digest,
            binding,
            evidence.database_bytes(),
            evidence.database_sha256(),
        )?;
        let mut result = Self {
            schema: RESULT_SCHEMA.to_owned(),
            snapshot_descriptor_digest: snapshot_descriptor_digest.to_owned(),
            binding: binding.clone(),
            database_bytes: evidence.database_bytes(),
            database_sha256: evidence.database_sha256().to_owned(),
            descriptor_digest: String::new(),
        };
        result.descriptor_digest = result.expected_digest()?;
        result.validate_for_candidate(registry, snapshot_descriptor_digest, binding, evidence)?;
        Ok(result)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        self.binding.validate(registry).map_err(|error| {
            restore_staging_invalid(format!(
                "The Control restore result binding is invalid: {}",
                error.message
            ))
        })?;
        if self.schema != RESULT_SCHEMA
            || !valid_sha256(&self.snapshot_descriptor_digest)
            || self.database_bytes == 0
            || self.database_bytes > super::control_restore_evidence::MAX_CONTROL_CANDIDATE_BYTES
            || !valid_sha256(&self.database_sha256)
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_staging_invalid(
                "The Control restore result is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_for_candidate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot_descriptor_digest: &str,
        binding: &ControlPayloadSnapshotBinding,
        evidence: &ControlCandidateEvidence,
    ) -> UseResult<()> {
        self.validate(registry)?;
        evidence.validate_exact(
            registry,
            snapshot_descriptor_digest,
            binding,
            self.database_bytes,
            &self.database_sha256,
        )?;
        if self.snapshot_descriptor_digest != snapshot_descriptor_digest
            || &self.binding != binding
            || self.database_bytes != evidence.database_bytes()
            || self.database_sha256 != evidence.database_sha256()
        {
            return Err(restore_staging_invalid(
                "The Control restore result differs from its exact snapshot candidate.",
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
            schema: &self.schema,
            snapshot_descriptor_digest: &self.snapshot_descriptor_digest,
            binding: &self.binding,
            database_bytes: self.database_bytes,
            database_sha256: &self.database_sha256,
        })
        .map_err(|error| {
            restore_staging_invalid(format!("Failed to encode Control restore result: {error}"))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_RESULT_BYTES {
            return Err(restore_staging_invalid(
                "The Control restore result exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(RESULT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}
