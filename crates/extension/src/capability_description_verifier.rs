//! Registry-owned cryptographic verification for agent-visible descriptions.
//!
//! a3s-use-core defines the canonical signed envelope, while this crate owns
//! the trust-store boundary. Private signing keys never enter this API. A
//! host injects a bounded, Registry-controlled set of public keys and receives
//! a verified wrapper only after Ed25519, identity, validity, and revocation
//! checks all succeed.

use std::collections::BTreeMap;

use a3s_use_core::{
    CapabilityDescriptionProof, CapabilityDescriptionSignatureAlgorithm, CapabilityDescriptorKind,
    SignedCapabilityDescription, UseError, UseResult,
};
use olpc_cjson::CanonicalFormatter;
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CAPABILITY_DESCRIPTION_TRUST_KEY_SCHEMA_V1: &str =
    "a3s.use.capability-description-trust-key.v1";
pub const CAPABILITY_DESCRIPTION_TRUST_STORE_SCHEMA_V1: &str =
    "a3s.use.capability-description-trust-store.v1";
const TRUST_ERROR: &str = "use.extension.capability_description_untrusted";
const MAX_TRUST_KEYS: usize = 128;
const MAX_TRUST_ID_BYTES: usize = 256;
const MAX_PUBLIC_KEY_HEX_BYTES: usize = 64;
const MAX_KEY_LIFETIME_SECONDS: u64 = 315_360_000;
const MAX_TRUST_STORE_BYTES: usize = 512 * 1024;

/// One Registry-controlled public verification key.
///
/// The structure intentionally contains no private key material. Multiple
/// non-revoked keys may exist for one signer during rotation; the signed
/// envelope's keyId selects exactly one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityDescriptionTrustKey {
    pub schema: String,
    pub key_id: String,
    pub signer_id: String,
    pub algorithm: CapabilityDescriptionSignatureAlgorithm,
    /// Lower-case hexadecimal Ed25519 public key (32 bytes / 64 characters).
    pub public_key: String,
    pub not_before_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_unix_seconds: Option<u64>,
}

impl CapabilityDescriptionTrustKey {
    /// Construct and validate a public key entry.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key_id: impl Into<String>,
        signer_id: impl Into<String>,
        algorithm: CapabilityDescriptionSignatureAlgorithm,
        public_key: impl Into<String>,
        not_before_unix_seconds: u64,
        expires_at_unix_seconds: u64,
        revoked_at_unix_seconds: Option<u64>,
    ) -> UseResult<Self> {
        let key = Self {
            schema: CAPABILITY_DESCRIPTION_TRUST_KEY_SCHEMA_V1.to_owned(),
            key_id: key_id.into(),
            signer_id: signer_id.into(),
            algorithm,
            public_key: public_key.into(),
            not_before_unix_seconds,
            expires_at_unix_seconds,
            revoked_at_unix_seconds,
        };
        key.validate()?;
        Ok(key)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_DESCRIPTION_TRUST_KEY_SCHEMA_V1
            || !valid_identity(&self.key_id)
            || !valid_identity(&self.signer_id)
            || self.algorithm != CapabilityDescriptionSignatureAlgorithm::Ed25519
            || !valid_public_key_hex(&self.public_key)
            || self.not_before_unix_seconds == 0
            || self.expires_at_unix_seconds <= self.not_before_unix_seconds
            || self
                .expires_at_unix_seconds
                .saturating_sub(self.not_before_unix_seconds)
                > MAX_KEY_LIFETIME_SECONDS
            || self.revoked_at_unix_seconds.is_some_and(|revoked| {
                revoked <= self.not_before_unix_seconds || revoked >= self.expires_at_unix_seconds
            })
        {
            return Err(trust_error(
                "The capability description trust key identity, encoding, or validity is invalid.",
            ));
        }
        Ok(())
    }

    pub fn is_valid_at(&self, now_unix_seconds: u64) -> bool {
        now_unix_seconds >= self.not_before_unix_seconds
            && now_unix_seconds < self.expires_at_unix_seconds
            && self
                .revoked_at_unix_seconds
                .is_none_or(|revoked| now_unix_seconds < revoked)
    }
}

