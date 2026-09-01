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

The inactive one-effect dispatcher now qualifies the transaction-to-provider
boundary. It retains one installation-wide shared maintenance fence across the
complete claim/effect/observation interval, commits a claim, releases the SQLite
transaction and bounded
executor, routes the exact identity through one of seven typed owner ports, and
records applied, deferred, rejected, or unknown evidence in a later
transaction. Deferred is allowed only when the owner proves that no effect was
accepted; its bounded durable not-before time blocks early claims and then
permits automatic retry of the same key. A hard provider timeout must leave a
fixed observation budget inside the claim lease and is recorded as unknown.
The maintenance fence owns neither a database transaction nor an executor
permit, but prevents an exclusive whole-installation restore from replacing
Control and owner state before the observation is durable. Timeout and caller
cancellation detach only the wait; the possibly accepted owner task continues
to hold a reference to that fence until it actually completes. Process exit,
expired claim, and
ambiguous acceptance can resume only by explicitly reusing the committed
idempotency key. Remaining production owner adapters and dispatcher
composition remain part of the indivisible cutover; they may derive full inputs
only from committed Control authority and immutable artifacts, never from the
legacy stores.

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
effect. Deferred effects remain safe-no-effect scheduling evidence; claimed and
unknown effects remain explicit ambiguity. None is payload authority. The same
verified snapshot can now stage an exact state-root-local
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
subset is accepted after activation starts. The Host protocol projection is the
third concrete snapshot and clean-target restore adapter. Its owner-native
scanner archives only request-to-plan records, optional outcomes, and canonical
cancellations; operation aliases and latest enablement diagnostics must validate
against those sources but never enter the archive. Bounded no-follow traversal,
a second scan, and no-clobber publication reject layout drift and substitution.
Every mutation Plan, completion or cancellation fact, desired package state,
selected surface set, package generation, and capability generation is joined
to the exact bound Control export before publication, while Host receipt and
health evidence remain observations. The path-free offline verifier also
preserves no-change requests without inventing an operation. A verified
snapshot can stage a private archive copy and construct a complete target-local
owner root from exact source bytes and owner-derived canonical indexes, without
restoring legacy aliases or locks. Under the exact exclusive maintenance guard,
activation requires an absent live owner root, revalidates the exact physical
and semantic inventory, writes a snapshot-bound durable marker, and publishes
the whole directory with one atomic no-clobber move. Archive, record, and marker
partials plus exact post-publication replay make every activation boundary
recoverable; drift, links, rebinding, and preexisting roots fail closed. The
Restore Coordinator is now the fourth concrete snapshot owner. Its owner-native
journal decoder archives only exact canonical completed operations for the
bound installation. Active marker and operation files are excluded while a
bounded digest inventory records them, including marker-only handoff. Orphaned
nonterminal records, pruning or temporary residue, unknown layout, links,
foreign history, and path rebinding fail closed. A second scan precedes
no-clobber publication, and a streaming offline verifier binds the path-free
receipt and exact terminal bytes to the Control export. Empty or active-only
history creates no archive. A verified snapshot now stages an immutable
target-local candidate. Activation requires the exact exclusive maintenance
guard and an active restore marker, binds its stable identity plus exact before
and target terminal inventories, retires replaced terminal directories into
staging, and publishes candidate records without replacing live paths. The
active marker/current operation is preserved, including marker-only handoff and
operation-status progress across replay. At native capacity, a legacy
whole-installation marker uses the same completion-time/start-time/plan-digest
ordering as journal retention and removes exactly the source-native oldest
record to reserve the active operation's slot. The typed complete-set marker
has no retained operation and preserves all 64 source records. Source and active
identities may never collide. Durable activation, retired, candidate,
and deterministic publication-partial evidence make each local crash boundary
replayable and tamper-evident. The private complete-set snapshot coordinator
now captures the canonical Control export and every registered owner beneath
one exclusive maintenance fence and timestamp. Its path-free canonical
manifest binds the fixed owner registry, receipts, schemas, digests, and byte
accounting. A fixed-order single-file archive is streamed to external staging,
audited with every owner-native offline verifier, and published only once with
no-clobber semantics; absent owners add no payload and the global Artifact Store
is excluded. The verified complete set can now stage one clean target under a
single retained exclusive maintenance fence. A canonical path-free attempt
descriptor binds the snapshot, installation, owner registry, Knowledge policy,
and fixed five-component set before the Control and four owner-native
candidates are created beneath `.control-installation-restore`. The Control
candidate is single-file checkpointed, export-round-tripped, and physically
digest-bound. No live authority path is changed, and exact retry or interrupted
Control construction is deterministic; contaminated targets, links, unknown
entries, rebinding, and candidate drift fail closed. Complete activation now
preflights every owner before durably recording top-level intent. The immutable
attempt descriptor stays authoritative; `activation.json` is the sole mutable
ordered journal, and the typed global `.maintenance.restore.json` marker binds
the same immutable operation and blocks ordinary shared access. Control Store,
Host projection, Knowledge, observations, and Restore Coordinator execute in
that fixed order; each step follows journal, marker, owner effect, checkpoint.
Every checkpoint binds its canonical path-free result by byte count and a
domain-separated digest. The Restore Coordinator verifies the exact complete
marker bytes, length, and digest before history mutation. Reopen reacquires the
exact exclusive fence, rebinds the same snapshot, attempt, registry, and policy,
and reconstructs or verifies every candidate/live boundary. Journal and marker
partials, each post-effect/pre-checkpoint boundary, the fifth checkpoint before
retirement, and exit after marker deletion converge. Marker absence is accepted
only with all five checkpoints; out-of-order live roots, ambiguous markers,
links, rebinding, and evidence drift fail closed. Exact completed replay performs
no owner effect and can only resume bounded fixed-order retirement of the five
link-free staging trees. A real-child-process matrix covers 18 top-level durable
exits, including each retirement boundary. The canonical `attempt.json` and
complete `activation.json` then remain as the exact installation-bound terminal
receipt. Legacy backup and artifact reachability exclude only that two-file
receipt; incomplete, extended, linked, or tampered evidence fails closed. This
qualifies complete-set assembly, staging, ordered activation, checkpointing,
marker and staging retirement, terminal receipt admission, and subprocess
recovery. Production backup/restore wiring and indivisible authority cutover
remain required before this gate can close.

