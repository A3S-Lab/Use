# A3S Use Plugin Platform Architecture

Status: development preview
Last updated: 2026-09-05

## Executive decision

A3S Use owns one reviewed, recoverable package-graph lifecycle for native and
cognitive capabilities. CLI, TUI, and agent management MCP are clients of
one host Plugin Manager. They do not implement separate discovery, planning,
authorization, installation, enablement, or recovery paths.

The cognitive package is the aggregate root. One immutable package generation
may contribute Tool, MCP, OKF, A3S Flow, Skill, and UI surfaces. Its complete
dependency closure publishes as one capability generation.

The platform has not shipped a supported release. It carries one current
protocol baseline and no pre-release compatibility branches.

## Architectural drivers

1. Install one package and its full SemVer dependency closure.
2. Select Registries and trust roots at the host boundary.
3. Preserve signed catalog and TUF provenance through the installed receipt.
4. Keep planning read-only and mutation bound to exact reviewed evidence.
5. Prevent mixed package, Grant, provider, and capability generations.
6. Resume process crashes from durable evidence without guessing.
7. Preserve each subsystem's native protocol and ownership.
8. Work across Linux, macOS, and Windows filesystem/process semantics.
9. Reject superseded preview state instead of maintaining migration code.

## System boundary

```text
                host-selected Registry set
          name · URL · trust root · enabled state
                           │
                           ▼
                  shared Plugin Manager
       search · resolve · policy · plan · confirmation
                           │ exact reviewed envelope
                           ▼
                    A3S Use engine
      verify · lock · journal · installation snapshot
                           │
          ┌────────────────┼────────────────┐
          ▼                ▼                ▼
   Runtime/Gateway      A3S Flow       Knowledge/static
     Tool · MCP          workflow       OKF · Skill · UI
          └────────────────┼────────────────┘
                           ▼
               atomic capability snapshot
                           ▼
                Code · OS · agents
```

### Ownership

| Component | Owns | Must not own |
| --- | --- | --- |
| Umbrella/managed host | Registry configuration, trust roots, policy, confirmation, secrets, provider injection, UX | Package storage or another lifecycle manager |
| Plugin Manager | Catalog queries, resolution request, immutable plan, reviewed apply, operation replay | Provider scheduling internals |
| A3S Use | Package verification, locks, generations, receipts, grants, journals, lifecycle order, capability evidence | Generic workload scheduler or UI renderer |
| Runtime | Task/Service execution, process identity, health, invocation, drain | Package trust, user authorization, Registry selection |
| Gateway | HTTP/MCP endpoint exposure, routing, health, drain | Package resolution or package receipts |
| A3S Flow | Workflow compilation, execution, replay, observation | A parallel package format or lifecycle |
| Knowledge host | OKF validation, indexing, promotion, cited retrieval, retention | Process execution |
| Code/OS | Product scope, sessions, rendering, placement, user experience | A second plan/apply implementation |

The concrete managed-host port is `CognitivePackageHostManager`. It composes
the same `CognitivePackageManager`, persisted Registry source set, lifecycle
factory, and authorization provider used by the host. Its additional durable
state is a narrow protocol index for request replay; it never owns package
admission, Grants, lifecycle checkpoints, enablement state, or capability
publication. Expired requests resume only when those existing Use-owned stores
prove exact prior admission or completion. Install and upgrade planning verifies
and caches the complete selected closure; reviewed digest-only apply consumes
that exact cache without another network request, so a Registry outage cannot
change or strand an already reviewed operation.

The presentation-facing boundary is `PluginManagerService`. It adds no state
machine: deterministic request IDs, opaque Registry cursors, stable installed
pages, reviewed-plan reopening, exact operation observation, bounded watch, and
safe cancellation all bind back to Host Manager evidence. The standard
`PluginManagerMcpServer` derives its current thirteen routes directly from
`PluginManagerToolset::v5()` and delegates every call to that service. Apply and
cancellation confirmation are supplied only by an injected trusted host
provider. The standalone CLI's Registry-backed install, upgrade, and uninstall
mutations use this boundary and return the exact Host plan/apply evidence
alongside their existing output. Its `plugin` command maps all thirteen frozen
manager operations to the same service: four reads, five non-mutating plans,
one apply, two read-only operation controls, and one explicit cancellation
control. Observation and cancellation require the exact package/scope/
operation/plan identity; cancellation also requires `--yes`. An ordinary MCP
invocation never creates `User` authority, and exact apply or replay performs
no Registry request. A3S Code CLI, TUI `/packages`, and the product-host
manager MCP now compose this same service. The Host Manager's signed
six-surface User/Workspace lifecycle and replay matrix is qualified; complete
Code product-host E2E and release qualification remain open, so the complete
M2 product convergence gate is not yet satisfied.

