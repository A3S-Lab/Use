# A3S Use Architecture

Status: development preview; not production-ready
Last updated: 2026-08-29

## Product boundary

A3S Use is the package, trust, resolution, and lifecycle control plane for A3S
native capabilities and cognitive packages. It targets Linux, macOS, and
Windows, but it does not replace an operating-system package manager.

Use owns:

- package identity, SemVer resolution, exact locks, and dependency order;
- replaceable Registry input, a host-selected network boundary, and end-to-end
  TUF provenance;
- globally deduplicated immutable artifacts and explicitly scoped package
  generations, receipts, plans, and operation journals;
- reviewed authorization and Workspace Grant transitions; and
- one atomic capability-Registry cutover per package graph mutation.

Runtime, Gateway, Flow, Knowledge, Skill, Code, and operating-system hosts
own execution and presentation. Package content describes requirements but
cannot select a provider or authorize itself.

## First-principles review

An AI-native package manager has one essential job: turn reviewed intent into
one authorized, observable capability generation. Everything else follows from
that constraint:

1. Each mutable fact must have one authority and one commit order.
2. Immutable content identity must not depend on an installation, version,
   route, source, or local path.
3. Installation identity and capability identity must be explicit stable keys;
   human-facing aliases are not ownership.
4. External agents receive portable protocols and opaque references, while the
   host retains local paths, credentials, providers, and generation leases.
5. Provider and device effects cannot join a local transaction, so durable
   checkpoints must distinguish rejected, applied, and unknown outcomes.
6. Steady-state discovery and watch work must scale with changed generations,
   not with the total installed filesystem.

Against those invariants, the current architecture is directionally sound but
not yet internally complete. It should be evolved in place through ROADMAP A1
to A4, not replaced wholesale and not split into more repositories before the
ownership boundaries stabilize.

| Area | Current evidence | Verdict |
| --- | --- | --- |
| Package aggregate | One manifest generation owns Tool, MCP, OKF, Flow, Skill, and UI surfaces; dependency preparation and retirement are ordered around one cutover. | Correct foundation. A surface must not become an independently installed mini-package. |
| Trust and planning | TUF provenance, exact SemVer locks, read-only planning, explicit confirmation, generation compare-and-swap, and crash replay are enforced. | Correct foundation. Keep source transport, trust evidence, and package identity separate. |
| Immutable bytes | Expanded packages now use the global content-addressed Artifact Store and are shared across installations. Verified archive, planning, and media targets are still retained inside each Registry-source datastore, and corrupted shared content has no automated quarantine/rehydration path. | Partially correct. Move raw blobs behind one global digest tier only with a global reachability, quota, audit, repair, and garbage-collection model; source observations remain source-scoped metadata. |
| Installation authority | `InstallationSnapshot` owns desired roots and the unified resolved graph, but receipts, routes, enablement, Grants, provider bindings, operations, and publication metadata still live in separate mutable stores. | Critical debt. A2 must make these facts transactional in one Control Store; filesystem sagas remain only for external provider effects. |
| Agent contract | The current serializable `CapabilityBinding` contains `packageRoot`, executable/release paths, Skill paths, and asset paths. | Critical portability debt. A3 must expose opaque `InvocationRef`, `ArtifactRef`, and `EndpointRef` contracts through the Capability MCP Gateway. |
| Identity | Package and generation keys exist, but route strings still select, lock, conflict-check, and dispatch capabilities. | High debt. Route becomes an optional alias indexed to a canonical package/surface/generation identity. |
| Observation cost | Registry watch polls at a fixed interval, and normal snapshot projection can reopen and rehash package assets. | High scalability debt. Materialize one immutable Capability Index at cutover, publish generation notifications, and reserve full hashing for admission, audit, or detected drift. |
| Built-ins and providers | Browser, OCR, Box, Runtime, Flow, and UI are still named directly by the facade and capability projection. | High coupling. A4 must classify true bootstrap providers explicitly and move ordinary domains to injected providers or first-party packages. |
| Code structure and language | `Plugin`, `Extension`, and `CognitivePackage` overlap; several production modules exceed 1,000 lines because persistence, orchestration, projection, and protocol concerns still meet in one file. | Medium structural debt. Rename once at a coordinated contract cutover and split files when A2/A4 move ownership; do not add forwarding facades or parallel registries. |
| MHS | The fixture composes standard MCP, Flow, Skill, and UI surfaces and depends on an exact managed gateway binding. | Correct boundary. MHS is a signed reference package and safety adapter, not a new package surface; the virtual laboratory remains a separate repository. |

