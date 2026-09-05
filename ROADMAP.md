# A3S Use Roadmap

Last updated: 2026-09-06

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
   host protocol v6, managed scope v2, manager toolset v5 (v4 migration
   contract retained), pending graph v4,
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
   `plugin_apply_plan` is the only package-state mutation tool; explicit
   pre-admission cancellation is a separate control-plane mutation.
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
- The authoritative per-installation `registry.json` snapshot has a bounded
  4 MiB read/write boundary. Readers validate the complete configured state
  directory chain, open the final file without following links or reparse
  points, allocate only the measured bounded size, and recheck file identity
  and length after reading. Writers create missing directories one component at
  a time only inside the configured state root, flush and sync a bounded
  temporary file, then atomically replace the snapshot. Oversized, linked,
  redirected, or concurrently replaced authority fails closed before JSON
  decoding or publication.
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
  receipts, non-cancelled package-graph operations, applying/rolling-back
  lifecycle journals, and immutable Runtime plan payloads. Runtime plan
  artifacts are decoded under the installation maintenance and plan-store locks
  before their Blob references are emitted. Source locks are joined without
  nesting unrelated locks;
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
  device I/O; retry owner-proven safe-no-effect deferrals automatically with
  the same key, and reconcile rejected or unknown outcomes explicitly.
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
for a clean installation. Schema v11 binds one exact `InstallationId` and stores
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
Skill/UI content digests. Deferred, rejected, and unknown outcomes retain
diagnostic evidence only. Deferred is allowed only when the owner proves that
it accepted no effect; a bounded durable not-before time then permits automatic
same-key retry without reconciliation. An applied capability-cutover
observation atomically retires the prior publication, publishes the candidate,
and advances the capability cursor
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

The inactive post-commit dispatcher now retains one installation-wide shared
maintenance fence from claim through durable observation, claims one effect at
a time, and leaves both the SQLite transaction and bounded executor before
owner I/O. Seven
separate typed ports cover Capability Index, invocation leases, Runtime, Flow,
Knowledge, Skill, and UI; each can return only owner-shaped application evidence
or an explicit deferred/rejected/unknown failure. A deferred observation binds
a maximum-five-minute not-before time, blocks early claims, survives export and
clean restore, and automatically retries only the original key when due. A hard
provider timeout must leave a fixed observation budget inside the claim lease;
timeout is recorded as unknown rather than being misclassified as rejection.
Timeout and caller cancellation stop waiting but do not cancel the possibly
accepted owner future; that detached task retains the same shared maintenance
guard until it actually completes. Process exit after an accepted effect, an
expired claim, and an unknown outcome all require explicit replay with the
original committed idempotency key. Qualification tests prove
commit-before-effect, Store re-entry during provider I/O, all owner routes,
action/evidence compatibility, timeout bounding, task-panic classification,
exact-key recovery, and that a concurrent restore cannot acquire its exclusive
fence before observation or while a timed-out/cancelled effect remains in
flight. Every successful claim now also
projects its
owner-shaped authority inside the claim transaction. Package owners receive
only the exact committed package selection, lifecycle incarnation, host,
snapshot identity, and Grant; Runtime additionally receives the complete
reviewed provider selection. Capability Index receives the complete candidate
generation and one latest terminal preparation for every enabled selected
surface, including retained multi-root surfaces from earlier generations and
explicit optional degradation. Missing Grant coverage, a nonterminal latest
observation, teardown masquerading as preparation, or generation drift fails
closed before provider I/O. The multi-root qualification exposed and fixed an
immediate-foreign-key ordering defect: generation commit now writes the complete
package node set before dependency edges and surfaces in the same transaction.
The Artifact Store now supplies the corresponding non-cloneable verified read
lease. Acquisition holds both coordinated read locks and binds one complete
verified catalog record to the full package fingerprint, manifest digest,
exact byte/file counts, manifest surface graph, surface-file validation,
quarantine state, and incomplete-GC fence. The handle exposes no package root,
bounded manifest reads precede ACL parsing, missing locks are not created by a
read, and repeat verification detects uncoordinated tampering. The first real
post-commit adapter uses that lease for immutable Skill and UI surfaces. It
re-derives the typed owner and original idempotency key from committed portable
fields, validates the exact package/lifecycle/host/snapshot/Grant authority,
reads only the named surface, re-verifies the full package after the bounded
read, and emits a stable path-free content receipt independent of retry claim
metadata. Artifact lock or I/O contention becomes a safe durable deferral;
tampering, missing content, and authority substitution become terminal
proved-no-effect rejection; this read-only adapter has no unknown-acceptance
state. Static stop/remove receipts require no artifact path or bytes. This
is now joined by a real OKF Knowledge adapter. It revalidates the exact
committed Knowledge owner and idempotency key, reads first-use OKF content as a
path-free verified byte payload, stores staged receipt evidence before
promotion, stores promoted evidence before returning applied, and can replay a
retained promoted generation without Artifact access. Stage, promotion,
removal, or post-effect receipt ambiguity is durable unknown evidence;
pre-effect contention is a safe deferral; authority or immutable-byte drift is
rejected. Stop is path-independent and remove is driven only by the retained
receipt. A real SQLite composition test exercises committed claim, detached
dispatcher coordinator, Knowledge materialization, and durable Control
observation together. Artifact-only admission is now distinct from legacy
lifecycle publication: it is idempotent, revalidates prepared bytes, creates no
installation receipt, and requires its reference-admission guard to span the
separate authority commit. The third real post-commit adapter now implements
Capability Index and invocation leases as one Capability Plane boundary. It
accepts a host-owned pure Agent-catalog projector only after validating the
committed candidate and exact terminal surface evidence. It rejects projected
descriptors outside enabled, prepared package incarnations, durably publishes
the catalog, and materializes a canonical content-addressed Index document
that binds the publication. Control's applied cutover observation advances the
only mutable cursor with that catalog digest/generation/revision in one
transaction. Admission reopens and rehashes the exact catalog before reading
the cursor around shared locks for every package incarnation; drain requires
an unpublished prior incarnation and an exclusive lock, safely deferring until
accepted calls release it. Immutable publication is no-follow, no-replace, and
crash-replayable. The Index and lease files remain derived operational state;
the legacy coordinated inventory now registers and verifies the catalog and
descriptor-snapshot payloads, while production owner-native restore/retention
still remains open. A real
composition test joins Knowledge, Skill, catalog/Index publication, exact
payload admission, stale admission, and same-key drain retry. The
inactive ADR-003 step-3 qualification now also includes a committed-authority
Flow owner. It consumes a path-free verified source snapshot, durably publishes
a no-clobber content-addressed source in its own workspace, and invokes only
the typed `a3s-flow` Native TypeScript preflight. Compiler/cache paths are
operational host configuration, never package authority; source substitution
and failed preflight reject without a Control observation, while Artifact Store
contention remains a same-key deferral. Stop/remove are path-independent
receipts. The same inactive qualification now includes a committed-authority
Runtime owner for release-backed Tool Tasks, Tool Services, and Streamable HTTP
MCP. First prepare reads a verified, path-free Tool/MCP release payload and
requires the injected plan/provider semantics to match the exact committed
package and provider selection. Task preparation persists no Runtime unit;
Services advance a durable `requested` -> `runtime-applied` -> `gateway-ready`
record before committing the final binding. Exact final receipts replay without
Artifact access, the final-binding/provisioning overlap reconciles without a
second Runtime apply, and retirement verifies receipt-owned provider evidence
before Gateway drain and Runtime stop/remove. Pre-effect contention safely
defers; authority or immutable-byte drift rejects; every ambiguity after a
Runtime, Gateway, or receipt effect remains unknown. The Runtime boundary now
provides a bounded canonical plan payload and a restart-safe resolver that
reconstructs the full plan from its committed semantics digest and rechecks
exact provider evidence. The installation-scoped, host-owned
`RuntimeSurfacePlanStore` is also qualified as a canonical digest-addressed
payload source with bounded batch publication, no-clobber writes, restart-safe
reads, and fail-closed tamper checks. An inactive lifecycle admission seam now
accepts the canonical cognitive-package Plan envelope, authorization evidence,
and optional planned Grant transition. It derives both prior Control cursors
from the immutable Plan and accepts no caller-selected generation. Its combined
qualification entry point retains one installation-wide maintenance fence while
registering the exact reviewed operation, deriving the complete Control
transition, validating exact Runtime prepare coverage and reviewed Grant
proposal digests, publishing immutable plan bytes, and committing the projected
generation. Production still needs to route the live lifecycle through this
seam and complete the atomic dispatcher composition; a
process-local selection must never become production authority, and no adapter
may read legacy authority or treat a path as authority. This narrows the
production cutover boundary without activating the private kernel.