/// Bounded public-key policy loaded from a Registry trust boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityDescriptionTrustStore {
    pub schema: String,
    pub keys: Vec<CapabilityDescriptionTrustKey>,
    #[serde(skip)]
    index: BTreeMap<String, usize>,
}

impl CapabilityDescriptionTrustStore {
    pub fn new(mut keys: Vec<CapabilityDescriptionTrustKey>) -> UseResult<Self> {
        if keys.len() > MAX_TRUST_KEYS {
            return Err(trust_error(
                "The capability description trust store is too large.",
            ));
        }
        for key in &keys {
            key.validate()?;
        }
        keys.sort_by(|left, right| left.key_id.cmp(&right.key_id));
        let mut index = BTreeMap::new();
        for (position, key) in keys.iter().enumerate() {
            if index.insert(key.key_id.clone(), position).is_some() {
                return Err(trust_error(
                    "The capability description trust store contains a duplicate key id.",
                ));
            }
        }
        let store = Self {
            schema: CAPABILITY_DESCRIPTION_TRUST_STORE_SCHEMA_V1.to_owned(),
            keys,
            index,
        };
        store.validate()?;
        Ok(store)
    }

    /// Decode a bounded key policy. Cryptographic admission still happens
    /// when a signed description is verified.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > MAX_TRUST_STORE_BYTES {
            return Err(trust_error(
                "The capability description trust store exceeds its size bound.",
            ));
        }
        let wire: WireTrustStore = serde_json::from_slice(input).map_err(|error| {
            trust_error(format!(
                "Failed to decode the capability description trust store at line {}, column {}.",
                error.line(),
                error.column()
            ))
        })?;
        let mut store = Self::new(wire.keys)?;
        store.schema = wire.schema;
        store.validate()?;
        Ok(store)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_DESCRIPTION_TRUST_STORE_SCHEMA_V1
            || self.keys.len() > MAX_TRUST_KEYS
            || self
                .keys
                .windows(2)
                .any(|pair| pair[0].key_id >= pair[1].key_id)
        {
            return Err(trust_error(
                "The capability description trust store identity or ordering is invalid.",
            ));
        }
        for key in &self.keys {
            key.validate()?;
        }
        if self.index.len() != self.keys.len()
            || self
                .keys
                .iter()
                .enumerate()
                .any(|(position, key)| self.index.get(&key.key_id) != Some(&position))
        {
            return Err(trust_error(
                "The capability description trust store index is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            trust_error(format!(
                "Failed to encode the canonical capability description trust store: {error}"
            ))
        })?;
        if bytes.len() > MAX_TRUST_STORE_BYTES {
            return Err(trust_error(
                "The canonical capability description trust store exceeds its size bound.",
            ));
        }
        Ok(bytes)
    }

    pub fn keys(&self) -> &[CapabilityDescriptionTrustKey] {
        &self.keys
    }

    pub fn find(&self, key_id: &str) -> Option<&CapabilityDescriptionTrustKey> {
        self.index
            .get(key_id)
            .and_then(|position| self.keys.get(*position))
    }

    /// Verify one signed description at an explicit wall-clock instant.
    ///
    /// The caller supplies time so replay and restore tests can be
    /// deterministic. Production hosts should obtain this value from their
    /// trusted clock source and re-run verification when restoring retained
    /// evidence.
    pub fn verify(
        &self,
        signed: &SignedCapabilityDescription,
        now_unix_seconds: u64,
    ) -> UseResult<VerifiedCapabilityDescription> {
        self.validate()?;
        signed.validate().map_err(|_| {
            trust_error("The signed capability description failed structural validation.")
        })?;
        if matches!(
            signed.descriptor().capability,
            CapabilityDescriptorKind::Tool {
                runtime_descriptor_digest: None,
                ..
            }
        ) {
            return Err(trust_error(
                "An agent-visible Tool must bind a schema-bearing Runtime release descriptor.",
            ));
        }
        let key = self.find(signed.key_id()).ok_or_else(|| {
            trust_error("The signed capability description references an unknown key id.")
        })?;
        if signed.signer_id() != key.signer_id
            || signed.algorithm() != key.algorithm
            || now_unix_seconds < signed.issued_at_unix_seconds()
            || now_unix_seconds >= signed.expires_at_unix_seconds()
            || signed.issued_at_unix_seconds() < key.not_before_unix_seconds
            || signed.expires_at_unix_seconds() > key.expires_at_unix_seconds
            || !key.is_valid_at(now_unix_seconds)
        {
            return Err(trust_error(
                "The signed capability description is outside its key or envelope validity window.",
            ));
        }

        let public_key = decode_hex(&key.public_key, MAX_PUBLIC_KEY_HEX_BYTES)?;
        let signature = signed.signature_bytes().map_err(|_| {
            trust_error("The signed capability description signature encoding is invalid.")
        })?;
        let signing_bytes = signed.signing_bytes().map_err(|_| {
            trust_error("The signed capability description payload is not canonical.")
        })?;
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(&signing_bytes, &signature)
            .map_err(|_| trust_error("The capability description signature is not valid."))?;

        let signature_digest = format!("sha256:{:x}", Sha256::digest(&signature));
        Ok(VerifiedCapabilityDescription {
            signed: signed.clone(),
            signature_digest,
            verified_at_unix_seconds: now_unix_seconds,
        })
    }

    pub fn verify_json(
        &self,
        input: &[u8],
        now_unix_seconds: u64,
    ) -> UseResult<VerifiedCapabilityDescription> {
        let signed = SignedCapabilityDescription::from_json(input)
            .map_err(|_| trust_error("The signed capability description could not be decoded."))?;
        self.verify(&signed, now_unix_seconds)
    }
}

