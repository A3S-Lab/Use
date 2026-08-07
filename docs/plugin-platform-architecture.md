# A3S Use Plugin Platform Architecture

Status: development preview
Last updated: 2026-08-07

## Executive decision

A3S Use owns one reviewed, recoverable package-graph lifecycle for native and
cognitive capabilities. CLI, TUI, Web, and agent management MCP are clients of
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
                Code · Web · OS · agents
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
| Code/Web/OS | Product scope, sessions, rendering, placement, user experience | A second plan/apply implementation |

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

## Replaceable Registry architecture

The host owns an ordered, bounded set of named Registry configurations:

```text
RegistryConfig
├── stable name
├── base URL
├── root metadata or root digest
├── enabled state
└── cache/storage location
```

The resolver receives this set per request. Packages cannot embed dependency
source URLs. A host may replace a mirror or trust root configuration, but that
does not mutate existing receipts. Installed provenance remains immutable.

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

## Lifecycle coordination

Package storage, Grants, Runtime, Gateway, Flow, Knowledge, static projection,
and capability visibility do not share a database transaction. A3S Use uses a
durable parent saga with idempotent typed checkpoints.

### Install

```text
verify plan and candidate lock
→ download changed packages
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
→ download and prepare only Add/Replace nodes
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

Capability snapshots contain complete `PlanScope`, package/surface identity,
generation, desired/observed state, readiness, dependencies, and evidence
digests. Watchers resume by generation plus revision and can hot-refresh
resident hosts without polling package directories.

## Storage model

Use owns separate roots for:

- immutable package generations;
- selected and retained receipts;
- installed/pending package graphs;
- lifecycle intents and journals;
- Registry snapshots and pending cutovers;
- Workspace Grants and operations;
- Runtime/Flow/OKF/static bindings; and
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
- CLI, TUI, Web, and agent MCP use one plan/apply service;
- exact package/Grant recovery passes failure injection at every checkpoint;
- Linux, macOS, and Windows pass the declared real-process matrix;
- Registry rotation, replacement, expiry, and offline recovery are exercised;
- distribution artifacts have reproducible provenance, checksums, signatures,
  and install verification; and
- retention, observability, repair, incident, and support procedures are tested.

Until then, the architecture and implementation remain a development preview.