Production lifecycle code still does not construct this kernel, and the live
state layout, reachability, diagnostics, backup, and restore orchestration do
not accept it as production authority. A private path-free registry contract
now freezes all six owner identities and their ACL backup policies. It excludes
the global Artifact Store and requires an exact canonical receipt set for the
remaining five owners, bound to one `InstallationId`, Control generation,
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
missing applied payload must have a same-incarnation remove effect. Deferred
outcomes prove no owner effect and remain scheduling evidence; claimed and
unknown outcomes remain explicit reconciliation evidence. None selects desired
state. This code remains inactive and does not replace the legacy path
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
The planning-and-diagnostic observation owner now has the second concrete
snapshot and clean-target restore adapter. It uses the diagnostic-history,
resolution-attempt, and download-attempt owners' own decoders and invariants
instead of copying their schemas. Only terminal diagnostic histories and
terminal resolution attempts enter the bounded no-clobber archive. Active
resolution/download records and locks are excluded, while a canonical
path/digest inventory of active records is bound to the manifest. Secure
traversal, a second pre-publication scan, and offline verification reject links,
moved or foreign records, unknown layouts, duplicate identities, substitution,
trailing bytes, and registered bound violations. Receipts remain path-free and
bound to the exact Control export. An offline-verified archive can be staged
beneath the target state root without touching live owner paths. Activation
requires the exact exclusive maintenance guard and a clean record inventory,
atomically marks the archive as activating, and publishes each owner-validated
record without replacement. Digest-named deterministic partials recover
interrupted record writes; after activation starts, only an exact snapshot
subset may replay. The final path-free result is bound to the owner manifest and
inventory. This adapter is not connected to the legacy scanner.
The Host protocol projection is now the third concrete snapshot and
clean-target restore adapter. Its owner-native scanner treats immutable
request-to-plan records, optional terminal outcomes, and cancellations as the
only semantic archive sources. Operation lookup aliases and latest-enablement
diagnostic indexes are derived: the scanner validates them against their source
requests, rejects missing, stale, orphaned, linked, or unknown layouts, and
excludes them from the archive. Exact and legacy cancellation aliases normalize
to one canonical binding. A bounded no-clobber archive is published only after
a second live scan and after every Host plan, outcome, cancellation time,
completion result digest, package identity, desired state, selected surface,
package generation, and capability generation is reconciled with the exact
bound Control export. Receipt and observed-health evidence remain Host
observations and cannot choose Control desired state. The path-free manifest
and receipt support exact offline verification, explicit zero-file absence, and
no-change Host requests without inventing an operation. An offline-verified
snapshot can stage its archive and build a complete target-local Host owner
root from exact semantic source bytes plus newly derived canonical operation and
latest-enablement indexes. Legacy aliases and locks are excluded. Activation
requires the exact exclusive maintenance guard and no existing live owner root,
re-runs the owner-native semantic scan, persists a snapshot-bound activation
marker, and atomically publishes the whole directory without replacement.
Deterministic archive, record, and marker partial recovery covers every staged
transition, and the same attempt reconciles the post-publication/pre-result
boundary. Drift, links, rebinding, and preexisting state fail closed; absence
creates no owner root and the result is path-free. This adapter remains
inactive. The Restore Coordinator is now the fourth concrete snapshot and
restore owner. Its owner-native scanner accepts only
exact canonically encoded completed restore operations for the bound
installation. It excludes the active marker and its exact operation while
binding their bounded count and digest inventory, including marker-only
handoff. Orphaned nonterminal operations, pruning or temporary residue,
unknown entries, links, foreign installation history, and path/record rebinding
fail closed. Snapshot creation performs a second scan before no-clobber
publication; its path-free receipt and streaming offline verifier bind exact
terminal bytes to the Control export, while empty or active-only history emits
no archive. An offline-verified snapshot now builds an immutable target-local
candidate. Activation requires the exact exclusive maintenance guard and an
active whole-installation restore marker; it binds that stable marker identity
plus exact before/source/target inventories before changing live state. It
atomically retires only terminal directories, publishes the target without
replacement, and leaves the current active operation untouched even as its
status advances between replays. A marker-only handoff is valid. If a legacy
whole-installation marker accompanies a 64-record source, the adapter applies
the journal's native `(completed_at_ms, started_at_ms, plan_digest)` ordering
and omits exactly the oldest source record to reserve the active operation's
slot. The typed complete-set marker has no retained operation and preserves all
64 records. Any source collision with a retained active plan fails closed before
pruning. Candidate, activation, retired,
and deterministic publication-partial evidence make every local boundary
replayable and tamper-evident, and the result remains path-free and
snapshot-bound. This adapter is still qualification-only. The Runtime plan
payload owner is now the fifth snapshotted owner: it captures immutable,
installation-scoped plan envelopes, verifies complete key/plan binding, and
restores them before Host projection activation. Runtime plan artifact digests
are included in installation reachability scanning so cleanup cannot remove a
blob still required by a committed plan. The private complete-set snapshot
coordinator now captures one canonical Control export and the Host projection,
Knowledge, planning/diagnostic observation, Restore Coordinator, and Runtime
plan snapshots under the same exclusive maintenance fence and timestamp. One
canonical path-free manifest binds the exact owner registry,
receipts, schemas, digests, and byte accounting. The coordinator streams a
single staged archive outside all Use data and state roots, reuses each
owner-native offline verifier, and publishes only the fully verified file with
no-clobber semantics. Explicitly absent owners add no payload bytes, and the
global Artifact Store remains excluded. This closes complete-set snapshot
assembly and offline verification. The same offline-verified aggregate can now
stage one deterministic clean-target restore attempt. A canonical path-free
descriptor first binds the exact complete snapshot, installation, owner
registry, Knowledge policy, and fixed six-component set. One exclusive target
maintenance fence is then retained while the Control database and all five
owner-native candidates are built beneath the fixed
`.control-installation-restore` directory. Control is reconstructed from the
canonical export, checkpointed to one SQLite file, round-tripped semantically,
and bound by durable physical digest evidence. No live Control, Host,
Knowledge, observation, or restore-history path is changed. Exact retries and
interrupted Control staging recover deterministically; target contamination,
links, unknown entries, snapshot or policy rebinding, and completed-candidate
drift fail closed. The complete-set coordinator now qualifies full ordered
activation. Before durable intent, it revalidates every owner candidate and its
clean live boundary. The immutable attempt descriptor remains the restore
identity. A canonical `activation.json` journal binds that attempt to an
immutable operation; the typed global `.maintenance.restore.json` marker binds
the same identity and blocks ordinary shared access. The fixed owner order is
Control Store, Runtime plans, Host projection, Knowledge, observations, then
Restore Coordinator; every step follows journal, marker, owner effect,
checkpoint.
Each ordered checkpoint retains only the canonical path-free result length and
a domain-separated digest. The Restore Coordinator additionally binds the exact
complete marker bytes, length, and digest before history mutation. Reopening
reacquires the exact exclusive guard, rebinds the same verified snapshot,
attempt, owner registry, and Knowledge policy, and reconstructs or verifies
every owner at its precise candidate/live boundary. Journal and marker partials,
all six post-effect/pre-checkpoint boundaries, the final checkpoint before
retirement, and exit immediately after marker deletion converge. A missing
marker is valid only beside the complete six-checkpoint journal; out-of-order
live roots, ambiguous markers, snapshot rebinding, links, and evidence drift
fail closed. Exact completed replay performs no owner effect and can only resume
bounded fixed-order retirement of the six link-free staging trees. A
real-child-process matrix qualifies 21 top-level durable exits, including each
retirement boundary. The surviving canonical `attempt.json` and complete
`activation.json` are the exact installation-bound terminal receipt. Legacy
backup and artifact reachability exclude only that receipt; incomplete,
extended, linked, or tampered evidence fails closed. Production Grant
conversion, Runtime/Flow dispatcher composition, production backup/restore
wiring, indivisible consumer cutover, and deletion of legacy mutable stores
remain open; no A2 checkbox is complete yet.
As a cutover prerequisite, lifecycle intent v4 and operation v3 now bind every
checkpoint key to the plan, installation kind and ID, package ID and
generation, action, sequence, kind, and surface. This removes collisions
between graph siblings before their effects enter one installation outbox.

