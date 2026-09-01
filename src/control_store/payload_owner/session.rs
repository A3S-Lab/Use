use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::StateMaintenanceGuard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotEvidence, ControlPayloadSnapshotReceipt, ControlPayloadSnapshotSet,
};
use crate::control_store::export::{self, GeneratedControlStoreExport, VerifiedControlStoreExport};
use crate::control_store::model::valid_sha256;

const CONTROL_PAYLOAD_SNAPSHOT_BINDING_SCHEMA: &str = "a3s.use.control-payload-snapshot-binding.v1";
const CONTROL_PAYLOAD_SNAPSHOT_BINDING_DOMAIN: &[u8] =
    b"a3s.use.control-payload-snapshot-binding.v1\0";
const MAX_CONTROL_PAYLOAD_SNAPSHOT_BINDING_BYTES: usize = 16 * 1024;

/// Immutable join key between one canonical Control export and every external
/// payload snapshot created while the same maintenance fence remains held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPayloadSnapshotBinding {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) control_generation: u64,
    pub(in crate::control_store) control_export_digest: String,
    pub(in crate::control_store) owner_registry_digest: String,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlPayloadSnapshotBinding {
    pub(in crate::control_store) fn new(
        registry: &ControlPayloadOwnerRegistry,
        installation: InstallationId,
        control_generation: u64,
        control_export_digest: String,
    ) -> UseResult<Self> {
        let mut binding = Self {
            schema: CONTROL_PAYLOAD_SNAPSHOT_BINDING_SCHEMA.to_string(),
            installation,
            control_generation,
            control_export_digest,
            owner_registry_digest: registry.descriptor_digest().to_string(),
            descriptor_digest: String::new(),
        };
        binding.descriptor_digest = binding.expected_descriptor_digest()?;
        binding.validate(registry)?;
        Ok(binding)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        registry.validate()?;
        self.validate_descriptor()?;
        if self.owner_registry_digest != registry.descriptor_digest() {
            return Err(snapshot_error(
                "The Control payload snapshot binding is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn validate_descriptor(&self) -> UseResult<()> {
        if self.schema != CONTROL_PAYLOAD_SNAPSHOT_BINDING_SCHEMA
            || self.installation.validate().is_err()
            || !valid_sha256(&self.control_export_digest)
            || !valid_sha256(&self.owner_registry_digest)
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(snapshot_error(
                "The Control payload snapshot binding is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    /// Verify that canonical Control bytes are the exact authority named by
    /// this binding before any external owner interprets their history.
    pub(in crate::control_store) fn verify_control_export(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        bytes: &[u8],
    ) -> UseResult<VerifiedControlStoreExport> {
        self.validate(registry)?;
        let verified = export::verify(bytes, &self.installation).map_err(|error| {
            snapshot_error(format!(
                "The bound Control export failed offline verification: {}",
                error.message
            ))
        })?;
        if verified.descriptor_digest != self.control_export_digest
            || verified.export.current_generation != self.control_generation
        {
            return Err(snapshot_error(
                "The canonical Control export does not match the payload snapshot binding.",
            ));
        }
        Ok(verified)
    }

    fn expected_descriptor_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            installation: &'a InstallationId,
            control_generation: u64,
            control_export_digest: &'a str,
            owner_registry_digest: &'a str,
        }

        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            installation: &self.installation,
            control_generation: self.control_generation,
            control_export_digest: &self.control_export_digest,
            owner_registry_digest: &self.owner_registry_digest,
        })
        .map_err(|error| {
            snapshot_error(format!(
                "Failed to encode the canonical Control payload snapshot binding: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_PAYLOAD_SNAPSHOT_BINDING_BYTES {
            return Err(snapshot_error(
                "The Control payload snapshot binding exceeds its canonical byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(CONTROL_PAYLOAD_SNAPSHOT_BINDING_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

/// Live snapshot boundary. Holding this value keeps the installation's
/// exclusive maintenance fence held without retaining a SQLite transaction or
/// bounded-executor permit across owner I/O.
#[derive(Debug)]
pub(in crate::control_store) struct ControlPayloadSnapshotSession {
    registry: ControlPayloadOwnerRegistry,
    binding: ControlPayloadSnapshotBinding,
    control_export: Vec<u8>,
    state_root: PathBuf,
    owned_roots: Vec<PathBuf>,
    maintenance: StateMaintenanceGuard,
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) fn new(
        registry: ControlPayloadOwnerRegistry,
        installation: InstallationId,
        control_export: GeneratedControlStoreExport,
        state_root: PathBuf,
        owned_roots: Vec<PathBuf>,
        maintenance: StateMaintenanceGuard,
    ) -> UseResult<Self> {
        registry.validate()?;
        let (control_export, control_generation, control_export_digest) =
            control_export.into_parts();
        let binding = ControlPayloadSnapshotBinding::new(
            &registry,
            installation,
            control_generation,
            control_export_digest,
        )?;
        Ok(Self {
            registry,
            binding,
            control_export,
            state_root,
            owned_roots,
            maintenance,
        })
    }

    pub(in crate::control_store) fn binding(&self) -> &ControlPayloadSnapshotBinding {
        &self.binding
    }

    pub(in crate::control_store) fn control_export(&self) -> &[u8] {
        &self.control_export
    }

    pub(super) fn registry(&self) -> &ControlPayloadOwnerRegistry {
        &self.registry
    }

    pub(super) fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub(super) fn owned_roots(&self) -> &[PathBuf] {
        &self.owned_roots
    }

    pub(in crate::control_store) fn receipt(
        &self,
        owner: ControlPayloadOwnerId,
        evidence: ControlPayloadSnapshotEvidence,
    ) -> UseResult<ControlPayloadSnapshotReceipt> {
        ControlPayloadSnapshotReceipt::new(&self.registry, &self.binding, owner, evidence)
    }

    pub(in crate::control_store) fn complete(
        &self,
        receipts: Vec<ControlPayloadSnapshotReceipt>,
    ) -> UseResult<ControlPayloadSnapshotSet> {
        ControlPayloadSnapshotSet::new(&self.registry, self.binding.clone(), receipts)
    }

    pub(super) fn maintenance(&self) -> &StateMaintenanceGuard {
        &self.maintenance
    }
}

fn snapshot_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.payload_snapshot_invalid", message)
}