The release-critical sequence is therefore A0 mutation correctness, A1 scoped
installation and global artifacts, A2 transactional mutable authority, and A3
portable agent consumption. A4 provider cleanup can then follow those stable
ports. Naming cleanup, crate splitting, extra package types, and MHS-specific
features must not bypass that order.

## Repository layers

```text
CLI / managed-host adapters
            │
            ▼
CognitivePackageManager       plan · confirm · apply · recover
            │
            ▼
PackageGraphLifecycle         prepare forward · cut over once · retire reverse
            │
    ┌───────┼────────┬────────┬────────┬────────┐
    ▼       ▼        ▼        ▼        ▼        ▼
 Runtime   MCP    Knowledge  Flow    Skill      UI
 hosts   hosts      host     host    host       host
            │
            ▼
Global Artifact Store · scoped control state · immutable capability index
```

`InstallationId(kind, id)` is the authority boundary. Every mutable selection,
receipt, route, enablement record, Grant, provider binding, and capability
projection belongs to that exact User or Workspace installation. Host
projections and deployed units are receipt-owned derived state; packages do not
scatter authoritative files across host directories. The current preview still
materializes mutable authority across multiple scoped stores.
`InstallationSnapshot` is already the sole installed-selection graph; ROADMAP
A2 tracks transactional consolidation of the remaining control facts.

Expanded package bytes are not installation authority. They live at
`<data-root>/artifacts/expanded-packages/sha256/<prefix>/<digest>/content` and
are serialized by one digest mutation lock across installations. User and
Workspace receipts may reference the same directory while retaining independent
lifecycle generations and publication state. Uninstall retires only scoped
authority and never deletes shared bytes. Installation backup excludes the
global store. Until a future global garbage collector can prove reachability
across every installation and durable operation, unreferenced expanded content
is retained deliberately.

This is not yet the complete Artifact Store. Verified archives, executable
planning targets, and presentation media remain content-addressed within their
Registry-source datastores, so identical bytes from different sources can still
be duplicated and governed by conflicting per-source cache policies. Their
future blob bytes belong in the global store; signed TUF/source observations,
freshness, and provenance do not.

Every Runtime, Flow, OKF binding/SQLite, and lifecycle journal store captures
one `InstallationId` at construction. Scope fields retained in receipts and
paths are integrity evidence, not a second caller-selected authority. The
store compares them with its captured installation before deriving a path,
acquiring a lock, opening SQLite, or mutating evidence. Independent User and
Workspace installations therefore require independent stores even when their
textual IDs match and their immutable inputs are shared.

The managed-host entry point is `CognitivePackageHostManager`, an adapter over
`CognitivePackageManager`, not another manager. Its protocol store contains
only request-to-plan, operation-to-request, pre-admission cancellation,
terminal-result, and observation-index bindings. None is admission or recovery
authority. All admission, recovery, package state, and capability publication
evidence stays in the existing Use-owned stores shown below it. Reviewed
install and upgrade apply consumes the exact verified cache populated by
planning and never depends on a second Registry request.

Registry/TUF resolution first creates a bounded pre-lock attempt under the
package-level planning lock. It records refreshed or cached access and
path-free per-Registry verification state before metadata access begins. A
failed or externally interrupted resolver therefore remains diagnosable; a
successful resolver writes its download-attempt successor before deleting the
pre-lock record.