## Domain model

### Stable identities

- Package: `<publisher>/<name>`
- Surface: `(package_id, kind, surface_id)`
- Immutable artifact: `(package_id, version, package_digest, manifest_digest)`
- Lifecycle generation: positive monotonic integer for one package artifact
- Scope: `(kind, id)`, where kind is `user` or `workspace`
- Operation: immutable operation ID plus canonical plan digest
- Capability generation: immutable Registry snapshot generation plus digest

Routes, display names, process IDs, ports, temporary paths, and UI positions are
not stable package identities.

### Package aggregate

```text
CognitivePackage
├── identity and SemVer version
├── requires_use and repository revision
├── package dependencies
├── verified catalog provenance
├── immutable package generation
└── named surface graph
    ├── Tool*
    ├── MCP*
    ├── OKF*
    ├── Flow*
    ├── Skill*
    └── UI*
```

The package owns install, upgrade, enable, disable, and uninstall state. Surface
selection can limit projection but cannot create an independently installed
surface generation.

### Desired and observed state

Use-owned desired state is package-scoped. Observation is evidence-scoped:

- package receipt and exact generation;
- Workspace Grant and ceiling;
- Runtime/Gateway binding and health;
- Flow compiled binding and observation;
- OKF promoted Knowledge binding;
- Skill/UI static projection; and
- immutable capability snapshot selection.

The Surface Reconciler publishes readiness only when all required evidence
agrees on package, surface, scope, and generation.

## Package contract

The only accepted manifest is ACL schema version 3. It requires `README.md`,
repository identity, `requires_use`, and at least one named surface. Dependency
blocks contain package IDs and SemVer requirements only.

The surface graph is explicit and acyclic:

```text
Tool ───────────────┐
MCP ────────────────┼──▶ Flow ──▶ Skill ──▶ UI
OKF ────────────────┘       └──────────────▶ UI
```

Actual edges are manifest-defined. UI can also bind Tool or MCP directly.

Package validation rejects path escape, links, duplicate normalized paths,
archive ambiguity, missing files, content drift, invalid dependency edges,
cycles, unbounded counts/bytes, and incompatible host ranges before mutation.

### Tool and MCP

Tool is a native Task or Service contract, not an MCP `tools/list` item. It
retains native argv/HTTP behavior. MCP remains the standard MCP protocol.

Packages describe workload requirements but cannot select a provider. The host
injects typed Runtime/Gateway providers. Planner selection occurs before
payload download and is rebound after Grant/policy evidence is fixed. Missing
or changed provider evidence fails closed; there is no native fallback.

### OKF

OKF is non-executable. Only format 0.2 is accepted. Use verifies package bytes
and bundle bounds; a Knowledge host owns promotion, indexing, search, and
retirement. Staged content is not visible. A current snapshot selects exactly
one promoted generation, while a session may retain a leased prior generation
until drain completes.

### A3S Flow

Flow uses the single `a3s-flow` engine identity. The package binds Native
TypeScript source/export and explicit Tool/MCP/OKF edges. The host returns
preflight and compiled-binding evidence for the exact package generation.

`flow.json` is a visual design/deployment document used by host tooling. It is
not another source of package truth. Code local execution and OS remote
placement consume the same package-owned Flow identity and lifecycle evidence.

### Skill and UI

Skill and UI are integrity-bound static projections. Skill points to
`SKILL.md`; UI points to a sandboxed static entry. They publish only after
declared dependencies are ready. Static content cannot acquire ambient Runtime,
network, filesystem, or secret authority.

The capability snapshot carries each published UI contribution's canonical,
sorted Skill/Tool/MCP/Flow dependency set from the package surface graph plus
the `a3s.use.ui-dependency-evidence.v1` completeness marker. The marker makes
an intentionally empty set distinct from a legacy producer that omitted the
evidence. A host stages that evidence with the UI value and does not reparse
the package manifest or infer backend authority from asset contents.

