# A3S Use Architecture

Status: development preview; not production-ready
Last updated: 2026-08-31

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
| Immutable bytes | Expanded packages and verified raw archive, planning, and media targets use global content-addressed Artifact Store tiers shared across sources and installations. One collection boundary covers bounded physical/reference inventory, checked usage, optional hard quota, full digest audit, logical quarantine, verified rehydration, and explicit confirmed GC without merging their authorities. GC requires a bounded exact target allowlist, a fresh zero-reference proof, a canonical physical/lifecycle plan, durable prepared/completed evidence, and same-shard atomic retirement before bounded tombstone deletion. Source observations and resumable partials remain source-scoped. | Correct A1 storage boundary. Keep source cleanup and scoped lifecycle retirement separate from global deletion, and never turn quota, audit, quarantine, rehydration, or unreachability into implicit GC authority. |
| Installation authority | `InstallationSnapshot` owns desired roots, the unified resolved graph, per-package enablement, and selected-surface publication intent. Receipts, Registry package bindings, recovery projections, Grants, provider bindings, operations, and materialized publication metadata still live in separate stores. | Critical debt. [ADR-003](adr-003-control-store-transaction-boundary.md) requires one coordinated Control Store cutover with no JSON/SQLite dual authority; sagas remain only for external provider effects. |
| Agent contract | The current serializable `CapabilityBinding` contains `packageRoot`, executable/release paths, Skill paths, and asset paths. | Critical portability debt. A3 must expose opaque `InvocationRef`, `ArtifactRef`, and `EndpointRef` contracts through the Capability MCP Gateway. |
| Identity | Registry ownership, accepted-call leases, cursors, and Tool/MCP host names use scoped package/generation/surface keys. The optional manifest `route` is retained only as a human alias; duplicates are legal and explicit alias lookup rejects ambiguity. | Qualified A1 boundary. Aliases may improve presentation but must never enter ownership or cursor identity. |
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

`InstallationId(kind, id)` is the authority boundary. Every desired package
selection lives in its `InstallationSnapshot`; each receipt, Registry package binding, enablement
recovery projection, Grant, provider binding, and capability projection belongs
to that exact User or Workspace installation. Host
projections and deployed units are receipt-owned derived state; packages do not
scatter authoritative files across host directories. The current preview still
materializes mutable authority across multiple scoped stores.
`InstallationSnapshot` is already the sole desired graph and activation
authority. The current receipt-backed materialization still carries immutable
package-lifecycle incarnation; ROADMAP A2 moves that identity and the remaining
control facts and applied observations into one transaction. ADR-003 fixes the
transaction, outbox, backup, and migration boundaries: the SQLite/WAL backend
may be qualified before activation, but every authoritative reader switches as
one cutover and live WAL files are never copied as backup payloads.
The current private Control Store kernel qualifies an installation-bound
schema-v9 aggregate, bounded blocking executor, typed transitions and outbox,
and canonical offline-verifiable export plus staged restore on otherwise clean
state. It persists each complete canonical reviewed Plan envelope and versioned
authorization record, while relational columns remain validated projections of
that evidence. It distinguishes installation generation, package desired-state
generation, immutable package-lifecycle generation, and Grant receipt revision.
Authorization evidence retains the exact prior Grant snapshot, reviewed change
set, and confirmations; complete target Grants are re-finalized rather than
accepted from callers. The next snapshot, both package generation axes, and
Grant inventory are a pure function of that evidence, the exact prior
generation, and bounded committed history; database commit, offline export, and
restore reject caller-selected divergence, including shared-root or reinstall
identity reuse. Reviewed Runtime provider selections for every enabled Tool
and MCP surface are projected from the canonical Plan and exact prior
generation, while unrelated selections are retained. A candidate capability
digest binds the exact target snapshot, lifecycle identities, Grant revisions,
and provider selections without claiming an endpoint or readiness before an
external effect succeeds. The same pure projection derives only the effects
that cannot join the transaction: typed surface prepare, capability cutover,
accepted-call drain, and surface stop or removal. It prepares dependencies
before dependants, retires in reverse order, and binds every Tool or MCP effect
to its exact reviewed Runtime selection. Package state, Grants, lifecycle
identity, and provider selection remain transaction facts rather than pseudo
effects. Canonical payload bytes, domain-separated idempotency key, digest, and
relational projection commit together. Applied outcomes retain a canonical
owner-specific descriptor that binds the exact intent to portable Runtime
Task/opaque `gateway:` Service readiness, Flow artifact, Knowledge projection,
Skill/UI content, Capability Index, or invocation-lease receipt evidence;
rejected and unknown outcomes cannot carry applied state. The applied cutover
observation atomically retires the prior publication and advances the
capability cursor before drain and teardown. A post-cutover required failure
stays pending for explicit same-key reconciliation rather than rolling back a
visible generation, and completion cannot predate its observations.

