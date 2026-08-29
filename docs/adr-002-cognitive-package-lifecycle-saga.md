# ADR-002: Cognitive Package Lifecycle Saga

- Status: accepted architecture; core graph, Grant, enablement, and durable
  cutover implementation complete; product qualification in progress
- Decision date: 2026-08-03
- Updated: 2026-08-07
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Lifecycle: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Runtime boundary: [ADR-001](adr-001-plugin-runtime-broker-boundary.md)
- Roadmap: [A3S Use Roadmap](../ROADMAP.md)

## Context

One A3S cognitive package may contain Tool, MCP, OKF, Flow, Skill, and UI
contributions. Those contributions cross independent stores and hosts: package
storage, Workspace Grants, Runtime, Gateway, A3S Flow, Knowledge, static
projection, route leases, and the capability Registry.

Treating each contribution as independently installable would split identity,
trust, version, permission review, upgrade, and uninstall ownership. Treating
the whole operation as one filesystem transaction would be false because the
hosts do not share an ACID database.

## Decision

The signed package is the only lifecycle aggregate. Its surfaces are named
package-owned contributions, never independently installed packages.

The lifecycle is a durable, idempotent saga. One intent binds:

```text
operation ID + action + complete scope
+ reviewed plan digest and confirmation
+ package ID, package digest, manifest digest, generation
+ prior/candidate package locks where applicable
+ authorization and Grant evidence
+ canonical surface graph and checkpoint schedule
```

Dependencies prepare in forward order. Stops, drains, and removals run in
reverse. An optional surface becomes required when a required surface depends
on it.

## Current schedules

### Install

```text
commit immutable package generation installed-disabled
→ persist candidate Grant
→ prepare Tool / MCP / OKF / Flow / Skill / UI dependency-forward
→ publish the complete changed package closure through one durable cutover
→ checkpoint package and Grant evidence
```

### Upgrade

```text
bind exact prior and candidate locks in plan v4
→ prepare Add/Replace candidates dependency-forward
→ publish candidates and remove obsolete routes once
→ mark prior receipts hidden after exact route absence
→ drain calls admitted by the prior snapshot
→ revoke prior Grants
→ remove Replace/Remove generations reverse-prior-lock
```

### Uninstall

```text
atomically hide the exact removal closure
→ checkpoint Grant cutover
→ drain accepted calls
→ revoke exact Grants
→ remove surfaces and package generations in reverse
```

### Enable

```text
plan exact retained-artifact transition in plan v4
→ persist candidate Grant when required
→ prepare selected surfaces
→ publish one exact package generation with durable cutover evidence
→ commit Grant cutover and terminal result
```

### Disable

```text
plan exact retained-artifact transition in plan v4
→ hide one exact package generation with durable cutover evidence
→ commit Grant cutover
→ drain accepted calls
→ revoke exact prior Grant
→ stop selected surfaces in reverse
→ persist terminal result
```

Enable and disable change a separate monotonic Use-owned state generation; they
do not replace immutable package bytes or the dependency lock. Planning returns
an exact plan or terminal `NoChange`. Manager clients apply the reviewed plan
through `plugin_apply_plan`; there is no direct mutation API.

## Typed host boundaries

The coordinator is orchestration, not a generic plugin protocol. It uses
separate `Send + Sync` ports for:

- immutable package commit/removal;
- graph and single-package capability cutover, retirement, and drain;
- Tool lifecycle;
- MCP lifecycle;
- OKF Knowledge lifecycle;
- A3S Flow lifecycle;
- Skill projection; and
- UI projection.

Surface semantics remain distinct:

- Tool Tasks and Services use selected Runtime providers;
- MCP remains standard MCP through stdio or Gateway/Runtime composition;
- OKF uses Knowledge stage/promote/observe/search/remove;
- Flow always uses the `a3s-flow` engine and exact source/export binding;
- Skill and UI are immutable static projections with explicit dependencies.

No host may turn Tool into a universal action RPC, treat OKF as executable,
publish Flow from source presence, or create another lifecycle around
`flow.json`.

## Atomic cutover boundary

Capability host traits require cutover-aware methods. Publication returns
package-keyed evidence plus exact Registry generation and snapshot digest. No
default/fallback publisher exists.

The Registry retains bounded replay evidence until package and Grant journals
own the cutover. Reusing an idempotency key for different content fails before
mutation.

Prior receipt retirement is explicitly separate. A prior generation can be
marked hidden only after its exact route is absent; otherwise retirement fails
before mutation.

## Durability and recovery

Journals are bounded, canonical, atomically replaced, cross-process locked, and
path-owned. Every checkpoint has a deterministic SHA-256 idempotency key.
Detailed provider/package/Grant/projection receipts remain the source of truth;
the parent journal stores validated non-secret evidence digests and typed error
codes.

A retry of the same operation resumes the next exact checkpoint. Re-entry while
work resumes is not reported as a completed replay. Only reading an already
completed operation returns its terminal replay result.

Changed or deleted plan, lock, authorization, journal, graph, cutover, receipt,
or provider evidence fails closed. Recovery never reconstructs missing
authority or guesses a removal closure.

## Registry ownership

Registry selection is host configuration and orthogonal to lifecycle
ownership. Sources are named and replaceable, but an installed receipt retains
the exact source name, URL, root digest, channel, target, TUF role versions, and
complete verified catalog record used for that generation.

Changing a Registry configuration does not migrate installed provenance.

## Implementation state

Implemented in Use:

- manifest-v3 six-surface inventory and dependency schedule;
- lifecycle intent/operation v2 journals;
- immutable content-addressed package artifacts and exact-generation route leases;
- dependency-forward graph install and reverse uninstall;
- prior/candidate lock-bound upgrade with Add/Replace/Remove/Retain;
- durable Registry cutover request/evidence/replay/acknowledgement;
- pre-cutover package and Grant rollback;
- drain-before-Grant-revoke retirement;
- plan-v4 reviewed enable/disable and terminal `NoChange`;
- standalone Task, stdio MCP, Skill/UI, OKF Knowledge, and explicitly
  configured real A3S Flow hosts;
- exact scope propagation and authorization-stable recovery; and
- unit/integration/failure-injection coverage for the core saga.

Still required for product release:

- production Runtime Service, HTTP MCP/Gateway, managed Knowledge, and UI
  sandbox composition in every declared host;
- complete A3S Code CLI/TUI/agent convergence;
- distributed Flow/OS placement qualification;
- full Linux/macOS/Windows real-process recovery matrix; and
- production Registry, distribution, retention, security, and support gates.

## Consequences

Benefits:

- one package identity and one reviewed plan cover all contributions;
- dependency and cleanup order cannot drift between surfaces;
- visibility, Grants, and package generation share one cutover boundary;
- crash recovery is deterministic and auditable; and
- uninstall ownership preserves user and unrelated data.

Costs:

- every embedding product must supply typed lifecycle providers;
- visibility requires a multi-resource saga instead of a directory copy; and
- missing provider or durable evidence fails closed, even when a best-effort
  path might appear convenient.

## Pre-release policy

This ADR governs the current preview only. Superseded preview schemas, APIs,
and disk records are removed rather than kept as compatibility paths. A future
post-release migration policy requires a separate product decision.
