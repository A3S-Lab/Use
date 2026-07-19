# Immutable MCP and Skill Release Descriptors

## Purpose

A3S Cloud needs one content-addressed release boundary for hosted MCP services
and Skill inputs. A3S Use owns the provider-neutral descriptor contract:

- `a3s.use.mcp-release.v1` describes a headless MCP Runtime Service;
- `a3s.use.skill-release.v1` describes immutable Skill input for an Agent; and
- neither schema contains a mutable artifact tag, secret value, provider
  configuration, or generic orchestration payload.

These are machine-owned, versioned JSON records. Human-authored extension
configuration remains A3S ACL and continues to be parsed with `a3s-acl`.

## Canonical identity

`McpReleaseDescriptor::canonical_bytes` and
`SkillReleaseDescriptor::canonical_bytes` validate a descriptor and encode it
as OLPC canonical JSON:

- object keys are sorted;
- strings are NFC-normalized;
- insignificant whitespace is removed;
- floating-point values are not permitted; and
- arrays that represent sets must already be sorted and unique.

The descriptor identity is:

```text
sha256(OLPC-canonical-JSON)
```

It is formatted as `sha256:<64 lowercase hexadecimal characters>`. Input JSON
is bounded to 256 KiB. Unknown fields, unknown enum variants, unsupported
schemas, duplicate set entries, and noncanonical identifiers fail closed.
Whitespace and input object-key order do not change the descriptor digest.

The repository publishes canonical cross-SDK fixtures and digest goldens:

| Schema | Canonical fixture | Expected descriptor digest |
| --- | --- | --- |
| MCP v1 | [`mcp-release-v1.json`](../crates/core/fixtures/releases/mcp-release-v1.json) | [`mcp-release-v1.sha256`](../crates/core/fixtures/releases/mcp-release-v1.sha256) |
| Skill v1 | [`skill-release-v1.json`](../crates/core/fixtures/releases/skill-release-v1.json) | [`skill-release-v1.sha256`](../crates/core/fixtures/releases/skill-release-v1.sha256) |

The text fixtures end with one repository newline. That newline is not part of
the canonical JSON bytes and is not included in the adjacent digest.

## Common fields

Both descriptors carry:

| Field | Contract |
| --- | --- |
| `schema` and `kind` | Exact matching v1 schema and `mcp` or `skill` kind |
| `name` | Lowercase `<publisher>/<name>` identity |
| `version` | Canonical semantic version |
| `provenance.sourceRepository` | Canonical HTTPS URL without credentials, query, or fragment |
| `provenance.commitSha` | Exact lowercase 40- or 64-character Git object ID |
| `provenance.manifestDigest` | SHA-256 of the admitted source manifest |
| `provenance.builderId` | Stable non-secret builder identity |
| `provenance.buildOperationId` | Stable non-secret build operation identity |
| `artifact` | Media type, exact SHA-256, and positive bounded byte size |
| `compatibility` | Sorted component names and semantic-version requirements |
| `dependencies` | Exact release kind, name, version, and descriptor digest, sorted by kind (`mcp` before `skill`) and then name with each kind/name identity unique |

Artifact records deliberately have no location or tag. Cloud resolves the
digest through its Artifact aggregate after validating the descriptor. A
registry tag, branch, channel, latest-version selector, URL credential, or
dependency version range cannot become release identity.

The schemas contain no environment map, command secret, credential, token, or
inline configuration value. Secret references and secret delivery remain
deployment policy above the descriptor and plaintext must never enter
descriptor bytes or diagnostics.

## MCP v1

An MCP v1 artifact is a digest-pinned OCI image manifest or image index. Its
service contract permits only standard MCP Streamable HTTP and declares:

- the exact MCP protocol date;
- one named container port and HTTP endpoint;
- an HTTP health path, interval, timeout, and success/failure thresholds;
- a bounded startup deadline; and
- a bounded graceful-shutdown interval.

The v1 lifecycle is headless:

1. The process starts without an interactive prompt, terminal, or stdin
   dependency.
2. Runtime does not report the release ready until the declared health check
   succeeds.
3. The endpoint completes standard MCP initialization and requests using the
   declared protocol version.
4. Termination drains within `shutdownGraceMs`.
5. Restarting the same descriptor and artifact digest reconstructs the same
   service identity and does not create mutable release state.

The descriptor supplies deterministic Runtime Service inputs; it does not
replace real process conformance. Before an MCP release is certified, a
digest-pinned fixture must still prove startup, health, initialize/request,
shutdown, cleanup, and restart behavior on the target Linux environment.

Stdio MCP remains a local native extension surface. It is not a hosted MCP v1
release transport and cannot be presented as a Runtime Service.

## Skill v1

A Skill v1 artifact uses
`application/vnd.a3s.skill.bundle.v1+tar+gzip`. It binds:

- one portable package-relative `SKILL.md` entrypoint;
- the exact SHA-256 of that entrypoint;
- a sorted set of required capability identifiers; and
- the sole binding target, `agent-input`.

The bundle digest protects the complete release artifact while
`entrypointDigest` protects the exact instructions loaded by the Agent. The
consumer verifies both before replacing a live binding. Changing either digest
creates a new workload revision and preserves the prior release for rollback.
Entrypoint paths use `/` separators and reject empty, `.` and `..` segments so
all SDKs resolve the same bundle member.

A Skill descriptor has no command, port, health check, Runtime unit,
restart policy, environment, or executable surface. Unknown fields are
rejected, so a Skill cannot silently become a standalone workload.

## Resolution gate

`ReleaseResolution` is the pre-deployment evidence supplied by the caller:

- `components` maps installed component names to exact semantic versions; and
- `dependencies` contains the resolved immutable dependency releases.

`verify_resolution` checks every compatibility requirement and every pinned
dependency before deployment. It returns stable errors:

| Code | Meaning |
| --- | --- |
| `use.release.compatibility_missing` | A required component is absent |
| `use.release.incompatible` | An observed component version does not satisfy the requirement |
| `use.release.dependency_missing` | A pinned release dependency is absent |
| `use.release.dependency_mismatch` | Dependency version or descriptor digest differs |
| `use.release.descriptor_invalid` | Descriptor or supplied resolution evidence is malformed or ambiguous |

Additional host components are allowed. Additional dependencies are allowed
only when their identities are unambiguous; they never satisfy a required
dependency unless kind, name, version, and descriptor digest match.

## Schema evolution

The schema string is the compatibility boundary. A decoder:

- accepts only the exact v1 schema it implements;
- rejects unknown fields and enum values;
- never guesses a default for a future field; and
- never reinterprets MCP as Skill or Skill as MCP.

Changing field meaning, adding a field, adding a transport or binding target,
or changing canonicalization requires a new schema identifier and new fixture
digest. V1 fixtures and their digests remain immutable. Consumers may support
multiple explicit schema versions during migration, but publication must select
one exact descriptor and digest.
