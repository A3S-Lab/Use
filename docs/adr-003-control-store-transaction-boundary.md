# ADR-003: Per-Installation Control Store Transaction Boundary

- Status: accepted target architecture; coordinated authority cutover pending
- Decision date: 2026-08-30
- Architecture: [A3S Use Architecture](architecture.md)
- Lifecycle: [ADR-002](adr-002-cognitive-package-lifecycle-saga.md)
- Cutover inventory: [Control Store Coordinated Cutover](control-store-cutover.md)
- Roadmap: [A3S Use Roadmap](../ROADMAP.md)

## Context

A package-manager mutation is correct only when a restart can observe a state
that the manager actually committed. A cross-process writer fence gives
install, upgrade, uninstall, enable, and disable one order, but ordering alone
does not make independent files atomic.

The preview currently commits one successful graph mutation through several
durable authorities:

1. provider, Grant, receipt, binding, and capability effects complete through
   lifecycle checkpoints;
2. `InstallationSnapshot` commits the desired graph and enablement generation;
3. a read-only terminal diagnostic is retained; and
4. the pending reviewed operation is removed.

Each file replacement is individually durable. A process failure between them
can nevertheless expose a combination that no single commit selected. The
existing saga can reconcile many such combinations, but it cannot turn related
Use-owned metadata into one transaction after the fact.

SQLite is therefore not the architectural decision by itself. Adding a
database while the JSON stores remain authoritative would create another
state machine and make the invariant harder to prove. The decision must define
which facts are authoritative, which effects remain external, and when the
old authority stops being accepted.

## Decision

### One control authority per installation

Each explicit `InstallationId(kind, id)` owns one logical Control Store. The
initial backend is one SQLite database in WAL mode under that installation's
state root. It is an internal package-manager boundary, not a package surface,
provider extension point, or generic application database.

The Control Store is the sole authority for related mutable control facts:

- installation and package state generations;
- requested roots, the resolved graph, selected surfaces, and desired
  enablement;
- canonical reviewed Plan envelope and versioned authorization evidence, with
  derived operation identity, digests, action, root, phase, result, and
  cancellation state;
- Grant intent and committed Grant generation;
- lifecycle checkpoint and effect-attempt identity;
- provider-binding identity and observed application state; and
- capability-index generation and publication checkpoint metadata.

ACL remains the source of truth for product configuration. Verified package
bytes, immutable expanded trees, backups, and immutable projection payloads
remain outside the database. A Control Store row may retain their canonical
digest and ownership metadata, never an ambient local path as authority.

### Typed aggregate commands, not a key-value facade

Callers use typed commands and typed read models around package-manager
invariants. The interface does not expose arbitrary SQL, table-shaped CRUD, or
`put_json(key, value)` operations. A command carries its expected installation
and package generations and either commits the whole local control transition
or changes nothing.

The implementation keeps schema details behind the store boundary, but it
does not introduce repository interfaces for every table. Tables are an
implementation of one package-control aggregate, not independent domain
services.

### The activation cutover is indivisible

The SQLite backend may be developed and qualified before production wiring,
but it must not be activated as a mirror of existing JSON authority. The first
production cutover switches all readers and writers for the following minimum
aggregate together:

```text
installation graph and package generations
+ reviewed graph/enablement operation state
+ desired and committed Grant state
+ lifecycle checkpoint/outbox identity
+ provider-binding identity
+ capability generation metadata
```

Artifact reachability, diagnostics, backup, restore, and clean-state detection
must switch in that same cutover. The old files are neither fallback reads nor
repair input. Under the preview policy, an installation containing unsupported
legacy mutable authority fails closed and is reinstalled into a clean state
root; a future released migration policy requires a separate decision.

This rule deliberately forbids dual writes. A derived export or immutable
projection may be regenerated from a committed database snapshot, but it can
never be consulted to choose desired package state.

### Transactions stop before external effects

Runtime, Gateway, Flow, Knowledge, filesystem, network, UI, OS, and device I/O
cannot join a SQLite transaction. A transaction therefore performs only
bounded local validation and database work:

