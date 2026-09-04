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
- Package identity, publication generation, opaque invocation references,
  leases, principals, and policy remain owned by the existing Gateway and
  host authorities.

## Current boundary and follow-up work

This increment intentionally keeps profile negotiation separate from
capability authorization. The current standard MCP adapter still publishes
only the catalog's schema-validated Tool surface. Flow, Knowledge, UI,
resources, and prompts are not fabricated from profile labels. A live host can
use `CapabilityGatewayMcpServer::from_verified_registry_snapshot_with_factory_and_options`
to bind verified descriptions, one snapshot cursor, a resolver factory, the
exact lease, and endpoint policy in one fail-closed composition step. The
factory still owns receipt/Runtime/Grant resolution and per-consumer
authorization; CLI wiring and the independent Rust/TypeScript/Python recovery
matrix remain A3 follow-up work.

Focused validation:

```bash
cargo test -p a3s-use-core --locked
cargo test --workspace --all-features --locked -- --test-threads=1
```
