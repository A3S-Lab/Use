# ADR-002: Cognitive Package Lifecycle Saga

- Status: accepted architecture; P0 hosts, P2-A Grant graph saga, P2-B graph wiring, and permission-bearing enablement core implemented
- Decision date: 2026-08-03
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Lifecycle: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Runtime boundary: [ADR-001: Host-Owned Plugin Runtime Broker](adr-001-plugin-runtime-broker-boundary.md)
- Roadmap: [A3S Use Plugin Platform Roadmap](../ROADMAP.md)

## Context

An A3S cognitive package may contain Tool, MCP, OKF, Flow, Skill, and UI
contributions. Those contributions cross several hosts and persistence systems:
the immutable package store, A3S Runtime, Gateway, A3S Flow, Skill and UI
projections, A3S Knowledge, route leases, and the capability registry.

Treating each contribution as an independently installable package would split
identity, trust, version, permission review, upgrade, and uninstall ownership.
Treating the operation as one filesystem transaction would also be false:
these hosts do not share an ACID database.

## Decision

The signed package is the only lifecycle aggregate. Tool, MCP, OKF, Flow,
Skill, and UI are named package-owned contributions, never independently installed
packages.

One lifecycle intent binds all authority needed to replay the operation:

```text
operation ID + reviewed plan digest + scope
+ package ID, package digest, and manifest digest
+ exact package generation + lifecycle action
+ canonical surface graph + checkpoint schedule
```

The surface graph comes from the admitted schema-v3 manifest. Dependencies are
prepared in forward topological order and stopped or removed in reverse order.
An optional surface becomes required when it is in the dependency closure of a
required Skill or UI.

The version-1 schedules are:

```text
install / candidate upgrade
  package committed installed-disabled
  -> prepare Tool | MCP | OKF | Flow | Skill | UI in dependency order
  -> atomically publish one package capability generation

enable
  persist plan-v4 retained-artifact authorization
  -> prepare exact candidate Grant when required
  -> prepare all selected surfaces in dependency order
  -> atomically publish one package capability generation with cutover evidence
  -> commit and complete the Grant operation

disable
  persist plan-v4 retained-artifact authorization and Grant retirement intent
  -> atomically hide the complete package generation with cutover evidence
  -> commit Grant cutover
  -> drain accepted calls
  -> revoke the exact prior Grant
  -> stop surfaces in reverse dependency order

uninstall
  hide the complete package generation
  -> drain accepted calls
  -> remove receipt-owned surfaces in reverse dependency order
  -> remove the package generation
```

Enable and disable do not replace package bytes. Their
`a3s.use.plugin-operation-plan.v4` envelope contains one exact `Retain`
transition with identical before/after artifact state while binding receipt,
manifest, scope, package-state revision, capability generation, provider,
OKF/secret, and Workspace Grant evidence. This makes visibility and
authorization reviewable without pretending that the immutable artifact
changed.

Every checkpoint has a deterministic SHA-256 idempotency key derived from the
operation, action, generation, sequence, and optional surface identity. The
journal stores only validated non-secret evidence digests and typed error codes;
it never stores credentials, endpoint tokens, Secret values, or
package-authored error text.

## Typed Host Boundaries

The coordinator is orchestration, not a generic plugin protocol. It dispatches
to separate `Send + Sync` host ports for:

- immutable package commit and removal;
- atomic capability publish, hide, and call drain;
- Tool lifecycle;
- MCP lifecycle;
- OKF Knowledge lifecycle;
- A3S Flow lifecycle;
- Skill projection lifecycle; and
- UI projection lifecycle.

The concrete foundation keeps surface semantics distinct:

- a native Tool executable and stdio MCP executable remain static launchers;
- a release-backed Tool Task uses an explicit Runtime Task selection;
- an HTTP Tool and Streamable HTTP MCP use explicit Runtime Services;
- an HTTP MCP Service additionally requires standard initialize evidence;
- a Flow always uses the `a3s-flow` engine; its runtime adapter is typed and
  its Tool/MCP/OKF dependencies are prepared first;
- Skill and UI preparation revalidates immutable content evidence;
- OKF uses stage, persist, promote, and persist, while disable only hides its
  capability and uninstall removes only the retained projection receipt.

No host may translate a Tool into a universal action RPC, treat OKF as an
executable workload, turn `flow.json` into a second workflow engine, or delete
mutable user data as part of normal uninstall. `flow.json` is a design and
deployment document adapted to the same A3S Flow identity and lifecycle.

## Durability and Ownership

The lifecycle journal is bounded, atomically replaced, cross-process locked,
and path-ownership checked. A retry of the same intent resumes the exact next
checkpoint. A different operation for the same scope and package is rejected
until the active record is terminal. Tampered JSON, unknown fields, symlinked
paths, reordered checkpoints, and substituted evidence fail closed.

Detailed Runtime, Knowledge, grant, package, and projection receipts remain the
source of truth for their owned resources. The parent journal records their
validated checkpoint evidence; it does not copy provider credentials or infer
ownership from a running process.

Registry selection is orthogonal to lifecycle ownership. Registry sources are
named and replaceable host configuration, while an installed receipt retains
the immutable source name, URL, TUF root, channel, target, and digest that were
reviewed for that package generation.

## Upgrade Boundary

The version-1 coordinator models candidate commit, preparation, and
publication. The package and Runtime storage layer now preserves exact N and
N+1 receipts concurrently. Snapshot-selected package reads and
generation-specific route leases keep N callable while N+1 is prepared;
pre-cutover package rollback and exact Runtime/package removal cannot overwrite
or retire the other generation.