## Non-negotiable cutover invariants

- A production process opens either the legacy stores or the Control Store for
  one installation, never both.
- One reviewed mutation compares its expected generation and commits the
  complete local transition plus exact outbox intents in one transaction.
- Provider, filesystem, network, Runtime, Gateway, Flow, Knowledge, UI, OS,
  and device effects execute only after commit and never while a database
  transaction or store-executor permit is held.
- Applied outcomes retain canonical owner-specific evidence bound to the exact
  idempotency identity and intent. Deferred, rejected, and unknown outcomes
  cannot carry applied state; ambiguity cannot mint a replacement effect.
  Deferred requires proof that no effect was accepted and can retry only the
  same key after its bounded durable not-before time.
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
transaction instead of becoming pseudo effects. Schema v10 also persists and
offline-verifies typed Capability Index, invocation-lease, Runtime
Task/opaque-Service readiness, Flow artifact, Knowledge projection, and
Skill/UI content application evidence. The cutover observation advances
publication before drain; post-cutover required failure remains pending for
same-key reconciliation. Real owner ports and the dispatcher that produce
those observations now receive committed owner-shaped context derived inside
the claim transaction. Static, lease, and Flow/Knowledge/Skill/UI owners see
only one exact package incarnation and Grant; Runtime also sees the complete
reviewed provider selection. Capability Index sees the candidate generation and
the latest terminal preparation for every enabled selected surface, including
retained observations across multi-root generations and optional degradation.
Missing Grant coverage, nonterminal or teardown observations, and generation
drift fail closed before provider I/O. Multi-package generation insertion now
writes all selected package nodes before immediate-foreign-key dependency edges
in the same transaction. The Artifact Store now provides a non-cloneable
verified package lease for those contexts: it holds the global reachability and
per-artifact mutation locks in shared mode; rejects quarantine and incomplete
GC; binds the full package fingerprint, bounded manifest, exact measurements,
catalog surface graph, and surface files; exposes no package root; and can
repeat verification before success is observed. The first concrete
post-commit adapter now qualifies immutable Skill and UI owners. It re-derives
the typed owner and committed idempotency key, reads only the exact named
surface through that lease, re-verifies the complete package after the read,
and returns a stable path-free receipt independent of claim attempt/deadline.
Contention is a safe same-key deferral; tampering, absence, and authority
substitution are proved-no-effect rejection; this read-only adapter never
claims unknown acceptance. Stop/remove remain path-independent projection
receipts. Remaining adapter implementations, dispatcher composition, and
production conversion into the reviewed inputs used by the inactive kernel
remain gate 3 work.

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
