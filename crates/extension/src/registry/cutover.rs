use std::collections::BTreeSet;

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};

use super::{ExtensionRegistrySnapshot, REGISTRY_SCHEMA_VERSION};

pub const EXTENSION_REGISTRY_CUTOVER_SCHEMA: &str = "a3s.use.registry-cutover.v1";
pub const MAX_PENDING_REGISTRY_CUTOVERS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRegistryCutoverRecord {
    pub schema: String,
    pub idempotency_key: String,
    pub request_digest: String,
    pub registry_generation_before: u64,
    pub registry_generation_after: u64,
    pub registry_snapshot_digest: String,
}

impl ExtensionRegistryCutoverRecord {
    pub(super) fn new(
        idempotency_key: impl Into<String>,
        request_digest: impl Into<String>,
        registry_generation_before: u64,
        registry_generation_after: u64,
        registry_snapshot_digest: impl Into<String>,
    ) -> UseResult<Self> {
        let record = Self {
            schema: EXTENSION_REGISTRY_CUTOVER_SCHEMA.to_string(),
            idempotency_key: idempotency_key.into(),
            request_digest: request_digest.into(),
            registry_generation_before,
            registry_generation_after,
            registry_snapshot_digest: registry_snapshot_digest.into(),
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        if self.schema != EXTENSION_REGISTRY_CUTOVER_SCHEMA
            || !valid_canonical_sha256(&self.idempotency_key)
            || !valid_canonical_sha256(&self.request_digest)
            || !valid_canonical_sha256(&self.registry_snapshot_digest)
            || self.registry_generation_before.checked_add(1)
                != Some(self.registry_generation_after)
        {
            return Err(UseError::new(
                "use.extension.registry_cutover_invalid",
                "The Registry cutover record has invalid identity or generation evidence.",
            ));
        }
        Ok(())
    }
}

impl ExtensionRegistrySnapshot {
    pub(crate) fn validate(&self) -> UseResult<()> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(UseError::new(
                "use.extension.registry_incompatible",
                format!(
                    "Extension registry schema {} is not supported.",
                    self.schema_version
                ),
            ));
        }
        if self.pending_cutovers.len() > MAX_PENDING_REGISTRY_CUTOVERS {
            return Err(UseError::new(
                "use.extension.registry_cutover_invalid",
                "The Registry contains too many unfinished cutover records.",
            ));
        }
        let mut keys = BTreeSet::new();
        for record in &self.pending_cutovers {
            record.validate()?;
            if record.registry_generation_after > self.generation
                || !keys.insert(record.idempotency_key.as_str())
            {
                return Err(UseError::new(
                    "use.extension.registry_cutover_invalid",
                    "The Registry contains duplicate or future cutover evidence.",
                ));
            }
        }
        Ok(())
    }
}

fn valid_canonical_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}