UI state belongs to the embedding host rather than the package origin. The
current Code host keys its bounded durable store by `PlanScope`, lifecycle
package ID, and UI surface ID. A state request is admitted only while an exact
published lifecycle-generation lease is held. Lifecycle intent v3 carries the
sorted set of `retained_ui_state_surfaces`: disable, rollback, replacement
retirement, and an upgrade that keeps a surface preserve that namespace; a
true uninstall or removed surface clears it. This retention contract does not
implement N+1 readiness fallback—the candidate selection, cutover, and rollback
decision remains a separate host responsibility.

## Replaceable Registry architecture

The host owns a bounded set of named Registry configurations. The standalone
host persists at most 64 entries in one canonical ACL document; managed hosts
inject the same typed model:

```text
RegistryConfig
├── stable name
├── base URL
├── bootstrap root digest and optional managed root bytes
├── enabled state
├── source-observation and partial-cache policy
├── source identity and isolated TUF/cache location
└── reviewed configuration revision
```

Managed hosts derive exact digest/version/size evidence without state or
network I/O through `inspect_bootstrap_root`, then admit those same bytes
through `TrustedRegistry::pin_trusted_root`. Both APIs share the engine's
public one-MiB bound and decoder. Pinning additionally enforces the exact
configured digest, regular-file defense, metadata lock, and immutable cache.
Neither operation certifies a Registry by itself. The ordinary
refresh must still verify the complete TUF chain, expiry, rollback state, and
catalog metadata before a result becomes trusted evidence. The bootstrap
version identifies the caller-pinned object; catalog provenance separately
reports the current root version reached by the verified TUF refresh.

The first enabled standalone source becomes the default. A request selects one
enabled root source and receives every other enabled source for dependency
resolution. Packages cannot embed dependency source URLs. Replacement,
default selection, enablement, disablement, and removal use compare-and-swap
against the exact reviewed ACL revision. A changed name/URL/bootstrap-root
identity receives a new datastore; disabling, removal, and replacement retain
the old datastore so restoring the exact identity can reuse its evidence. None
of these operations mutate existing receipts. Installed provenance remains
immutable.

`GitHubRegistryRepository` is a typed address adapter, not another Registry
implementation. It accepts one exact `owner/repository` slug plus canonical
ref/path components and derives the ordinary HTTPS Registry base URL. The
result then enters the same source identity, mandatory bootstrap-root pin,
TUF refresh, cache, catalog, planning, and receipt path as an explicit `--url`.
The client does not invoke Git, consume a GitHub API token, or treat a branch,
tag, commit, pull request, or repository signature as package authority.

TUF target metadata contains the complete current catalog record. There is no
partial metadata or older-catalog fallback. Remote preparation retains:

- verified catalog record;
- Registry and TUF provenance;
- archive target/length/digest;
- expanded package/manifest digests; and
- provider and permission planning evidence.

Receipt loading requires all of that evidence for Registry/TUF installations.

## Resolution and planning

Resolution starts with one root package request and produces an exact package
lock. It is deterministic, bounded, and host-aware. The same package appearing
in more than one enabled source is an error, not a priority fallback.

Plan v4 is the immutable mutation boundary. It binds:

- action and actor;
- complete User/Workspace scope;
- policy authority and decision;
- host capability inventory;
- prior and candidate locks;
- package transitions and selected surfaces;
- current state revision;
- provider, permission, OKF, Flow, byte, download, and process impact;
- expiry and confirmation; and
- canonical digest.

Apply re-derives this evidence immediately before mutation. A separately
reviewed package-lock digest can be required before download.

Once an exact lock and Add/Replace archive set exist, Use persists one
non-authoritative pre-plan download attempt under a process-held package lock.
The exact lock also selects any separately signed executable-planning targets.
Both target families survive interruption for independent byte observation,
but cannot authorize planning, apply, or recovery. The record is removed only
after the reviewed pending graph is durable; the graph then becomes the current
operation diagnostic source.
After a graph or enablement operation reaches a validated terminal outcome,
Use appends its path-free snapshot to the bounded per-scope/package operation
history before removing recovery authority. Exact replay deduplicates
`(operationId, planDigest)`, so the history survives process exit and package
removal without becoming a second lifecycle or recovery state machine.

Before enablement admission, a digest-bound Host observation index selects the
newest exact reviewed enable/disable request for a public PlanScope/package by
`(plannedAtMs, requestId)`. The index retains the managed scope only to resolve
the immutable private request and cannot authorize apply or recovery. The
standalone diagnostic reconstructs a transient expected lifecycle schedule from
the installed receipt/manifest, projects `planned` or exact `cancelled`
evidence, and yields to active or completed Use-owned state.