1. compare the reviewed installation and package generations;
2. commit the desired control transition and exact effect intents;
3. commit deterministic idempotency keys in an outbox; and
4. return only after the local commit is durable.

After commit, a dispatcher claims one outbox item without retaining an open
database transaction, calls the typed provider port, and records one of these
observations in a later transaction:

- `applied`: a canonical owner-specific application descriptor rebinds the
  exact idempotency key, intent, provider selection, and portable materialized
  evidence;
- `rejected`: the provider proves that no effect was accepted; or
- `unknown`: acceptance cannot be proven either way, so retry or operator
  reconciliation must reuse the same identity and fail closed on conflicting
  evidence.

An in-progress claim is a lease, not completion authority. Expiry permits an
exact replay; it does not mint a new effect identity. No transaction or store
executor permit may be held while awaiting provider or filesystem I/O.

Only facts that cannot join the Control transaction enter this outbox. Package
selection, lifecycle generations, Grants, reviewed provider selection, and the
candidate capability descriptor are aggregate facts; inventing apply, revoke,
commit, or binding effects for them would create a second authority. The
external inventory is limited to typed surface preparation, capability
cutover, accepted-call drain, and surface stop or removal. Install and enable
prepare dependency surfaces before dependants and then cut over. Upgrade
prepares the candidate, cuts over, drains prior calls, and removes prior
surfaces in reverse dependency order. Disable and uninstall cut over before
drain and reverse-order retirement.

Each intent names the port that owns it: Capability Index, invocation leases,
the exact reviewed Runtime provider selection, or a static Flow, Knowledge,
Skill, or UI host. A caller cannot choose the sequence, owner, required policy,
generation, or idempotency identity. Selected optional surface preparation may
be rejected as degraded; the dependency closure of mandatory surfaces and all
retirement effects remain required.

Capability publication occurs when the exact typed cutover application is
recorded, not when the surrounding operation later completes. That observation
transaction retires the prior publication, publishes the candidate, and
advances the capability cursor atomically. Drain and retirement then reconcile
against an already-visible generation. A required failure after cutover must
retain `effects-pending` state and reuse the same effect identity; rolling the
cursor back would expose a combination that never existed. Event timestamps
remain ordered as transition commit, provider observations, then terminal
operation completion.

### Relational constraints carry ownership invariants

The schema uses foreign keys, `CHECK` constraints, uniqueness, and
compare-and-swap predicates to make invalid ownership combinations
uncommittable. At minimum it enforces:

- exactly one validated installation identity per database;
- one selected package identity per installation generation;
- roots and dependency edges referencing selected packages in that generation;
- monotonically increasing installation and package state generations, with a
  separate positive immutable lifecycle generation for each selected package;
- one canonical reviewed Plan envelope and authorization record per operation
  ID, with digests, action, root package, installation scope, and generation
  cursors checked against relational projections;
- installation-scoped graph checkpoints referencing the exact target
  installation and capability generation, while package and surface
  checkpoints additionally reference their immutable lifecycle generation;
- capability and provider-binding metadata referencing the generation that
  selected it.

Domain validation still runs before SQL. Database constraints are the final
commit guard, not a replacement for typed Rust invariants.

### Bounded asynchronous access

SQLite work runs through a bounded store executor. Async callers may wait for
capacity, but they never execute blocking database work on a Tokio runtime
worker. Connection count, queued work, statement input, result size, and busy
time are bounded. Connections enable foreign keys, use WAL with full
synchronous durability, and reject an unknown schema version or installation
identity before serving reads.

The existing cross-process installation-mutation fence remains the outer order
for reviewed lifecycle mutations. SQLite transactions enforce local atomicity
and compare-and-swap for all writers, including recovery and maintenance
tools; the fence is not treated as a substitute for database constraints.

### Backup and restore use store-owned snapshots

A live WAL database is never backed up by copying its main file and hoping the
WAL is empty. The Control Store supplies a consistent snapshot/export under
the installation maintenance boundary. Backup inventory is derived from:

- the Control Store schema/version and its deterministic export; and
- a typed registry of external payload owners, each of which produces an
  immutable snapshot for that backup.

