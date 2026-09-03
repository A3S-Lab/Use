use serde::{Deserialize, Serialize};

use crate::UseResult;

use super::{capability_error, CapabilityDescriptor, CAPABILITY_ERROR};
use crate::plugin::{parse_contract, validation::valid_sha256};

/// Verification envelope for an agent-visible capability description.
///
/// The envelope is intentionally separate from [`CapabilityDescriptor`]. A
/// descriptor is the wire projection sent to an agent, while this envelope is
/// the host-owned witness that the exact projection was checked against the
/// Registry's signed publication before it was admitted to a catalog.
pub const CAPABILITY_DESCRIPTION_PROOF_SCHEMA_V1: &str = "a3s.use.capability-description-proof.v1";

const MAX_CAPABILITY_SIGNER_ID_BYTES: usize = 256;

/// Host-produced proof that one complete capability description was verified
/// against a signed Registry publication.
///
/// Signature verification belongs to the Registry/host trust boundary; the
/// core contract must not invent a second key store or accept a key supplied by
/// an agent. Once that boundary has verified the signed source, it wraps the
/// exact descriptor in this envelope. The descriptor digest makes the
/// hand-off auditable and prevents a caller from replacing the user-visible
/// text or JSON schemas after verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityDescriptionProof {
    pub schema: String,
    pub descriptor: CapabilityDescriptor,
    pub descriptor_digest: String,
    pub signer_id: String,
}

impl CapabilityDescriptionProof {
    /// Build a proof after the embedding host has verified the signed
    /// publication. This constructor computes the digest from the exact
    /// descriptor and deliberately does not perform cryptographic verification
    /// itself; key custody and signature policy remain outside the universal
    /// package-manager core.
    pub fn from_verified(
        descriptor: CapabilityDescriptor,
        signer_id: impl Into<String>,
    ) -> UseResult<Self> {
        descriptor.validate()?;
        let proof = Self {
            schema: CAPABILITY_DESCRIPTION_PROOF_SCHEMA_V1.to_owned(),
            descriptor_digest: descriptor.descriptor_digest()?,
            descriptor,
            signer_id: signer_id.into(),
        };
        proof.validate()?;
        Ok(proof)
    }

    /// Decode and validate a bounded proof document.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "Capability description proof",
            CAPABILITY_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_DESCRIPTION_PROOF_SCHEMA_V1
            || !valid_sha256(&self.descriptor_digest)
            || !valid_signer_id(&self.signer_id)
        {
            return Err(capability_error(
                "The capability description proof identity or signer is invalid.",
            ));
        }
        self.descriptor.validate()?;
        if self.descriptor.descriptor_digest()? != self.descriptor_digest {
            return Err(capability_error(
                "The capability description proof does not match its descriptor.",
            ));
        }
        Ok(())
    }

    pub fn descriptor(&self) -> &CapabilityDescriptor {
        &self.descriptor
    }

    pub fn into_descriptor(self) -> CapabilityDescriptor {
        self.descriptor
    }
}

fn valid_signer_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CAPABILITY_SIGNER_ID_BYTES
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
        && !value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
}