Production lifecycle code does not construct it, so it neither mirrors nor
replaces the current JSON authority. Lifecycle
conversion still must feed the reviewed Grant evidence, dispatch effects
through real typed external owners, and populate the qualified observation
contract.
The inactive kernel now has a path-free external-payload registry/evidence
contract: five fixed owner identities and ACL backup policies, explicit global
Artifact Store exclusion, and one exact canonical receipt set for the four
snapshotted owners. Receipts bind installation, Control generation, registry
and owner schemas, manifest/inventory digests, and bounded accounting; they do
not embed host paths. A private session now binds one canonical Control export
digest under the exclusive maintenance fence and retains that fence, but no
database transaction or executor permit, across owner I/O. The Knowledge owner
has the first real adapter: it snapshots and offline-verifies a bounded
scope-local OKF SQLite/FTS5 archive with canonical retained-binding/selection
inventory evidence, or emits a zero-file absent manifest without mutating live
state. The other three owner adapters, Knowledge staged restore/activation and
Control-effect reconciliation, complete-set orchestration, and the coordinated
reader/writer cutover remain A2 work. The machine-checked
[cutover contract](control-store-cutover.md) now freezes the current authority,
external-owner, operational-state, and consumer inventory against the actual
state layout. It is an implementation prerequisite, not an activated migration
or a completed A2 gate.

Snapshot v2 gives every selected package one monotonic state generation,
desired enablement bit, and exact selected-surface closure. Enable/disable
commits that package state through compare-and-swap while advancing the global
installation generation. The receipt, Registry package binding, and
`cognitive-package-enablement-projection.v3` record are materialization and
recovery evidence. They are validated against the snapshot and cannot select
desired state independently.

Expanded package bytes are not installation authority. They live at
`<data-root>/artifacts/expanded-packages/sha256/<prefix>/<digest>/content` and
are serialized by one digest mutation lock across installations. User and
Workspace receipts may reference the same directory while retaining independent
lifecycle generations and publication state. Uninstall retires only scoped
authority and never deletes shared bytes. Installation backup excludes the
global store. The joined reachability inventory now captures which installations
and nonterminal operations retain each digest together with its physical
measurements and checked usage. Unreferenced expanded content is still retained
until audit and confirmed deletion are implemented.