It is not derived from a second hand-maintained list of mutable state paths.
WAL, shared-memory, lock, staging, and temporary files are operational state
and are excluded.

Offline verification checks archive integrity, canonical export encoding,
schema version, installation identity, row constraints, referenced payload
digests, and inventory bounds without consulting live state. Restore stages a
new database and external payload set, verifies both completely, and activates
them under the exclusive maintenance boundary. Corruption is diagnostic
evidence; the manager must not reconstruct missing authority from receipts or
provider observations.

## Implementation order

1. Separate current installation-snapshot persistence from shared JSON I/O and
   pending-operation persistence, and keep behavior unchanged while the
   transactional boundary is introduced.
2. Implement the bounded SQLite/WAL engine, schema, typed aggregate commands,
   deterministic export, offline verifier, and clean/corrupt-state tests with
   no production dual-write wiring.
3. Convert lifecycle orchestration to persist desired transitions and outbox
   intents before effects, then record exact provider observations after each
   effect.
4. Switch the complete minimum aggregate and every reader, reachability,
   diagnostic, backup, and restore consumer in one coordinated cutover.
5. Delete legacy mutable stores, manual mutable-state inventory entries, and
   any recovery path that could reintroduce a second authority.

No roadmap checkbox is complete merely because a table or trait exists. The A2
exit gate is satisfied only by crash tests that prove every observable graph,
Grant, enablement, operation, binding, and capability combination corresponds
to a committed transaction plus explicit external-effect observations.

### Current qualification status

The checked-in inactive kernel implements most of the local Control Store work
in step 2, not the authority cutover. Schema v10 binds one exact installation
and stores the complete generation history, canonical reviewed Plan envelopes,
versioned authorization evidence, exact installation snapshots and relational
graph projections, canonical full Workspace Grants, provider bindings,
capability publication states, lifecycle checkpoints, and effect outbox
observations. Plan and authorization bytes are size-bounded canonical JSON;
their derived operation identity, digests, action, root package, installation
scope, and generation cursors are revalidated against relational projections
after restart, in deterministic export verification, and during staged restore.
Installation generation, desired package-state generation, immutable
package-lifecycle generation, and Grant receipt revision remain separate.
Authorization evidence v2 stores the exact prior Grant snapshot, reviewed
change set, and confirmation facts, but not caller-supplied resolved Grants or
ceilings. A deterministic projection derives the complete next snapshot, both
package generation axes, and the complete target Grant inventory from the
canonical reviewed evidence, exact prior generation, and bounded committed
history. The same projection runs before a database commit and during offline
export or restore verification; it covers all lifecycle actions, User and
Workspace installations, shared dependencies across roots, and removal
followed by reinstall without reusing lifecycle identity. The projection also
derives the complete reviewed Runtime provider selection for enabled Tool and
MCP surfaces, preserves unrelated selections, and removes disabled or removed
ones. Each selection retains canonical provider build, capability, semantics,
and enforcement evidence. The candidate capability digest is derived from the
target snapshot, package lifecycle identities, Grant revisions, and those
selections. Neither projection claims an applied endpoint, readiness, compiled
artifact, or Knowledge observation before the corresponding external effect.
The same projection derives the complete external-effect sequence for all five
actions. It binds typed owners, exact reviewed Runtime selections, artifact and
surface incarnations, dependency order, required policy, current or immediately
prior generation, and the installation-scoped capability cutover. Canonical
payload bytes, a domain-separated idempotency key, their digest, and relational
projection commit together. Package, Grant, lifecycle, and reviewed provider
facts do not reappear as pseudo effects. Typed transactions
enforce cursor compare-and-swap, root/action semantics, exact replay,
required-effect failure, and capability retirement. Applied observations store
canonical owner-specific application descriptors: Capability Index and
invocation-lease receipts, exact Runtime selection plus portable Task or opaque
`gateway:` Service readiness evidence, Flow artifact digests, Knowledge
projection digests, and immutable Skill/UI content digests. Rejected and
unknown outcomes cannot carry applied state and retain only bounded diagnostic
evidence. The cutover application advances publication in its observation
transaction; post-cutover required failures stay reconciliation-pending rather
than inventing a rollback. An owner-proven safe-no-effect result persists a
bounded not-before time and becomes automatically eligible only with the same
key. Expired, unknown, or post-cutover rejected claims require an explicit
reconciliation request and retain that effect idempotency key, including after
process restart. Completion and offline verification enforce
commit-before-observation-before-terminal time ordering.

