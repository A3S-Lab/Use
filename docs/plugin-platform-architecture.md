# A3S Use Plugin Platform Architecture

Status: development preview
Last updated: 2026-08-11

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
      verify · lock · journal · immutable package graph
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
├── verified-target cache policy
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
→ atomically publish candidate routes and remove obsolete routes
→ mark prior generations hidden only after route absence is proven
→ drain calls admitted by the prior snapshot
→ revoke exact prior Grants
→ remove prior generations in reverse prior-lock order
```

A pre-cutover failure rolls unpublished package and Grant candidates back.
After cutover, recovery finishes retirement; it does not revert visibility to a
mixed or unreviewed graph.

### Uninstall

```text
verify installed graph + reverse order
→ atomically hide the removal closure
→ checkpoint Grant cutover
→ drain each prior route
→ revoke exact Grants
→ remove surfaces and packages in reverse
→ remove installed graph record
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
the exact prior route is already absent. If the route is present, retirement
fails before mutation.

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
and route leases retain package generation identity. An N+1 candidate cannot
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

Capability snapshot schema v2 contains complete `PlanScope`, package/surface
identity, generation, desired/observed state, readiness, dependencies, and
evidence digests. A release-backed Tool Task enters `toolTasks` only when its
v4 binding matches the published package digest, scope, surface, and lifecycle
generation. The projection carries a stable host tool name, original command,
bounded argv contract metadata, exact lifecycle identity, and reviewed provider
ID; missing or mismatched bindings remain unpublished. Watchers resume by
generation plus revision and can hot-refresh resident hosts without polling
package directories.

The steady-state watch path reads immutable publications without acquiring the
Registry writer lock. A watcher may take that lock once to repair a verified
receipt/publication mismatch after a crash. Lifecycle mutations absorb this
short reconciliation window with a bounded asynchronous wait, while a real
concurrent writer still returns `use.extension.busy`. The wait never turns a
read into an unbounded mutation queue.

## Storage model

Use owns separate roots for:

- immutable package generations;
- selected and retained receipts;
- pre-plan archive/planning-target attempts and installed/pending package graphs;
- Host request bindings and their observation-only enablement index;
- lifecycle intents and journals;
- Registry snapshots and pending cutovers;
- Workspace Grants and operations;
- Runtime provisioning, Runtime/Flow/OKF/static bindings; and
- capability projections and enablement state.

Every path is derived from normalized identity, bounded, and checked against
symlink/reparse-point traversal. Atomic replacement syncs the file and parent
directory where supported. Tests must leave no temporary roots or locks.

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