Verified archives, executable planning targets, and presentation media now use
the global sharded Blob tier. Registry-source datastores retain only canonical
source observations, TUF freshness/provenance, and resumable partials. This is
still not a complete Artifact Store. One cross-process shared/exclusive
boundary now serializes durable raw-target observations, lifecycle receipts,
applying lifecycle journals, installation snapshots, and pending graph
operations against future maintenance. Admissions bind to the exact Artifact
Store and precede subordinate source or installation locks; incomplete
downloads do not hold the boundary while waiting on the network. Under its
store-bound exclusive guard, the v1 physical inventory now enumerates both
tiers deterministically, separates canonical content from abandoned staging,
accounts files and bytes, and rejects unowned or unbounded layout. The separate
Registry-reference v1 inventory derives canonical blob observations from every
preserved source datastore, including replaced sources, and fails closed on
unknown or incomplete source state. Global reference v1 joins those facts with
every installed selection, current and retained receipt, non-cancelled package
graph, and applying or rolling-back lifecycle journal. It validates source and
installation identity, rejects conflicting physical expectations, and retains
missing-content references. Joined reachability v1 captures those reference and
physical facts under the same exclusive guard, reports metadata expectation
status, and derives checked storage usage plus bounded quota assessment. It does
not rehash content or grant deletion authority. The Artifact Store separately
owns optional canonical `storage-quota.acl` policy state. Policy mutation uses
revision compare-and-swap. All publishers take reference admission before the
storage boundary and per-digest lock. With no policy, the storage lock is
shared; with a policy, one exclusive lock covers physical scan, exact
logical-byte/container projection, staging reclamation, and final publication.
This serialized protocol prevents concurrent overcommit without a parallel
reservation ledger. It also permits only non-worsening replay or cleanup when a
tightened policy is already exceeded. No collector may delete shared raw or
expanded bytes on this evidence alone. The separate v1 digest audit holds the
same exclusive guard, sequentially rehashes complete Blobs and expanded
packages with their canonical identity algorithms, reports path-free
verified/mismatch/incomplete evidence, and repeats physical inventory to reject
observable drift. It has no mutation authority. No collector may delete or
replace shared bytes on audit evidence alone. Exact-plan logical quarantine now
re-audits one complete mismatch under that guard, requires the reviewed
canonical plan digest, and atomically publishes a bounded no-clobber marker.
The marker is validated by physical inventory, excluded from content/staging
quota measurements, and blocks new ordinary Blob and expanded-package access
without moving forensic content. It does not revoke existing handles or grant
replacement or deletion authority. `ArtifactStoreMaintenance` now implements
verified rehydration only after a fresh global zero-reference proof under that
same guard. It binds an external candidate and the quarantine record into an
exact path-free plan, reverifies both at apply, persists prepared/completed
crash evidence, accounts for quota peak, and keeps access closed until the
canonical switch completes. It does not revoke arbitrary already-open handles;
the zero-reference proof prevents replacement under an admitted generation.
Once completion is durable, exact replay is read-only and verifies canonical
content without reopening the external candidate or reacquiring deletion
authority from later owners.
Confirmed GC is another `ArtifactStoreMaintenance` operation, not a side effect
of audit, quota, uninstall, or source pruning. Its policy explicitly names at
most 1,024 exact Blob or expanded-package digests. Plan and nonterminal apply
hold the same collection guard across the complete logical-owner scan and
physical work. The path-free plan binds physical measurements, stable
ordinary/quarantined/rehydrated state, zero required references, and the prior
completion digest. Apply rescans owners, requires the reviewed plan digest,
publishes a global fail-closed prepared record, atomically renames each exact
container within its shard, and deletes only a bounded owned tombstone tree.
Prepared or temporary evidence blocks new reference admission after a crash;
retry resumes the same target set. Completed replay is read-only, and
predecessor chaining prevents an old confirmation from being mistaken for a
later object that reused the same digest.

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

The A1 qualification matrix composes two managers over one shared Artifact
Store and the same textual scope ID, then runs the same signed OKF package
through apply, restart, snapshot, leased query, upgrade, uninstall, and
terminal replay in both User and Workspace installations. During each upgrade
or uninstall, the opposite installation retains its exact cursor and continues
to answer through its already admitted generation lease. Shared immutable
bytes are therefore an optimization, never lifecycle or invocation authority.

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
| Receipt | numeric schema 6 |
| Installation snapshot | `a3s.use.installation-snapshot.v2` |
| Extension Registry snapshot | numeric schema 3 |
| Capability snapshot | numeric schema 5 |
| Extension/capability cursor | `a3s.use.extension-snapshot-cursor.v3` / `a3s.use.capability-snapshot-cursor.v4` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v6`, protocol 6 |
| Host managed scope | `a3s.use.plugin-managed-scope.v2` |
| Manager tools | `a3s.use.plugin-manager-tools.v4` |
| Pending graph | `a3s.use.pending-package-graph-operation.v4` |
| Pre-lock resolution attempt/diagnostic | `a3s.use.plugin-resolution-attempt.v1` / `a3s.use.plugin-resolution-attempt-diagnostic.v1` |
| Pre-plan download attempt/diagnostic | `a3s.use.plugin-download-attempt.v1` / `a3s.use.plugin-download-attempt-diagnostic.v1` |
| Operation diagnostic/history | `a3s.use.plugin-operation-diagnostic.v1` / `a3s.use.plugin-operation-history-diagnostic.v1` |
| Enablement recovery projection/operation | `a3s.use.cognitive-package-enablement-projection.v3` / `a3s.use.cognitive-package-enablement-operation.v3` |
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

The optional ACL `route` attribute is a presentation/CLI alias. It may be
duplicated, is never a generation-cursor package key or physical lease key,
and cannot influence Tool/MCP host identity. The cursor revision still commits
the complete projection, including aliases, for snapshot consistency.
Canonical automation uses the scoped package ID plus lifecycle generation and,
where needed, surface kind and surface ID. Explicit lookup of a duplicated
alias fails as ambiguous.

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
be marked hidden only after its exact package binding is absent from the
published Registry.

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
`a3s.use.capability-snapshot-cursor.v4` binds the exact installation, current
Installation Snapshot generation and digest, complete capability revision,
authoritative Registry revision, and canonically sorted immutable package
identities. Acquiring the cursor obtains every shared package-generation lease and
then re-reads both authorities. If any identity is hidden, stale,
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