The inactive kernel now includes a one-effect dispatcher that exercises this
boundary without activating it. Claim is one short Control transaction. The
dispatcher first acquires one installation-wide shared maintenance fence and
retains it through the later observation, then releases both the claim
transaction and the bounded executor before
routing the exact committed identity through a separate typed Capability Index,
invocation-lease, Runtime, Flow, Knowledge, Skill, or UI port. A second short
transaction records owner-shaped applied evidence or explicit
deferred/rejected/unknown failure evidence. A deferred observation proves that
the owner accepted no effect, blocks claims until its bounded durable
not-before time, and then permits automatic same-key retry. Provider timeout is
enforced by the dispatcher, must leave a fixed observation budget inside the
claim lease, and becomes an unknown observation; it is never inferred to be a
rejection. The provider runs in an owned task holding a reference to the same
shared maintenance guard. Timeout or caller cancellation detaches the wait but
cannot cancel that possibly accepted task; an exclusive restore remains fenced
until the task actually completes. Process exit between effect and observation,
expired claim, and unknown acceptance converge only through explicit replay of
the original idempotency key. Qualification tests also let a provider re-enter
the Store during its call, prove no executor permit spans the I/O, retain the
fence across timeout and cancellation, and classify task panic as unknown. The
immutable Skill/UI port is now the first
concrete post-commit adapter. It reconstructs and revalidates its typed owner
and original idempotency key from the owner-shaped committed request, acquires
package evidence only through the verified Artifact Store lease, reads one
exact named surface without exposing a package root, and re-verifies the full
package after that bounded read. Its path-free receipt is stable across claim
attempts and deadlines. Artifact contention is deferred only with proof that no
effect was accepted; package drift or authority substitution is rejected; the
read-only operation cannot produce unknown acceptance. Stop and remove are
path-independent projection receipts. The OKF Knowledge port is now the second
concrete post-commit adapter. It accepts only the exact committed owner-shaped
request, obtains first-use OKF files as a verified path-free payload, persists
staged receipt evidence before promotion and promoted evidence before applied
observation, and replays an existing promoted projection without Artifact
access. Safe pre-effect contention is deferred, invalid authority or content is
rejected, and every potentially accepted stage, promotion, removal, or later
receipt-write failure is unknown. A real SQLite composition fixture now proves
claim, owner effect, and Control observation under the dispatcher fence.
Immutable package admission has also been separated from lifecycle authority:
it revalidates and idempotently materializes prepared bytes without creating an
installation receipt, while the caller retains global reference admission
through the distinct Control commit. The third concrete adapter implements the
Capability Index and invocation-lease ports as one consistency boundary. It
materializes a canonical content-addressed document from the committed
candidate generation and exact terminal surface observations, but creates no
second mutable publication store: the applied Control cutover observation is
the only current cursor. Admission reads that cursor before and after acquiring
shared locks for every exact package lifecycle incarnation. Drain requires the
old incarnation to be absent from the current cursor and then acquires its
exclusive lock, safely deferring under an active call. Immutable Index files
publish with no-replace/no-follow semantics and exact crash staging replay.
They and the lease files are derived operational state excluded from backup;
restore must rebuild the Index from verified Control evidence before consumer
cutover. A real composition fixture covers Knowledge and Skill preparation,
Index publication, stale admission, and same-key drain retry. Inactive Flow and
Runtime owner adapters are now qualified. The Runtime owner covers
release-backed Tool Task/Service and Streamable HTTP MCP, consumes only a
verified path-free Artifact payload on first prepare, persists monotonic
Runtime/Gateway recovery evidence, replays final receipts without Artifact
access, and treats every post-effect ambiguity as unknown. These ports are not
production-composed yet. The qualified host-owned
`RuntimeSurfacePlanStore` now gives production a bounded, canonical,
no-clobber payload source, but lifecycle conversion must publish the exact
records before committing their Control effects. Production must reconstruct
the complete Runtime plan after restart from that source plus exact committed
semantics evidence and immutable artifacts, then cut over the dispatcher
without any legacy mutable-file authority.