## Lifecycle coordination

Package storage, Grants, Runtime, Gateway, Flow, Knowledge, static projection,
and capability visibility do not share a database transaction. A3S Use uses a
durable parent saga with idempotent typed checkpoints.

### Install

```text
verify plan and candidate lock
→ retain exact non-authoritative download attempt
→ download changed packages
→ retain reviewed pending graph and remove the download attempt
→ commit immutable generations as installed-disabled
→ persist candidate Grants
→ prepare packages dependency-forward
→ atomically publish the complete candidate snapshot
→ checkpoint cutover and retire replay evidence
```

No capability becomes visible until every required candidate is prepared.

### Upgrade

```text
verify prior lock + candidate lock
→ classify Add / Replace / Remove / Retain
→ retain exact non-authoritative download attempt
→ download and prepare only Add/Replace nodes
→ retain reviewed pending graph and remove the download attempt
→ atomically publish candidate package bindings and remove obsolete bindings
→ mark prior generations hidden only after binding absence is proven
→ drain calls admitted by the prior snapshot
→ revoke exact prior Grants
→ remove prior generations in reverse prior-lock order
```

A pre-cutover failure rolls unpublished package and Grant candidates back.
After cutover, recovery finishes retirement; it does not revert visibility to a
mixed or unreviewed graph.

### Uninstall

```text
derive exact root lock from installation snapshot
→ atomically hide the removal closure
→ checkpoint Grant cutover
→ drain each prior package generation
→ revoke exact Grants
→ remove surfaces and packages in reverse
→ commit the next installation snapshot generation without that root
```

Installed dependents prevent removal. A retained shared dependency is not part
of the removal closure.

### Enable and disable

The immutable artifact and dependency graph do not change. Enablement has a
separate monotonic Use-owned state generation.

Planning returns plan v4 or terminal `NoChange`. Apply verifies the exact
retained artifact, receipt digest, expected state generation, authorization,
and confirmation. Enable prepares surfaces then publishes once. Disable hides
once, checkpoints the Grant cutover, drains prior calls, revokes the exact
Grant, and stops surfaces in reverse.

There is no direct mutation API. Manager clients call plan then
`plugin_apply_plan`.

## Atomic capability boundary

Every visibility mutation returns:

- Registry generation before and after;
- immutable snapshot digest; and
- package-keyed lifecycle evidence in canonical order.

Graph host traits require cutover-aware methods. They have no default or
fallback publisher. Durable cutover records remain in the Registry until the
package and Grant journals own the same evidence.

Retirement is a different operation. It may update a prior receipt only when
the exact prior package binding is already absent. If the binding is present,
retirement fails before mutation.

## Crash recovery

Recovery is exact replay, not reconstruction. Durable state includes:

- planning request/result;
- operation plan and confirmation;
- prior/candidate package locks;
- authorization, Grant snapshot/change/resolution, and ceilings;
- package and surface intents/checkpoints;
- Runtime Service provisioning and final binding receipts;
- Registry cutover request/evidence; and
- terminal operation result.

The same operation resumes from the next checkpoint. Re-entry after a process
crash returns `replayed = false` while work resumes; only reading a completed
operation returns `replayed = true`.

Deleted or mismatched recovery evidence fails closed. The engine does not infer
a replacement plan, generation, source, Grant, or cutover.

## Runtime and capability observation

The Runtime Broker uses typed provider objects and exact selection evidence.
Runtime bindings, Flow bindings, Knowledge projections, static projections,
and generation leases retain package generation identity. An N+1 candidate cannot
overwrite N before snapshot cutover.

A release-backed Runtime Task receipt is a durable invocation template, not an
operation-history pointer. It retains the reviewed argument-free unit spec,
Grant and descriptor digests, provider evidence, capture bounds, and lifecycle
generation. Dispatch reconstructs only the invocation-specific unit ID and
argv, revalidates the receipt-owned provider, and holds the exact published
package-generation lease until output capture and Runtime cleanup finish.

