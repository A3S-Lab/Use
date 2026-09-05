//! Canonical signed envelopes for agent-visible capability descriptions.
//!
//! The universal core deliberately describes the bytes and the fields that a
//! Registry trust boundary must authenticate.  It does not own keys or choose
//! a cryptographic implementation.  A host/Registry adapter can therefore
//! verify these bytes with its own key custody while every language shares the
//! same bounded, domain-separated payload.

use serde::{Deserialize, Serialize};

use crate::UseResult;

use super::{
    canonical_json, capability_error, parse_contract, CapabilityDescriptor, CAPABILITY_ERROR,
};
use crate::plugin::canonical_digest;

/// Schema for a signed capability-description envelope.
pub const CAPABILITY_DESCRIPTION_SIGNATURE_SCHEMA_V1: &str =
    "a3s.use.capability-description-signature.v1";
/// The first supported public-key algorithm.  New algorithms require a new
/// explicit enum variant and a separate verifier implementation.
pub const CAPABILITY_DESCRIPTION_SIGNATURE_ALGORITHM_ED25519: &str = "ed25519";

const CAPABILITY_SIGNATURE_DOMAIN: &[u8] = b"a3s.use.capability-description-signature.v1\0";
const MAX_CAPABILITY_SIGNATURE_ID_BYTES: usize = 256;
const MAX_CAPABILITY_SIGNATURE_HEX_BYTES: usize = 128;
const MAX_CAPABILITY_SIGNATURE_LIFETIME_SECONDS: u64 = 31_536_000;

/// Algorithms accepted by the signed-description contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityDescriptionSignatureAlgorithm {
    Ed25519,
}

impl CapabilityDescriptionSignatureAlgorithm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ed25519 => CAPABILITY_DESCRIPTION_SIGNATURE_ALGORITHM_ED25519,
        }
    }
}

/// Canonical bytes authenticated by a Registry key.
///
/// The signature itself is intentionally absent.  Signers serialize this
/// value with [`Self::canonical_bytes`], prepend the fixed domain separator,
/// and sign the resulting bytes.  Including the descriptor digest and all
/// identity/time fields prevents an otherwise valid signature from being
/// replayed for another key, signer, or validity window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityDescriptionSignaturePayload {
    pub schema: String,
    pub descriptor: CapabilityDescriptor,
    pub descriptor_digest: String,
    pub signer_id: String,
    pub key_id: String,
    pub algorithm: CapabilityDescriptionSignatureAlgorithm,
    pub issued_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

impl CapabilityDescriptionSignaturePayload {
    /// Build and validate a payload before handing it to a signer.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        descriptor: CapabilityDescriptor,
        signer_id: impl Into<String>,
        key_id: impl Into<String>,
        algorithm: CapabilityDescriptionSignatureAlgorithm,
        issued_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> UseResult<Self> {
        let payload = Self {
            schema: CAPABILITY_DESCRIPTION_SIGNATURE_SCHEMA_V1.to_owned(),
            descriptor_digest: descriptor.descriptor_digest()?,
            descriptor,
            signer_id: signer_id.into(),
            key_id: key_id.into(),
            algorithm,
            issued_at_unix_seconds,
            expires_at_unix_seconds,
        };
        payload.validate()?;
        Ok(payload)
    }

    /// Return the exact domain-separated bytes to sign or verify.
    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = CAPABILITY_SIGNATURE_DOMAIN.to_vec();
        bytes.extend(canonical_json(
            self,
            "capability description signature payload",
            CAPABILITY_ERROR,
        )?);
        Ok(bytes)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_DESCRIPTION_SIGNATURE_SCHEMA_V1
            || !valid_identity(&self.signer_id)
            || !valid_identity(&self.key_id)
            || self.algorithm != CapabilityDescriptionSignatureAlgorithm::Ed25519
            || self.issued_at_unix_seconds == 0
            || self.expires_at_unix_seconds <= self.issued_at_unix_seconds
            || self
                .expires_at_unix_seconds
                .saturating_sub(self.issued_at_unix_seconds)
                > MAX_CAPABILITY_SIGNATURE_LIFETIME_SECONDS
        {
            return Err(capability_error(
                "The capability description signature identity or validity window is invalid.",
            ));
        }
        self.descriptor.validate()?;
        if self.descriptor.descriptor_digest()? != self.descriptor_digest {
            return Err(capability_error(
                "The capability description signature does not match its descriptor.",
            ));
        }
        Ok(())
    }
}

/// A complete signed capability-description envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedCapabilityDescription {
    #[serde(flatten)]
    pub payload: CapabilityDescriptionSignaturePayload,
    /// Lower-case hexadecimal Ed25519 signature (64 bytes / 128 characters).
    pub signature: String,
}

impl SignedCapabilityDescription {
    /// Assemble an envelope from a canonical payload and a signature produced
    /// by the Registry key owner.
    pub fn from_parts(
        payload: CapabilityDescriptionSignaturePayload,
        signature: impl Into<String>,
    ) -> UseResult<Self> {
        let envelope = Self {
            payload,
            signature: signature.into(),
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Decode and structurally validate a bounded signed envelope.  This does
    /// not verify the cryptographic signature; use the Registry trust-store
    /// adapter for that operation.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "signed capability description",
            CAPABILITY_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        self.payload.validate()?;
        if !valid_signature_hex(&self.signature) {
            return Err(capability_error(
                "The capability description signature encoding is invalid.",
            ));
        }
        Ok(())
    }

    /// Return the exact bytes a verifier must authenticate.
    pub fn signing_bytes(&self) -> UseResult<Vec<u8>> {
        self.payload.canonical_bytes()
    }

    /// Return canonical envelope bytes suitable for durable replay evidence.
    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "signed capability description", CAPABILITY_ERROR)
    }

    /// Digest the exact signature bytes, not the textual envelope.
    pub fn signature_digest(&self) -> UseResult<String> {
        self.validate()?;
        let signature = decode_hex(&self.signature)?;
        Ok(canonical_digest(&signature))
    }

    pub fn descriptor(&self) -> &CapabilityDescriptor {
        &self.payload.descriptor
    }

    pub fn signer_id(&self) -> &str {
        &self.payload.signer_id
    }

    pub fn key_id(&self) -> &str {
        &self.payload.key_id
    }

    pub fn algorithm(&self) -> CapabilityDescriptionSignatureAlgorithm {
        self.payload.algorithm
    }

    pub fn issued_at_unix_seconds(&self) -> u64 {
        self.payload.issued_at_unix_seconds
    }

    pub fn expires_at_unix_seconds(&self) -> u64 {
        self.payload.expires_at_unix_seconds
    }

    /// Decode the fixed-size signature for a crypto adapter.
    pub fn signature_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        decode_hex(&self.signature)
    }
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CAPABILITY_SIGNATURE_ID_BYTES
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

fn valid_signature_hex(value: &str) -> bool {
    value.len() == MAX_CAPABILITY_SIGNATURE_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn decode_hex(value: &str) -> UseResult<Vec<u8>> {
    if !valid_signature_hex(value) {
        return Err(capability_error(
            "The capability description signature encoding is invalid.",
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0]).ok_or_else(|| {
                capability_error("The capability description signature encoding is invalid.")
            })?;
            let low = hex_nibble(pair[1]).ok_or_else(|| {
                capability_error("The capability description signature encoding is invalid.")
            })?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
