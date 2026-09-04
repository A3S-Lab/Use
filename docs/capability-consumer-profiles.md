# Capability consumer profiles

Status: implemented as a contract and embedding boundary in the development
preview. This document does not claim that the A3 production exit gate is
complete.

## Purpose

The Capability Gateway has one universal agent-facing protocol: standard MCP.
Hosts may still need to distinguish an ordinary MCP client from an A3S product
host that can consume optional Flow, Knowledge, or UI metadata. The typed
consumer-profile contract records that choice without adding a second RPC
protocol or changing package-generation authority.

The contract is implemented by `a3s-use-core`:

| Type | Schema | Role |
| --- | --- | --- |
| `CapabilityConsumerProfile` | `a3s.use.capability-consumer-profile.v1` | A bounded client request |
| `CapabilityConsumerNegotiation` | `a3s.use.capability-consumer-negotiation.v1` | The host's explicit accepted result |

`CapabilityConsumerKind::GenericMcp` is the default and requests no optional
extension. `CapabilityConsumerKind::A3s` can request the typed
`Flow`, `Knowledge`, and `Ui` extension labels.

## Negotiation

An embedding host constructs and validates a profile, then negotiates it against
the extensions it actually owns:

```rust
use a3s_use_core::{
    CapabilityConsumerExtension, CapabilityConsumerNegotiation,
    CapabilityConsumerProfile,
};

let profile = CapabilityConsumerProfile::a3s([
    CapabilityConsumerExtension::Flow,
    CapabilityConsumerExtension::Ui,
])?;
let negotiation = CapabilityConsumerNegotiation::negotiate(
    profile,
    [
        CapabilityConsumerExtension::Flow,
        CapabilityConsumerExtension::Knowledge,
        CapabilityConsumerExtension::Ui,
    ],
)?;
```

The completed negotiation is bound to the Gateway so clones retain the same
consumer decision:

```rust,ignore
let gateway = CapabilityGatewayMcpServer::with_consumer_negotiation(
    catalog,
    provider,
    negotiation,
)?;
assert!(gateway
    .consumer_negotiation()
    .accepts(CapabilityConsumerExtension::Flow));
```

The existing Gateway constructors continue to select the generic MCP profile.
Use an explicit negotiation constructor when an A3S host has completed its
profile handshake; there is no implicit profile upgrade.

## Invariants

- Wire documents use strict, canonical JSON and are bound by a SHA-256 digest.
- Extension lists are sorted, unique, and bounded to eight entries.
- A generic MCP profile cannot request an A3S extension.
- A requested extension that the host does not support is rejected. The host
  cannot report success while silently dropping part of the request.
- Accepted extensions must equal the requested extensions. The negotiation is
  descriptive metadata, not an authorization grant.
- A descriptor may declare a bounded, sorted `requiredExtensions` set. The
  Gateway projects the immutable catalog for the completed negotiation before
  compiling any MCP route; a descriptor whose requirements are not accepted is
  absent from both discovery and direct invocation lookup.
- Package identity, publication generation, opaque invocation references,
  leases, principals, and policy remain owned by the existing Gateway and
  host authorities.

## Principal-scoped discovery

The HTTP edge authenticates a bearer credential and attaches the configured
`CapabilityGatewayPrincipal` to the trusted request context. Hosts that serve
multiple principals should additionally attach a
`CapabilityGatewayDiscoveryPolicy`:

```rust,ignore
use std::sync::Arc;

let gateway = CapabilityGatewayMcpServer::new(catalog, provider)?
    .with_discovery_policy(Arc::new(MyDiscoveryPolicy));
```

The policy receives each immutable descriptor and the host-authenticated
transport/principal context. `Ok(false)` removes that descriptor from
`tools/list`, `resources/list`, and `prompts/list`; direct Tool, Resource, and
Prompt requests receive the same unpublished-route response and never reach
the provider. Policy errors are converted to a generic internal MCP error and
never expose policy diagnostics. This is an information boundary, not an
authorization grant: the provider's pre-operation principal, scope, Grant,
and generation checks still run for every visible operation.

The Gateway evaluates a policy lazily and freezes the resulting descriptor
indices per trusted context for the server lifetime. The bounded cache holds
at most 64 contexts (matching the HTTP credential mapping), and `OnceCell`
coordination makes concurrent requests share one view. Numeric cursors are
therefore stable for a principal; changing policy requires constructing a new
Gateway from a refreshed immutable catalog/policy snapshot. Existing
constructors use an allow-all compatibility policy, so a production
multi-principal host must inject an explicit policy.

## Request cancellation

The standard MCP adapter also consumes rmcp's per-request
`RequestContext.ct`. When a client sends `notifications/cancelled`, the Gateway
stops waiting for the in-flight Tool, Resource, or Prompt provider operation
and returns the typed `use.plugin.capability_gateway_cancelled` boundary result
when a response is still deliverable. Dropping the provider future releases
the Gateway admission permit and any resolver/Invocation lease held by that
operation; the server-wide snapshot lease remains intact for other requests.

Cancellation is cooperative at the async boundary. Host providers should
make downstream I/O cancellation-safe and must not detach work that can keep
mutating state after the request has been cancelled. A cancellation token is
request-local and is never included in catalog metadata, MCP arguments, or
principal discovery cache keys.

## Current boundary and follow-up work

Profile negotiation intentionally stays separate from capability
authorization. The standard MCP adapter publishes catalog-authorized Tools,
Resources, and Prompts. `requiredExtensions` is a discovery boundary, not an
allow-list: every visible operation still passes through the host's principal,
Grant, policy, and generation-fenced provider. A generic MCP consumer receives
only descriptors with an empty requirement set; an A3S consumer receives those
plus descriptors whose complete requirement set it negotiated.

The current contract does not fabricate Flow, Knowledge, or UI metadata from a
label. Producers must first publish a schema-validated descriptor and mark any
non-universal interpretation with its required extension. Projecting the
actual Flow/Knowledge/UI payloads, composing the policy with production
receipt/Runtime/Grant authorization, CLI wiring, and the independent
Rust/TypeScript/Python recovery matrix remain A3 follow-up work. A live host
can use
`CapabilityGatewayMcpServer::from_verified_registry_snapshot_with_factory_and_options`
to bind verified descriptions, one snapshot cursor, a resolver factory, the
exact lease, consumer projection, and endpoint policy in one fail-closed
composition step.

Focused validation:

```bash
cargo test -p a3s-use-core --locked
cargo test --workspace --all-features --locked -- --test-threads=1
```