After exact lock resolution, a process-held pre-plan download-attempt record
retains the exact lock-selected archive and separately signed executable-
planning-target observation sets until a reviewed pending graph is durable. It
survives process exit for path-free byte diagnosis but is never planning, apply,
or recovery authority.

Host protocol v6 binds an explicit User or Workspace scope kind and projects
package state separately from exact operation state. Operation observation,
revision-bound watch, and explicit-user pre-admission cancellation are derived
from those same Host bindings and Use-owned graph, enablement, and lifecycle
stores; the adapter does not infer progress from time or maintain another
operation journal. Equal textual IDs in different scope kinds do not alias a
Host fence or durable request replay directory.

When no graph or active Use enablement exists, the standalone operation
diagnostic may follow a digest-bound index to the newest Host-reviewed
enable/disable plan for the same public PlanScope/package. The index orders
plans by `(plannedAtMs, requestId)`, retains the exact managed scope only for
private request lookup, and exposes neither Host/fence/request identity nor a
new authority path. It projects `planned` or exact `cancelled` evidence and is
suppressed by active or completed Use state and durable Host outcomes.

## One current contract line

The cognitive-package product has not shipped a supported release. The current
code accepts one preview baseline only:

| Contract | Current baseline |
| --- | --- |
| Manifest | ACL schema 3 |
| Catalog | `a3s.use.plugin-catalog.v3` |
| Receipt | numeric schema 5 |
| Extension Registry snapshot | numeric schema 2 |
| Capability snapshot | numeric schema 3 |
| Extension/capability cursor | `a3s.use.extension-snapshot-cursor.v2` / `a3s.use.capability-snapshot-cursor.v2` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v6`, protocol 6 |
| Host managed scope | `a3s.use.plugin-managed-scope.v2` |
| Manager tools | `a3s.use.plugin-manager-tools.v4` |
| Pending graph | `a3s.use.pending-package-graph-operation.v4` |
| Pre-lock resolution attempt/diagnostic | `a3s.use.plugin-resolution-attempt.v1` / `a3s.use.plugin-resolution-attempt-diagnostic.v1` |
| Pre-plan download attempt/diagnostic | `a3s.use.plugin-download-attempt.v1` / `a3s.use.plugin-download-attempt-diagnostic.v1` |
| Operation diagnostic/history | `a3s.use.plugin-operation-diagnostic.v1` / `a3s.use.plugin-operation-history-diagnostic.v1` |
| Enablement state/operation | v2 |
| Runtime Task binding | `a3s.use.runtime-task-binding.v4` |
| Runtime Service provisioning | `a3s.use.runtime-service-provisioning.v1` |
| Runtime Service binding | `a3s.use.runtime-service-binding.v3` |

Superseded preview state fails closed with cleanup and reinstall guidance.
SemVer, `requires_use`, operating-system/target selection, and provider
capability checks remain mandatory package-manager correctness rules.

## Package and surface model

Manifest schema 3 describes one npm-like package generation with optional
package dependencies and named Tool, MCP, OKF, Flow, Skill, and UI surfaces.
The surface graph and package graph must both be acyclic.

| Surface | Readiness evidence |
| --- | --- |
| Tool Task/Service | Exact executable or release descriptor plus Runtime observation |
| MCP | Exact stdio binding or Runtime/Gateway health and standard initialization |
| OKF | Exact OKF v0.2 bundle promoted by the Knowledge host |
| A3S Flow | Content digest, `a3s-flow` preflight, and exact compiled binding |
| Skill | Content digest plus ready dependency closure |
| UI | Asset integrity plus authorized backend bindings and sandbox ownership |

A surface is selectable for projection but is never installed, upgraded, or
removed independently of its package generation.

## Resolution and provenance

Registries are named, replaceable host configuration shared across
installations. The standalone host persists a canonical, revision-addressed ACL
set and isolates TUF/cache state by the exact name/URL/bootstrap-root identity.
Packages cannot select their source. The resolver uses only enabled sources,
applies SemVer,
`requires_use`, host target, and provider requirements, then freezes the
selected catalog-v3 records in one exact package lock.

The Extension crate also publishes the closed JSON Schema fragments for the
canonical catalog host, bounded search, and inspection-selector inputs. REST,
MCP, and other presentation adapters compose those fragments instead of
copying A3S Use's field, enum, cursor, or page-limit contract.

Every remote receipt retains the verified catalog record and its complete TUF
provenance. Replacing a Registry never rewrites historical provenance. Missing
or partial catalog evidence is invalid and cannot be reconstructed from an
archive or local package files.

## Reviewed graph lifecycle

All graph mutations follow one durable sequence:

```text
search/inspect verified metadata
→ resolve and freeze the package lock
→ build an immutable plan-v4 envelope
→ review operation ID, digest, impact, and authorization
→ revalidate sources and state
→ persist candidate Grants
→ prepare packages and surfaces dependency-forward
→ publish one exact capability snapshot
→ checkpoint Grant cutover
→ drain prior-generation leases
→ revoke prior Grants and retire packages in reverse
→ persist the terminal result
```

Upgrade carries both the prior and candidate lock. The candidate lock cannot
authorize retirement of the prior installed selection. A prior generation may
be marked hidden only after its exact route is absent from the published
Registry.

Every external mutation uses stable operation, package, surface, and
generation idempotency evidence. Before publication, recovery may roll back
the complete candidate. After publication, recovery finishes retirement; it
does not restore a mixed or guessed graph.

## Capability observation

Consumers receive immutable snapshots and monotonic observation evidence. A
snapshot changes only through the graph cutover path. New calls resolve the
current generation; calls admitted before cutover keep exact-generation leases
until drain completes.

Embedding hosts use `CapabilityRegistry` with the same injected
`ExtensionRegistry` that owns planning and cutover. A typed
`a3s.use.capability-snapshot-cursor.v2` binds the exact installation, complete
capability revision, authoritative Registry revision, and canonically sorted
immutable package identities. Acquiring the cursor obtains every shared package route
lease and then re-reads the publication. If any identity is hidden, stale,
mixed, contended, digest-mismatched, or lacks lifecycle evidence, the whole
attempt fails and Rust RAII releases any earlier locks. No partial lease can
escape.

The resulting non-clone `CapabilitySnapshotLease` owns an `Arc` of the exact
projection plus the complete upstream lease set. A3S Code retains that value
for an admitted Run; a later hot-plug affects only a later admission. `Drop`
performs synchronous lock release only. Use lifecycle coordinators continue to
own bounded asynchronous drain and retirement.

The current Registry projection is host-internal in practice and still exposes
local package, executable, Skill, and asset paths in serializable bindings. It
must not be treated as the arbitrary-agent contract. The A3 Capability MCP
Gateway will expose identities, schemas, content evidence, and opaque host
references while retaining exact-generation leases server-side. Tool contracts
remain native CLI/HTTP behind that host boundary, MCP remains standard MCP, and
Flow execution remains owned by `a3s-flow`.

## Built-in capabilities

Browser, OCR, Search, and component-backed routes remain typed A3S Use
capabilities. Their installers and providers follow the same principles:
bounded acquisition, immutable provenance, staged activation, owned receipts,
and no unreviewed executable discovery. They do not create an alternate
cognitive-package lifecycle.

## Failure policy

Use fails before mutation when identity, provenance, provider, authorization,
scope, capability generation, or state evidence drifts. Unknown record
versions, deleted recovery evidence, path ownership violations, and partial
publication are corruption, not migration opportunities.

The development and production gates are tracked in [ROADMAP.md](../ROADMAP.md).
Detailed contracts are in [Plugin Contracts](plugin-contracts.md), and the
multi-host security model is in [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md).