/// Exact evidence produced by CapabilityDescriptionTrustStore::verify.
///
/// The fields are private so callers cannot manufacture a verified value by
/// setting a boolean or signer string. Persist the canonical signed envelope
/// (canonical_bytes) and re-run reverify after restart or restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedCapabilityDescription {
    signed: SignedCapabilityDescription,
    signature_digest: String,
    verified_at_unix_seconds: u64,
}

impl VerifiedCapabilityDescription {
    /// Decode a retained signed envelope and verify it against the current
    /// public-key policy. The input is deliberately the signed envelope, not
    /// a serialized `VerifiedCapabilityDescription` wrapper; replay must
    /// perform cryptographic verification again after restart or restore.
    pub fn from_json(
        input: &[u8],
        trust_store: &CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
    ) -> UseResult<Self> {
        trust_store.verify_json(input, now_unix_seconds)
    }

    pub fn descriptor(&self) -> &a3s_use_core::CapabilityDescriptor {
        self.signed.descriptor()
    }

    pub fn signed(&self) -> &SignedCapabilityDescription {
        &self.signed
    }

    pub fn signature_digest(&self) -> &str {
        &self.signature_digest
    }

    pub fn verified_at_unix_seconds(&self) -> u64 {
        self.verified_at_unix_seconds
    }

    /// Return canonical bytes for durable replay evidence.
    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        self.signed.canonical_bytes()
    }

    pub fn validate(&self) -> UseResult<()> {
        self.signed.validate()?;
        if self.verified_at_unix_seconds == 0
            || self.signature_digest != self.signed.signature_digest()?
        {
            return Err(trust_error(
                "The verified capability description evidence is inconsistent.",
            ));
        }
        Ok(())
    }

    /// Recheck retained evidence against the current key policy.
    pub fn reverify(
        &self,
        trust_store: &CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
    ) -> UseResult<Self> {
        trust_store.verify(&self.signed, now_unix_seconds)
    }

    /// Convert only after cryptographic verification to the core proof
    /// consumed by the inactive Capability Plane.
    pub fn into_proof(self) -> UseResult<CapabilityDescriptionProof> {
        self.validate()?;
        CapabilityDescriptionProof::from_verified(
            self.signed.descriptor().clone(),
            self.signed.signer_id().to_owned(),
        )
    }

    pub fn proof(&self) -> UseResult<CapabilityDescriptionProof> {
        self.clone().into_proof()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireTrustStore {
    schema: String,
    keys: Vec<CapabilityDescriptionTrustKey>,
}

impl<'de> Deserialize<'de> for CapabilityDescriptionTrustStore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WireTrustStore::deserialize(deserializer)?;
        let mut store = Self::new(wire.keys).map_err(D::Error::custom)?;
        store.schema = wire.schema;
        store.validate().map_err(D::Error::custom)?;
        Ok(store)
    }
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TRUST_ID_BYTES
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