### A3 - Deliver the arbitrary-agent capability plane

- [x] Ship two standard MCP service entry points: a privileged Package Manager
  endpoint and a lower-authority Capability Gateway endpoint. Do not introduce
  a private Use JSON-RPC protocol. The Gateway embedding also exposes standard
  Streamable HTTP at `/mcp` with host-owned bearer, Origin, and bounded
  admission configuration.
- [x] Define portable `CapabilityDescriptor` contracts with opaque
  `InvocationRef`, `ArtifactRef`, `EndpointRef`, and `ResourceRef` values.
  Remove executable paths, package roots, provider release paths, and secrets
  from external JSON.
- [ ] Let the Use Host resolve an invocation reference and retain the exact
  package-generation lease for the entire call, stream, or server connection;
  drain and retirement operate on those server-side leases.
- [x] Define consumer profiles. Generic coding agents receive standard MCP
  Tools, Resources, and Prompts; the typed profile/negotiation contract keeps
  optional A3S extension labels explicit without changing the universal
  contract.
- [ ] Project negotiated Flow, UI, and Knowledge metadata for A3S consumers
  without weakening the lower-authority boundary. Principal-scoped discovery
  filtering is now available as a separate host policy seam; actual typed
  extension payload projection remains open.
- [x] Propagate standard MCP request cancellation through the Capability
  Gateway. rmcp `RequestContext.ct` now bounds Tool, Resource, and Prompt
  provider operations; cancellation drops in-flight provider futures and
  resolver/admission leases, with a typed secret-free boundary result when a
  response is still deliverable. Detached downstream work remains a host
  provider responsibility.
