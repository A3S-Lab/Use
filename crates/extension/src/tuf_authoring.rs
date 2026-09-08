//! Production TUF signing primitives for A3S Use registries.
//!
//! The bytes produced here are the signing half of the registry contract the
//! client in [`crate::remote`] verifies with `tough`. Test support and the
//! registry tooling share this module so a signed document can only be
//! produced one canonical way: canonical JSON (olpc-cjson) over the signed
//! role object, one Ed25519 signature, hex-encoded, wrapped in the standard
//! `{"signatures": [...], "signed": ...}` envelope.

use olpc_cjson::CanonicalFormatter;
use ring::signature::Ed25519KeyPair;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// SHA-256 over arbitrary bytes, formatted as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Lowercase hex encoding.
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Canonical (RFC 8785 subset used by TUF) JSON serialization.
pub fn canonical_json(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).expect(
        "canonical JSON serialization of a serde_json::Value is infallible for this formatter",
    );
    bytes
}

/// The TUF key object for one Ed25519 public key.
pub fn ed25519_key_value(public: &[u8]) -> Value {
    json!({
        "keytype": "ed25519",
        "scheme": "ed25519",
        "keyval": {"public": hex_lower(public)}
    })
}

/// The TUF key id: SHA-256 over the canonical form of the key object.
pub fn ed25519_key_id(public: &[u8]) -> String {
    sha256_hex(&canonical_json(&ed25519_key_value(public)))
}

/// Sign one TUF role document and wrap it in the standard envelope.
///
/// The signature covers only the canonical serialization of `signed`; the
/// envelope itself is serialized compactly, matching the layout the client
/// and the frozen registry fixtures expect.
pub fn sign_tuf_document(key: &Ed25519KeyPair, key_id: &str, signed: Value) -> Vec<u8> {
    let signature = key.sign(&canonical_json(&signed));
    serde_json::to_vec(&json!({
        "signatures": [{"keyid": key_id, "sig": hex_lower(signature.as_ref())}],
        "signed": signed
    }))
    .expect("serializing a TUF signature envelope cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;

    #[test]
    fn canonical_json_orders_object_keys_deterministically() {
        let value = json!({"b": 1, "a": 2});
        assert_eq!(canonical_json(&value), br#"{"a":2,"b":1}"#);
    }

    #[test]
    fn key_id_is_stable_for_a_fixed_public_key() {
        let key = Ed25519KeyPair::from_seed_unchecked(&[7_u8; 32]).unwrap();
        let public = key.public_key().as_ref().to_vec();
        let first = ed25519_key_id(&public);
        assert_eq!(first.len(), 64);
        assert_eq!(first, ed25519_key_id(&public));
        assert_ne!(first, ed25519_key_id(&[0_u8; 32]));
    }

    #[test]
    fn signed_documents_cover_their_canonical_bytes_deterministically() {
        let key = Ed25519KeyPair::from_seed_unchecked(&[7_u8; 32]).unwrap();
        let key_id = ed25519_key_id(key.public_key().as_ref());
        let signed = json!({"_type": "targets", "version": 3});
        let document = sign_tuf_document(&key, &key_id, signed.clone());
        let parsed: Value = serde_json::from_slice(&document).unwrap();
        assert_eq!(parsed["signed"], signed);
        // Ed25519 signatures are deterministic, so an independent signature
        // over the canonical bytes must equal the envelope's signature.
        let signature = parsed["signatures"][0]["sig"].as_str().unwrap();
        assert_eq!(
            signature,
            &hex_lower(key.sign(&canonical_json(&signed)).as_ref())
        );
    }
}
