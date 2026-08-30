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

- `applied`: the exact idempotency key and evidence prove the effect;
- `rejected`: the provider proves that no effect was accepted; or
- `unknown`: acceptance cannot be proven either way, so retry or operator
  reconciliation must reuse the same identity and fail closed on conflicting
  evidence.

An in-progress claim is a lease, not completion authority. Expiry permits an
exact replay; it does not mint a new effect identity. No transaction or store
executor permit may be held while awaiting provider or filesystem I/O.

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
in step 2, not the authority cutover. Schema v6 binds one exact installation
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
followed by reinstall without reusing lifecycle identity. Canonical effect payloads bind
scope, plan, action, provider, artifact digests, subject, and capability
identity; their bytes and digest are committed with a relational projection.
Package and surface foreign keys bind the exact current or immediately prior
incarnation, while one installation-scoped graph
effect represents the real atomic capability cutover. Typed transactions
enforce cursor compare-and-swap, root/action semantics, exact replay,
required-effect failure,
and capability retirement. Expired or explicitly unknown claims require an
explicit reconciliation request and retain the same effect idempotency key,
including after process restart.

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

The kernel remains private and no production lifecycle constructs it. It does
not yet register external artifact/projection payload owners or participate in
live state-layout, reachability, diagnostics, backup, or restore orchestration.
Existing JSON stores remain the only production authority. Deterministic
lifecycle conversion into reviewed Grant evidence, plus derivation of provider
bindings, capability descriptors, and effect intents, the full process-exit
matrix, the indivisible reader/writer cutover, and deletion of legacy mutable
stores remain open;
activating or mirroring the kernel before that coordinated change would violate
this decision.

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
