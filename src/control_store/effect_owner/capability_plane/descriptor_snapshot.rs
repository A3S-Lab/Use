//! Durable, content-addressed snapshots for verified capability descriptions.
//!
//! A signed description is host evidence, not Control authority.  The host
//! must nevertheless be able to retry a Capability Index cutover after a
//! process restart without consulting a changed Registry view.  This module
//! stores the exact proof set and signer policy under a key derived from the
//! committed Control capability identity.  It has no mutable current pointer
//! and never chooses a package or generation on behalf of Control.

use std::io;
use std::path::Path;

use a3s_use_core::{CapabilityDescriptionProof, InstallationId, UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::super::model::ControlCapabilityEffectAuthority;
use super::descriptor::{ControlCapabilityDescriptorProjection, ControlCapabilitySignerPolicy};

#[path = "descriptor_snapshot_store.rs"]
mod storage;
pub(in crate::control_store) use storage::ControlCapabilityDescriptorSnapshotStore;

pub(in crate::control_store) const CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-capability-descriptor-snapshot.v1";
pub(in crate::control_store) const MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS: usize =
    4_096;
pub(in crate::control_store) const MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES: usize =
    16 * 1024 * 1024;
const SNAPSHOT_ERROR: &str = "use.control.capability_descriptor_snapshot_invalid";
const SNAPSHOT_IO: &str = "use.control.capability_descriptor_snapshot_io";
const SNAPSHOT_CONFLICT: &str = "use.control.capability_descriptor_snapshot_conflict";
const SNAPSHOT_BUSY: &str = "use.control.capability_descriptor_snapshot_busy";
pub(in crate::control_store) const SNAPSHOT_MISSING: &str =
    "use.control.capability_descriptor_snapshot_missing";
pub(in crate::control_store) const SNAPSHOT_RETRYABLE_IO: &str = SNAPSHOT_IO;
pub(in crate::control_store) const SNAPSHOT_RETRYABLE_BUSY: &str = SNAPSHOT_BUSY;
const SNAPSHOT_KEY_DOMAIN: &[u8] = b"a3s.use.control-capability-descriptor-snapshot-key.v1\0";
const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-capability-descriptor-snapshot.v1\0";
const PROOF_SET_DOMAIN: &[u8] = b"a3s.use.control-capability-proof-set.v1\0";
const SIGNER_POLICY_DOMAIN: &[u8] = b"a3s.use.control-capability-signer-policy.v1\0";

/// The immutable Control identity to which one proof snapshot is bound.
///
/// `control_descriptor_digest` is the candidate capability descriptor digest
/// computed by Control.  It is deliberately not the eventual Agent catalog
/// digest; the latter is derived only after this evidence has been checked.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotKey {
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) capability_generation: u64,
    pub(in crate::control_store) control_descriptor_digest: String,
}

impl ControlCapabilityDescriptorSnapshotKey {
    pub(in crate::control_store) fn new(
        installation: InstallationId,
        installation_generation: u64,
        capability_generation: u64,
        control_descriptor_digest: String,
    ) -> UseResult<Self> {
        let key = Self {
            installation,
            installation_generation,
            capability_generation,
            control_descriptor_digest,
        };
        key.validate()?;
        Ok(key)
    }

    pub(in crate::control_store) fn from_authority(
        authority: &ControlCapabilityEffectAuthority,
    ) -> UseResult<Self> {
        Self::new(
            authority.generation.snapshot.installation.clone(),
            authority.generation.snapshot.generation,
            authority.generation.capability.generation,
            authority.generation.capability.descriptor_digest.clone(),
        )
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            snapshot_error("A descriptor snapshot key contains an invalid installation.")
        })?;
        if self.installation_generation == 0
            || self.capability_generation == 0
            || !valid_sha256(&self.control_descriptor_digest)
        {
            return Err(snapshot_error(
                "A descriptor snapshot key contains an invalid generation or digest.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn digest(&self) -> UseResult<String> {
        self.validate()?;
        let bytes = canonical_json(self, "descriptor snapshot key")?;
        Ok(digest_with_domain(SNAPSHOT_KEY_DOMAIN, &bytes))
    }
}

/// Exact proof and trust-policy bytes captured before a Control cutover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshot {
    key: ControlCapabilityDescriptorSnapshotKey,
    proofs: Vec<CapabilityDescriptionProof>,
    signer_policy: ControlCapabilitySignerPolicy,
    proof_set_digest: String,
    signer_policy_digest: String,
}

