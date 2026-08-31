# A3S Use Roadmap

Last updated: 2026-08-29

## Product status

A3S Use is a development preview. The cognitive-package platform has not
shipped a supported product release and is not production-ready.

This roadmap is deliberately release-oriented. Completed internal contracts
or green unit tests are evidence of implementation progress, not a release
claim. A product release requires the cross-repository, cross-platform, supply
chain, recovery, and operational gates in this document.

## Product outcome

A3S Use will be the AI Native Package Manager for arbitrary coding agents and
A3S hosts on Linux, macOS, and Windows. It must install platform-native
capabilities and versioned cognitive packages whose dependency graph can
contribute:

- Tool Tasks and Services;
- standard MCP servers;
- OKF Knowledge bundles;
- A3S Flow workflows;
- Skills; and
- sandboxed UI assets.

One command resolves the complete SemVer closure, verifies every source and
artifact, freezes exact locks, prepares dependencies before dependents,
publishes one capability generation, and retires unused generations in reverse.

## Product decisions

1. **The package is the lifecycle unit; the scoped installation is the
   consistency unit.** A surface cannot be installed, upgraded, enabled,
   disabled, or removed independently of its owning package. A graph mutation
   commits one complete scoped installation generation, never a collection of
   independently authoritative root-package graphs.
2. **There is one current cognitive-package format.** Manifest v3, catalog v3,
   receipt v6, Installation Snapshot v2, Extension Registry snapshot v3,
   capability snapshot v5, extension cursor v3, capability cursor v4, plan v4,
   host protocol v6, managed scope v2, manager toolset v4, pending graph v4,
   pre-lock resolution attempt/diagnostic v1, pre-plan download
   attempt/diagnostic v1, enablement recovery projection v3, and enablement
   operation v3 are the only accepted baseline.
3. **No pre-release compatibility debt.** Superseded schemas, receipts,
   metadata, APIs, and disk state are rejected. The user must clean the
   unsupported state and reinstall. No migration or fallback is inferred.
4. **Package-manager compatibility remains.** SemVer dependencies,
   `requires_use`, OS/target checks, and host/provider capability checks are
   required correctness rules, not legacy branches.
5. **Registries are replaceable host input.** URLs, trust roots, source state,
   and mirror selection are not compiled into packages or the resolver.
6. **Trust evidence is end-to-end.** TUF `custom.a3s` carries a complete
   catalog-v3 record; the exact verified record and provenance survive download,
   planning, lock creation, installation, and receipt loading.
7. **Planning and mutation are separate.** Install, upgrade, uninstall,
   enable, and disable return an immutable reviewed plan before apply.
   `plugin_apply_plan` is the only manager mutation tool.
8. **There is one Flow lifecycle.** `a3s-flow` owns workflow compilation and
   execution. `flow.json` may describe visual design/deployment but cannot
   create another package identity, receipt, or journal.
9. **OKF is a Knowledge surface, not a process.** Publication requires exact
   promoted Knowledge evidence. Only OKF v0.2 is accepted.
10. **Hosts own providers and UX.** Runtime, Gateway, Flow, Knowledge, Code,
    Web, and OS inject their typed providers; Use never hides missing ownership
    with native or source-only fallback.
11. **Hardware adapters reuse MCP.** The MHS research-preview profile adds no
    package surface or private protocol. Use owns package trust and exact
    publication evidence; the hardware control gateway and device safety layer
    own physical authorization, interlocks, and ambiguous-operation
    reconciliation.
12. **Artifacts are global; authority is scoped.** Immutable content-addressed
    bytes may be deduplicated globally, but package selection, dependency
    ownership, enablement, Grants, provider bindings, and capability
    publication belong to an explicit User or Workspace installation.
13. **Agents consume capabilities, not host paths.** The portable agent
    contract is standard MCP plus opaque invocation, artifact, and endpoint
    references. Local executable paths, package roots, credentials, and
    provider internals are never part of the external capability contract.
14. **Management and consumption are separate trust planes.** The Package
    Manager MCP endpoint performs privileged reviewed lifecycle operations.
    The Capability MCP Gateway exposes only the capabilities authorized for a
    consumer and holds exact-generation leases on the consumer's behalf.
15. **Use-owned mutable authority is transactional.** ACL remains the product
    configuration format and immutable artifacts remain files, but related
    installation, operation, Grant, enablement, and publication state commits
    through one transactional Control Store. Sagas remain only around external
    provider effects that cannot join that transaction.
16. **The official package feed is a Registry deployment.** Its target name is
    `A3S-Lab/Use-Registry`, with root submodule path `use-registry/`.
    `a3s-use` owns Registry formats and tooling; the Registry repository owns
    reviewed admission data, signed TUF metadata, and immutable published
    targets. Package source remains in each owning repository.
17. **Domain packages stay outside the package manager.** `a3s-use-science` is
    not an A3S Use workspace member, runtime dependency, CI target, or release
    artifact. A future Science capability remains owned by its independent
    repository and may integrate only as a signed Registry-distributed package.

## Architecture convergence program

Status: release-critical, planned from the 2026-08-28 first-principles review.

The current implementation has strong artifact verification, reviewed plans,
immutable generations, drain, and crash-replay foundations. A0 gives every
accepted mutation one serial order inside its exact installation and rejects
stale publication generations. A1 now has one canonical `InstallationId` and
scopes filesystem state, receipts, Registry and capability snapshots, leases,
backup/restore, and maintenance/mutation locks by that identity. A0 and A1 are
qualified on the declared five-platform CI matrix. The implementation is still
not the target architecture: one `InstallationSnapshot` now owns the desired root set,
unified resolved graph, per-package enablement, and selected-surface
publication intent, but receipts, Grants, bindings, operation checkpoints, and
materialized publication state remain split across several stores, and a non-A3S agent
cannot consume an exact leased capability without learning local execution
details. A checked item elsewhere in this roadmap is implementation evidence;
it does not waive the convergence gates below.

### Target responsibility model

| Boundary | Sole responsibility |
| --- | --- |
| Catalog Source | Locate TUF metadata and targets; a Git or GitHub address is transport shorthand, never trust authority. |
| Artifact Store | Retain verified immutable bytes by digest and deduplicate them globally; it owns no installation or activation authority. |
| Installation Snapshot | Atomically describe one scope generation: requested roots, resolved packages, dependency edges, selected artifacts, enablement, and publication intent. |
| Control Store | Serialize and transact Use-owned mutable authority, operation records, checkpoints, and materialized capability generations. |
| Provider Set | Apply typed Runtime, Gateway, Flow, Knowledge, UI, Web, and OS effects through explicit ports. |
| Capability Index | Materialize an immutable, scope-specific projection only after lifecycle cutover. |
| Capability MCP Gateway | Present portable discovery and invocation to arbitrary agents while the Use Host owns exact-generation leases. |
| Package Manager MCP | Present privileged planning, review, apply, observation, cancellation, and diagnostics to an authorized operator or host. |

Canonical authority keys are explicit:

- installation: `(scope_kind, scope_id)`;
- installed package: `(installation, package_id, package_generation)`;
- capability: `(installed_package, surface_kind, surface_id)`; and
- invocation: an opaque, expiring host reference bound to the exact capability
  generation and consumer context.

`Package`, `CatalogSource`, `ArtifactStore`, `Installation`, `CapabilityIndex`,
and `Provider` are the preferred architecture terms. `Plugin`, `Extension`,
and `Registry` type names with older meanings are removed at the next contract
cutover instead of being preserved as parallel abstractions.

### Dependency order

```text
A0 mutation correctness -> A1 scoped installation -> A2 transactional control
                             |                         `-> A3 agent capability gateway
                             `--------------------------> A4 provider boundary cleanup
A0 mutation correctness ------------------------------> A5 official Registry
A3 agent gateway + A4 provider boundary + A5 Registry -> A6 MHS qualification
```

Work may proceed in parallel only where this graph permits it. In particular,
new surfaces, package types, or host-specific integrations do not take priority
over A0 through A3.

### A0 - Make graph mutation serializable

- [x] Add a deterministic barrier-based regression for the shared-dependency
  race: with `Y -> D` installed, one operation plans removing `Y` and `D`
  while another plans installing `X -> D`; no interleaving may publish `X`
  with `D` absent.
- [x] Introduce one cross-process installation mutation lease keyed by
  `(scope_kind, scope_id)`. Install, upgrade, uninstall, enable, and disable are
  exclusive writers within that domain; another installation remains
  independently available.
- [x] Bind every reviewed mutation to the expected complete installation
  generation and make lifecycle publication an exact compare-and-swap from
  installation generation `G` to `G + 1`.
- [x] After acquiring the mutation lease, revalidate the complete requested
  root set, resolved closure, dependency ownership, selected artifacts, and
  expected publication generation. A stale plan fails without provider or
  filesystem effects.
- [x] Validate live dependents in every retirement branch, including nodes
  previously classified as retained, before hiding a package binding or deleting bytes.
- [x] Advertise exclusive managed-scope mutation in host capabilities only
  when the active coordinator actually enforces it.