A persistent Tool or Streamable HTTP MCP Service writes exact provisioning
authority before calling Runtime. Its monotonic phases retain the original
lifecycle/apply key, reviewed provider and spec evidence, healthy Runtime
observation, and finally the opaque Gateway binding plus MCP initialize
evidence where applicable. A final Service binding is made durable before that
authority is removed. Restart therefore replays the same Runtime/Gateway
effect, reconciles the safe both-files window, or completes candidate cleanup;
it never infers a unit or route from package files.

Registry publication is keyed by the installation-scoped package lifecycle
identity, and each capability adds its canonical surface kind and ID. The
manifest `route` value is an optional human alias only: duplicate aliases may
coexist, alias ambiguity fails closed, and aliases never influence generation
locks, canonical cursor package keys, or Tool/MCP host names. The snapshot
revision still commits the full presentation projection.

Capability snapshot schema v5 contains the exact `InstallationId`, current
Installation Snapshot generation and digest, package/surface identity,
generation, desired/observed state, readiness, dependencies, and evidence
digests. A release-backed Tool Task enters
`toolTasks` only when its v4 binding matches the published package digest,
installation, surface, and lifecycle
generation. The projection carries a stable host tool name, original command,
bounded argv contract metadata, exact lifecycle identity, and reviewed provider
ID; missing or mismatched bindings remain unpublished. Watchers resume by
generation plus revision and can hot-refresh resident hosts without polling
package directories.

The additive `mcpServers` projection preserves every published extension MCP
surface instead of collapsing a package to one alias. Each entry binds its
canonical surface ID, collision-resistant host name, activation, exact package
lifecycle identity, and recomputed file-evidence digest. Stdio entries expose
only the package-relative executable and bounded arguments. Streamable HTTP
entries expose only the package-relative release plus an opaque endpoint
reference/path and the exact Runtime provider/build/generation/descriptor
evidence whose canonical binding digest matched the package receipt. No
resolved URL, header, OAuth value, secret, or credential crosses this boundary.
Mismatched or missing Runtime/Gateway initialize evidence leaves the surface
unpublished.

For admission that spans more than one package, the injected
`CapabilityRegistry` derives `a3s.use.capability-snapshot-cursor.v4`. It binds
the exact installation, Installation Snapshot generation and digest, and
capability revision to the Extension Registry digest and sorted package,
manifest, and lifecycle generations. Acquisition takes every shared generation
lease in canonical order and rechecks both snapshot authorities only after the
complete batch is held. A concurrent cutover, hidden or contended generation,
digest drift, or enabled non-lifecycle package yields no partial lease. The
non-clone `CapabilitySnapshotLease` is `Send + Sync`; Code may own it for a Run
without receiving package mutation authority. Rust `Drop` releases only the
synchronous locks, while Use keeps asynchronous drain and retirement in its
explicit lifecycle coordinator.