impl ControlCapabilityDescriptorSnapshot {
    pub(in crate::control_store) fn new(
        key: ControlCapabilityDescriptorSnapshotKey,
        proofs: Vec<CapabilityDescriptionProof>,
        signer_policy: ControlCapabilitySignerPolicy,
    ) -> UseResult<Self> {
        key.validate()?;
        let projection = ControlCapabilityDescriptorProjection::new(proofs, signer_policy)?;
        let (proofs, signer_policy) = projection.into_parts()?;
        let proof_set_digest = proof_set_digest(&proofs)?;
        let signer_policy_digest = signer_policy_digest(&signer_policy)?;
        let snapshot = Self {
            key,
            proofs,
            signer_policy,
            proof_set_digest,
            signer_policy_digest,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub(in crate::control_store) fn proofs(&self) -> &[CapabilityDescriptionProof] {
        &self.proofs
    }

    pub(in crate::control_store) fn signer_policy(&self) -> &ControlCapabilitySignerPolicy {
        &self.signer_policy
    }

    pub(in crate::control_store) fn digest(&self) -> UseResult<String> {
        let bytes = storage::encode_snapshot(self)?;
        Ok(digest_with_domain(SNAPSHOT_DOMAIN, &bytes))
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.key.validate()?;
        if self.proofs.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
            return Err(snapshot_error(
                "The descriptor proof snapshot exceeds its proof bound.",
            ));
        }
        let normalized = ControlCapabilityDescriptorProjection::new(
            self.proofs.clone(),
            self.signer_policy.clone(),
        )?
        .into_parts()?;
        if normalized.0 != self.proofs || normalized.1 != self.signer_policy {
            return Err(snapshot_error(
                "The descriptor proof snapshot is not canonically ordered.",
            ));
        }
        if self.proof_set_digest != proof_set_digest(&self.proofs)?
            || self.signer_policy_digest != signer_policy_digest(&self.signer_policy)?
        {
            return Err(snapshot_error(
                "The descriptor proof snapshot digest does not match its evidence.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotRecord {
    schema: String,
    key: ControlCapabilityDescriptorSnapshotKey,
    proofs: Vec<CapabilityDescriptionProof>,
    signer_policy: ControlCapabilitySignerPolicy,
    proof_set_digest: String,
    signer_policy_digest: String,
}

impl From<ControlCapabilityDescriptorSnapshot> for SnapshotRecord {
    fn from(snapshot: ControlCapabilityDescriptorSnapshot) -> Self {
        Self {
            schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_SCHEMA.to_owned(),
            key: snapshot.key,
            proofs: snapshot.proofs,
            signer_policy: snapshot.signer_policy,
            proof_set_digest: snapshot.proof_set_digest,
            signer_policy_digest: snapshot.signer_policy_digest,
        }
    }
}

impl TryFrom<SnapshotRecord> for ControlCapabilityDescriptorSnapshot {
    type Error = UseError;

    fn try_from(record: SnapshotRecord) -> Result<Self, Self::Error> {
        if record.schema != CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_SCHEMA {
            return Err(snapshot_error(
                "The descriptor proof snapshot schema is unsupported.",
            ));
        }
        let snapshot = Self {
            key: record.key,
            proofs: record.proofs,
            signer_policy: record.signer_policy,
            proof_set_digest: record.proof_set_digest,
            signer_policy_digest: record.signer_policy_digest,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

/// Evidence returned after an immutable snapshot is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotPublication {
    pub(in crate::control_store) key: ControlCapabilityDescriptorSnapshotKey,
    pub(in crate::control_store) key_digest: String,
    pub(in crate::control_store) snapshot_digest: String,
    pub(in crate::control_store) proof_set_digest: String,
    pub(in crate::control_store) signer_policy_digest: String,
}

impl ControlCapabilityDescriptorSnapshotPublication {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.key.validate()?;
        if self.key.digest()? != self.key_digest
            || !valid_sha256(&self.snapshot_digest)
            || !valid_sha256(&self.proof_set_digest)
            || !valid_sha256(&self.signer_policy_digest)
        {
            return Err(snapshot_error(
                "Descriptor snapshot publication evidence is not canonical.",
            ));
        }
        Ok(())
    }
}

fn canonical_json<T: Serialize>(value: &T, label: &str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|error| snapshot_error(format!("Failed to encode {label}: {error}")))?;
    Ok(bytes)
}

fn proof_set_digest(proofs: &[CapabilityDescriptionProof]) -> UseResult<String> {
    let bytes = canonical_json(&ProofSetMaterial::new(proofs), "descriptor proof set")?;
    Ok(digest_with_domain(PROOF_SET_DOMAIN, &bytes))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProofSetMaterial<'a> {
    schema: &'static str,
    proofs: &'a [CapabilityDescriptionProof],
}

impl<'a> ProofSetMaterial<'a> {
    fn new(proofs: &'a [CapabilityDescriptionProof]) -> Self {
        Self {
            schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_SCHEMA,
            proofs,
        }
    }
}

fn signer_policy_digest(policy: &ControlCapabilitySignerPolicy) -> UseResult<String> {
    let bytes = policy.canonical_bytes()?;
    Ok(digest_with_domain(SIGNER_POLICY_DOMAIN, &bytes))
}

fn digest_with_domain(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn lock_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn snapshot_error(message: impl Into<String>) -> UseError {
    UseError::new(SNAPSHOT_ERROR, message)
}

fn snapshot_io(message: impl Into<String>) -> UseError {
    UseError::new(SNAPSHOT_IO, message)
}

fn snapshot_conflict() -> UseError {
    UseError::new(
        SNAPSHOT_CONFLICT,
        "The descriptor proof snapshot is missing, substituted, or malformed.",
    )
}

fn path_invalid() -> UseError {
    UseError::new(
        SNAPSHOT_ERROR,
        "The descriptor snapshot path is not an owned regular-file layout.",
    )
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    snapshot_io(format!("Failed to {action} '{}': {error}", path.display()))
}
