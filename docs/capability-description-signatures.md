# Capability description signatures

Status: qualified trust-boundary mechanism; production Registry and Control
activation are still pending.

An MCP Tool description is executable policy input, not presentation-only
metadata. A package must not be able to change its name, JSON schemas, or
opaque invocation identity after a host has reviewed it. A3S Use therefore
separates the portable contract from the cryptographic trust boundary:

1. a3s-use-core defines SignedCapabilityDescription and the canonical bytes
   that are signed.
2. a3s-use-extension provides CapabilityDescriptionTrustStore, the
   Registry-owned Ed25519 verifier.
3. The verifier returns VerifiedCapabilityDescription, whose fields are
   private. Only that value can be converted into the existing
   CapabilityDescriptionProof hand-off used by the inactive Capability Plane.

## Signed bytes

The envelope uses schema
a3s.use.capability-description-signature.v1. Its payload contains the exact
descriptor, its sha256: canonical digest, signerId, keyId, algorithm,
issuedAtUnixSeconds, and expiresAtUnixSeconds. The signature field is not
part of the signed payload.

The signer authenticates:

    a3s.use.capability-description-signature.v1\0
    <OLPC-CJSON(payload)>

The first implementation accepts only Ed25519 and lower-case hexadecimal
signatures (64 bytes). The descriptor is validated before signing and again
before verification. A signed Tool must carry both bounded closed JSON schemas
and the exact Runtime release-descriptor digest; executable-only legacy Tools
remain host-only.

## Registry key policy

CapabilityDescriptionTrustKey contains public material only:

- a unique keyId and its exact signerId;
- the explicit ed25519 algorithm;
- a 32-byte public key;
- a bounded validity interval; and
- an optional revocation time.

CapabilityDescriptionTrustStore sorts and validates at most 128 keys. Multiple
keys may overlap for one signer during rotation. Verification rejects an
unknown key, signer or algorithm mismatch, an envelope outside either
validity interval, an expired or revoked key, invalid canonical bytes, and an
invalid signature. The verifier accepts an explicit clock value so production
and restore tests cannot silently use different time assumptions.

The trust store is public policy, not private key custody. A production host
must load it from the Registry/TUF trust root or another independently
authenticated configuration source. Private signing keys never enter Use,
agent arguments, package archives, or receipts.

## Replay and restore

VerifiedCapabilityDescription::canonical_bytes() returns the exact signed
envelope for durable evidence. A serialized wrapper is not accepted as proof by
itself: restart and restore code must call reverify against the current
Registry key policy. This re-checks expiry and revocation as well as the
signature, so withdrawing a key cannot be bypassed by replaying old evidence.

The mechanism is intentionally not a second Gateway protocol and does not
choose package lifecycle state. The remaining activation work is to source
signed description envelopes from the official Registry, persist them in the
Control-bound proof snapshot, and route the live lifecycle and Gateway through
that one authority. Until then CapabilityDescriptionProof::from_verified
remains a compatibility constructor for explicitly host-verified preview
integrations and must not be treated as cryptographic evidence.