The Gateway catalog has two deliberately different clocks. Its top-level
`generation` is the immutable capability-publication generation and must match
the leased snapshot cursor. A descriptor's `generation` is the lifecycle
generation of its owning package; those values may differ when one publication
contains several independently upgraded packages. The catalog rejects a
duplicate `(package, surface kind, surface ID)` identity even when the proposed
descriptors carry different lifecycle generations. This prevents a stale
publication from being mistaken for a package incarnation and keeps the
publication cut separate from package lifecycle retirement (PRs
[#199](https://github.com/A3S-Lab/Use/pull/199) and
[#200](https://github.com/A3S-Lab/Use/pull/200)).

PR [#202](https://github.com/A3S-Lab/Use/pull/202) adds host-owned bounded
admission at both the Gateway invocation boundary and the standard Streamable
HTTP `/mcp` boundary. A shared in-flight semaphore and rolling call window
return bounded 429/503 outcomes instead of creating an unbounded queue. PR
[#203](https://github.com/A3S-Lab/Use/pull/203) hardens the HTTP edge with one
constant-time bearer credentials, duplicate-header rejection, optional exact
Origin checking (native clients may omit Origin), `WWW-Authenticate` and
`Retry-After` responses, and real `rmcp` client discovery/invocation tests.
The HTTP method deliberately does not provide TLS: a host must use loopback or
a trusted TLS-terminating reverse proxy. These controls authenticate and bound
the endpoint, but do not themselves resolve opaque references from a live host
authority. `CapabilityGatewayHttpConfig::for_principals` now supports a
bounded (64-entry) immutable token-to-principal registry; authentication scans
the complete configured set without early exit, rejects duplicate credentials,
and carries only the selected typed principal into the provider context.

The invocation provider is also the authorization seam. Its required
`authorize` hook runs after the published input schema is validated and before
the provider can perform any effect; a denial is projected as the bounded
`use.plugin.capability_gateway_forbidden` result and the invocation is not
called. Hosts should bind one provider instance to their authenticated
principal and keep principal, Grant, and scope policy private to that provider.
The HTTP embedding offers both `CapabilityGatewayHttpConfig::for_principal`
and `CapabilityGatewayHttpConfig::for_principals`; the boundary passes its
typed principal/transport context to both hooks, and the principal is never
serialized into MCP discovery or results. There is no implicit allow
implementation: a contract-only provider must still state its policy
explicitly. This hook is a policy boundary, not a replacement for the still-
pending production receipt/Runtime/Grant composition.

The live resolver boundary is now represented by
`CapabilityGatewayInvocationResolver`. It resolves one catalog descriptor to a
`CapabilityGatewayInvocationLease`; the lease's private handle owns the exact
package-generation guard and remains alive until the call completes.
`CapabilityGatewayResolvedProvider` performs resolution and authorization once
for a call, verifies that the returned lease carries the descriptor's exact
opaque `InvocationRef`, and exposes only the provider result to MCP. The
production host must still implement the resolver against its receipt,
Runtime, Grant, and scope authorities; a generic resolver cannot authorize a
binding by itself.

The host can now derive the Gateway catalog boundary directly from one
`CapabilityRegistrySnapshot` with
`CapabilityRegistrySnapshot::capability_gateway_catalog`. The projection
revalidates each host-supplied, schema-checked descriptor against the snapshot
cursor, public projection revision, package and manifest digests, reviewed
publication-record evidence, selected surfaces, and ready/enabled binding
before constructing the canonical catalog. It may expose a bounded subset for
a consumer, but it cannot invent a
package or surface outside the reviewed publication. The helper deliberately
does not verify signatures or resolve opaque references: those authorities stay
with the host, and `CapabilityGatewayMcpServer::from_registry_snapshot` acquires
the exact RAII lease only after the projection succeeds. A publication change
or a draining package returns no server rather than serving a mixed snapshot.

For hosts that have crossed the signed-description boundary, the preferred
entry point is
`CapabilityGatewayMcpServer::from_verified_registry_snapshot_with_factory_and_options`.
It observes one snapshot, projects the verified proofs, captures that same
cursor in `CapabilityGatewayRegistryResolver`, acquires the exact server
lease, and retains the negotiated consumer policy and bounded admission limits
together. The resolver still receives a per-call lease and the host-owned
factory remains responsible for receipt, Runtime, Grant, principal, and scope
authorization; a publication race returns no server instead of serving mixed
catalog and provider state.

Consumer selection is now explicit at the embedding boundary. The core
`CapabilityConsumerProfile` contract distinguishes `generic-mcp` from `a3s`,
and `CapabilityConsumerNegotiation` accepts only a complete host-supported
extension set (`flow`, `knowledge`, and `ui`). Canonical bytes and a digest bind
the decision, and `CapabilityGatewayMcpServer` retains it through clones and
snapshot leases. This metadata does not grant authorization or manufacture
MCP resources/prompts; the current adapter still exposes only schema-validated
Tools, while profile-aware projection and production host composition remain
open.

The steady-state watch path reads immutable publications without acquiring the
Registry writer lock. After one reconciliation read it registers a bounded
cross-platform filesystem subscription against the atomic Registry commit
point, closes the read-to-subscribe race with a second publication read, and
then re-reads only when a target event arrives. Native notifications are
preferred, and a bounded target-metadata probe runs alongside them to cover
platform backends that coalesce or omit an atomic replacement; a metadata-only
polling watcher is also available when the native backend cannot be registered.
If the installation directory does not yet exist, only the closest existing
ancestor's immediate entries are observed; each relevant directory creation
advances the subscription toward the exact commit point, so no recursive drive
or user-state watch is introduced. Every existing path
component below the configured Use state root must remain an owned directory;
symlinks and Windows reparse points fail closed before registration. Callback
events are filtered before entering a capacity-one coalescing channel, and at
most 64 Registry watchers may exist in one process. The operating-system event
is never treated as authority: every wake must still decode and validate
`registry.json`. A watcher may take the writer lock once to repair a verified
receipt/publication mismatch after a crash. Lifecycle mutations absorb this
short reconciliation window with a bounded asynchronous wait, while a real
concurrent writer still returns `use.extension.busy`.

The facade capability watcher delegates its steady-state wait to that
generation notification. It builds the complete capability projection at
subscription setup, after a real generation advance, and once at timeout to
close the final race, rather than rescanning receipts and rehashing immutable
Skill, Flow, and UI assets every 100 ms. The Gateway exposes a shared,
bounded `CapabilityGatewayNotificationHub` for the protocol half: initialized
MCP peers are retained up to a fixed limit, exact publication keys are
coalesced, older generations are rejected, and standard `tools/list_changed`,
`resources/list_changed`, and `prompts/list_changed` notifications are sent
concurrently with a bounded timeout. This hub only tells clients to re-list; it does not mutate the
immutable server or silently move a session across generations. A host must
durably publish the replacement catalog, route new sessions to it, and keep
the old lease through drain. The Control lifecycle still has to persist the
complete agent-facing descriptor catalog and connect its cutover to this hub.

The payload-owner side is now explicit: `CapabilityGatewayCatalogStore` stores
the canonical catalog bytes under an installation-scoped, bounded SHA-256
layout. It uses deterministic crash staging, no-follow path checks, and
physical resolution of platform ancestor aliases before no-follow I/O. It then
uses create-if-absent hard-link publication and permits exact digest plus
generation/revision reads after restart. This store is intentionally not a
Control cursor or a latest pointer; lifecycle cutover, session retirement, and
catalog retention remain higher-level authorities.

The authoritative `registry.json` file is itself treated as a hostile mutable
boundary. Its complete configured state-directory chain must be an owned,
non-link directory; the final file is opened with no-follow/reparse protection,
is limited to 4 MiB before allocation and JSON decoding, and is revalidated for
file identity and length after the read. Publication creates missing directories
one component at a time inside the configured state root, flushes and syncs a
bounded temporary file, and atomically replaces the target. A linked,
oversized, redirected, or concurrently replaced snapshot fails closed rather
than becoming lifecycle or capability authority.

## Storage model

Use separates global immutable/derivable inputs from installation authority.
The global roots contain Registry source configuration, TUF metadata and
source-scoped target observations and partials, raw blobs and expanded packages
in the global Artifact Store, and content-addressed compiled artifacts. They
cannot contain selection, activation, or authorization state. Raw blobs are
stored at `data/artifacts/blobs/sha256/<prefix>/<digest>/content`; expanded
content is stored at
`data/artifacts/expanded-packages/sha256/<prefix>/<digest>/content`; one
cross-process digest lock makes concurrent installations converge on the same
complete content identity. Registry-source observations, partials, metadata,
and provenance remain source-scoped. A store-bound shared admission is required
when source observations, lifecycle receipts, installation snapshots, or
applying lifecycle journals, or durable operations publish Artifact Store
references; exclusive maintenance therefore cannot race a new reference. The
exclusive guard now also authorizes a deterministic, path-free, bounded
physical inventory of canonical content and abandoned staging across both
tiers. A separate Registry-reference inventory derives every blob observation
from all preserved source datastores and rejects unknown, linked, incomplete,
or unbounded state. Global reference v1 aggregates those observations with all
installation snapshots, current and retained receipts, non-cancelled graph
operations, and applying or rolling-back lifecycle journals under the same
exclusive guard. It remains path-free, preserves missing references, and fails
on identity or physical-expectation conflicts. Joined reachability v1 captures
the physical and logical inventories before releasing that guard, preserves
their orthogonal evidence in one row per artifact, and derives checked storage
usage plus bounded quota assessment. It does not reserve concurrent capacity or
authorize deletion. The Artifact Store separately owns an optional canonical
`data/artifacts/storage-quota.acl`. Policy changes use revision compare-and-swap
under the lock order `reference admission -> global storage -> digest
mutation`. With no policy, publishers share the storage boundary. With a policy,
Blob and expanded-package commits hold it exclusively across bounded physical
scan, exact logical-byte/container projection, same-digest staging cleanup, and
atomic publication. This serialized admission prevents concurrent overcommit;
it is intentionally not a parallel durable reservation ledger. Explicit digest
audit now rehashes both tiers sequentially under the same exclusive guard,
repeats bounded physical inventory, and reports path-free mismatch evidence
without mutation.
Shared-content corruption fails closed. Exact-plan logical quarantine re-audits
one complete mismatch under that guard, requires the reviewed canonical plan
digest, and publishes a bounded no-clobber marker while preserving canonical
content as forensic evidence. Inventory validates marker state without charging
it as content or staging; new ordinary Blob and expanded-package access is
denied. Verified rehydration is now a global, reference-aware operation rather
than an implicit overwrite performed by one source or installation. The facade
holds the same collection guard while proving zero Registry, installation,
receipt, pending-graph, and lifecycle-operation references; the Artifact Store
then reverifies an external candidate, persists exact prepared/completed
evidence, accounts for quota peak, and switches both Blob and expanded-package
content fail-closed. Matching terminal replay verifies the durable record and
canonical replacement without reopening the candidate or requiring references
published after completion to retire. Confirmed GC also remains a separate
reviewed authority: a bounded policy explicitly names exact Blob or expanded-
package digests; plan and apply repeat the complete zero-reference scan under
the collection guard and bind physical measurements, stable lifecycle state,
and predecessor completion. Apply persists a global admission fence before
same-shard atomic retirement and bounded no-link tombstone deletion. Crash
retry resumes only that recorded set, while matching terminal replay is
read-only and cannot delete a later recreated object.

Each `InstallationId(kind, id)` owns separate roots for:

- selected and retained receipts;
- pre-plan archive/planning-target attempts, the installation snapshot, and
  pending package-graph operations;
- Host request bindings and their observation-only enablement index;
- lifecycle intents and journals;
- Registry snapshots and pending cutovers;
- Workspace Grants and operations;
- Runtime provisioning, Runtime/Flow/OKF/static bindings; and
- capability projections and enablement state.

Every scoped path is derived from the validated installation kind and a
collision-resistant key over kind plus ID. Every path is bounded and checked
against symlink/reparse-point traversal. Atomic replacement syncs the file and
parent directory where supported. Windows state publication uses extended-length
native paths where required. Tests must leave no temporary roots or locks.

Uninstall, rollback, and failed publication retire only scoped authority. They
never delete global package content. The coordinated installation backup also
excludes global artifacts. Unreferenced expanded trees remain until a future
collector can prove that no installation or durable operation can reach them;
eager per-installation deletion is unsafe.

Runtime, Flow, OKF binding/SQLite, and lifecycle journal stores are constructed
for one exact `InstallationId`. A scope carried by a receipt, request, or
recovery record is checked against that installation before path derivation or
any filesystem effect. The nested kind/key remains integrity evidence for the
current preview layout; it is not a caller-selectable second storage domain.

The pre-A1 unscoped package/state layout and installation-scoped package-byte
directories are rejected rather than ignored or migrated. Operators must
preserve them for review, remove only independently proven legacy entries, and
reinstall into an explicit identity backed by the global Artifact Store. Global
Registry source configuration, trust/TUF caches, and current artifact inputs
are not legacy installation state and remain intact.

Unknown pre-release state is never migrated. The error instructs cleanup and
reinstallation with the current build.

## Security invariants

1. Package bytes never authorize themselves.
2. Catalog, lock, plan, receipt, and artifact identities must agree exactly.
3. A package cannot choose its Registry, provider, secrets, or Grant.
4. Apply cannot broaden the reviewed plan.
5. Required missing readiness remains unpublished.
6. Capability visibility changes once per reviewed graph operation.
7. Accepted calls drain before prior authorization or generation removal.
8. Recovery requires exact durable evidence.
9. Static UI and Skills receive no ambient authority.
10. Unknown schema, field, path, or state fails closed.

## Pre-release evolution

The current product has no compatibility commitment. When a contract changes
before the first supported release, update all producers, consumers, fixtures,
tests, and documentation together; delete the superseded path; and reject stale
state. Keep only rejection fixtures needed to prove the boundary.

SemVer dependencies, host ranges, target checks, and provider capability checks
remain required because they describe current package correctness, not old
protocol support.

## Architecture acceptance gates

This architecture is release-qualified only when:

- all six surfaces run through production providers in each declared host;
- CLI, TUI, and agent MCP use one plan/apply service;
- exact package/Grant recovery passes failure injection at every checkpoint;
- Linux, macOS, and Windows pass the declared real-process matrix;
- Registry rotation, replacement, expiry, and offline recovery are exercised;
- distribution artifacts have reproducible provenance, checksums, signatures,
  and install verification; and
- retention, observability, repair, incident, and support procedures are tested.

Until then, the architecture and implementation remain a development preview.
