# Model Hardware Standard Integration Profile

Status: research-preview integration
Last updated: 2026-08-28

## Decision

A3S Use integrates a Model Hardware Standard (MHS) adapter as an ordinary
cognitive package with MCP, Flow, Skill, and optional UI surfaces. MHS is not a
seventh package surface and A3S Use does not define an MHS wire protocol.

The public MHS material is currently a research preview rather than a
versioned, implementable specification. This profile therefore fixes the A3S
ownership and safety boundaries while leaving device vocabulary and protocol
translation inside the adapter. The profile can bind to a published MHS
specification later without changing the package lifecycle.

Official preview material:

- <https://www.anthropic.com/news/model-hardware-standard-research-preview>
- <https://www.modelhardwarestandard.com/>

## First-principles boundary

Physical state is authoritative at the device and its safety controller. A
package manifest is authoritative only for immutable software identity and
dependency topology. A capability snapshot is authoritative only for the exact
software generation that a host may address. These sources of truth must not be
collapsed.

```text
Code or agent
    |
    | immutable capability snapshot
    v
A3S Use
    | package identity, grant, lifecycle, opaque endpoint evidence
    v
A3S Runtime + A3S Gateway
    | hosted MCP adapter, scope-bound credential
    v
MHS control gateway
    | device grant, lease, bounds, interlocks, operation evidence
    v
driver -> safety controller -> hardware
```

| Layer | Owns | Does not own |
| --- | --- | --- |
| A3S Use | Package trust, permission ceiling, reviewed grant, generation, dependency graph, publication evidence | Device state, motion planning, actuator safety, or an MHS protocol fork |
| Runtime and A3S Gateway | Adapter process/service isolation, health, MCP initialization, opaque routing, drain | Package trust or physical authorization policy |
| MHS adapter | MCP-to-MHS translation and domain error preservation | Ambient credentials, package lifecycle, or safety overrides |
| MHS control gateway | Scoped device authorization, leases, parameter bounds, command receipts, reconciliation | A3S package installation |
| Device safety layer | Interlocks, limits, emergency stop, fail-safe behavior | Agent intent inference |

## Package contract

An MHS adapter package uses the existing schema-v3 surfaces:

1. One or more `mcp` surfaces expose adapter capabilities. A managed remote
   adapter uses a digest-pinned `streamable-http` release. A package-local
   adapter may use `stdio` when process execution is explicitly granted.
2. A `flow` may orchestrate repeatable observations or procedures and declares
   its MCP dependency by canonical surface ID.
3. A `skill` provides operator guidance and depends on the exact MCP and Flow
   surfaces it references.
4. A `ui` is static, host-rendered content. Its projected dependency evidence
   identifies the MCP, Flow, and Skill surfaces it needs, but grants no ambient
   network or device authority.

The reference fixture is at
`crates/extension/fixtures/packages/plugin-v3-mhs-bridge/package`. It is a
contract fixture with placeholder provenance and artifact digests. It is not an
MHS implementation and must not be deployed against equipment.

## Authority and publication invariants

The adapter is published only when all generic A3S evidence is exact:

- the package, manifest, selected surfaces, and lifecycle generation match the
  installed receipt;
- package-owned launch or release files pass bounded reinspection;
- the reviewed workspace grant is within the catalog permission ceiling;
- a managed service binding matches the package digest, scope, generation, and
  release descriptor digest;
- the reviewed provider semantics profile binds the non-secret hardware policy
  identity expected for that scope, and the resulting Runtime binding digest
  changes if that profile changes;
- Runtime and Gateway report healthy service evidence and a successful MCP
  initialization for that exact binding;
- required Flow, Skill, and UI dependencies are prepared.

The capability snapshot exposes an opaque endpoint reference and readiness
digests. It never exposes a resolved URL, authentication header,
host-injected token, device credential, or live device state.

The MHS adapter deployment must not report healthy until its scope-bound MHS
session has been accepted by the control gateway. A3S Use deliberately treats
that handshake as provider health evidence rather than learning the private
credential or duplicating the gateway's authorization model.

The semantics profile may bind a policy identifier, grant class, or canonical
non-secret claims digest. It must never hash or embed the session token itself;
secret rotation must not require a package generation change.

The sample permission ceiling demonstrates the narrow boundary:

- one exact MCP surface;
- no native execution, child process, or filesystem access;
- one exact gateway host and port;
- one named, host-injected session secret;
- a private service and bounded compute resources.

The permission ceiling authorizes the adapter runtime to contact a gateway. It
does not authorize a physical operation. The downstream MHS session and device
safety layer remain authoritative for every physical action.

## Mutation semantics

Hardware writes differ from ordinary network requests because a timeout after
dispatch does not prove that no action occurred.

- Read-only observation may use bounded retry when the adapter declares it
  side-effect free.
- A physical mutation is attempted once unless the control gateway proves an
  idempotency contract for that exact operation.
- An ambiguous response is an unknown outcome, not a failed operation that may
  be replayed. The caller must observe and reconcile device state before
  issuing another mutation.
- Device identity, leases, parameter bounds, preconditions, interlocks, and
  approval requirements are checked at the control gateway and again as close
  to the actuator as the hardware permits.
- Emergency stop and fail-safe behavior must remain independent of A3S Use,
  the MCP adapter, the agent, and the UI.

The reference `monitor` Flow uses one attempt to make the no-implicit-retry
boundary executable in package evidence.

## Dynamic state

Discovery results, telemetry, poses, alarms, leases, and command receipts are
data-plane values retrieved through MCP. They do not belong in the immutable
A3S Use capability snapshot. Keeping dynamic state out of the control-plane
snapshot prevents stale telemetry from being mistaken for lifecycle or
authorization evidence.

## Test scope

The checked-in tests prove that:

- the package reuses only standard MCP, Flow, Skill, and UI surfaces;
- the complete dependency graph is canonical;
- all package-owned files are bounded and integrity inspected;
- the permission ceiling is canonical and contains no ambient authority;
- publication requires the exact Runtime/Gateway binding and prepared
  dependency graph;
- the host projection retains only opaque endpoint and non-secret evidence.

They do not claim MHS protocol conformance. Conformance tests belong with the
future adapter implementation and a published MHS specification.

## Conditions for revisiting the decision

Introduce a dedicated hardware surface only if a public MHS specification
creates lifecycle, permission, or attestation semantics that cannot be
represented without weakening the existing MCP boundary. Device vocabulary or
UI presentation alone is not sufficient reason to add another surface kind.