- [x] Add multi-process stress and crash-replay tests for different roots with
  shared dependencies on Linux, macOS, and Windows. CI run
  [33158712152](https://github.com/A3S-Lab/Use/actions/runs/33158712152)
  passed the main release gate and native Linux x86_64/ARM64, macOS
  x86_64/ARM64, and Windows x86_64 jobs from exact commit
  `5a78b32f1db1880fe456ced1b76a027981381b52`.

Exit gate: every accepted graph operation has one serial order, and no stale
plan can publish a graph whose dependency closure is incomplete.

Implementation evidence (2026-08-28):

- Each installation's `.installation-mutation.lock` is a cross-process,
  non-Tokio-blocking writer fence held by install, upgrade, uninstall, enable,
  disable, and recovery from live-state inspection through terminal
  persistence. State backup recognizes it as excluded infrastructure rather
  than portable authority.
- The admitted pending graph record or active enablement record is the durable
  mutation owner after process exit. Graph and enablement recovery reject each
  other until the exact owner reaches a terminal state; no second ownership
  file can drift from the operation record.
- Lifecycle publication binds reviewed install, upgrade, uninstall, enable,
  and disable plans to Registry generation `G` and commits only an exact
  `G -> G + 1` cutover. Stale generation, root ownership, dependency closure,
  manifest, artifact, and dependent checks run before authorization or
  lifecycle effects.
- Barrier-based, independent-process, stale-plan, root-adoption, dependent,
  cross-domain interruption, and Registry cutover regressions cover the A0
  invariants. Existing process-exit lifecycle suites continue to cover exact
  replay after cutover and removal checkpoints.

### A1 - Make the scoped installation the authority

- [x] Define `InstallationSnapshot` as the single installed-selection source
  of truth for one explicit User or Workspace scope and monotonically
  increasing generation.
  It contains the desired root set and one resolved graph, rather than one
  authoritative graph file per root package.
- [x] Move expanded package directories into a global content-addressed
  Artifact Store while keeping selections, receipts, package bindings, enablement,
  Grants, provider bindings, and capability publication under an
  `InstallationId`.
- [x] Move verified archive, executable-planning, and presentation-media bytes
  behind a global sharded Blob tier. Keep canonical source observations and
  resumable partials in the Registry source datastore. Blob commit is
  digest-locked, no-clobber, handle-rehashed, and durable before observation
  publication; source prune never deletes global bytes.
- [x] Introduce one global cross-source/cross-installation/cross-operation
  reference inventory before deleting any raw blob or expanded tree.
- [x] Join global references with physical inventory in one guarded collection
  pass and expose checked usage plus bounded quota assessment.
- [x] Enforce an optional durable hard quota with concurrency-safe cross-process
  admission. Policy-disabled publications share the storage boundary;
  policy-enabled publications serialize physical scan, exact projection,
  staging cleanup, and final commit so two writers cannot spend the same
  capacity.
- [x] Add an explicit bounded global digest audit for raw Blobs and expanded
  packages. Reuse the admission fingerprint, hold the exact collection guard,
  report mismatches without mutation, and fail closed on unsafe or unstable
  physical state.
- [x] Require an explicit confirmed garbage-collection policy before deletion.
- [x] Add exact-plan logical corruption quarantine. Re-audit under the exact
  collection guard, require the reviewed canonical plan digest, atomically
  publish a bounded marker, preserve forensic content in place, and fail new
  ordinary Blob and expanded-package access closed.
- [x] Add verified rehydration. Recovery must not silently replace bytes
  underneath an admitted generation and must never derive replacement
  authority from a quarantine marker alone.
- [x] Bind enablement and capability-publication intent to the exact
  `InstallationSnapshot` generation instead of reconciling separate mutable
  authorities.
- [x] Require scope in extension paths, receipts, package bindings, snapshots, and every
  `CapabilityRegistry` constructor. Remove implicit `User/current` projection.
- [x] Bind Runtime, Flow, OKF binding/SQLite, and lifecycle journal stores to
  one constructor-supplied `InstallationId`. Reject a different or invalid
  identity before path derivation, lock acquisition, database creation, or
  evidence mutation.
- [x] Make the same package independently selectable at different versions in
  User and Workspace installations while safely sharing identical artifact
  bytes.
- [x] Replace route strings as identity with the canonical keys above. The ACL
  `route` attribute is now an optional human alias only. Duplicate aliases are
  legal; explicit alias lookup fails as ambiguous instead of selecting an
  arbitrary package. Ownership, leases, cursors, and host surface names use
  scoped package/generation/surface identity.
- [x] Freeze the new contract versions together. Because Use is pre-release,
  reject superseded disk state with a documented clean-reinstall procedure
  instead of maintaining a second live authority model.
- [x] Prove apply, restart, snapshot, leased invocation, upgrade, and uninstall
  for the same package in two scopes, including identical textual scope IDs
  with different scope kinds.

Exit gate: all lifecycle, authorization, and capability queries can be answered
from one exact scoped installation generation plus immutable artifact evidence.

Implementation evidence (2026-08-30; exit gate passed):

- `InstallationId(kind, id)` is the sole installation identity. Its validated
  kind and collision-resistant storage key partition every installation data
  and state root; equal textual IDs in User and Workspace installations do not
  alias.
- Receipt v6, Extension Registry snapshot v3, capability snapshot v5, and the
  extension cursor v3/capability cursor v4 contracts carry the exact
  installation and reject
  cross-installation loading or lease acquisition. The CLI requires explicit
  scope kind and ID for every installation-scoped command.
- Registry source configuration, trust roots, TUF metadata, target observations
  and partials, global artifact blobs, and derivable Flow compilation artifacts
  remain installation-independent inputs. Receipts,
  package bindings, enablement, Grants, provider bindings, capability publication,
  backup/restore, and both maintenance and mutation locks are installation
  scoped. Installation backup rejects the global cache families.
- Provider and lifecycle evidence stores no longer accept a second scope as
  storage authority beneath an already installation-scoped root. Their scope
  fields and nested keys remain integrity evidence and must exactly match the
  constructor-bound installation; cross-installation reads and writes fail
  with `use.installation.identity_mismatch` before filesystem effects.
- Windows publication and SQLite/Flow access use a shared extended-length path
  primitive. Native regressions cover long scoped roots, atomic publication,
  same-text-ID scope-kind isolation, and independent installation locks.
- `a3s.use.installation-snapshot.v2` is the only installed-selection and
  desired-activation authority. It binds the exact `InstallationId`, a
  monotonic installation generation, one resolution host, a sorted desired
  root set, and one unique package selection per ID. Each selection carries
  the immutable lock node, monotonic package state generation, desired
  enablement, and exact selected-surface closure. Root locks are derived;
  conflicting shared selections, disabled dependencies of enabled packages,
  and orphan nodes fail closed. Removing the final root retains an empty next
  generation so authority never resets.
- Installed-selection persistence is one atomic
  `state/installation-snapshot.json` file. The former per-root
  `state/package-graphs/<publisher>/<package>.json` layout is rejected rather
  than migrated, and backup/restore inventories accept only the new snapshot.
- Expanded package content is stored once at
  `data/artifacts/expanded-packages/sha256/<prefix>/<digest>/content`, guarded
  by a cross-process per-digest mutation lock. Two installation registries can
  commit the same digest concurrently and converge on that one complete tree,
  while their receipt, generation, visibility, and lease authority remain
  independent.
- Artifact reads validate the complete owned directory chain and exact digest
  path before package integrity is rechecked. Link/reparse substitution fails
  closed. Interrupted writes use bounded `.artifact-staging-*` trees and are
  reclaimed only while holding the digest lock.
- A global cross-process reachability boundary now separates shared reference
  admission from exclusive maintenance. Raw-target observations, lifecycle
  receipts, applying/rolling-back lifecycle journals, installation snapshots,
  and durable package-graph operations must acquire a store-bound shared
  admission before their subordinate lock and atomic publication. Incomplete
  network downloads release admission until the bounded
  blob-commit/observation transaction. This closes the collector TOCTOU
  prerequisite.
- `ArtifactStore::inspect_inventory` now uses the exact store-bound exclusive
  guard to enumerate both physical tiers deterministically. Its path-free v1
  report distinguishes canonical `content` from abandoned staging, accounts
  regular-file bytes and files, bounds the complete traversal, and rejects
  unknown layout, links/reparse points, and special files. This is physical
  evidence only: it neither infers reachability nor verifies path digests and
  grants no deletion authority.
- `RegistrySourceStore::inspect_artifact_references` now derives the first
  reference-source inventory under that exact exclusive guard. Its path-free
  v1 evidence scans every preserved Registry datastore, including a source no
  longer selected by current config, and reports each canonical blob digest
  with its signed byte expectation. Unknown layouts, missing cache locks,
  links/reparse points, malformed observations, and traversal bounds fail
  closed. This inventory is one input to—not a replacement for—the global
  joined view and its still-open audit and deletion policy.
- `ArtifactReachabilityInspector::inspect_references` now derives the path-free
  `a3s.use.artifact-reference-inventory.v1` view under the same exclusive
  global guard. It validates every installation storage key and identity, then
  aggregates Registry observations, installed selections, current and retained
  receipts, non-cancelled package-graph operations, and applying/rolling-back
  lifecycle journals. Source locks are joined without nesting unrelated locks;
  unknown state, links/reparse points, malformed or unbounded records, and
  conflicting physical expectations fail closed. Missing physical content does
  not erase a durable reference. Whole-installation restore now enters global
  reference admission before its maintenance lock and publication, closing the
  restore-to-collector race.
- `ArtifactReachabilityInspector::inspect_reachability` now joins logical and
  physical evidence while retaining the same exclusive guard. Its path-free
  `a3s.use.artifact-reachability-inventory.v1` output has one canonical row per
  `(kind, digest)`, keeps reference owners separate from physical state,
  classifies only metadata expectation availability/match, and derives checked
  global storage usage. Reference retirement may leave conservative extra
  owners. A bounded quota assessment reports observed excess but deliberately
  provides no deletion authority.
- The global Artifact Store owns optional canonical
  `data/artifacts/storage-quota.acl` policy state. Revision compare-and-swap
  serializes operator changes with publications. Every Blob and expanded-tree
  writer takes reference admission, then the global storage boundary, then its
  digest mutation lock. Without a policy, the storage lock is shared. With a
  policy, one exclusive lock covers bounded physical inventory, exact
  logical-byte/container projection, same-digest staging reclamation, and final
  publication. Real subprocess competition proves that only one of two
  distinct writers can consume one remaining slot. Prepared expanded-package
  byte/file measurements and a bounded exact copy prevent source growth from
  creating unaccounted staging. Tightening below current usage stops growth but
  permits non-worsening replay or cleanup. This correctness-first protocol is
  serialized, not a parallel durable reservation ledger, and grants no deletion
  authority.
- `ArtifactStore::audit_digests` now emits deterministic, path-free
  `a3s.use.artifact-store-digest-audit.v1` evidence while the exact collection
  guard freezes admitted publication. It reuses raw SHA-256 for Blobs and the
  canonical admission fingerprint for expanded packages, hashes sequentially,
  reports complete mismatches instead of mutating them, retains incomplete
  staging evidence without hashing it, and repeats the bounded physical scan
  before returning. Package file opens do not follow the final link/reparse
  component and revalidate the opened measurement. The audit itself grants no
  quarantine, rehydration, or deletion authority.
- `ArtifactStore::plan_quarantine` now derives one canonical path-free plan
  only from a fresh complete digest mismatch. `apply_quarantine` re-audits
  under the same exact collection guard, compares the reviewed plan digest,
  and atomically publishes a no-clobber `quarantine.json` record. Exact replay
  is idempotent; bounded interrupted publication can be retried without
  removing its fail-closed sentinel first. Inventory
  validates marker state without charging it as content or staging, while new
  Blob open/observe/commit and expanded-package validate/commit paths fail
  closed. Canonical content remains untouched as forensic evidence. The marker
  grants neither replacement nor deletion authority.
- `ArtifactStoreMaintenance` now coordinates verified rehydration across the
  facade/extension boundary. Planning and apply keep the exact collection guard
  across a fresh global zero-reference proof and Artifact Store work.
  Candidates must resolve outside the store and match the expected raw or
  canonical expanded digest. Exact path-free v1 plans bind the quarantine
  record, corrupt measurement, replacement measurement, and required reference
  count. Apply reverifies all evidence, publishes canonical prepared/completed
  records, stages under the digest mutation lock, accounts for peak hard-quota
  bytes, and only then reopens access. Bounded interrupted preparation,
  retired-content, and completion states resume; moved or conflicting records
  fail closed. Matching terminal replay validates durable completion and the
  canonical replacement without reopening the external candidate or requiring
  later references to be retired again. The reviewed replacement consumes
  corrupt forensic content, so external evidence retention remains an operator
  decision rather than hidden Artifact Store GC.
- `ArtifactStoreMaintenance` now owns explicit confirmed global garbage
  collection. A policy names 1..=1024 exact Blob or expanded-package digests;
  there is no implicit sweep. Plan and nonterminal apply retain one collection
  guard across the complete reference scan and physical work, require zero
  Registry, installation, receipt, snapshot, graph-operation, and lifecycle-
  operation owners, and bind canonical physical measurements plus ordinary,
  quarantined, or completed-rehydration lifecycle evidence. Apply requires the
  reviewed plan digest, durably publishes a global prepared fence before any
  deletion, atomically renames each container to a deterministic same-shard
  tombstone, rejects links/reparse points and unowned residual entries, and
  resumes bounded partial deletion after restart. While prepared or temporary
  state exists, new reference admission fails closed. Completion is durable and
  exact replay is read-only. Each new plan binds the previous completion digest,
  so an old confirmation cannot delete an identical digest recreated later.
- Upgrade, rollback, and uninstall retire installation-scoped authority but do
  not delete global content. Installation backup excludes global artifacts.
  Unreferenced content remains retained unless an operator explicitly selects
  it and confirms the exact global garbage-collection plan.
- Enable and disable use package-state compare-and-swap inside the next
  Installation Snapshot generation. Receipts, Registry package bindings, and the v3
  enablement file are applied evidence or crash-recovery projections; none can
  independently select desired state. The projection binds the exact
  installation generation and digest, and capability snapshot v5 plus cursor
  v4 expose the same binding before any selected surface can publish.
- Registry publication and accepted-call drain are keyed by
  `(InstallationId, package_id, lifecycle_generation, package_digest,
  manifest_digest)`. Physical locks live under `generation-leases`; capability
  surfaces add their canonical kind and ID. Human aliases are retained only in
  projections, never serve as cursor package keys, and cannot change Tool/MCP
  host names. The cursor revision still commits the complete projection so an
  alias-only projection change cannot evade snapshot consistency.
- `same_package_two_scope_matrix_preserves_exact_authority_and_leased_invocation`
  installs the same signed OKF package into concurrent User and Workspace
  installations with an identical textual ID and one shared Artifact Store.
  Both installations survive Host reconstruction, expose distinct
  `InstallationSnapshot` authority, reject cross-scope snapshot and invocation
  leases, upgrade independently while the other installation's v1 or v2 lease
  remains callable, uninstall independently without advancing the other
  capability cursor, and replay both terminal removals after restart.

### A2 - Consolidate mutable authority in a Control Store

- [ ] Introduce a typed `ControlStore` interface with an initial SQLite/WAL
  backend for Use-owned mutable metadata. Keep ACL configuration and immutable
  package, backup, and projection payloads outside the database.
- [ ] Commit installation generations, reviewed-operation state, lifecycle
  checkpoints, Grants, enablement, provider-binding identity, and capability
  generation metadata in explicit transactions with foreign-key and generation
  constraints.
- [ ] Use an outbox/checkpoint boundary for provider effects. Never hold a
  database transaction across Runtime, Gateway, Flow, filesystem, network, or
  device I/O; reconcile idempotent, rejected, and unknown outcomes explicitly.
- [ ] Derive backup/restore inventory from the Control Store schema and
  registered external payload owners instead of maintaining a second manual
  allowlist that can drift from the state model.
- [ ] Provide deterministic export, offline verification, restore, corruption
  diagnostics, and clean-state initialization tests for the new store.
- [ ] Keep async callers non-blocking through an async database driver or a
  bounded dedicated store executor.

Exit gate: a process failure cannot expose a combination of graph, Grant,
enablement, operation, and capability metadata that never committed together.

Implementation order is fixed by
[ADR-003](docs/adr-003-control-store-transaction-boundary.md). In particular,
the SQLite backend must not become a mirror beside the current JSON stores.
The preparatory extraction of installation-snapshot persistence from shared
package-graph file I/O gives the coordinated cutover an explicit replacement
boundary; it does not complete an A2 checkbox by itself. Production activation
must switch the complete mutable control aggregate and its reachability,
diagnostic, backup, and restore readers together.

The checked-in [coordinated cutover contract](docs/control-store-cutover.md)
freezes that replacement boundary in versioned ACL. A unit test accounts for
every supported installation-state leaf exactly once as legacy authority,
registered external ownership, or excluded operational state; it also pins the
seven consumer groups that must switch without fallback. This closes the
inventory prerequisite only. It neither activates the database nor completes
an A2 checkbox.

The inactive `src/control_store/` kernel now qualifies most of ADR-003 step 2
for a clean installation. Schema v9 binds one exact `InstallationId` and stores
contiguous installation generations, canonical complete reviewed Plan
envelopes, versioned authorization evidence, exact snapshots, full Workspace
Grants, provider bindings, capability candidates, lifecycle checkpoints, and an
idempotent effect outbox behind relational and compare-and-swap constraints.
Plan and authorization bytes are bounded canonical JSON; operation ID, both
digests, action, root package, installation scope, and generation cursors are
derived and revalidated against relational projections after restart, in
offline export verification, and during staged restore. Selected packages now
keep immutable lifecycle generation separate from installation generation and
desired-state generation. A pure projection derives the complete next
snapshot, per-package desired-state generations, and globally monotonic
lifecycle incarnations from the exact reviewed Plan, prior generation, and
bounded committed history. Database commit, offline export verification, and
staged restore all recompute it. Authorization evidence v2 persists only the
exact prior Grant snapshot, reviewed change set, and confirmation facts. The
same projection re-finalizes full target Grants and their independent receipt
revisions, retains unrelated active Grants, and rejects caller-selected Grant
bytes, digests, or revisions. The projection covers all five actions, User and
Workspace installations, multiple roots sharing a dependency, and removal
followed by reinstall without reusing a package identity; callers can no longer
select these fields.
The same projection now derives the complete dynamic provider selection for
every enabled Tool and MCP surface from canonical reviewed Plan evidence and
the exact prior generation. It preserves unrelated package selections, removes
disabled or removed surfaces, and stores canonical provider build, capability,
semantics, and enforcement evidence with a derived digest. Static Flow, OKF,
Skill, and UI host ownership is not fabricated as Runtime selection. The
candidate capability descriptor digest is independently derived from the exact
target snapshot, package lifecycle identities, Grant revisions, and provider
selections. It intentionally contains no endpoint, readiness, compiled
artifact, or Knowledge application claim; those facts can exist only as typed
post-commit observations.
The projection also derives the complete bounded external-effect inventory.
Only work that cannot join the local transaction enters the outbox:
`surface-prepare`, `capability-cutover`, `calls-drain`, `surface-stop`, and
`surface-remove`. Package selection, lifecycle identity, Grants, and reviewed
provider selection are transaction facts, not pseudo provider effects.
Installation and enablement prepare dependency surfaces before dependants and
then cut over. Upgrade prepares the candidate, cuts over, drains prior calls,
and removes prior surfaces in reverse dependency order. Disable and uninstall
cut over before drain and reverse-order retirement. Each intent binds a typed
Capability Index, invocation-lease, Runtime, Flow, Knowledge, Skill, or UI
owner; Runtime effects carry the exact reviewed provider selection. Optional
selected surface preparation may be rejected without blocking cutover, while
its required dependency closure and every teardown remain required. Sequence,
owner, policy, generation, and a domain-separated idempotency key are all
derived rather than accepted from callers. Payload bytes, digest, and relational
projection commit together and survive restart and offline verification. Claim
and completion rebind every payload to the committed generation and reject an
incomplete checkpoint/outbox inventory. Applied outcomes now retain a canonical
owner-specific application descriptor, not a caller-selected success digest.
It binds the exact effect identity to Capability Index or invocation-lease
receipts, the reviewed Runtime selection and portable Task/opaque `gateway:`
Service readiness evidence, or Flow artifact, Knowledge projection, and
Skill/UI content digests. Rejected and unknown outcomes retain diagnostic
evidence only. An applied capability-cutover observation atomically retires the
prior publication, publishes the candidate, and advances the capability cursor
before drain or teardown. A required failure after that boundary remains
effects-pending for explicit same-key reconciliation and cannot roll back the
published generation; terminal completion must follow every observation.
Typed commands prove atomic transition rollback, action/root-state semantics,
terminal replay, pre-cutover required-effect rejection, post-cutover
reconciliation, and explicit reconciliation of unknown or expired claims
across restart. Its bounded canonical export includes the complete aggregate,
is semantically verifiable without the live database, and supports clean-state
staged restore with exact authority round-trip. WAL/full durability, foreign
keys, exact-schema/integrity checks, linked-path rejection, and the 16-entry
bounded worker remain qualified.

Production lifecycle code still does not construct this kernel, and the live
state layout, reachability, diagnostics, backup, and restore orchestration do
not accept it. A private path-free registry contract now freezes all five
external owner identities and their ACL backup policies. It excludes the
global Artifact Store and requires an exact canonical receipt set for the
remaining four owners, bound to one `InstallationId`, Control generation,
registry digest, owner snapshot schemas, manifest/inventory digests, and
bounded file/byte accounting. Deserialized evidence must pass the same
semantic validation before hashing. This removes the duplicated owner-ID and
policy list from the cutover test. A private snapshot session now binds the
canonical Control export digest, generation, installation, and owner-registry
digest under one exclusive maintenance fence, then releases the SQLite
transaction and bounded-executor permit before owner I/O. The Knowledge owner
now produces and offline-verifies a non-overwriting, size-bounded OKF
SQLite/FTS5 archive plus canonical binding/selection inventory evidence. A
missing Knowledge database is represented by a zero-file manifest without
mutating live state, and linked owner roots fail closed. Snapshot creation and
offline verification now also require the exact canonical Control export named
by the binding. Every retained Knowledge lifecycle incarnation must map to its
originating prepare intent and committed OKF bundle. Applied prepare evidence
must match the retained observation and capability projection; removed or
missing applied payload must have a same-incarnation remove effect. Claimed and
unknown outcomes remain explicit reconciliation evidence and never select
desired state. This code remains inactive and does not replace the legacy path
scanner. An offline-verified Knowledge owner snapshot can now stage its exact
SQLite database into a caller-owned directory beneath the target state root,
re-audit the staged database and canonical binding/selection inventory, and
activate only into a clean target while the exact installation-wide exclusive
maintenance fence is held. Activation rejects linked paths, candidate drift,
unowned live-layout entries, unexpected absent-state bytes, an existing live
payload, and a guard for another root. It publishes by atomic rename and
replays an exact completed partial. While the staged attempt and exclusive
guard remain held, it also reconciles the post-publication/pre-result boundary
without creating a second binding authority; the canonical result is path-free
and snapshot-bound.
Snapshot/verification adapters for the other three retained owners,
complete-set orchestration, the full process-exit matrix, production Grant
conversion and effect dispatch, indivisible consumer cutover, and deletion of
legacy mutable stores therefore remain open;
no A2 checkbox is complete yet.
As a cutover prerequisite, lifecycle intent v4 and operation v3 now bind every
checkpoint key to the plan, installation kind and ID, package ID and
generation, action, sequence, kind, and surface. This removes collisions
between graph siblings before their effects enter one installation outbox.

### A3 - Deliver the arbitrary-agent capability plane

- [ ] Ship two standard MCP service entry points: a privileged Package Manager
  endpoint and a lower-authority Capability Gateway endpoint. Do not introduce
  a private Use JSON-RPC protocol.
- [ ] Define portable `CapabilityDescriptor` contracts with opaque
  `InvocationRef`, `ArtifactRef`, and `EndpointRef` values. Remove executable
  paths, package roots, provider release paths, and secrets from external JSON.
- [ ] Let the Use Host resolve an invocation reference and retain the exact
  package-generation lease for the entire call, stream, or server connection;
  drain and retirement operate on those server-side leases.
- [ ] Define consumer profiles. Generic coding agents receive MCP tools,
  resources, and prompts; A3S consumers may negotiate additional Flow, UI, and
  Knowledge metadata without changing the universal contract.
- [ ] Require signed descriptions and JSON input/output schemas for every
  agent-visible Tool. Legacy executable-only Tool Tasks remain host-only until
  a schema-valid descriptor is bound to them.
- [ ] Materialize one immutable Capability Index at lifecycle cutover and emit
  generation-change notifications. Remove fixed-interval full filesystem
  rescans and repeated asset hashing from the normal watch path.
- [ ] Add CLI/service wiring, fail-closed trusted confirmation for management
  apply, bounded authentication, authorization, rate limits, and secret-free
  diagnostics for both endpoints.
- [ ] Prove one-endpoint discovery and invocation from independent Rust,
  TypeScript, and Python clients, including a container or remote client with
  no shared package filesystem. Cover install, live upgrade, prior-generation
  drain, uninstall, restart, and denied cross-scope access.

Exit gate: an arbitrary MCP-capable coding agent can discover and invoke an
authorized package without an A3S SDK, local package path, or duplicated
lifecycle implementation.

### A4 - Invert providers and reduce facade coupling

- [ ] Make the Use Engine own lifecycle coordination, journaling, retries, and
  recovery. Factories inject a typed `ProviderSet` or lifecycle ports; they do
  not construct and return concrete coordinators.
- [ ] Negotiate the actual supported operations, surfaces, protocol versions,
  concurrency guarantees, and provider readiness. Remove default trait methods
  that make unsupported behavior appear supported.
- [ ] Treat Browser, OCR, Box, Runtime, Flow, and UI integrations as provider
  components or ordinary first-party packages. A bundled profile may install
  them for convenience, but the universal engine and capability projection do
  not hardcode their domains.
- [ ] Split the current all-purpose capability binding into consumer catalog,
  invocation binding, and operation diagnostic views so management evidence
  and local provider details cannot leak into agent discovery.
- [ ] Keep A3S Flow and UI as negotiated consumer extensions over the same
  package generation; do not make A3S-specific surfaces mandatory for generic
  agents.
- [ ] Refactor along the target boundaries before creating more repositories:
  contracts, catalog/artifacts, control store, engine, host/gateway, and
  provider adapters. Split oversized files when responsibility moves; do not
  add forwarding facades or duplicate registries.

Exit gate: the core engine runs against deterministic in-memory providers, and
each product host composes only the providers and consumer extensions it owns.

### A5 - Build and operate the official Registry

Decision: rename `A3S-Lab/Use-Packages` to `A3S-Lab/Use-Registry` before the
first production bootstrap root is created. The current repository already
contains admission material, `registry/` TUF state, immutable targets, and
Registry verification; `Use-Packages` incorrectly suggests a package-source
monorepo. The pre-initialization rename avoids creating a second trusted source
identity later.

- [ ] Rename the GitHub repository and root submodule path to `use-registry/`;
  update `.gitmodules`, remotes, documentation, tests, CI, and examples in one
  reviewed change. Do not compile the official URL into the resolver.
- [ ] Treat any preview configuration using the old address as an explicit
  source replacement: re-add the renamed source with its pinned bootstrap-root
  digest. Do not silently turn a GitHub redirect into trust authority.
- [ ] Keep package source, build logic, and releases in owning repositories
  such as MHS. `Use-Registry` accepts reviewed admission records, immutable
  release artifacts, provenance, SBOMs, and signed TUF publication state.
- [ ] Add package-authoring commands for lint, deterministic build/pack,
  manifest and expanded-content digesting, permission review, provenance
  verification, and isolated install tests. These formats and commands are
  versioned by `a3s-use`, not reimplemented by the Registry repository.
- [ ] Add Registry assembly and verification commands that preserve canonical
  catalog metadata, validate the complete staged tree with a released client,
  and produce a reviewable publication delta before signing.
- [ ] Document and exercise offline threshold root custody, delegated targets,
  online snapshot/timestamp custody, expiry monitoring, every-intermediate-root
  rotation, emergency withdrawal, mirror replacement, and rollback recovery.
- [ ] Publish staging and production channels through reviewed GitHub CI with
  no signing key in the repository or package-manager client. Retain witness,
  provenance, SBOM, and prior-generation recovery evidence outside the mutable
  delivery boundary.

Exit gate: a clean machine can add `A3S-Lab/Use-Registry` using an independently
obtained root digest, inspect one exact reviewed plan, install offline from the
verified cache, and recover or roll back using published operator procedures.

### A6 - Qualify MHS as the reference hardware package

- [ ] Keep MHS source in the `crates/mhs` submodule and publish only its signed
  package artifacts and admission records through `Use-Registry`.
- [ ] Express MHS through existing MCP, Flow, Skill, Knowledge, and optional UI
  surfaces. Do not add a hardware-specific package surface or private protocol.
- [ ] Keep the virtual industrial laboratory in its own repository. Its
  simulator connects through the same MHS control-gateway contract used by
  physical adapters and is test infrastructure, not Use runtime code.
- [ ] Model read operations as safe observations and physical mutations as
  explicitly authorized operations with idempotency evidence or an
  `unknown-outcome` state. Never retry an ambiguous device mutation implicitly.
- [ ] Prove least-authority Grants, gateway health, dependency publication,
  exact-generation lease/drain, reconnect, and reconciliation against the
  virtual laboratory before enabling any physical adapter profile.
- [ ] Run the same signed package from a generic MCP client and A3S Code:
  install, discover, observe, invoke a simulated mutation, interrupt/reconcile,
  upgrade without mixed generations, uninstall, and verify no Registry
  bindings, Gateway routes, Grants, processes, or projections remain.
- [ ] Keep the adapter labeled research preview until the external MHS profile
  is stable and the package passes its published conformance and hardware
  safety-gateway requirements.

Exit gate: MHS demonstrates the complete Registry-to-agent capability path in
the separate virtual laboratory without granting Use direct physical-device
authority.

The protocol table below describes the currently implemented preview. A2 and
A3 will intentionally supersede affected contracts in one coordinated cutover;
version numbers are assigned only after their invariants and negative fixtures
are frozen.

## Current protocol baseline

| Contract | Accepted version |
| --- | --- |
| Cognitive-package manifest | schema version 3 |
| Signed catalog record | `a3s.use.plugin-catalog.v3` |
| Installed receipt | schema version 6 |
| Package lock | `a3s.use.plugin-package-lock.v1` |
| Installation snapshot | `a3s.use.installation-snapshot.v2` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v6`, protocol 6 |
| Host managed scope | `a3s.use.plugin-managed-scope.v2` |
| Host operation observation | `a3s.use.plugin-host-operation-observation-request/result.v1` |
| Host operation watch | `a3s.use.plugin-host-operation-watch-request.v1` |
| Host cancellation | `a3s.use.plugin-host-cancel-request/result.v1` |
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v4` |
| Pending package graph | `a3s.use.pending-package-graph-operation.v4` |
| Pre-lock resolution attempt | `a3s.use.plugin-resolution-attempt.v1` |
| Pre-plan download attempt | `a3s.use.plugin-download-attempt.v1` |
| Lifecycle diagnostic | `a3s.use.plugin-lifecycle-diagnostic.v1` |
| Operation diagnostic | `a3s.use.plugin-operation-diagnostic.v1` |
| Operation history | `a3s.use.plugin-operation-history.v1` / `a3s.use.plugin-operation-history-diagnostic.v1` |
| Pre-lock resolution diagnostic | `a3s.use.plugin-resolution-attempt-diagnostic.v1` |
| Pre-plan download diagnostic | `a3s.use.plugin-download-attempt-diagnostic.v1` |
| Enablement recovery projection | `a3s.use.cognitive-package-enablement-projection.v3` |
| Enablement operation | `a3s.use.cognitive-package-enablement-operation.v3` |
| Runtime Task binding | `a3s.use.runtime-task-binding.v4` |
| Runtime Service provisioning | `a3s.use.runtime-service-provisioning.v1` |
| Runtime Service binding | `a3s.use.runtime-service-binding.v3` |
| Extension Registry snapshot | schema version 3 |
| Capability snapshot | schema version 5 |
| Capability snapshot cursor | `a3s.use.capability-snapshot-cursor.v4` |
| Extension snapshot cursor | `a3s.use.extension-snapshot-cursor.v3` |
| Coordinated Use state backup | `a3s.use.state-backup.v2` |
| Coordinated Use state backup retention plan | `a3s.use.state-backup-retention-plan.v2` |
| Coordinated Use state backup retention result | `a3s.use.state-backup-retention-result.v2` |
| Coordinated Use state restore plan | `a3s.use.state-restore-plan.v1` |
| Coordinated Use state restore operation | `a3s.use.state-restore-operation.v1` |
| Coordinated Use state restore result | `a3s.use.state-restore-result.v1` |
| Coordinated Use state restore diagnostic | `a3s.use.state-restore-diagnostic.v1` |
| OKF Knowledge backup | `a3s.use.okf-knowledge-backup.v1` |
| OKF Knowledge backup retention plan | `a3s.use.okf-knowledge-backup-retention-plan.v1` |
| OKF Knowledge backup retention result | `a3s.use.okf-knowledge-backup-retention-result.v1` |
| OKF Knowledge restore plan | `a3s.use.okf-knowledge-restore-plan.v2` |
| OKF Knowledge restore operation | `a3s.use.okf-knowledge-restore-operation.v2` |
| OKF Knowledge restore result | `a3s.use.okf-knowledge-restore-result.v2` |
| OKF Knowledge restore diagnostic | `a3s.use.okf-knowledge-restore-diagnostic.v2` |

Negative fixtures for superseded inputs remain only to prove fail-closed
rejection. They are not supported decode paths.

## Implemented baseline

### Package and catalog contracts

- [x] ACL manifest v3 with named Tool, MCP, OKF, Flow, Skill, and UI surfaces.
- [x] Required bounded UTF-8 `README.md`, package path validation, archive
  bounds, shared Unix symlink/Windows reparse-point rejection, and content
  fingerprinting.
- [x] Canonical catalog-v3 record with complete surface inventory, package and
  manifest digests, planning target, provider requirements, and permission
  ceiling.
- [x] Complete current TUF metadata validation and cache verified archives and
  signed planning targets by SHA-256.
- [x] Expose one state-free bootstrap-root evidence inspector and one bounded,
  digest-pinned, immutable admission API for managed hosts. They share the
  exact digest/version/size decoder and public size bound; admitted bytes still
  require the ordinary complete TUF refresh before catalog evidence is trusted.
  Standalone root imports share that same public size bound.
- [x] Provide one strict public-Internet Registry transport policy for managed
  hosts: HTTPS only, per-request DNS validation and address pinning, no ambient
  proxy or automatic redirects, per-hop validation of bounded target redirects,
  and fail-closed denial of non-public address space across metadata,
  bootstrap-root, planning-target, and package-target downloads.
- [x] Persist up to 64 named Registry sources in canonical ACL with one enabled
  default, revision-bound confirmed authority changes, managed digest-bound
  root import, and source-identity-isolated TUF/cache datastores. Install and
  upgrade consume that same enabled set for cross-Registry dependencies.
- [x] Accept a typed GitHub `owner/repository` Registry address with bounded
  ref/path overrides while retaining the ordinary mandatory TUF bootstrap root;
  never clone or execute Git repository content on the client.
- [x] Support explicit zero-network install and upgrade from only unexpired,
  revalidated cached metadata and targets; reject missing or tampered evidence
  without implicit online-to-cache fallback.
- [x] Registry/TUF receipts require the exact verified catalog record and
  source provenance.

### Resolution and planning

- [x] Bounded SemVer dependency resolution with deterministic install/removal
  order, cycle detection, host/target checks, and cross-source ambiguity
  rejection.
- [x] Exact package locks bind every selected version, dependency edge,
  artifact digest, Registry identity, and TUF role version.
- [x] Operation plan v4 binds complete impact, current state, confirmation,
  host/provider evidence, and package transitions.
- [x] Upgrade binds both prior and candidate locks and classifies
  Add/Replace/Remove/Retain.
- [x] Reviewed enablement planning returns either an exact plan-v4 envelope or
  terminal `NoChange`.

### Package lifecycle

- [x] Dependency-forward prepare and one atomic graph publication.
- [x] Reverse uninstall and exact dependency garbage collection.
- [x] Immutable N/N+1 package roots and receipt-owned retirement.
- [x] Durable Registry cutover replay, lifecycle journals, operation locking,
  exact terminal result replay, and tamper rejection.
- [x] Package-scoped latest/previous lifecycle checkpoint diagnostics with
  bounded status, digest, timing, failure-code, and rollback evidence; output
  excludes idempotency keys, credentials, tokens, secret values, and
  package-authored error text.
- [x] Both applying and rolling-back journals retain exclusive operation
  ownership until terminal completion.
- [x] Cutover-aware host traits only; no fallback publication API.
- [x] Prior-generation retirement fails unless the graph package binding is already
  absent.
- [x] Hosts can acquire an exact currently published lifecycle generation by
  package, manifest, and generation identity; the lease participates in the
  same accepted-call drain as alias dispatch.
- [x] Hosts can derive a typed capability/Registry cursor and atomically lease
  every callable package generation in canonical order. Publication is
  rechecked after the complete batch is held; stale, hidden, mixed,
  digest-mismatched, contended, or non-lifecycle package bindings fail closed without a
  partial lease.
- [x] Missing exact recovery evidence fails closed instead of reconstructing
  state heuristically.

### Surfaces and authorization

- [x] Typed lifecycle hosts for Tool, MCP, OKF, Flow, Skill, and UI.
- [x] Standalone executable Task, stdio MCP, immutable Skill/UI, and
  SQLite/FTS5 OKF Knowledge composition.
- [x] Scope-kind-isolated OKF storage policy with atomic receipt-accounted byte
  and projection quotas, per-surface generation bounds, global tombstone
  pruning, SQLite/WAL compaction, and exact-scope usage diagnostics.
- [x] Scope-local OKF SQLite/receipt/FTS integrity audit, non-overwriting
  digest-bound database backup and offline verification, exact-scope bounded
  oldest-first rotation with canonical plan confirmation, plus repair limited
  to rebuilding the derived search index from validated documents.
  Authority-bound restore binds exact package/lifecycle/Registry/Grant
  authority, an exact-subset binding inventory, and live main/WAL/SHM
  evidence; it can restore missing binding files without overwriting
  conflicts, preserves prior files, and converges a durable six-state journal
  after interruption.
- [x] Real `a3s-flow` Native TypeScript preflight and exact-generation binding
  in injected hosts and the explicitly configured standalone CLI lifecycle.
- [x] Self-contained release-backed Runtime Task binding and exact-generation
  dispatch with receipt-owned provider reconnection, restart reconstruction,
  stale-generation rejection, bounded output cleanup, and Registry lease drain.
- [x] Capability snapshot v5 projection for exact installation/package/generation
  matched release-backed Runtime Tool Task bindings.
- [x] Capability snapshot v5 projection for every exact extension MCP surface,
  preserving canonical IDs, collision-resistant host names, activation,
  package/file identity, bounded package-local stdio launch evidence, and
  credential-free managed HTTP binding evidence.
- [x] Research-preview MHS adapter profile and fixture using only MCP, Flow,
  Skill, and UI surfaces, with a canonical least-authority permission ceiling,
  fail-closed gateway/dependency publication, and explicit unknown-outcome
  semantics for physical mutations. This does not claim MHS conformance.
- [x] Workspace Grant proposal/change/resolution/ceiling binding.
- [x] Candidate Grant persistence before prepare, cutover checkpointing,
  drain-before-revoke, and joint pre-cutover rollback.
- [x] Manager MCP toolset v4 with explicit install-time Registry selection,
  read-only planning, and one apply tool.
- [x] Shared typed `PluginManagerService` over the production Host Manager,
  with deterministic request replay, Registry-bound catalog cursors, stable
  installed-state pagination, exact/ranged SemVer selection, durable reviewed
  plan reopening, and all ten frozen operations. Its standard MCP adapter
  derives names, schemas, and annotations from toolset v4 and obtains apply
  confirmation only from an injected trusted host provider.
- [x] Production `CognitivePackageHostManager` for one exact managed-scope
  fence, with durable request/operation binding, selected-surface planning,
  digest-only graph and enablement apply, restart replay, provenance
  revalidation, zero-network install/upgrade apply from the exact planning
  cache, and expired-plan recovery only after Use-owned durable admission or
  completion evidence. Host protocol v6 binds an explicit User or Workspace
  scope kind, observes exact operations from durable Host, graph, enablement,
  and lifecycle evidence, long-polls a status revision, and persists
  explicit-user cancellation only before durable admission.

### Validation and documentation

- [x] Exact `a3s-flow` `1.0.0-rc.1` candidate qualification for the extension
  facade through the complete all-feature workspace gate.
- [x] Canonical fixtures and digest goldens for the current contract line.
- [x] Unit, integration, remote Registry, crash-replay, grant, Flow, OKF, and
  CLI tests in the Use workspace.
- [x] Test-binary subprocess exit after a durable host effect and before
  receipt persistence at every canonical install, upgrade, enable, disable,
  and uninstall checkpoint, with exact-key recovery, one durable effect, and
  no host call on terminal replay.
- [x] Test-binary subprocess exit after a grant-bearing install, upgrade, or
  uninstall graph publish/hide effect but before package publication receipts
  and Grant cutover evidence, with exact-key recovery, one graph effect,
  completed package/Grant journals, and no publication on terminal replay.
  Three externally killed managed-host processes also cover five-node install,
  upgrade, and uninstall after Registry publish/hide but before one dependency
  receipt and Grant cutover/retirement. Restart forbids reauthorization,
  performs no network request, preserves the exact candidate Grant, retires
  only the bound prior Grant, and completes without another Registry generation.
  Five real `CognitivePackageHostManager` protocol children also cover every
  reviewed mutation after the Registry is taken offline. Install, upgrade, and
  uninstall are killed at the five-node graph publish/hide boundaries; disable
  is killed after root hide and Grant cutover while accepted-call drain blocks;
  enable is killed after publication while its candidate Grant is still
  prepared. Digest-only apply reuses the durable reviewed plan and
  confirmation; install and upgrade also use only the planning cache. Recovery
  completes lifecycle/Grant journals, converges the exact candidate/prior
  Grants or enablement regrant/revocation without another Registry generation,
  and persists a replayable terminal Host outcome.
- [x] Test-binary subprocess exit after all 14 Grant Store durable checkpoints
  in the canonical two-candidate/two-retirement lifecycle across forward
  prepare, cutover/retirement, and pre-cutover rollback, with exact
  candidate/prior convergence and terminal journal replay.
- [x] Real `a3s-use` process exit after a nine-node install Registry publish
  cutover but before dependency journal and installation snapshot completion,
  followed by zero-network exact replay with one complete visible closure and no
  capability-generation inflation; and after an uninstall Registry hide
  cutover but before its package hide receipt, followed by exact-plan restart,
  an observed accepted-call drain, and physical removal. Missing generation
  state without the exact durable cutover is rejected without changing graph,
  pending-plan, or Registry evidence.
- [x] Signed standalone CLI Flow/OKF/Skill/UI install, process-restart
  observation, exact upgrade, uninstall, failed-preflight non-publication, and
  repaired exact replay coverage on Unix and Windows x86_64. The OKF fixture
  also exercises audit, backup, offline verification, confirmed FTS repair, and
  reviewed restore. Test subprocess exits cover the active-marker handoff,
  every restore journal state, and partial main/WAL/SHM movement; replay works
  from the durable candidate without the external backup and terminal replay
  does not rewrite state. A path-free `restore-status` projection reports the
  global active phase and bounded scope history/capacity at every exit window
  without changing restore or database evidence.
- [x] Linux CI, macOS workspace tests, and Windows preview compile/facade plus
  signed Registry, dependency-graph, Grant, Flow, OKF lifecycle, and
  killed-process cutover-replay gates.
- [x] GitHub Pages documentation application and bilingual documentation.

## Remaining development plan

### M1 — Complete managed-host provider composition

Status: in progress

- [x] Persist exact Service provisioning before Runtime apply, advance it
  monotonically through Runtime-applied and Gateway-ready evidence, reconcile
  the final-binding commit window, and remove interrupted candidates without
  creating duplicate Runtime effects.
- [x] Exit real test subprocesses at all six nested Service provisioning
  windows for Tool and HTTP MCP, then prove exact-key recovery, one Runtime
  and Gateway effect, terminal replay, and drain/remove without residue.
- [ ] Compose production Runtime Service providers in A3S Code and managed
  hosts with exact plan-time and apply-time evidence.
- [x] Consume the reviewed Runtime Task projection in Code CLI/TUI and agent
  tool discovery, then route invocation through the leased Use dispatcher. A3S
  CLI `main` commit `e77d318beba3cba7f193da8d83bb9ac5c46fc0f7`
  extends the resident TUI projection to scoped Code Exec: the one-shot host
  freezes provider-qualified reviewed Tasks with exact count/digest evidence,
  retains the same trusted Plugin Manager through Session teardown, and invokes
  through the existing exact-generation dispatcher and Use lease. A missing
  named provider omits only its Task; MCP, Knowledge, Flow, and Plugin Manager
  presentation surfaces remain outside the scoped host. CI run
  [32797862154](https://github.com/A3S-Lab/CLI/actions/runs/32797862154)
  passed the main all-target check, Linux release sandbox, Linux ARM64 local
  inference, and native macOS/Windows cross-platform jobs.
- [x] Compose HTTP/streamable MCP through Gateway with health, drain, and
  exact-generation retirement. A3S CLI `main` commit
  `563e7e139740e845369f9102a2d47026733797a8` qualifies four real Linux Tool
  and MCP processes across retained N/N+1 routing, Gateway and lifecycle-host
  restart, stop/drain, exact receipt-owned removal, and zero residual routes,
  Runtime units, receipts, or PIDs. CI run
  [32739505482](https://github.com/A3S-Lab/CLI/actions/runs/32739505482)
  passed the full Linux all-target suite, release sandbox, Linux ARM64 local
  inference, and macOS/Windows cross-platform jobs with the exact merged Box
  and Gateway revisions.
- [x] Complete bounded storage quota, projection retention, tombstone garbage
  collection, and physical compaction in the standalone Knowledge backend.
- [x] Complete managed A3S Code Knowledge Workspace/session carriers and prove
  leased prior-generation query semantics through those hosts.
- [x] Validate UI entry points and exact asset digests during package lifecycle
  changes, and clear receipt-owned UI state on true surface removal.
- [ ] Complete reviewed UI backend bindings and sandboxed rendering in
  supported hosts. Current CLI and TUI hosts remain static-integrity-only.
- [x] Prove that every required surface remains unpublished when its owner or
  evidence is missing.

Exit gate: a six-surface signed package completes install, enable, upgrade,
disable, and uninstall through the same reviewed plan/apply service in each
supported managed host.

### M2 — Finish A3S Code TUI hot-plug qualification

Status: in progress

- [x] Converge the standalone CLI on the shared `PluginManagerService` without
  a second catalog, plan, confirmation, or mutation implementation. The exact
  manager-v4 read and planning inventory is available under `plugin`; apply
  reopens a durable operation ID plus plan digest, requires explicit `--yes`,
  and uses the verified cache without network access. Compatibility install,
  upgrade, and uninstall fields remain intact.
- [x] Migrate the A3S Code TUI to that service and compose the standard manager
  MCP in Code. CLI, TUI `/packages`, and the exact ten-tool manager-v4 MCP now
  reuse one host-owned service without a second plan, confirmation, or mutation
  path at A3S CLI commit `ce1240891d6926c132aed8212efabaf6c925f4db`.
- [x] Verify TUI `/packages` and CLI output show the exact plan, package graph,
  source, permission ceiling, and confirmation boundary. A3S CLI `main` commit
  `bef7c913cbefba62638b37f91ce9263f4db2ffbb` derives one deterministic,
  read-only human review from the immutable Manager envelope while preserving
  the standard machine JSON contracts. CLI and TUI show exact plan/lock,
  source, transition, permission, provider/impact/state, and confirmation
  evidence; the TUI scrolls every wrapped line before exact apply. CI run
  [32786647662](https://github.com/A3S-Lab/CLI/actions/runs/32786647662)
  passed the main all-target check, Linux release sandbox, Linux ARM64 local
  inference, and macOS/Windows cross-platform jobs.
- [x] Prove install → invoke → exact-generation upgrade → invoke → uninstall
  → process restart for Tool, MCP, Flow, Skill, UI, and OKF. The complete
  signed six-surface Host Manager matrix now exercises native Tool and stdio
  MCP launchers, Flow preflight, Skill/UI integrity, and an exact OKF lease
  across install/replay, upgrade/replay, and uninstall/replay.
- [x] Prove watcher resumption, no duplicate side effects, and path-free
  retained history after process restart. A real Host-protocol install now
  carries its pre-restart status revision across an externally killed apply
  and offline recovery process, observes exactly one completed revision, and
  then times out without changing the terminal revision. Recovery retains one
  Registry generation, exact apply replay performs no authorization or
  publication side effect, and scoped retained history excludes filesystem
  paths, Registry URLs, Host request IDs, and idempotency material.
- [x] Run the same scenarios for User and Workspace scope and reject scope-kind
  substitution under the same textual ID. Permission-free Skill and
  permission-bearing Tool matrices cover the individual scope fences, and the
  complete six-surface Host Manager matrix now covers both User and Workspace
  install/restart, upgrade/restart, uninstall/restart, exact Tool/MCP/OKF
  observations, and plan/apply/operation-observation scope-fence rejection.

Exit gate: Code CLI/TUI and agent tools produce the same plan digest and
terminal operation result for the same request.

### M3 — Distributed Flow and OS integration

Status: pending

- [ ] Bind package-owned Flow identity to distributed scheduling, resumption,
  cancellation, and observation without a second `flow.json` lifecycle.
- [ ] Prove local Code and remote OS targets consume the same source/export,
  package generation, dependency edges, and authorization.
- [ ] Define and test failure/retry ownership across Use, Flow, Runtime, and
  remote target boundaries.

Exit gate: remote execution changes placement only; package receipts, locks,
and lifecycle journals remain Use-owned and singular.

### M4 — Cross-platform real-process release matrix

Status: in progress

- [x] Run full workspace and real-process package lifecycle tests on Linux
  x86_64/arm64 and macOS arm64/x86_64. Native CI run
  [32604181662](https://github.com/A3S-Lab/Use/actions/runs/32604181662)
  passed the then-current Use-owned workspace suite on all four targets from exact
  `main` commit `40bc5593cbf58ca2da171d85ba578c2d6bd911c8` while the
  matching Windows job and general release gates also passed.
- [x] Run signed Registry trust/lock, dependency-graph install/upgrade/uninstall,
  Grant, standalone Flow preflight/lifecycle, and OKF cutover scenarios through
  real `a3s-use` processes on Windows x86_64, including killed-process replay
  of removed-dependency cleanup without capability-generation inflation.
- [x] Run the complete current Use-owned workspace suite on Windows x86_64
  and reject directory junctions across package, Registry/cache, Grant,
  lifecycle, Runtime, Flow, and Knowledge trust boundaries. Flow Runtime
  qualification now also exercises exact-generation retention, artifact
  substitution, tampered or moved binding records, same-text scope-kind
  isolation, and directory-junction rejection on Windows. Shared native link
  qualification also covers the maintenance lock, target-cache partials and
  observations, retained lifecycle receipts, package graph and diagnostic
  stores, enablement locks, Runtime and lifecycle records, whole-state backup
  and restore paths, and OKF database, binding, backup, and restore paths with
  real Windows directory junctions. Native Windows tests additionally prove
  single-package and graph cutover-capacity rejection happens before lifecycle
  receipt replacement, and that Box CLI delegation preserves arguments,
  output, and exit status through a `.cmd` component.
- [x] Run the Runtime Service provisioning subprocess-exit matrix for Tool and
  HTTP MCP on the configured platform CI jobs. Real managed-provider and CLI
  process-kill qualification remains open.
- [ ] Expand the remaining Windows gate to the complete filesystem, Runtime,
  MCP, watcher, failure-injection, and crash-recovery matrix. All production
  temporary-file publications for Registry state/cache, Workspace Grants,
  package and Host records, lifecycle, Runtime, Flow, Knowledge, enablement,
  backup, restore, and diagnostics now share one bounded Windows retry for
  transient access, sharing, and lock violations while preserving replace
  versus no-clobber semantics. Restore journal evidence and Knowledge recovery
  preserve their source after a bounded rename failure; whole-state restore
  candidates preserve reviewed file attributes, and lifecycle-generation plus
  restore-history directory moves use the same retry bound. A released
  exclusive file or directory lock converges atomically, and a persistent
  replacement lock leaves the old target intact. Resumable Registry partials
  use one final-component no-follow handle from discovery through append,
  checkpoint, verification, and copying into the global Blob tier. Commit
  rehashes while copying, publishes without clobber under the digest lock,
  reopens the final blob without following it, and retains that exact handle
  through staging. The source observation is durable only after the blob, and
  partial cleanup is last. A live Windows partial or blob handle permits readers
  but denies external writes, removal, and replacement, while Unix commit and
  staging remain bound to their held handles after path replacement.
  Windows-native scanner tests prove a transient no-delete-share handle
  converges within the two-second cleanup bound. If cleanup stays locked after
  publication, the next transaction rehashes the durable blob and removes the
  redundant complete partial without a network transfer. Invalid-partial
  cleanup and source deletion of stale files, partials, and observations use the
  same bounded blocking retry; source deletion never removes the global blob.
  Native tests prove transient scanner release converges for each cleanup path;
  a persistent selected-target lock stops at two seconds, preserves that entry,
  and a later prune rescans and finishes after any earlier durable deletions.
  Recursive cleanup of bounded abandoned `.artifact-staging-*` trees plus
  lifecycle receipt deletion uses the same retry without blocking Tokio.
  Native tests prove transient receipt and nested-staging contention lets the
  same authority retirement or artifact commit finish. A persistent reader of
  a complete global artifact never delays uninstall because scoped retirement
  does not delete shared bytes. Native tests also hold the active
  artifact-staging directory at its atomic content rename: transient contention
  lets the same commit finish, while persistent contention fails before receipt
  or Registry-snapshot mutation, retains residual staging, and permits exact
  commit replay after release. Selected upgrade-receipt replacement has the
  same native scanner qualification. Transient contention completes the same
  upgrade; persistent contention stops at the bound, retains the valid global
  candidate artifact, removes its retained-receipt candidate, preserves the
  byte-exact prior receipt and published generation, leaves no temporary
  receipt, and permits exact replay after release. Reboot
  recovery, antivirus contention beyond these exact blob publication,
  source-cache removal, active package-commit, upgrade-receipt replacement, and
  lifecycle-removal boundaries, product-host contention, and the remaining
  platform scenarios stay open.
- [x] Test real-process uninstall interruption between durable Registry cutover
  and its package receipt, then hold the prior generation lease through restart
  to prove drain-before-removal and exact generation replay.
- [ ] Complete the interrupted download, archive extraction, graph/Grant
  cutover, drain, removal, process crash, reboot, remaining antivirus contention
  outside blob publication, source-cache removal, active package commit,
  upgrade-receipt replacement, and lifecycle removal, and reparse-point
  replacement matrix. A real `a3s-use` process-kill test now
  proves digest-bound target download resume without partial publication. A
  second real-process test kills installation while a verified high-entry
  archive is being extracted, proves no receipt, installation snapshot,
  pending operation, or package root was published, and completes an exact
  zero-network retry from
  the verified cache. A third real-process test kills the following immutable
  package copy after its pending plan and applying journal are durable, then
  proves retry reclaims the actual bounded artifact-staging tree, publishes
  only the exact generation once, and removes the pending operation. Package
  commit also rejects staging or Artifact Store ancestor links/reparse points.
  A fourth integration test proves uninstall retires scoped receipt and package-binding
  authority without deleting or waiting on global artifact bytes. A fifth
  real-process test kills a nine-node install after the complete atomic graph
  is visible but before one dependency journal and the installation snapshot
  complete; offline replay uses the retained cutover, performs no network
  request, completes every journal, and keeps the original Registry generation.
  Sixth through eighth externally killed managed-host tests cover install,
  upgrade, and uninstall publish/hide boundaries with a permission-bearing root
  and four dependencies while the Grant journal is still prepared. Replay is
  rejected if it requests authorization again or performs a network request;
  otherwise it preserves the exact candidate Grant, retires only the bound
  prior Grant, and completes package and Grant journals without generation
  inflation. Ninth through thirteenth externally killed
  `CognitivePackageHostManager` protocol applies cover all five reviewed
  mutations after the Registry server is stopped. Install, upgrade, and
  uninstall use the five-node graph publish/hide boundaries; disable stops
  after root hide and Grant cutover while drain is blocked; enable stops after
  publication with its candidate Grant prepared. Recovery consumes the durable
  reviewed request and confirmation, uses only the exact planning cache for
  install/upgrade, converges the exact candidate/prior Grant or enablement
  regrant/revocation, completes drain, and persists the terminal Host outcome
  without reauthorization or generation inflation.
  Actual Code/Runtime product-host, platform, reboot, contention, and
  replacement-race qualification stays open.
- [x] Verify release archives install and run without repository-local paths.
  Non-publishing qualification run
  [32600882148](https://github.com/A3S-Lab/Use/actions/runs/32600882148)
  scanned all five target archives for checkout paths and ran the installed
  native executables with isolated homes and working directories from exact
  `main` commit `e3d5f955a63cc136dbb07e9419a32760328df320`.

Exit gate: every supported target passes the same signed six-surface package
and failure-injection scenarios.

### M5 — Production supply chain and operations

Status: in progress

- [ ] Initialize and operate `A3S-Lab/Use-Registry` as the documented official
  Registry with root rotation, expiry, mirror replacement, offline recovery,
  and incident procedures. Complete architecture track A5 before publishing
  its first production bootstrap root.
- [x] Provide durable Registry source add/list/replace/default/enable/disable/
  remove operations; preserve immutable receipts and identity-bound evidence
  across replacement and exact-provenance restoration.
- [x] Persist verified archives and planning targets in a content-addressed
  cache and support explicit fail-closed offline install/upgrade.
- [x] Enforce typed per-Registry byte/entry limits, minimum free-space
  admission, oldest-first retention, stale-write cleanup, zero-network usage,
  and confirmed garbage collection.
- [x] Add bounded, integrity-preserving download resume with durable
  digest-bound partials, exact HTTP range validation, full-file verification,
  and cache-policy/GC accounting.
- [x] Publish checksum-verifying Linux/macOS and Windows installers from the
  release workflow with HTTPS downgrade prevention, safe extraction,
  packaged OCR/Skill binding, versioned atomic activation, and tamper/conflict
  tests.
- [x] Deterministically serialize multi-platform archives and publish one SPDX
  SBOM per platform, GitHub OIDC provenance/SBOM attestations, and a locally
  reverified keyless Sigstore bundle for complete checksum evidence from one
  workflow with pinned Actions and release tools.
- [x] Make both platform installers require Cosign, authenticate the checksum
  manifest against the exact tag workflow identity and GitHub OIDC issuer
  before archive download, fail closed on invalid evidence, and retain the
  verified manifest and bundle with the installed version.
- [x] Pass byte-for-byte independent rebuilds for every shipped native
  executable on all five targets. The tagged `v0.3.2` attempt exposed drift on
  four targets and did not publish a Release. After deterministic symbol
  stripping and platform linker metadata controls were applied,
  non-publishing qualification run
  [32600882148](https://github.com/A3S-Lab/Use/actions/runs/32600882148)
  rebuilt every shipped native executable without a build cache and
  byte-matched all five primary archives from exact `main` commit
  `e3d5f955a63cc136dbb07e9419a32760328df320`.
- [ ] Add an externally operated witness for the complete staged tree and final
  archive digest, and retain verification evidence outside the Release asset
  trust boundary.
- [ ] Define storage retention, quota, garbage collection, backup, and repair
  procedures for packages, cutover evidence, Grants, Flow history, UI state,
  and OKF projections. A deterministic `a3s.use.state-backup.v2` exact-installation
  inventory now snapshots all allowlisted installation-owned families under its
  exclusive maintenance fence, binds the installation, Registry generation/snapshot,
  and installed receipt digests, excludes locks and global Registry/TUF/Flow
  caches, rejects nonterminal or unknown state, and
  verifies every payload offline without extraction. Signed-package real-process
  coverage proves path-free inventory and zero-network verification. Coordinated
  retention now verifies every managed whole-install archive under one external
  directory lock, returns a path-free oldest-first canonical plan, rejects stale
  plans or changed candidates, and removes nothing without the exact plan digest
  and explicit confirmation while retaining at least two recovery generations.
  Scope-local OKF database audit, verified backup and exact-plan rotation,
  derived-index repair, and authority-bound database plus missing-binding
  restore are also implemented. Binding recovery accepts only an exact subset
  of the verified backup inventory and independently retained Registry/package/lifecycle/Grant
  authority; conflicting or newer binding evidence fails closed. Reviewed
  same-version/OS/architecture whole-install restore is now implemented with a
  path-free Add/Replace/Remove/Retain plan, exact live Registry and Grant
  authority, a verified external rollback archive, link/reparse-safe staging,
  seven durable phases, 15 subprocess-exit recovery boundaries, terminal
  replay, read-only diagnostics, and bounded crash-recoverable history. Missing
  authority recovery, clean-machine disaster recovery, cross-platform drills,
  whole-product policy, and complete operational exercises remain open.
- [x] Expose bounded, secret-free latest/previous lifecycle checkpoint
  diagnostics through `extension inspect --json`.
- [x] Add broader telemetry and diagnostics for plan, download, provider
  readiness, cutover, drain, rollback, and recovery without exposing secrets.
  `extension diagnose --json` now exposes bounded, path-free Registry/TUF,
  reviewed-plan, provider, Grant, cutover, lifecycle publication/drain/
  rollback, and recovery evidence for one exact retained planned/admitted/
  cancelled install, upgrade, or uninstall graph, active admitted enable/
  disable operation, or newest Host-reviewed pre-admission enable/disable plan
  or cancellation. Standalone Knowledge recovery exposes bounded active/
  history/capacity evidence. Retained install/upgrade graphs and durable
  pre-plan download attempts expose expected and retained archive and signed
  executable-planning-target bytes plus exact-target missing/partial/complete
  state from historical Registry provenance without network I/O, writes,
  cache-lock acquisition, or paths. Real killed-process tests prove active
  archive and planning-target partial observation, retained evidence, exact
  Range resume, and cleanup only after the reviewed graph is durable. Partials
  and complete observations are never planning/apply/recovery authority.
  Before an exact lock exists, a durable pre-lock resolution attempt records
  refreshed/cached access, requested version/channel, per-Registry
  pending/verifying/verified/failed state, path-free source/trust digests, TUF
  role versions, bounded failures, and terminal package-lock evidence. The
  diagnostic survives resolver failure or process exit and is deleted only
  after its download-attempt successor is durable. Real CLI tests cover a
  killed online resolution, terminal verification failure, and zero-network
  offline cache failure.
  `extension diagnose --history --json` now retains the newest 16 exact
  completed or rolled-back operations and cancelled graph plans within 8 MiB
  per scope/package.
  Retention happens before recovery evidence is removed, exact replay is
  deduplicated by `(operationId, planDigest)`, history survives uninstall, and
  malformed, linked, or oversized state fails closed without path or secret
  leakage. A real CLI install/uninstall/reinstall sequence proves zero-network
  newest-first history and legitimate textual operation-ID reuse; a Host graph
  cancellation proves zero-network cancelled outcome and replay deduplication;
  the managed Workspace Host kill/recovery path proves exact-scope history
  without a second entry. A digest-bound Host observation index selects the
  newest reviewed enablement plan by `(plannedAtMs, requestId)` while retaining
  its exact managed scope only for private request lookup. Real CLI assertions
  prove `planned`/`cancelled` projection, selected provider and awaiting-Grant
  state, exact zero-observed lifecycle counts, zero network/authorization/
  admission, no Host/fence/path leakage, active Use-evidence precedence, and
  suppression after Use completion before the Host outcome is durable.
- [ ] Complete threat model review, privilege boundaries, security response,
  upgrade policy, and support runbooks.

Exit gate: a release candidate can be installed, upgraded, recovered, audited,
and removed by an operator using only published artifacts and documentation.

## Release blockers

The first supported product release is blocked until all of the following are
green:

1. A0 proves serializable graph mutation and stale-generation rejection across
   different roots with shared dependencies.
2. A1 proves one authoritative installation generation for every explicit User
   and Workspace scope, including independent selection of the same package.
3. A2 proves atomic Use-owned control state and deterministic recovery around
   every external provider-effect boundary.
4. A3 lets an arbitrary MCP-capable agent discover and invoke capabilities
   through opaque references and server-owned exact-generation leases.
5. One reviewed Package Manager serves CLI, TUI, and management MCP, with a
   distinct lower-authority Capability Gateway for agents.
6. A4 composes all declared production providers without hardcoding
   A3S-specific domains into the universal engine.
7. Exact graph and Grant recovery passes failure injection at every checkpoint.
8. Linux, macOS, and Windows pass the declared real-process matrix.
9. A5 Registry operations, signing, provenance, and release installation are
   independently reproducible from `A3S-Lab/Use-Registry`.
10. Storage retention, repair, observability, incident response, and support
    procedures are documented and exercised.
11. A6 qualifies the signed MHS reference package against the separate virtual
    laboratory without claiming physical-device conformance.
12. Website and README examples pass against the release candidate.

Until then, README and website copy must say **development preview** and must
not advertise production readiness or a stable cognitive-package contract.

## Completion definition

A3S Use is ready to publish only when a user can select a trusted Registry,
review one exact scoped-installation plan, and atomically install a signed
dependency graph; and when an arbitrary MCP-capable coding agent can discover
and invoke its authorized capabilities without learning host paths or holding
lifecycle authority. The system must recover from interruption without
guessing, upgrade without exposing mixed generations, and uninstall without
leaving routes, Grants, leases, processes, projections, or package-owned state
behind.