The kernel uses WAL with full synchronous durability and foreign-key
enforcement, rejects unknown schema or filesystem state, and serializes
blocking work through one 16-entry bounded worker. Its size-bounded canonical
export contains the full aggregate, performs semantic verification without the
live database, and can be restored only through a clean staged database whose
authority must round-trip exactly. Tests cover atomic rollback, concurrent
generation races, corruption and relational drift, linked-path substitution,
multi-generation capability history, independent package generation axes,
empty-graph uninstall, restart-stable canonical payloads, self-consistent
artifact/capability tampering, incomplete outbox inventories, outbox ambiguity,
deterministic export, lifecycle-reference tamper detection, and staged restore.

The kernel remains private and no production lifecycle constructs it. Its
path-free external-payload contract now freezes five typed owner identities and
their ACL backup policies. The global Artifact Store is excluded; the other
four owners must form one complete, deterministic receipt set bound to an exact
installation, Control generation, registry digest, owner snapshot schema,
manifest/inventory digests, and bounded accounting. Decoded registry and
snapshot evidence must revalidate before use. This registers the identity and
evidence boundary for every owner. A private live snapshot session additionally
freezes one canonical Control export and binds its digest, generation,
installation, and owner-registry digest while the same exclusive maintenance
fence remains held. No SQLite transaction or bounded-executor permit crosses
owner I/O. The Knowledge owner implementation snapshots an existing
scope-local OKF SQLite/FTS5 Knowledge database into a non-overwriting archive,
binds its exact retained-binding and selection inventory, enforces the
registered archive/manifest bounds before publication, and re-verifies the
archive offline. Absence is an explicit zero-file state and creates no live
Knowledge directory; linked roots and decoded or rebound evidence fail closed.
Both live snapshot issuance and offline acceptance semantically verify the
exact canonical Control export named by the binding. Each retained Knowledge
incarnation must join its originating prepare intent and committed OKF bundle;
applied prepare evidence must match the retained observation and capability
projection. The join runs against the temporary SQLite snapshot before the
destination archive is written, so semantic failure publishes neither archive
nor receipt. Removed or pruned applied payload requires a same-incarnation
remove effect. Deferred effects remain safe-no-effect scheduling evidence;
claimed and unknown effects remain reconciliation evidence. None can mint
desired state. The same owner now has a clean-target two-phase
restore boundary. Only an offline-verified snapshot can stream its exact
database into a caller-owned directory beneath the target state root. Staging
does not touch the live Knowledge root and re-audits the database plus the
canonical binding/selection inventory. Activation requires the exact
installation's exclusive maintenance guard, refuses any existing live payload
or unowned live-layout entry, rejects ambiguous candidate state, and publishes
by atomic rename. An exact completed partial is replayable. While the same
staged attempt and exclusive guard remain held, retry after rename but before
the canonical path-free result reconciles the exact live database; an absent
snapshot creates no Knowledge state. This owner primitive deliberately does
not choose cross-owner activation order or create a second restore journal: the
complete-set restore coordinator must own those facts.
The planning-and-diagnostic observation owner is the second concrete snapshot
and clean-target restore adapter. It reuses the cognitive stores' record
decoders and invariants, archives only terminal diagnostic histories and
terminal resolution attempts, and excludes active resolution/download attempts
plus locks from restore authority. The excluded active count and canonical
path/digest inventory remain manifest-bound. Bounded no-follow traversal,
duplicate checks, a second live scan, no-clobber publication, and offline
verification reject drift, substitution, foreign or moved records, unknown
layouts, and trailing bytes. The receipt is path-free and bound to the exact
Control export. Only an offline-verified archive can enter a target-local
staging directory. First activation requires a clean record inventory and the
exact exclusive maintenance guard, then changes the archive candidate to an
activating marker before any per-record publication. Digest-named deterministic
partials make incomplete publication replayable; an activating attempt accepts
only an exact snapshot subset and the final result is path-free and
snapshot-bound. This owner primitive does not choose cross-owner activation
order or create a second restore journal.
The Host protocol projection is the third concrete snapshot and clean-target
restore adapter. Its owner-native validator treats request-to-plan records,
optional outcomes, and cancellations as semantic sources while operation
aliases and latest-enablement diagnostics remain validation-only derived
indexes. Missing, stale, or orphaned indexes fail closed; exact and legacy
cancellation aliases collapse to one canonical binding. A bounded no-follow
scan and a second scan precede no-clobber publication. Before any archive is
published, the exact Host Plan, completion or cancellation time, completion
result digest, package identity, desired state, selected surfaces, package
generation, and capability generation must reconcile with the bound Control
export. Receipt and observed-health fields remain Host observations. The
path-free manifest and offline verifier preserve active, terminal, and
no-change request semantics without allowing the projection to choose Control
state. Only an offline-verified snapshot can enter a target-local staging
directory and build a complete candidate owner root. Exact semantic source
bytes are retained; canonical exact operation and latest-enablement indexes are
re-derived with owner code, while legacy aliases and locks are omitted.
Activation requires the exact target's exclusive maintenance guard and an
absent live owner root, validates the complete physical tree and owner-native
semantic inventory, persists a snapshot-bound activation marker, and publishes
the whole root with one atomic no-clobber directory move. Deterministic archive,
record, and marker partials recover each pre-publication boundary, and the same
attempt revalidates exact live state after publication before returning its
path-free result. This primitive still does not choose cross-owner activation
order or create a second restore journal.
The Restore Coordinator is the fourth concrete snapshot owner. Its native
journal decoder admits only exact canonically encoded completed operations that
belong to the bound installation. The active marker and its exact operation are
excluded from payload authority, but a bounded digest inventory records those
files and the marker-only handoff window. Orphaned nonterminal records, pruning
or temporary residue, unknown entries, links, foreign history, and path/record
rebinding fail closed. Snapshot publication is no-clobber after a second live
scan; its path-free receipt and streaming verifier bind the exact terminal
bytes to the Control export. Empty or active-only state produces no archive.
An offline-verified snapshot now stages an immutable target-local candidate.
The adapter cannot use the other owners' clean-target rule because the active
whole-installation restore itself owns this journal. Instead, activation
requires the exact exclusive maintenance guard and active marker, preserves the
marker and current operation, and durably binds the stable active identity plus
exact before/source/target inventories before mutation. It atomically moves
replaced terminal directories into retained staging evidence and publishes
candidate records without replacement. Marker-only handoff remains valid, and
replay permits a retained active operation status to advance only while its
marker binding stays unchanged. With a legacy whole-installation marker, a full
64-record source drops exactly the source-native oldest terminal record under
the journal's `(completed_at_ms, started_at_ms, plan_digest)` ordering,
reserving one slot for the active operation. A typed complete-set marker has no
retained operation and preserves all 64 source records. An active/source
identity collision is rejected before pruning. Every candidate, activation
marker, retired record, and deterministic
publication partial is validated at each replay boundary, and the result is
path-free and snapshot-bound.
The private complete-set snapshot coordinator now acquires the already-frozen
Control export and captures every registered external owner under the same
exclusive maintenance fence and timestamp. One canonical path-free manifest
binds the fixed registry, all receipts, owner schemas and digests, and exact
byte accounting. Its fixed-order streaming archive is staged outside all Use
data and state roots, fully audited through the existing owner-native offline
verifiers, and only then published without replacement. Explicitly absent
owners add no payload bytes; the global Artifact Store remains excluded. This
implementation still does not participate in production state layout,
reachability, diagnostics, backup, or restore orchestration. Existing JSON
stores remain the only production authority. The verified aggregate now has a
qualification-only clean-target staging coordinator. It retains one exact
exclusive maintenance guard, durably binds the snapshot descriptor,
installation, owner registry, Knowledge policy, and fixed component set in a
path-free attempt descriptor, and stages Control plus every external owner
beneath one fixed `.control-installation-restore` directory. Control is rebuilt
from and round-tripped against the canonical export, checkpointed to a single
SQLite file, and physically digest-bound. External candidates reuse their
owner-native adapters under the same guard. Staging changes no live authority
path; exact retries and interrupted Control construction recover, while
contaminated targets, links, unknown state, rebinding, and completed-candidate
drift fail closed. The complete-set coordinator now qualifies full ordered
activation. It preflights every candidate and clean target before durable
intent. The immutable attempt descriptor remains the restore identity. One
canonical `activation.json` is the mutable ordered journal, and the typed
global `.maintenance.restore.json` marker binds the same attempt and immutable
operation while blocking ordinary shared access. Control Store, Host
projection, Knowledge, observations, and Restore Coordinator execute in that
fixed order; each follows journal, marker, owner effect, checkpoint. Every
checkpoint stores only the canonical path-free result length and a
domain-separated digest. The Restore Coordinator also verifies the exact
complete marker bytes, length, and digest before replacing terminal history.
Reopen reacquires the exact exclusive guard and rebinds the same verified
snapshot, attempt, registry, and Knowledge policy before reconstructing or
verifying every owner at its candidate/live boundary. Journal and marker
partials, each post-effect/pre-checkpoint boundary, the final checkpoint before
marker retirement, and process exit after deletion converge deterministically.
Marker absence is accepted only after all five checkpoints are durable;
out-of-order live roots, ambiguous markers, rebinding, links, and evidence drift
fail closed. Exact completed replay performs no owner effect and can only resume
bounded fixed-order retirement of the five link-free staging trees. A
real-child-process matrix covers 18 top-level durable exits, including every
retirement boundary. The canonical `attempt.json` and complete `activation.json`
then remain as the exact installation-bound terminal receipt. Legacy backup and
artifact reachability exclude only that receipt; incomplete, extended, linked,
or tampered evidence fails closed. Production lifecycle conversion and
dispatch, backup/restore wiring, indivisible reader/writer cutover, and deletion
of legacy mutable stores remain open; activating or mirroring the kernel before
that coordinated change would violate this decision.

