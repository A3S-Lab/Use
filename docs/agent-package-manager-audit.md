# A3S Use First-Principles Agent Package Manager Audit

Status: development preview (2026-09-06)

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
| Signed Tool description | `CapabilityDescriptionProof`, package signer allowlist, durable signed v2 snapshots, exact descriptor and envelope digests, canonical Ed25519 envelopes, bounded public-key trust store with expiry/revocation, signed Gateway composition constructors, and replay-time re-verification | Inactive qualification plus verifier/composition mechanism | Registry/TUF key-source binding and production Registry-to-proof lifecycle wiring remain open; `from_verified` and proof-only v1 snapshots are still explicit compatibility host assertions |
| Runtime contract continuity | Tool release input/output schemas and domain-separated `RuntimeToolSchemaAttestation` now flow through plans, task/service receipts, provisioning and Control evidence; verified payload admission and strict projection compare digests | Implemented in the inactive kernel (PR #238) | Production Control/Runtime/receipt/Grant composition and real schema-bearing release fixtures |
| Live invocation authorization | Gateway resolver/factory seam, principal context, discovery policy, generation leases and provider `authorize` hook; inactive Control resolver now reopens the durable cursor, validates the exact descriptor, and retains an external Control lease through the operation | Embedding mechanism qualified | A production host factory must still join the principal, scope, Grant, receipt and Runtime provider; the inactive Control composition is not yet the production authority |
| Generation-safe upgrade and drain | Immutable session factory, snapshot leases, list-change hub, explicit retention plans, paired catalog/descriptor retention coordinator, durable Control cursor reopening, an internal lease guard that follows cloned Gateway servers, a bounded session-factory drain state machine that closes admission and releases the source lease, a replay-safe graph cutover activation hook wired to a Control lease-backed Gateway adapter, and a composition retention boundary that derives the durable current payload set and applies under an exclusive fence | Mechanism qualified | Production lifecycle must attach the adapter, invoke drain at endpoint shutdown, add any non-Control rollback/session identities, and retire payloads in one host transition |
| Crash/restart convergence | Durable journals, exact-key replay, no-generation-inflation tests across package, Grant, Runtime, Gateway and restore paths | Broad preview coverage | Code/Runtime product-host kill tests, reboot and remaining Windows contention/reparse races |
| Backup/restore authority | Whole-installation inventory, offline verification, reviewed restore plan, rollback archive and bounded recovery journal; canonical Capability Gateway catalog and descriptor-snapshot records are now admitted as the `CapabilityPayloads` family with owner-byte/content-address validation; artifact reachability now traverses the same payload-owner tree and fails closed on nested drift or in-flight publication evidence; both immutable owners now have plan-bound clean-target candidate/activation/replay adapters, with signed descriptor replay requiring current trust verification; dedicated restore and retention coordinators bind both owner plans under one exclusive fence with preflight, fixed-order replay, and a durable cross-owner phase journal that blocks backup/reachability until recovery; the inactive composition can derive the durable published Control cursor and reopen its exact Index, catalog, and package-generation lease set after restart | Qualified for listed legacy/Use-owned families, the Capability payload coordinators, and the cursor-reopen mechanism | Production Control owner registration, live Gateway session reconstruction from the reopened lease, lifecycle retention/lease activation, clean-machine recovery and operational drills |
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
   explicitly close admission and await the bounded drain, and retire payloads
   only after the exact receipt is terminal.
4. Remove production reads, writes, fallbacks and repair paths for the legacy
   JSON/SQLite authorities.

Until this is done, the excellent inactive-kernel proofs do not constitute a
product lifecycle: two authorities can still be composed by a host.

### P0 — Make “signed Tool” cryptographically meaningful

The current proof envelope is intentionally a host-owned hand-off. Its
`signerId` and descriptor digest are useful evidence, but `from_verified` does
not verify a signature and a signer allowlist is not a key store. The release
path now has a qualified canonical Ed25519 envelope and public-key verifier;
the remaining production trust boundary must:

- source the trust store from Registry-controlled/TUF-authenticated keys;
- bind key id, algorithm, signer, descriptor digest and expiry/revocation;
- persist the exact verified envelope for restart replay;
- recheck the same policy during projection and restore; and
- expose only schema-bearing Tools to generic agents.

The verifier must remain outside the universal Gateway protocol, but its
result must be a typed, non-forgeable input to the Control owner. The inactive
Control snapshot owner now admits canonical signed v2 envelopes, retains the
envelopes as replay authority, and rejects the proof-only projector for those
records. The coordinated backup inventory also validates and archives the
canonical snapshot record, but intentionally leaves current trust-policy
reverification to restore/replay. A caller-supplied boolean or signer string
is not sufficient evidence. The snapshot owner now has plan-bound retention,
per-unlink journal checkpoints, and exact restart recovery. The Capability
payload restore coordinator binds the catalog and descriptor plans under one
exclusive fence, preflights both clean targets, and retries fixed-order
activation without clobbering an already-published owner. Its retention sibling
preflights both inventories and exact pending journals before fixed-order
deletion. Production Registry/TUF key-source binding, owner registration,
live Gateway session reconstruction from the restart-reopened Control lease,
and lifecycle wiring are still required. Destructive owner retention now also
has an exclusive-fence path: it waits for live Control snapshot leases, derives
the currently published catalog and matching descriptor snapshot, and rejects
a reviewed plan that would prune the durable cursor. Hosts still have to add
any independently managed rollback or legacy endpoint identities explicitly.

Implementation note (2026-09-06): the package-graph coordinator now exposes a
replay-safe `PluginGraphCapabilityCutoverActivation` hook. When attached, it
runs after a durable publish (including a replay) and before any prior-
generation drain or retirement. The inactive Control composition provides a
lease-bound adapter that reopens the durable cursor, rejects an unleased or
newer in-memory endpoint, and avoids a redundant replacement when the current
catalog already matches. This closes the ordering seam; production hosts still
must attach it to their live lifecycle. The composition's retention helper
derives the durable current protection set and its apply path rechecks that
set under an exclusive maintenance fence; independently managed rollback or
legacy endpoint payloads remain explicit caller inputs.

Implementation note (2026-09-06): the inactive Control composition now also
provides a Control-backed opaque invocation resolver. It reopens the durable
published cursor for each operation, compares the complete descriptor against
the immutable catalog before opening host state, and wraps the host handle in
the same external generation lease used by the Gateway session. A forged or
cross-generation descriptor therefore fails before provider I/O. The injected
factory remains responsible for the private principal/Grant/Runtime join, and
the production lifecycle still has to compose that factory and remove legacy
authority paths.

Implementation note (2026-09-06): `CapabilityGatewaySessionFactory::drain`
now provides the missing endpoint-retirement boundary. It serializes with
catalog replacement, transitions the shared live adapter to a non-admitting
draining state, waits for every already-admitted operation under a caller
deadline, and detaches the factory's source generation lease only after the
operation count reaches zero. A timed-out drain remains closed for new work and
can be resumed; independent immutable server clones retain their own leases
until dropped. This makes the subsequent exclusive payload-retention fence
observable rather than dependent on dropping an implementation detail.

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
| Production owner-native payload restore/retention, official Registry and runbooks | 5–10 working days | Operations ownership and external witness |
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