The package manager now drives those primitives as one durable
dependency-closure upgrade. It binds exact prior and candidate locks, prepares
Add/Replace generations dependency-first, performs one capability cutover,
automatically rolls back unpublished candidates, atomically removes
prior-only routes, and retires replaced or unreferenced prior generations in
reverse order. Operation plan v3 binds both exact locks and host capabilities
v3 gates managed-host acceptance. The grant-aware graph path persists candidate
Grants before package preparation, requires exact Registry snapshot and
generation evidence, restores package and Grant candidates together before
cutover, drains accepted prior calls after cutover, and only then revokes prior
Grants. The standalone manager now binds trusted authority, exact confirmation,
canonical Grant changes, resolved Grants, and signed ceilings into the pending
operation and selects this path whenever the plan carries Grants. Use now also
supplies a reviewed-host provider that preserves the complete external
operation identity, lifetime, scope, policy, lock-bound envelope, and
confirmation across apply and crash replay. The Grant-aware package manager
now retains the complete User or Workspace `PlanScope`; pending replay compares
both kind and ID so a managed workspace cannot be reopened as user authority.
The umbrella and fenced managed-host planners now snapshot the exact Grant
scope/revision, bind canonical impacts with the final host authority, and
invoke this provider without regenerating identity or confirmation. Production
blue/green completion still requires Runtime Service, Gateway, Knowledge, and
projection providers. A3S Use now applies the same authorization provider and
Grant sub-saga to plan-v4 enable/disable operations. The existing Code/Web and
managed-host enablement request v1 still lacks plan/confirmation fields, so
permission-bearing toggle UX requires a versioned adapter.

## Implementation State

Implemented:

- canonical manifest surface inventory shared with reconciliation;
- package-level intent and deterministic dependency schedule;
- durable checkpoint and failure journal with crash-safe replay;
- typed package, capability, Tool, MCP, OKF, Flow, Skill, and UI ports;
- production package commit/removal and atomic capability publish/hide/drain
  adapters over generation-bound receipt schema v3, immutable snapshots, and
  route leases;
- bounded symlink-safe package and Runtime N/N+1 stores, exact snapshot receipt
  resolution, generation-specific leases, pre-cutover rollback, and
  receipt-owned prior-generation removal;
- concrete Runtime, immutable Flow/Skill/UI evidence, and OKF Knowledge adapters;
- a public lifecycle factory for embedding host composition;
- standalone signed-Registry graph install/uninstall with durable lock,
  admission, manifest, and generation evidence;
- standalone signed-Registry graph upgrade with exact prior/candidate locks,
  automatic pre-cutover rollback, reverse prior retirement, and crash replay;
- plan-bound Grant graph install, upgrade, and uninstall entry points with
  candidate-first persistence, exact cutover evidence, durable rollback,
  drain-before-revoke retirement, and generation-stable replay;
- standalone authorization-provider composition, pending-v2 exact confirmation
  and Grant evidence, mandatory Grant-aware path selection for permission-bearing
  operations, authority-stable replay, and tamper/legacy-bypass rejection;
- exact User/Workspace plan-scope propagation across Grant-bearing graph
  install, upgrade, uninstall, and pending replay;
- public canonical host Grant-impact planning plus umbrella and fenced
  managed-host snapshot/revision/policy binding and reviewed-provider
  invocation;
- immutable draft-v3/plan-v4 enablement over one exact retained artifact,
  including derived OKF, secret, provider, visibility, and Workspace Grant
  impact;
- permission-bearing enable Grant prepare before atomic publication and
  disable atomic hide/cutover plus accepted-call drain before exact Grant
  revocation;
- enablement state-v2 and operation-v2 persistence of the plan, confirmation,
  Grant snapshot/change set, resolved Grants, ceilings, and admission time,
  with provider/artifact/receipt revalidation and authorization-free completed
  replay;
- bounded `a3s.use.registry-cutover.v1` evidence embedded in the atomic
  Registry visibility write, with operation-key/request/generation/snapshot
  binding, replay after unrelated Registry mutation, conflict and capacity
  rejection before mutation, and acknowledgement without capability identity
  inflation;
- stable replay evidence for Runtime preparation and removal;
- a content-addressed package fixture containing all six contribution kinds;
  and
- unit and integration coverage for forward preparation, reverse removal,
  optional failure, required failure, root/receipt/snapshot crash replay,
  published-install journal repair, pending-only reverse-uninstall recovery,
  drain timeout, symlink/tamper rejection, legacy compatibility, and
  receipt-owned cleanup.

Remaining before the product can claim complete cognitive-package lifecycle:

- injection of production Runtime Service, Gateway/HTTP MCP, and A3S Knowledge
  hosts;
- a versioned Code/Web/managed-host enablement adapter carrying the immutable
  plan and exact confirmation for permission-bearing toggles, while management
  MCP remains intentionally read-only;
- cross-platform install/use/upgrade/disable/uninstall crash-injection E2E.

## Consequences

Benefits:

- one install, permission review, version, capability cutover, and uninstall
  boundary covers every contribution in the package;
- dependency order and cleanup order cannot drift between hosts;
- restart and concurrent replay converge on deterministic checkpoints; and
- uninstall ownership is explicit enough to preserve user and unrelated data.

Costs:

- every product host must provide typed lifecycle adapters;
- capability publication requires a multi-resource saga; and
- production-provider-backed upgrades remain unavailable through the umbrella
  Plugin Manager until it composes the remaining Service, Gateway, and
  Knowledge evidence.