The machine-checked
[cutover inventory](control-store-cutover.md) now freezes every supported
legacy authority leaf, retained external owner, operational file, and consumer
that must change with production activation. It proves inventory completeness
against the current state layout and forbids dual-write or fallback-read
activation. This completes an architecture prerequisite only; it does not wire
the kernel or satisfy an A2 exit gate.

## Consequences

Benefits:

- related Use-owned control facts gain one commit point;
- recovery distinguishes committed intent from external effect observation;
- generation and ownership invariants become enforceable below orchestration;
- backup inventory follows actual ownership instead of filesystem convention;
  and
- arbitrary agent integrations observe portable committed generations rather
  than transient provider layout.

Costs:

- A2 requires a coordinated cutover rather than isolated store migrations;
- provider calls remain sagas and need explicit unknown-outcome handling;
- backup and restore must use database-aware snapshot APIs; and
- legacy preview state cannot be silently imported or treated as fallback
  authority.

## Rejected alternatives

### Keep atomic JSON files and add more reconciliation

This preserves several independent commit points. More replay logic can repair
known windows but cannot prove that a combination was ever committed.

### Add SQLite as a cache or mirror

Two writable representations require conflict resolution and immediately
violate the single-authority invariant.

### Hold one database transaction across provider calls

Provider latency and failure would retain locks and transactions without
making the external effect atomic. A timeout would still leave acceptance
unknown.

### Put package bytes and projections in SQLite

Large immutable content has different deduplication, verification, serving,
and retention needs. Moving it into the Control Store would combine byte
custody with lifecycle authority and make global sharing installation-scoped.

### Make the Control Store a public pluggable backend now

The invariant boundary is still converging. A public backend contract would
freeze storage mechanics and encourage implementations that cannot provide
the required transactions, export, restore, and failure semantics.