- [ ] Require signed descriptions and JSON input/output schemas for every
  agent-visible Tool. Legacy executable-only Tool Tasks remain host-only until
  a schema-valid descriptor is bound to them.
- [x] Expose bounded, catalog-authorized MCP Resources and Prompts through the
  standard `resources/list`, `resources/read`, `prompts/list`, and `prompts/get`
  methods. Resource URIs are opaque and exact-match checked; prompt arguments
  are closed against reviewed declarations; provider content is size-bounded,
  path-free, and held under the same generation lease as Tool calls.
- [ ] Materialize one immutable Capability Index at lifecycle cutover and emit
  generation-change notifications. Remove fixed-interval full filesystem
  rescans and repeated asset hashing from the normal watch path. The inactive
  Control kernel now durably publishes and transactionally binds the exact
  catalog/Index identities, while the notification hub and watcher mechanisms
  are independently qualified. Complete descriptor projection and production
  host wiring still keep this exit gate open.
- [ ] Add CLI/service wiring, fail-closed trusted confirmation for management
  apply, bounded authentication, authorization, rate limits, and secret-free
  diagnostics for both endpoints. Gateway HTTP bearer authentication,
  optional exact Origin policy, duplicate-header rejection, bounded in-flight
  and rolling-window admission, sanitized HTTP errors, an explicit
  pre-invocation provider authorization hook, and typed propagation of the
  host-authenticated transport/principal context are implemented; bounded
  HTTP token-to-principal mapping is now also available. Production
  receipt/Runtime/Grant authorization and product CLI wiring remain. A host can
  now inject a bounded, fail-closed `CapabilityGatewayDiscoveryPolicy` so
  authenticated principals receive frozen per-context Tool/Resource/Prompt
  views; this metadata boundary remains separate from invocation authorization.
- [ ] Prove one-endpoint discovery and invocation from independent Rust,
  TypeScript, and Python clients, including a container or remote client with
  no shared package filesystem. Cover install, live upgrade, prior-generation
  drain, uninstall, restart, and denied cross-scope access.

