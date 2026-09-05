# A3S Use First-Principles Agent Package Manager Audit

Status: development preview (2026-09-05)

This document audits A3S Use against the job it must perform as an Agent
Package Manager. It separates a mechanism that exists in the repository from a
production authority that is actually composed, and from a release gate that
has been exercised on supported hosts. Passing unit tests alone is not enough
to move a row into the last category.

## The product invariant

An Agent Package Manager is a trusted state machine, not a package downloader
with an MCP adapter. For every agent-visible capability it must be possible to
answer, after a restart and without host paths:

1. Which reviewed package and immutable generation owns this capability?
2. Which signed description and exact input/output contract were admitted?
3. Which principal, scope, Grant, and policy authorize this request?
4. Which isolated provider process or service will receive it?
5. Which lease keeps that generation alive for the whole call or stream?
6. Which durable record makes an interrupted transition converge without
   guessing or repeating an external side effect?

The identities must be bound together, while their authorities remain separate:
Registry trust chooses immutable bytes; the installation chooses a desired
generation; Control chooses a committed transition; Runtime owns provider
execution; the Gateway exposes only an opaque, authorized projection.

## Audit matrix

| Invariant | Repository evidence | Status | What still prevents release |
| --- | --- | --- | --- |
| Immutable package and dependency identity | TUF-backed Registry source, digest-pinned targets, global Artifact Store, graph lock and exact plan digests | Qualified for the current preview | Official Registry bootstrap, rotation, mirror and incident drills |
| One installation authority | Installation snapshot, graph/Grant journals, stale-generation rejection, User/Workspace scope fences | Qualified in A0/A1 tests | A2 Control Store must become the sole production reader/writer; legacy stores must be deleted from production paths |
| Atomic lifecycle | Reviewed plan/apply service, six-surface lifecycle, cutover and retirement journals, subprocess recovery matrices | Qualified in inactive and managed-host test paths | Production Code/managed-host composition and the remaining platform/reboot fault matrix |
| Provider isolation and resource ceilings | Runtime plans bind unit class, isolation, mounts, secrets, resources, provider build and semantics digest | Qualified for Runtime contracts | Production Runtime Service composition, actual host secret delivery, and provider admission under the live Control authority |
| Agent-facing contract | Standard MCP Tools/Resources/Prompts, bounded closed JSON schemas, opaque references, consumer negotiation and cancellation | Contract-complete | A3S Flow/UI/Knowledge extension payloads and independent client interoperability |
| Signed Tool description | `CapabilityDescriptionProof`, package signer allowlist, durable proof snapshots and exact descriptor digests | Inactive qualification only | `from_verified` is a host assertion; cryptographic signature verification, key custody, rotation/revocation and Registry-to-proof production wiring are absent |
| Runtime contract continuity | Tool release input/output schemas and domain-separated `RuntimeToolSchemaAttestation` now flow through plans, task/service receipts, provisioning and Control evidence; verified payload admission and strict projection compare digests | Implemented in the inactive kernel (PR #238) | Production Control/Runtime/receipt/Grant composition and real schema-bearing release fixtures |
| Live invocation authorization | Gateway resolver/factory seam, principal context, discovery policy, generation leases and provider `authorize` hook | Embedding mechanism qualified | A production resolver must bind the opaque reference to the exact scope, Grant, receipt and Runtime provider; no generic adapter can infer this safely |
| Generation-safe upgrade and drain | Immutable session factory, snapshot leases, list-change hub, explicit retention plans and drain leases | Mechanism qualified | Lifecycle must publish the new catalog, replace sessions, retain old leases, and retire payloads in one production transition |
| Crash/restart convergence | Durable journals, exact-key replay, no-generation-inflation tests across package, Grant, Runtime, Gateway and restore paths | Broad preview coverage | Code/Runtime product-host kill tests, reboot and remaining Windows contention/reparse races |
| Backup/restore authority | Whole-installation inventory, offline verification, reviewed restore plan, rollback archive and bounded recovery journal | Qualified for listed legacy/Use-owned families | Capability catalog/proof payload registration, Control database cutover, clean-machine recovery and operational drills |
| Cross-language/remote use | Standard Streamable HTTP, bearer/Origin/admission controls and an independent Rust contract test | Partial | TypeScript and Python clients, remote/container client with no shared filesystem, and install/upgrade/drain/restart/denied-scope matrix |
| Extensible package surfaces | Typed Flow, OKF/Knowledge, Skill and UI owners plus consumer profile negotiation | Partial | Negotiated Flow/UI/Knowledge metadata projection, distributed Flow identity and reviewed UI backend/rendering |
| Supply chain and operations | Reproducible five-target preview archives, Cosign/Sigstore checks, SBOMs, installers and bounded diagnostics | Preview-qualified | External witness, official Use-Registry, key/incident response, retention/repair runbooks and exercised support procedures |
| Reference package and release usability | MHS fixture and documentation/README contracts | Partial | A6 virtual-lab qualification and release-candidate examples against published artifacts |

## Critical path to a production Agent Gateway

The rows above are not independent checkboxes. The shortest safe order is:

### P0 — Make the authority real

1. Activate the A2 Control Store in one host composition.
2. Register the Runtime, Capability Index, Gateway catalog, Flow, Knowledge,
   Skill and UI effect owners behind one dispatcher and maintenance fence.
3. Make the live session factory consume the Control-bound cursor, publish a
   new immutable catalog before notification, retain old leases through drain,
   and retire payloads only after the exact receipt is terminal.
4. Remove production reads, writes, fallbacks and repair paths for the legacy
   JSON/SQLite authorities.

Until this is done, the excellent inactive-kernel proofs do not constitute a
product lifecycle: two authorities can still be composed by a host.

### P0 — Make “signed Tool” cryptographically meaningful

The current proof envelope is intentionally a host-owned hand-off. Its
`signerId` and descriptor digest are useful evidence, but `from_verified` does
not verify a signature and a signer allowlist is not a key store. The release
path needs one explicit trust boundary that:

- verifies a canonical signed description using Registry-controlled keys;
- binds key id, algorithm, signer, descriptor digest and expiry/revocation;
- persists the exact verified envelope for restart replay;
- rechecks the same policy during projection and restore; and
- exposes only schema-bearing Tools to generic agents.

The verifier must remain outside the universal Gateway protocol, but its
result must be a typed, non-forgeable input to the Control owner. A caller
supplied boolean or signer string is not sufficient evidence.

### P0 — Compose the real invocation path

An opaque `InvocationRef` deliberately omits package paths and secrets, so a
generic Gateway cannot reconstruct a User or Workspace scope. The production
host must supply a resolver that joins:

`principal → scope → committed Grant → package generation → Runtime receipt →
provider lease`

and rechecks the join at open time. The same handle must be used for authorize
and invoke; upgrade, disable and uninstall must drain it. This is the main
remaining implementation item behind the A3 exit gate.

### P1 — Prove interoperability and recovery

The endpoint is not complete until an independent Rust, TypeScript and Python
client can discover and invoke it remotely, with no package filesystem access,
while install, upgrade, prior-generation drain, uninstall, restart and denied
cross-scope access all converge. The matrix must run on Linux, macOS and
Windows, including reboot and persistent antivirus/rename contention where the
platform permits it.

### P1/P2 — Operate it as a product

The official Registry, external release witness, key rotation/revocation,
backup/restore of every selected payload owner, incident response, retention,
repair, support runbooks and MHS qualification are release gates. They cannot
be inferred from green Rust tests or a development-preview archive.

## Definition of done

The A3S Use release gate can close only when all of these statements are true:

- one production Control Store is the sole lifecycle authority;
- every published Tool has a cryptographically verified signed description,
  both bounded JSON schemas, and a Runtime attestation matching the exact
  release descriptor;
- a principal-scoped resolver authorizes and invokes through the same
  generation-fenced handle;
- the catalog/session/lease transition is durable and drain-safe;
- independent remote clients pass the complete lifecycle and recovery matrix;
- all selected payload owners participate in backup/restore and retention; and
- the official Registry, release witness and operational response procedures
  are independently exercised.

## Schedule estimate (engineering, not a promise)

At the current single-line development pace, the remaining work is several
independent iterations rather than one final feature:

| Workstream | Expected focused engineering time | External/CI dependency |
| --- | ---: | --- |
| Control activation, live Runtime/Gateway/Grant composition and legacy-path deletion | 5–10 working days | A3S Code/host integration and migration review |
| Cryptographic description verification and key policy | 3–6 working days | Registry key format, rotation and security review |
| Extension projection plus real client/recovery matrix | 5–10 working days | TypeScript/Python clients and all supported platforms |
| Backup/restore payload registration, official Registry and runbooks | 5–10 working days | Operations ownership and external witness |
| Final MHS/release-candidate qualification | 3–7 working days | CI capacity, virtual lab and release sign-off |

The first three rows describe the A3 Agent Gateway/capability-plane critical
path. They overlap only when the corresponding owners work in parallel; on a
single implementation stream, that slice is roughly **3–6 focused weeks**.
That is not an estimate for the complete A3S-USE product. Backup/restore and
the official Registry, A2 production activation, A4 host/provider adoption,
MHS qualification, cross-platform recovery, and security/operations sign-off
remain separate product gates. For one focused implementation stream, a
production-ready release is therefore roughly **9–18 focused weeks** in total
(the A3 slice plus those productization gates), before external waiting time.
With multiple staffed owners, the calendar can compress to about **6–12
weeks**, but only if Registry, host, platform, and security dependencies are
available and accepted in parallel. No fixed date is responsible until those
external gates are staffed. The current PR #238 reduces one P0 mechanism but
does not close those gates.
