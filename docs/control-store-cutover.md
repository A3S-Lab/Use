# Control Store Coordinated Cutover

- Status: inventory frozen; implementation and production activation pending
- Machine-checked inventory: [`control-store-cutover.acl`](control-store-cutover.acl)
- Transaction decision: [ADR-003](adr-003-control-store-transaction-boundary.md)
- Roadmap: [A2](../ROADMAP.md#a2---consolidate-mutable-authority-in-a-control-store)

## Purpose

SQLite is the initial transaction engine, not the architecture by itself. The
architectural requirement is that one installation has one mutable control
authority. Activating the database while any old file can still select desired
package state would create two state machines and make crash correctness
unprovable.

The cutover inventory freezes the current replacement boundary before
production wiring begins. It classifies every supported installation-state
leaf into exactly one of four ownership classes:

1. **Legacy authority** chooses graph, Grant, enablement, lifecycle, binding,
   or capability state today. It must move into the Control Store transaction
   and its old path must be deleted.
2. **External owner** retains bytes, bounded observations, or restore
   coordination that cannot or should not be stored in the control aggregate.
   It must register a typed snapshot/verification boundary and may never choose
   desired state.
3. **Operational state** coordinates live processes. Locks, leases, and active
   restore markers are excluded from backup and are not recovery authority.
4. **Cutover consumer** reads or mutates one or more of the above facts. Every
   listed consumer must switch in the same production change, with no legacy
   fallback.

The ACL file is the single inventory source. Its unit test verifies the schema,
the exact current state-layout leaf set, disjoint ownership, referenced source
files, inactive status, clean-state-only preview policy, and the prohibitions
on dual writes and fallback reads. This document explains the policy and does
not duplicate the file-by-file matrix.

## Frozen aggregate boundary

The indivisible Control Store aggregate contains:

```text
installation graph and package generations
+ reviewed graph and enablement operations
+ Workspace Grant intent, revisions, and committed state
+ lifecycle checkpoints and effect-attempt identities
+ provider-binding identities and applied observations
+ capability generation and publication checkpoints
```

The current Registry receipt files are included because they still participate
in package selection, lifecycle identity, and capability publication. Package
trees and archive bytes are not included; they remain in the global
content-addressed Artifact Store. Scope-local OKF content also remains an
external payload, while its selected binding and observed lifecycle state move
into the aggregate.

Planning attempts, download observations, terminal diagnostic history, Host
request indexes, and the out-of-database restore journal remain separate only
because none may authorize a package mutation. Before activation, each owner
must provide a bounded typed inventory, deterministic digest, snapshot method,
offline verifier, and restore contract where its backup policy requires one.

The inactive kernel now freezes these five identities and their exact backup
policies in a path-free typed registry contract. Artifact Store exclusion is
explicit; receipts for the other four owners must be complete, canonically
ordered, bounded, and tied to one installation and Control export. A private
snapshot session now freezes the canonical export digest and generation under
one exclusive maintenance fence without holding a database transaction across
owner I/O. The Knowledge owner is the first qualified adapter: existing
SQLite/FTS5 state is archived, bounded, inventoried, and verified offline;
absence is recorded as zero files without creating live state. The adapter now
also verifies the exact bound Control export and joins each retained lifecycle
incarnation to its prepare intent, committed OKF bundle, applied observation
and projection evidence, and—when removed or pruned—the matching remove
effect. Claimed and unknown effects remain explicit ambiguity, not payload
authority. The same verified snapshot can now stage an exact state-root-local
database candidate without changing the live payload and activate it only into
a clean target under the exact exclusive maintenance fence. Candidate and
inventory drift, linked paths, unowned live-layout entries, foreign guards,
existing payloads, and unexpected bytes for an absent snapshot fail closed.
Atomic publication is idempotent across the post-rename/pre-result boundary
while the same staged attempt and exclusive fence remain held. This does not
replace the production backup or restore scanner and does not choose
cross-owner activation order. The planning-and-diagnostic observation owner is
the second concrete snapshot adapter. It reuses each owning store's semantic
validator and archives only terminal diagnostic histories and terminal
resolution attempts. Active resolution/download attempts and locks are
excluded from restore authority, but a canonical count and path/digest
inventory of active records remain bound to the manifest. Bounded no-follow
traversal, duplicate checks, a second live scan, no-clobber publication, and
offline verification reject drift, substitution, foreign records, unknown
layouts, and trailing bytes. The adapter is path-free and Control-export-bound.
An offline-verified archive can be staged beneath the target state root without
touching live owner paths. First activation requires a clean record inventory
and the exact exclusive maintenance guard, then atomically marks the candidate
as activating before no-clobber record publication. Digest-named deterministic
partials make incomplete publication replayable, and only an exact snapshot
subset is accepted after activation starts. The Host projection and Restore
Coordinator adapters and complete-set orchestration are still required before
this gate can close.

## Non-negotiable cutover invariants

- A production process opens either the legacy stores or the Control Store for
  one installation, never both.
- One reviewed mutation compares its expected generation and commits the
  complete local transition plus exact outbox intents in one transaction.
- Provider, filesystem, network, Runtime, Gateway, Flow, Knowledge, UI, OS,
  and device effects execute only after commit and never while a database
  transaction or store-executor permit is held.
- Applied outcomes retain canonical owner-specific evidence bound to the exact
  idempotency identity and intent. Rejected and unknown outcomes cannot carry
  applied state; ambiguity cannot mint a replacement effect.
- Recording the exact applied capability-cutover observation retires the prior
  publication, publishes the candidate, and advances the capability cursor in
  one transaction. Later drain or teardown failure cannot roll that cursor
  back and must reconcile with the same effect identity.
- Capability discovery, leases, artifact reachability, diagnostics, backup,
  restore, and clean-state detection read the committed Control generation or
  a digest-bound registered external owner.
- No receipt, diagnostic projection, provider observation, backup hash, or
  immutable payload can reconstruct missing control authority.
- The preview accepts activation only in a clean installation state root.
  Unsupported legacy state fails closed; importing released state requires a
  separate migration decision.

## Implementation and deletion gates

The implementation proceeds in this order:

1. **Complete deterministic input derivation.** Derive the full target Grant
   set, receipt revisions, provider bindings, capability descriptor, and effect
   inventory solely from the canonical reviewed Plan, authorization evidence,
   prior committed generation, and reviewed provider-selection evidence.
2. **Register external owners.** Replace manual path discovery with typed
   owner descriptors and prove bounded snapshot, digest, verification, and
   restore behavior. Operational files remain explicitly excluded.
3. **Build Control-backed orchestration.** Persist intent first, dispatch
   outbox effects through typed provider ports, and record observations in
   later transactions. Qualification uses isolated fixtures; it does not dual
   write live legacy installations.
4. **Switch every listed consumer together.** Lifecycle, capability index,
   leases, reachability, diagnostics, backup, restore, and state-layout logic
   land as one activation change.
5. **Delete legacy code and paths.** Remove the file-store constructors,
   semantic readers, manual mutable-state inventory, package-graph lock, and
   every recovery branch that accepts old authority.
6. **Run the failure matrix.** Kill the process before and after each local
   commit, provider acceptance, observation commit, publication, drain,
   retirement, backup, and restore activation boundary for all five actions.

Current inactive-kernel progress covers deterministic graph and package
generation projection, full Grant projection, reviewed Runtime provider
selection, the candidate capability descriptor, and the complete ordered
effect-intent inventory for all five lifecycle actions. Provider selection is
derived only for Tool and MCP surfaces; it is not an applied binding
observation. Package, Grant, lifecycle, and selection facts stay inside the
transaction instead of becoming pseudo effects. Schema v9 also persists and
offline-verifies typed Capability Index, invocation-lease, Runtime
Task/opaque-Service readiness, Flow artifact, Knowledge projection, and
Skill/UI content application evidence. The cutover observation advances
publication before drain; post-cutover required failure remains pending for
same-key reconciliation. Real owner ports and the dispatcher that produce
those observations remain gate 3 work, as does production conversion into the
reviewed inputs used by the inactive kernel.

Production activation is blocked until all gates below are true:

| Gate | Required evidence |
| --- | --- |
| Aggregate completeness | Online commit and offline export verification derive the same graph, Grants, bindings, capability, and effects without caller-selected fields. |
| Single authority | Static inventory and integration tests find no production reader, writer, fallback, or repair path for a legacy authority. |
| External ownership | Every retained payload family is registered, bounded, digest-bound, and unable to select desired state. |
| Atomic visibility | Every observable combination corresponds to one committed Control generation plus explicit external-effect observations. |
| Recovery | Restart reuses exact operation and effect identities without network access, reauthorization, or generation inflation where replay should be local. |
| Backup and restore | Store-owned snapshot/export and registered owner snapshots round-trip offline; WAL, SHM, locks, leases, staging, and active journals are excluded. |
| Portability | Linux, macOS, and Windows process-exit matrices pass from product entry points. |

Freezing this inventory is a prerequisite, not completion of an A2 roadmap
item. The checked-in kernel remains private and inactive until the coordinated
cutover and legacy deletion gates pass.