Implementation notes (2026-09-03): PR [#192](https://github.com/A3S-Lab/Use/pull/192)
landed the portable descriptor and catalog contracts plus an embedding
`CapabilityGatewayMcpServer` that speaks standard MCP and dispatches through an
injected provider. PR [#199](https://github.com/A3S-Lab/Use/pull/199) then added
an exact `CapabilitySnapshotLease` constructor path: the host acquires all
callable package-generation leases in canonical order, rechecks the cursor, and
retains the non-clone lease through Gateway clones and calls. PR
[#200](https://github.com/A3S-Lab/Use/pull/200) corrected a first-principles
clock error in the catalog contract: catalog `generation` is the immutable
publication generation, while each descriptor `generation` is its owning
package lifecycle generation. A single publication may therefore contain
independently upgraded packages, but it cannot contain two lifecycle
incarnations of one package/surface identity. PR
[#202](https://github.com/A3S-Lab/Use/pull/202) adds shared host-configured
in-flight and rolling-window admission to Gateway calls and Streamable HTTP.
PR [#203](https://github.com/A3S-Lab/Use/pull/203) adds duplicate-header
rejection, native-client Origin compatibility, standard HTTP challenge/backoff
headers, and a real independent Rust client test. These are contract and
embedding increments only; the A3 exit gate remains open until live-host
reference resolution, authorization, CLI wiring, and the independent
client/recovery matrix are implemented. The HTTP transport remains
caller-TLS/loopback only; authentication and rate limiting are endpoint
safeguards, not a substitute for live reference authorization. The provider
boundary now requires a pre-invocation `authorize` hook; denials are sanitized
to `use.plugin.capability_gateway_forbidden` and never reach `invoke`, with no
implicit allow implementation. A host must bind its principal and policy
explicitly.

The host can derive a Gateway catalog from one immutable
`CapabilityRegistrySnapshot` through
`CapabilityRegistrySnapshot::capability_gateway_catalog`. The bounded
projection rechecks the snapshot cursor and public projection revision,
package and manifest digests, reviewed publication-record evidence, selected
surfaces, and ready/enabled package binding before constructing the canonical
catalog. It accepts a consumer subset, but does not verify signatures or
resolve opaque references on behalf of the host.
`CapabilityGatewayMcpServer::from_registry_snapshot` acquires the
matching RAII snapshot lease only after that projection and returns no server
when the publication changes or is already draining. This closes the
snapshot-to-catalog composition gap without claiming the remaining live
resolver, receipt-owned provider, or multi-principal production wiring.

The verified live-host composition boundary is now explicit as well:
`CapabilityGatewayMcpServer::from_verified_registry_snapshot_with_factory_and_options`
observes one snapshot, consumes host-verified description proofs, captures the
same cursor in `CapabilityGatewayRegistryResolver`, acquires the exact server
lease, and retains consumer negotiation plus bounded admission policy. A
publication race returns no server. The injected factory still owns receipt,
Runtime, Grant, principal, and scope authorization, so the overall A3 exit gate
remains open.

Implementation note (2026-09-04): the typed
`CapabilityConsumerProfile`/`CapabilityConsumerNegotiation` contract now
distinguishes the default `generic-mcp` consumer from an explicit `a3s`
consumer. Extension requests are canonical, sorted, bounded, and digest-bound;
fail closed when the host cannot support the complete requested set. The
embedding Gateway retains the completed negotiation across its clones and
leased constructors; legacy constructors remain generic-MCP by default.
Descriptors can now carry a canonical `requiredExtensions` set, and every
Gateway constructor projects the immutable catalog against the completed
negotiation before compiling discovery or invocation routes. This closes the
generic-consumer information-leak path, but it is still only the profile
boundary: actual Flow/UI/Knowledge payload projection, principal-specific
discovery policy, production receipt/Runtime/Grant composition, and product
host wiring remain open, so the consumer-profile checkbox is intentionally not
marked complete yet.

Implementation note (2026-09-04): the standard MCP projection now includes
catalog-authorized Resources and Prompts in addition to Tools. Resource
references are opaque, exact-match checked, and never interpreted as paths or
URLs; prompt arguments are closed against the reviewed declaration; every
standard discovery list is deterministic, bounded, and cursor-paginated; and
provider output is validated before it crosses the agent boundary. This does
not yet project A3S-specific Flow/UI/Knowledge metadata or principal-specific
discovery policy.

Implementation note (2026-09-04): Gateway catalog projection now evaluates
descriptor `requiredExtensions` against the immutable consumer negotiation.
Unaccepted descriptors are removed before MCP route compilation, so they are
absent from both list responses and direct lookup. Tool discovery is explicitly
sorted because the underlying router uses a hash map; cursors therefore cannot
silently reorder or skip capabilities between pages.

Implementation note (2026-09-04): the Gateway now exposes an explicit
`CapabilityGatewayDiscoveryPolicy` seam for host-authenticated principal
filtering. Policy decisions are evaluated once per trusted context and cached
in a bounded `OnceCell` view shared by server clones, so `tools/list`,
`resources/list`, `prompts/list`, and direct Tool/Resource/Prompt requests use
the same stable visibility set. Denied routes behave like unpublished routes;
policy errors are sanitized and fail closed, while the provider's per-call
principal/Grant/generation authorization remains mandatory. Existing
constructors retain an allow-all compatibility default, so production
multi-principal hosts must opt in explicitly.

Implementation note (2026-09-04): the standard MCP adapter now consumes rmcp's
per-request cancellation token. Tool, Resource, and Prompt provider futures
are selected against `RequestContext.ct`; a cancellation drops the in-flight
future before the adapter can validate or publish a result, releasing the
short-lived admission permit and any resolver-owned invocation lease. The
adapter returns the bounded `use.plugin.capability_gateway_cancelled` result
when a response remains deliverable, while the server-wide snapshot lease is
left available to other requests. Integration tests exercise real rmcp
`notifications/cancelled` traffic for all three operation classes.

Implementation note (2026-09-05): Extension Registry watches now follow the
atomic `registry.json` publication through a bounded cross-platform filesystem
subscription instead of a fixed 50 ms read loop. Native notifications are
preferred, with a bounded target-metadata probe running alongside them to cover
platform backends that coalesce or omit an atomic replacement; an explicit
metadata-only polling backend is retained when the native backend cannot be
registered. Callback events are target-filtered and coalesced to one signal,
and every wake-up re-reads the validated publication.
`CapabilityRegistry::wait_for_change` no longer rebuilds, scans, and hashes the
complete capability projection every 100 ms; it projects at subscription
setup, after a real generation advance, and once at timeout to close the final
race. The Gateway now exposes a bounded host-owned notification hub that
registers initialized MCP peers, advertises all three standard
`list_changed` capabilities, coalesces exact publication keys while rejecting
older generations, and retires closed or back-pressured peers without
introducing a private wire method. The hub is
deliberately separate from catalog/session replacement: a host must durably
publish the new immutable catalog, route new sessions to it, and retain old
leases through drain. The roadmap item remains open until the complete
agent-facing catalog is materialized into the lifecycle Capability Index and
product hosts connect that cutover to the hub.

Implementation note (2026-09-05): `CapabilityGatewayCatalogStore` now gives
the embedding host a durable owner for the exact Agent-facing catalog payload.
It validates the installation scope and canonical bytes, stores bounded
SHA-256-addressed records behind a cross-process mutation lock, uses
no-follow checks plus deterministic staging and create-if-absent hard-link
publication, and supports exact generation/revision reads after restart.
Malformed top-level state, linked entries, tampered records, and over-bound
inventories fail closed; incomplete regular staging artifacts can be replayed
under the same digest. The store deliberately has no mutable current pointer,
so payload durability alone does not select a live generation. The inactive
Control composition now supplies the transactional binding described below;
production activation and host coordination of session replacement, lease
drain, and retention remain required before the A3 catalog gate can close.

Implementation note (2026-09-05): `CapabilityGatewaySessionFactory` now gives
an embedding host a bounded live-endpoint cutover seam. It serializes
replacement of immutable servers, rejects cross-installation and stale
publication generations, keeps consumer negotiation and lease mode stable, and
routes each MCP operation through a current-server snapshot. A replacement is
made visible before the shared standard list-change fan-out; old in-flight
operations retain their prior immutable server and lease, while subsequent
operations on the same endpoint observe the new catalog. This is an adapter
mechanism, not the lifecycle authority: production Control activation,
receipt-owned provider composition, lease retirement, and catalog-retention
coordination remain open.

Implementation note (2026-09-05): the session factory now also offers
`from_published` and `replace_published`. These paths read the exact
installation/generation/revision/digest from `CapabilityGatewayCatalogStore`,
re-project it for the server's completed consumer negotiation, and reject a
missing, forged, tampered, or unpersisted catalog before the in-memory swap.
The unverified compatibility method remains available for hosts with another
persistence authority; selecting the Control-bound publication and retiring
payload leases remain lifecycle responsibilities.

Implementation note (2026-09-05): catalog payload retention now has an
explicit plan/apply protocol. The lifecycle owner supplies the protected
digest set; the store emits a canonical inventory partition, rechecks the
exact plan under its mutation lock, verifies each regular record before
removal, fsyncs the affected shard, and supports read-only terminal replay.
The store refuses an empty protection set for a non-empty inventory and never
infers liveness from a mutable pointer. Control cursor and session-lease
coordination remain the authority that chooses the protected set.

Implementation note (2026-09-05): retention apply now persists a bounded,
canonical append-only recovery journal before each destructive unlink. It
repairs a torn final record, reconciles an in-flight unlink against the
immutable inventory after restart, blocks conflicting publication/planning,
and exposes `CapabilityGatewayCatalogStore::recover_retention` so a host can
resume from the journal's stored reviewed plan. This hardens the payload-owner
recovery boundary; it does not choose the protected generations or close the
session-lease lifecycle gate.

Implementation note (2026-09-05): inactive Control Store schema v11 now binds
the immutable Agent-facing catalog to the actual capability publication
transaction. A host-owned projection port receives only the committed
candidate generation and terminal surface observations. The concrete
Capability Plane rejects descriptors outside enabled, prepared package
incarnations, durably publishes the catalog and canonical Capability Index,
and returns their identities as one typed cutover application. Recording that
applied observation stores the catalog digest/generation/revision and advances
the published cursor in the same SQLite transaction. Admission reopens and
rehashes the exact payload before taking package-generation leases; missing or
tampered bytes fail closed. This closes the inactive-kernel cursor-binding
mechanism, not production activation, complete receipt/Runtime/Grant-backed
descriptor projection, production payload-owner restore/retention activation,
or session drain coordination.

Implementation note (2026-09-05): the inactive Capability Plane now also has
an explicit descriptor-evidence projector. It accepts only host-verified
`CapabilityDescriptionProof` values and an immutable package-scoped signer
allowlist. Every supplied descriptor is checked against the committed enabled
package and lifecycle incarnation, exact catalog-record provenance, selected
surface dependency graph, terminal prepared owner receipt, active Grant
coverage, and reviewed Tool/MCP workload or transport before the projector
derives domain-separated opaque invocation, endpoint, artifact, and resource
references. Optional degraded surfaces and substituted owner evidence remain
unpublishable, and projection failures are safe deterministic rejections with
no payload write. This was a strict subset gate at the time of that projector
change; cryptographic key custody, a durable cryptographic snapshot, production
Runtime payload admission/receipt wiring, and production Control/Runtime/
receipt wiring remained open before the complete A3 catalog exit gate could
close.

Implementation note (2026-09-05): the descriptor evidence boundary now has an
installation-owned durable snapshot store for crash and restart replay. It
captures the exact normalized proof set and package-scoped signer policy under
a key bound to the installation, installation generation, capability
generation, and candidate Control descriptor digest. The record itself is
content-addressed by its canonical bytes, with bounded no-follow staging,
create-if-absent publication, cross-process locking, exact canonical/digest
revalidation, and no mutable current pointer. A durable projector reads only
the exact Control-bound key; absent evidence defers safely, while substitution,
tampering, duplicate keys, and unknown layout fail closed. The coordinated
state inventory now recognizes and semantically verifies its canonical
descriptor-snapshot records alongside Gateway catalogs; locks, staging, and
retention journals remain nonterminal. The store is still an inactive external
payload owner: key custody, production owner-native restore/retention
activation, and Control/Runtime/receipt wiring remain open. Runtime Tool release
planning now carries a canonical input/output-schema attestation through plans,
binding receipts, and Control evidence; verified artifact admission and strict
descriptor projection compare the same descriptor and schema digests. The
implementation is qualified in the inactive kernel, while production
lifecycle activation remains open. The signed v2 admission path described
below now makes the retained envelope, rather than the proof projection, the
cryptographic replay authority.

Implementation note (2026-09-05): the signed-description trust boundary now
has a canonical `SignedCapabilityDescription` envelope in `a3s-use-core` and
an Ed25519 `CapabilityDescriptionTrustStore` in `a3s-use-extension`. The
envelope domain-separates the exact descriptor bytes, key/signer identities,
and bounded validity window. The verifier owns no private keys, rejects
identity, expiry, revocation, canonical-byte, and signature mismatches, and
returns a private replay wrapper that must be reverified after restart. Multiple
keys for one signer are supported for rotation, and schema-bearing Runtime
Tool descriptors are required. The root Gateway facade now has signed-
description constructors that verify envelopes before snapshot lease and
provider-resolver composition; the legacy proof constructors remain only for
explicit preview hosts. This qualifies the cryptographic mechanism and its
composition seam but does not mark the A3 checkbox: Registry/TUF key-source
binding and production Registry-to-Control lifecycle wiring remain open.

Implementation note (2026-09-05): the Control descriptor snapshot owner now
has a signed v2 admission path. `publish_signed` verifies every canonical
Ed25519 envelope before content-addressed publication and stores the exact
envelopes beside a derived proof projection. A signed projector re-verifies
those envelopes against the current trust store and clock on every replay;
expiry, revocation, substitution, and proof/envelope mismatch fail closed. The
legacy v1 proof-only snapshot remains an explicit compatibility path and is
not allowed to consume a signed v2 record. This closes the Control
proof-snapshot admission mechanism in the inactive kernel; official
Registry/TUF key-source binding, production owner-native restore/retention
activation, and Registry-to-Control/Runtime/receipt wiring remain release
gates.

Implementation note (2026-09-05): coordinated state backup now has an explicit
`CapabilityPayloads` family for the two immutable Capability Gateway owners.
The scanner admits only the catalog shard and descriptor-snapshot record
layouts, verifies installation binding, canonical bytes, and content-addressed
digests during inventory and archive verification, and rejects unknown paths,
staging residue, mutation locks, and retention journals. This is a qualified
legacy inventory/restore-plan boundary, not the A2 owner-registry cutover:
production clean-target activation, owner retention policy, and current
Registry/TUF trust revalidation on signed replay remain required.

Implementation note (2026-09-05): Artifact Reachability now traverses the
same Capability Gateway payload-owner tree instead of silently ignoring the
new root. Catalog and descriptor-snapshot records are revalidated against
their installation and content address; unknown nested paths, links, staging
residue, and retention journals fail closed before a garbage-collection view
is returned. The scanner intentionally emits no Artifact Store references for
these opaque projections; lifecycle receipts remain the artifact authority.

Implementation note (2026-09-06): the Control descriptor-snapshot owner now
supports the same explicit retention contract as the Gateway catalog owner.
`plan_retention` names the protected digest set and the complete removal
complement; `apply_retention` binds the canonical plan digest, rechecks every
record under the owner lock, and persists one bounded checkpoint per unlink.
`recover_retention` resumes the embedded plan after a process interruption,
repairs only a torn journal tail, and blocks publication or inspection while
the journal is pending. The non-empty inventory invariant prevents an empty
protection set from deleting every snapshot. Production Control owner
registration, clean-target restore activation, and Registry/TUF policy wiring
remain separate gates.

Implementation note (2026-09-06): `CapabilityGatewayCatalogStore` now also
exposes an owner-native clean-target restore boundary. A reviewed plan binds
the installation, canonical byte counts, and the complete digest-sorted
inventory; apply re-derives every supplied catalog, stages and rescans a full
candidate tree, persists a plan-bound activation marker, and publishes with a
no-clobber directory move. Existing owner state is never merged or replaced,
foreign staged plans are rejected, and a durable candidate/marker can be
replayed after interruption. This closes the catalog half of the restore
primitive, but descriptor-snapshot restore, Control owner registration,
signed replay policy, session drain, and production rollback coordination
remain release gates.

Implementation note (2026-09-06): the Control descriptor-snapshot owner now
has a matching plan-bound clean-target restore adapter. The reviewed inventory
binds snapshot and key digests, Control generation identity, canonical byte
counts, and signed/proof-only mode. Apply re-derives every snapshot, requires
current trust-store verification for signed v2 evidence, stages and rescans a
complete candidate, persists a plan-bound activation marker, and publishes
without clobbering an existing owner. Durable candidate/marker evidence is
replayable after interruption, while foreign staged plans and retention
journals fail closed. The remaining gate is coordinated Control reopening and
production Registry/TUF-to-owner authority, not another local payload writer.

Implementation note (2026-09-04): Runtime Task publication and dispatch now
cross-bind each durable receipt to the installed package's retained planning
evidence and exact release descriptor digest. Registry-trusted packages must
retain catalog-bound signed planning evidence; substituted descriptors,
cross-generation bindings, and missing evidence are omitted or rejected before
provider connection. Local explicit packages retain their host-owned
qualification path. This closes a Runtime integrity gap but does not complete
the broader A3 receipt/Runtime/Grant authorization or independent-client exit
gate.

Implementation note (2026-09-03): PR [#197](https://github.com/A3S-Lab/Use/pull/197)
added a secret-free error projection at the Package Manager MCP boundary. The
adapter retains only validated `use.*` contract codes and a bounded public
message; paths, URLs, suggestions, details, provider-owned identifiers, and
package-authored diagnostics are omitted or collapsed to a generic code. This
is defense-in-depth for the existing adapter, not an A3 exit-gate claim. The
Gateway HTTP edge now adds endpoint bearer authentication and bounded
admission, and the injected provider exposes a sanitized pre-invocation
authorization seam; the HTTP `for_principal`/`for_principals` configuration now
carries the selected verified principal into both provider hooks without
exposing it to agents. PR [#208](https://github.com/A3S-Lab/Use/pull/208) adds
the lease-scoped resolver and bounded multi-principal mapping. Production host
receipt/Runtime/Grant composition, product wiring, and independent-client
recovery remain open.

The embedding seam is now explicit: PR [#208](https://github.com/A3S-Lab/Use/pull/208)
adds `CapabilityGatewayInvocationResolver` and
`CapabilityGatewayResolvedProvider`, which perform one resolution and
authorization for each call before invoking a private lease. The handle
implementation owns the exact package-generation guard and must retain it
until the invocation returns. The same PR adds a bounded 64-entry immutable
HTTP token-to-principal registry with duplicate-token rejection and complete
credential scans. These are host embedding contracts; they still have to be
composed with the production Use receipt, Runtime, and Grant authorities before
the A3 exit gate can be checked.

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
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v5` (v4 migration contract remains readable) |
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
| Capability descriptor | `a3s.use.capability-descriptor.v1` |
| Control descriptor evidence snapshot | `a3s.use.control-capability-descriptor-snapshot.v1` (proof-only compatibility) / `v2` (signed envelope) |
| Capability Gateway catalog | `a3s.use.capability-gateway-catalog.v1` |
| Capability Gateway catalog restore plan | `a3s.use.capability-gateway-catalog-restore-plan.v1` |
| Capability Gateway catalog restore result | `a3s.use.capability-gateway-catalog-restore-result.v1` |
| Capability Gateway catalog retention plan | `a3s.use.capability-gateway-catalog-retention-plan.v1` |
| Capability Gateway catalog retention result | `a3s.use.capability-gateway-catalog-retention-result.v1` |
| Capability Gateway catalog retention journal | `a3s.use.capability-gateway-catalog-retention-journal.v1` (internal) |
| Control descriptor snapshot retention plan | `a3s.use.control-capability-descriptor-snapshot-retention-plan.v1` |
| Control descriptor snapshot retention result | `a3s.use.control-capability-descriptor-snapshot-retention-result.v1` |
| Control descriptor snapshot retention journal | `a3s.use.control-capability-descriptor-snapshot-retention-journal.v1` (internal) |
| Control descriptor snapshot restore plan | `a3s.use.control-capability-descriptor-snapshot-restore-plan.v1` |
| Control descriptor snapshot restore result | `a3s.use.control-capability-descriptor-snapshot-restore-result.v1` |
| Capability consumer profile | `a3s.use.capability-consumer-profile.v1` |
| Capability consumer negotiation | `a3s.use.capability-consumer-negotiation.v1` |
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
- [x] Manager MCP toolset v5 with explicit install-time Registry selection,
  read-only planning, digest-bound operation observation/watch, one apply tool,
  and trusted explicit cancellation; the ten-tool v4 inventory remains a
  migration contract.
- [x] Shared typed `PluginManagerService` over the production Host Manager,
  with deterministic request replay, Registry-bound catalog cursors, stable
  installed-state pagination, exact/ranged SemVer selection, durable reviewed
  plan reopening, exact operation observation/watch, and all thirteen frozen
  operations. Its standard MCP adapter derives names, schemas, and annotations
  from toolset v5 and obtains apply/cancellation confirmation only from an
  injected trusted host provider.
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
- [x] Qualify the inactive committed-authority Runtime owner for release-backed
  Tool Task/Service and Streamable HTTP MCP payloads, monotonic Service
  provisioning, exact final-receipt replay, typed Gateway readiness, and
  receipt-owned retirement without exposing Artifact Store paths.
- [x] Define the canonical Runtime plan payload and restart-safe resolver
  boundary, with exact plan-time and provider-evidence validation.
- [x] Qualify the installation-scoped host-owned Runtime plan store with
  canonical digest addressing, bounded batch publication, restart-safe reads,
  no-clobber immutability, and fail-closed tamper detection.
- [x] Register Runtime plan payloads as the fifth snapshotted owner, include
  them in complete-set snapshot/staging/six-checkpoint activation, and retain
  referenced Runtime blobs through installation artifact reachability.
- [x] Qualify a reviewed-operation-only Control composition that projects the
  complete transition, validates Runtime publication authority, and orders
  immutable plan publication before the generation commit under one shared
  maintenance fence. This remains an inactive cutover proof.
- [ ] Compose production Runtime Service providers in A3S Code and managed
  hosts with a durable host source and atomic dispatcher cutover, preserving
  exact plan-time and apply-time evidence.
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
  manager-v5 read, planning, observation, watch, and cancellation inventory is
  available under `plugin`; apply and cancellation reopen a durable operation ID
  plus plan digest, require explicit trusted user authority (and CLI `--yes`),
  and use the verified cache without network access. Compatibility install,
  upgrade, and uninstall fields remain intact.
- [x] Migrate the A3S Code TUI to that service and compose the standard manager
  MCP in Code. CLI, TUI `/packages`, and the exact thirteen-tool manager-v5 MCP
  now reuse one host-owned service without a second plan, confirmation, or
  mutation path at A3S CLI commit `ce1240891d6926c132aed8212efabaf6c925f4db`.
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
  [33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660)
  scanned all five target archives for checkout paths and ran the installed
  native executables with isolated homes and working directories from exact
  `main` commit `4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd`.

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
  four targets and did not publish a Release. The current non-publishing
  qualification run
  [33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660)
  rebuilt every shipped native executable without a compiled-artifact cache,
  with one release codegen unit, and byte-matched all five primary archives
  from exact `main` commit
  `4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd`.
- [x] Publish the development-preview `v0.3.6` GitHub Release after the exact
  tagged source, five verified platform archives, deterministic
  SBOM/reproducibility evidence, installers, and Use-owned typed crates passed
  the release workflow. Release workflow run
  [33675697857](https://github.com/A3S-Lab/Use/actions/runs/33675697857) passed
  all 13 jobs for exact `main` commit
  `54758910f2f4ad9498137410e0a2207d412e99a1`; the release publishes
  `a3s-use-core 0.2.5`, `a3s-use-extension 0.3.6`, and `a3s-use 0.3.6`.
  The cancelled `v0.3.5` attempt did not create a GitHub Release because the
  public core crate was stale; do not treat that tag as published evidence.
  This does not close the external-witness or product-readiness gates.
- [x] Publish the development-preview `v0.3.7` GitHub Release after the exact
  tagged source, five verified platform archives, deterministic
  SBOM/reproducibility evidence, installers, and Use-owned typed crates passed
  the release workflow. Release workflow run
  [33687297386](https://github.com/A3S-Lab/Use/actions/runs/33687297386) passed
  all 13 jobs for exact `main` commit
  `48a0b76f8a4a87a11d16627c7bd7567920852508`; the release publishes
  `a3s-use-core 0.2.6`, `a3s-use-extension 0.3.7`, and `a3s-use 0.3.7`.
  The prior `v0.3.6` release remains historical evidence. This does not close
  the external-witness or product-readiness gates.
- [x] Publish the development-preview `v0.3.8` GitHub Release after the exact
  tagged source, five verified platform archives, deterministic
  SBOM/reproducibility evidence, installers, and Use-owned typed crates passed
  the release workflow. Release workflow run
  [33720485826](https://github.com/A3S-Lab/Use/actions/runs/33720485826) passed
  all 13 jobs for exact `main` commit
  `6d3a7baf32ce998a2e487c40fbf78b4a6cda2579`; the release publishes
  `a3s-use-core 0.2.7`, `a3s-use-extension 0.3.8`, and `a3s-use 0.3.8`.
  The prior `v0.3.6` and `v0.3.7` releases remain historical evidence. This
  does not close the external-witness or product-readiness gates.
- [x] Publish the development-preview `v0.3.9` GitHub Release after the exact
  tagged source, five verified platform archives, deterministic
  SBOM/reproducibility evidence, installers, and Use-owned typed crates passed
  the release workflow. Release workflow run
  [33756618837](https://github.com/A3S-Lab/Use/actions/runs/33756618837) passed
  all 13 jobs for exact `main` commit
  `a5f3cc40bfb0a1021ca150d2ce4295409b74d220`; the release publishes 19 assets,
  `a3s-use-core 0.2.7`, `a3s-use-extension 0.3.9`, and `a3s-use 0.3.9`.
  The prior `v0.3.6`, `v0.3.7`, and `v0.3.8` releases remain historical
  evidence. This does not close the external-witness or product-readiness
  gates.
- [x] Publish the development-preview `v0.3.10` GitHub Release after the exact
  tagged source, five verified platform archives, deterministic
  SBOM/reproducibility evidence, installers, and Use-owned typed crates passed
  the release workflow. Release workflow run
  [33791616307](https://github.com/A3S-Lab/Use/actions/runs/33791616307) passed
  all 13 jobs for exact `main` commit
  `c4c80a223bfff3698ca4b4598e7175c6e3303239`; the release publishes 19 assets,
  `a3s-use-core 0.2.8`, `a3s-use-extension 0.3.10`, and `a3s-use 0.3.10`.
  The prior `v0.3.6`, `v0.3.7`, `v0.3.8`, and `v0.3.9` releases remain
  historical evidence. This does not close the external-witness or
  product-readiness gates.
- [x] Publish the development-preview `v0.3.11` GitHub Release after the exact
  tagged source, five verified platform archives, deterministic
  SBOM/reproducibility evidence, installers, and Use-owned typed crates passed
  the release workflow. Release workflow run
  [33830280138](https://github.com/A3S-Lab/Use/actions/runs/33830280138) passed
  the validation, five-target primary-build, typed-crate, and five-target
  independent-rebuild gates for exact `main` commit
  `c25028ae0245ba1d28f7e2837e2a87f7e9f6fe40`; the release publishes 19 assets,
  `a3s-use-core 0.2.9`, `a3s-use-extension 0.3.11`, and `a3s-use 0.3.11`.
  The prior `v0.3.6`, `v0.3.7`, `v0.3.8`, `v0.3.9`, and `v0.3.10` releases
  remain historical evidence. This does not close the external-witness or
  product-readiness gates.
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

The first-principles capability and release-gate audit is recorded in
[docs/agent-package-manager-audit.md](docs/agent-package-manager-audit.md). It
distinguishes qualified mechanisms from inactive Control proofs and from the
production composition, trust, interoperability, and operations gates below.

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