fn valid_public_key_hex(value: &str) -> bool {
    value.len() == MAX_PUBLIC_KEY_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn decode_hex(value: &str, expected_chars: usize) -> UseResult<Vec<u8>> {
    if value.len() != expected_chars
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(trust_error(
            "The capability description public key is invalid.",
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])
                .ok_or_else(|| trust_error("The capability description public key is invalid."))?;
            let low = hex_nibble(pair[1])
                .ok_or_else(|| trust_error("The capability description public key is invalid."))?;
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

fn trust_error(message: impl Into<String>) -> UseError {
    UseError::new(TRUST_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_use_core::{
        CapabilityDescriptor, CapabilityDescriptorKind, CapabilityPublicationEvidence,
        CapabilityToolAnnotations, InvocationRef, PluginPackageId, PluginSurfaceKind,
        PluginSurfaceRef, CAPABILITY_DESCRIPTOR_SCHEMA_V1,
    };
    use ring::signature::{Ed25519KeyPair, KeyPair};

    const SEED: [u8; 32] = [7; 32];

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn descriptor() -> CapabilityDescriptor {
        let package = PluginPackageId::parse("acme/assistant").unwrap();
        let surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "search".to_owned(),
        };
        CapabilityDescriptor {
            schema: CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
            package_id: package.clone(),
            surface: surface.clone(),
            generation: 3,
            package_digest: digest('a'),
            manifest_digest: digest('b'),
            title: "Search".to_owned(),
            description: "Search verified data".to_owned(),
            invocation_ref: InvocationRef::derive(&package, &surface, 3, &digest('c')).unwrap(),
            artifact_ref: None,
            endpoint_ref: None,
            dependencies: Vec::new(),
            required_extensions: Vec::new(),
            publication: CapabilityPublicationEvidence {
                catalog_record_digest: digest('d'),
                signature_digest: digest('e'),
            },
            capability: CapabilityDescriptorKind::Tool {
                name: "search".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }),
                output_schema: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"count": {"type": "integer"}}
                }),
                annotations: CapabilityToolAnnotations::new(true, false, true, false),
                runtime_descriptor_digest: Some(digest('f')),
            },
        }
    }

    fn digest(letter: char) -> String {
        format!("sha256:{}", letter.to_string().repeat(64))
    }

    fn signed(key_id: &str, signer_id: &str) -> (SignedCapabilityDescription, Ed25519KeyPair) {
        signed_with_seed(key_id, signer_id, &SEED)
    }

    fn signed_with_seed(
        key_id: &str,
        signer_id: &str,
        seed: &[u8; 32],
    ) -> (SignedCapabilityDescription, Ed25519KeyPair) {
        let key_pair = Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
        let payload = a3s_use_core::CapabilityDescriptionSignaturePayload::new(
            descriptor(),
            signer_id,
            key_id,
            CapabilityDescriptionSignatureAlgorithm::Ed25519,
            1_000,
            2_000,
        )
        .unwrap();
        let signature = key_pair.sign(&payload.canonical_bytes().unwrap());
        (
            SignedCapabilityDescription::from_parts(payload, hex(signature.as_ref())).unwrap(),
            key_pair,
        )
    }

    fn store(
        key_id: &str,
        signer_id: &str,
        revoked_at: Option<u64>,
    ) -> CapabilityDescriptionTrustStore {
        let (_, key_pair) = signed(key_id, signer_id);
        let key = CapabilityDescriptionTrustKey::new(
            key_id,
            signer_id,
            CapabilityDescriptionSignatureAlgorithm::Ed25519,
            hex(key_pair.public_key().as_ref()),
            900,
            2_500,
            revoked_at,
        )
        .unwrap();
        CapabilityDescriptionTrustStore::new(vec![key]).unwrap()
    }

    #[test]
    fn verifies_ed25519_and_returns_a_private_replay_wrapper() {
        let (signed, _) = signed("registry/official/2026", "registry/official");
        let store = store("registry/official/2026", "registry/official", None);
        let verified = store.verify(&signed, 1_500).unwrap();
        verified.validate().unwrap();
        assert_eq!(verified.descriptor().tool_name(), Some("search"));
        assert_eq!(verified.signature_digest().len(), 71);
        assert_eq!(
            verified.canonical_bytes().unwrap(),
            signed.canonical_bytes().unwrap()
        );
        let replayed = VerifiedCapabilityDescription::from_json(
            &signed.canonical_bytes().unwrap(),
            &store,
            1_600,
        )
        .unwrap();
        assert_eq!(replayed.descriptor(), verified.descriptor());
        assert_eq!(replayed.signature_digest(), verified.signature_digest());
        assert!(verified.into_proof().is_ok());
    }

    #[test]
    fn rejects_tampering_unknown_keys_expiry_and_revocation() {
        let (mut tampered, _) = signed("registry/official/2026", "registry/official");
        let trust_store = store("registry/official/2026", "registry/official", None);
        tampered.payload.descriptor.title = "tampered".to_owned();
        assert!(trust_store.verify(&tampered, 1_500).is_err());

        let (signed_again, _) = signed("registry/official/2026", "registry/official");
        let unknown = store("registry/other", "registry/official", None);
        assert!(unknown.verify(&signed_again, 1_500).is_err());

        let (signed_again, _) = signed("registry/official/2026", "registry/official");
        assert!(trust_store.verify(&signed_again, 2_000).is_err());

        let revoked = store("registry/official/2026", "registry/official", Some(1_400));
        assert!(revoked.verify(&signed_again, 1_500).is_err());
        assert!(revoked.verify(&signed_again, 1_300).is_ok());
    }

    #[test]
    fn supports_rotation_and_canonical_key_store_replay() {
        let (old_signed, old_pair) = signed("registry/official/old", "registry/official");
        let (new_signed, new_pair) =
            signed_with_seed("registry/official/new", "registry/official", &[9; 32]);
        let old = CapabilityDescriptionTrustKey::new(
            "registry/official/old",
            "registry/official",
            CapabilityDescriptionSignatureAlgorithm::Ed25519,
            hex(old_pair.public_key().as_ref()),
            900,
            2_200,
            Some(1_300),
        )
        .unwrap();
        let new = CapabilityDescriptionTrustKey::new(
            "registry/official/new",
            "registry/official",
            CapabilityDescriptionSignatureAlgorithm::Ed25519,
            hex(new_pair.public_key().as_ref()),
            900,
            2_500,
            None,
        )
        .unwrap();
        let store = CapabilityDescriptionTrustStore::new(vec![new, old]).unwrap();
        let encoded = store.canonical_bytes().unwrap();
        let decoded = CapabilityDescriptionTrustStore::from_json(&encoded).unwrap();
        assert_eq!(decoded, store);
        let direct: CapabilityDescriptionTrustStore = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(direct, store);
        assert!(decoded.verify(&old_signed, 1_250).is_ok());
        assert!(decoded.verify(&old_signed, 1_350).is_err());
        assert!(decoded.verify(&new_signed, 1_500).is_ok());
    }

    #[test]
    fn rejects_duplicate_keys_and_invalid_store_wire_schema() {
        let (_, pair) = signed("registry/official/2026", "registry/official");
        let key = CapabilityDescriptionTrustKey::new(
            "registry/official/2026",
            "registry/official",
            CapabilityDescriptionSignatureAlgorithm::Ed25519,
            hex(pair.public_key().as_ref()),
            900,
            2_500,
            None,
        )
        .unwrap();
        assert!(CapabilityDescriptionTrustStore::new(vec![key.clone(), key]).is_err());
        let malformed = serde_json::json!({"schema": "wrong", "keys": []});
        assert!(CapabilityDescriptionTrustStore::from_json(
            serde_json::to_vec(&malformed).unwrap().as_slice(),
        )
        .is_err());
    }

    #[test]
    fn rejects_legacy_host_only_tool_descriptions() {
        let (mut signed, _) = signed("registry/official/2026", "registry/official");
        if let CapabilityDescriptorKind::Tool {
            runtime_descriptor_digest,
            ..
        } = &mut signed.payload.descriptor.capability
        {
            *runtime_descriptor_digest = None;
        }
        let trust_store = store("registry/official/2026", "registry/official", None);
        assert!(trust_store.verify(&signed, 1_500).is_err());
    }
}
