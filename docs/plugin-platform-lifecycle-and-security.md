# A3S Use Plugin Lifecycle and Security

Status: development preview
Last updated: 2026-08-07

## Purpose

This document defines lifecycle ordering, trust boundaries, recovery rules, and
security invariants for the current A3S Use cognitive-package baseline.

The product has not shipped a supported release. Superseded preview formats and
disk records are rejected rather than migrated or interpreted.

## Threat model

A3S Use assumes that package archives, Registry responses, package manifests,
surface payloads, local package directories, managed state directories, and
provider processes may be malformed or malicious. It also assumes ordinary
process crashes, partial filesystem writes, stale plans, expired metadata,
concurrent commands, and host/provider drift.

The engine must prevent:

- package or path escape;
- unsigned or provenance-drifting code/data activation;
- dependency confusion across enabled sources;
- mixed package or capability generations;
- permission escalation after review;
- provider substitution after planning;
- visibility before required readiness;
- removal while prior calls still hold leases;
- replay under a different scope, policy, plan, or confirmation; and
- heuristic recovery after exact evidence was deleted.

## Trust chain

```text
host Registry configuration
  → TUF root/timestamp/snapshot/targets
  → complete catalog-v3 record in custom.a3s
  → archive target + length + digest
  → expanded package + manifest digest
  → exact package lock
  → immutable operation plan + confirmation
  → package receipt + lifecycle journal
  → Registry cutover + capability evidence
```

Every arrow is verified. No later object may omit or weaken an earlier
identity. Registry/TUF receipts require both resolved source provenance and the
complete verified catalog record.

## Security principals

| Principal | Authority |
| --- | --- |
| User/host policy | Select sources, approve plan, grant bounded permissions |
| Plugin Manager | Produce immutable plan and apply only reviewed evidence |
| A3S Use | Verify/install packages and coordinate exact lifecycle |
| Runtime/Gateway/Flow/Knowledge host | Execute or project only assigned, authorized generation |
| Package process/content | Implement its declared native protocol; no policy authority |
| UI/Skill | Static content with only explicit host bindings |

A package cannot choose its trust root, source, provider, secret, workspace
scope, or Grant.

## Operation state machine

```text
planned ──confirm──▶ applying ──all checkpoints──▶ completed
                       │
                       ├── pre-cutover failure ──▶ rolling-back ──▶ rolled-back
                       │
                       └── process crash ──▶ exact replay of applying
```

`completed` and `rolled-back` are terminal. A rolled-back operation ID cannot
be reused with a new plan. A completed operation returns its stored result.

Operation IDs, scope, actor, policy authority, plan digest, lock digests,
confirmation, and authorization are immutable once apply begins.

## Package install

1. Refresh/verify the host-selected Registry set.
2. Resolve the bounded SemVer dependency closure.
3. Freeze the exact package lock and plan-v4 envelope.
4. Obtain policy decision and exact confirmation.
5. Revalidate catalog, lock, host/provider capability, scope, and state.
6. Download only selected archive targets and verify length/digests.
7. Validate package root, ACL manifest, required `README.md`, and surface graph.
8. Commit immutable package generations as installed-disabled.
9. Persist candidate Workspace Grants.
10. Prepare dependencies before dependents across typed surface hosts.
11. Publish the complete changed closure through one durable Registry cutover.
12. Record package and Grant cutover evidence, then retire the Registry replay
    record.

No required surface is visible before step 11. A failure before cutover removes
unpublished candidates and restores the prior Registry snapshot.

## Package upgrade

Upgrade binds both exact prior and candidate locks. Each union node is
classified as Add, Replace, Remove, or Retain.

```text
download Add/Replace only
→ prepare Add/Replace dependency-forward
→ publish candidates and remove obsolete routes atomically
→ mark prior receipts hidden after route absence is proven
→ drain calls admitted by the prior snapshot
→ revoke exact prior Grants
→ remove Replace/Remove generations reverse-prior-lock
```

Retain is accepted only when installed receipt, catalog evidence, source
provenance, selected surfaces, and package generation match the lock exactly.

After cutover, rollback to an old mixed graph is forbidden. Recovery finishes
retirement. Before cutover, package and Grant candidates roll back together.

## Package uninstall

Uninstall begins from the installed graph record and exact lock. It refuses to
remove a package still referenced by another installed graph.

1. Plan the exact removal closure in reverse dependency order.
2. Atomically remove its routes from the Registry snapshot.
3. Record package-keyed hide evidence and Grant cutover.
4. Drain route leases for every removed generation.
5. Revoke exact prior Grants.
6. Stop and remove surfaces in reverse dependency order.
7. Delete only receipt-owned immutable roots and records.
8. Remove the installed graph record after all package removals succeed.

The engine never scans and deletes “similar” paths. Missing exact graph evidence
is an error, not permission to infer an uninstall set.

## Enable and disable

Enablement changes desired visibility without changing package bytes or the
dependency lock. It uses a separate monotonic state generation.

### Planning

The request binds package ID, complete scope, desired boolean, expected state
generation, and operation ID. Planning returns:

- `Planned` with an exact plan-v4 retained-artifact transition; or
- terminal `NoChange` without a mutation plan.

### Enable apply

```text
verify exact installed artifact and receipt
→ persist candidate Grant if required
→ prepare surfaces dependency-forward
→ publish exact package generation through durable cutover
→ commit Grant cutover
→ store terminal result
```

### Disable apply

```text
verify exact installed artifact and receipt
→ hide exact package generation through durable cutover
→ commit Grant cutover
→ drain calls admitted by prior generation
→ revoke prior Grant
→ stop surfaces reverse-order
→ store terminal result
```

Manager clients cannot call direct enable/disable mutation tools. They plan and
then call `plugin_apply_plan` with the reviewed operation ID and plan digest.

## Atomic cutover rules

Visibility mutation is owned by cutover-aware host methods. Each returns:

- package-keyed lifecycle evidence;
- Registry generation before and after; and
- exact immutable snapshot digest.

Host traits do not contain a fallback publisher. A host unable to prove the
cutover cannot implement the current trait.

The Registry persists bounded replay records until both package and Grant
journals own the evidence. Reusing an idempotency key with a different request
fails before mutation.

Prior-generation receipt retirement is separate from visibility mutation. It
requires the exact prior route to be absent. If the route is still present,
retirement fails before changing the receipt.

## Workspace Grants

The signed catalog contains the maximum permission ceiling. Policy can grant a
subset only. Grant identity binds:

- complete User/Workspace scope;
- package and selected surfaces;
- immutable package generation;
- operation and plan;
- permission set and ceiling digest;
- policy authority/revision; and
- expiry/confirmation evidence.

Planning snapshots current Grants and emits canonical changes and resolutions.
Apply recomputes them. Scope-kind substitution, stale revision, altered
confirmation, changed ceiling, or provider drift fails before side effects.

Two active granted generations for the same package make the scope unstable and
block planning until the owning operation recovers.

## Registry concurrency

Lifecycle mutations remain serialized by the cross-process Registry lock.
Steady-state snapshot and watch reads consume immutable publications without
that lock. A read may briefly acquire it only for crash reconciliation when
receipt state and the last publication disagree.

A lifecycle writer waits asynchronously for at most two seconds for that
transient reconciliation to finish. If the lock is still held, the operation
returns `use.extension.busy` and keeps its existing durable recovery evidence.
This prevents a live Code watcher from spuriously rejecting the next reviewed
upgrade without hiding genuine concurrent mutation.

## Provider security

### Runtime and Gateway

Tool and MCP provider selection is host-owned. The separately signed planning
target may select the built-in native provider only for an exact package-local
Tool Task or stdio MCP launcher. Release-backed Tasks, Services, and HTTP MCP
require an explicitly injected Runtime or Gateway provider. Final apply binds
the same provider ID, build, capabilities, target, interface, and activation
semantics.

After download, the planning target is rebound to the digest-bound manifest
and release descriptors before lifecycle admission. Required provider failure
stays unpublished; there is no `PATH` lookup, unsigned native fallback, or
provider substitution.

The installed schema-v3 receipt retains the exact signed planning bundle after
validating it against catalog, manifest, and package bytes. A host can therefore
plan enablement after restart without fetching mutable Registry metadata. It
persists the bundle, Grant snapshot, and provider generations with the reviewed
plan, then reconstructs and compares the provider evidence again at apply.

Retirement is not provider selection. Disable, uninstall, and prior-generation
upgrade cleanup reconnect the provider named by the durable binding receipt.
Services reverify provider ID, build, normalized capabilities, and lifecycle
features before Gateway drain and provider removal. A stopped same-generation
binding whose authorization semantics changed is retired before re-enable
creates its replacement.

Persistent Services consume only the exact loopback endpoint published inside
the matching Runtime observation. During retirement, the Gateway binding is
hidden and drained before the Runtime unit is stopped; the route is removed
before the unit is removed, and the binding receipt is deleted last. Every
step is generation-bound and idempotent.

### A3S Flow

The host must inject the declared `a3s-flow` adapter. Exact source/export,
package generation, dependency edges, and preflight evidence are bound. A
`flow.json` document cannot authorize or publish a Flow independently.

### Knowledge

OKF promotion is atomic and scope-isolated. Query authorization requires an
exact current or leased projection. Search results cite package, surface,
generation, index, concept path, and source digest. Removed projections become
invalid immediately after receipt-owned retirement. The Use Registry exposes
exact published-generation lease acquisition from package, manifest, and
lifecycle-generation identity. A managed host must hold that lease for the
entire accepted query so lifecycle drain precedes receipt-owned removal. The
standalone backend
accounts immutable receipt `expanded_bytes` across the complete scope before
staging, independently bounds retained projections and per-surface
generations, globally prunes removal tombstones, and compacts SQLite plus its
WAL after removal. User and Workspace scopes with the same textual ID remain
physically distinct.

### UI and Skill

Static content is integrity-bound and host-rendered. UI requires sandbox,
origin, CSP, navigation, and backend-binding policy from the embedding host.
Neither UI nor Skill receives ambient filesystem, network, process, or secret
authority.

## Filesystem and archive safety

The package and state stores enforce:

- canonical relative paths;
- no parent traversal or absolute paths;
- no symlink/reparse-point traversal;
- no hard-link or duplicate normalized archive entry ambiguity;
- bounded file count, per-file bytes, and expanded bytes;
- immutable content-addressed generation roots;
- package-owned selected and retained receipt paths;
- exclusive operation/Registry locks; and
- atomic file replacement with directory synchronization where available.

Temporary files are created inside validated owned directories. Tests must not
leave roots, locks, sockets, or provider processes behind.

## TUF and Registry failure rules

Fail closed on:

- expired or rollbacked metadata;
- root, timestamp, snapshot, targets, length, or digest mismatch;
- incomplete or malformed `custom.a3s` catalog data;
- catalog/archive/manifest/package disagreement;
- same package in multiple enabled sources;
- changed source identity for an installed receipt; and
- missing verified catalog evidence in a Registry/TUF receipt.

Online target verification stores archives and separately signed planning
targets by SHA-256 under the Registry datastore. Writes stream the target,
verify its signed length and digest, atomically replace the destination, and
synchronize the file and containing directory. A dedicated target-cache lock
coordinates readers and writers. Cache paths, entries, and staging names reject
links, non-regular files, traversal, and unexpected lengths.

An interrupted online download retains only
`.target-<sha256>.part` in the same Registry cache. Its regular-file length is
the next requested byte. HTTPS redirects cannot downgrade; a `206` response
must carry the exact `Content-Range` and remaining length. If a server ignores
Range with a complete `200`, the old partial is truncated before the response
is written. A mismatched range, oversized body, symlink, non-regular partial,
or final digest mismatch fails closed; unsafe partials are discarded. A fully
written partial left before promotion is reverified and promoted without a
network request on the next operation.

Cached install or upgrade is available only through an explicit offline path.
It revalidates the locally trusted TUF metadata, including signatures and
expiry, and binds the same Registry name, URL, trust-root digest, catalog,
package lock, planning target, archive length, and archive digest as an online
operation. Every cached target is streamed into fresh staging and rehashed;
the digest-shaped filename is never trusted as evidence. Missing, expired, or
tampered evidence fails closed.

There is no implicit cache fallback after a network or metadata-refresh
failure, and explicit offline mode performs no network request. Registry
replacement changes future source selection, not historical receipt evidence.

The standalone host persists the bounded enabled source set in canonical ACL.
Authority-changing source operations require the exact revision returned by
the preceding list operation plus explicit confirmation. An optional bootstrap
root file is copied only after regular-file, size, JSON, and complete SHA-256
checks. Name, canonical URL, and bootstrap-root digest derive the source
identity and its isolated datastore, so replacement cannot reinterpret old TUF
metadata or cached targets under new trust. Disable/remove preserve that state;
restoring the exact identity reuses it without rewriting installed provenance.

Each `TrustedRegistry` carries a typed verified-target cache policy. The
standalone default permits 4 GiB and 4,096 entries while retaining 256 MiB of
free space. Verified targets and resumable partials share those byte and entry
bounds. Target length must fit before any target request. The downloader checks
the complete target requirement on its temporary staging filesystem and the
remaining target bytes on the cache filesystem because they may be different
volumes.

Under the exclusive target-cache lock, admission and confirmed pruning remove
bounded stale atomic-write files, then resumable partials, then verified targets
in oldest-modified-time and digest order. The active digest is protected and
only its remaining bytes are reserved. Directory synchronization follows
deletion. Inventory rejects unknown names, links, non-regular entries,
zero/oversized verified targets, oversized partials, and more than 100,000
scanned entries. Source-bound usage and pruning construct no network transport
and validate a retained catalog-cache identity when present.

GC changes only future cache availability. It never removes installed package
generations, receipts, Grants, bindings, capability state, or journals. A later
explicit offline operation fails closed if GC removed one of its exact targets.
Partial downloads are not offline evidence and never authorize package staging.

## Crash recovery

Recovery loads the exact stored request, plan, confirmation, authorization,
locks, lifecycle intents, Grant operation, and cutover record. It resumes the
first incomplete checkpoint with the same idempotency key.

`replayed = false` means an interrupted operation resumed work. `replayed =
true` is reserved for returning a previously completed terminal result.

Recovery rejects:

- changed plan/lock/catalog/receipt/provider/policy evidence;
- missing required package or Grant journal;
- conflicting operation ownership;
- cutover key reused for another request;
- unknown schema or unknown fields; and
- deleted graph evidence needed to determine the closure.

The engine does not reconstruct missing authority or guess a cleanup graph.

## Storage schemas

Current disk schemas are listed in [plugin-contracts.md](plugin-contracts.md).
All records are bounded and canonical. Unknown preview versions fail closed.

The SQLite Knowledge backend accepts only its current `user_version`. It creates
new state atomically and never migrates an unknown pre-release database. Its
default policy allows 512 MiB of retained expanded content, 256 retained
projections, 32 generations per surface, and 256 tombstones per complete
scope. Hard ceilings are 8 GiB, 1,024 projections, 32 generations per surface,
and 1,024 tombstones. `a3s-use knowledge usage --json` exposes non-secret
scope-local usage and allocation evidence. `knowledge audit` checks SQLite,
foreign keys, exact receipt/scope accounting, and FTS consistency. `knowledge
backup` produces a non-overwriting `a3s.use.okf-knowledge-backup.v1` database
snapshot with exact length and SHA-256; `verify-backup` reopens and audits it
offline. Confirmed `repair-search-index` may rebuild only FTS rows derived from
validated documents. It cannot rewrite receipts, projections, bindings,
lifecycle evidence, or Grants.

The scope-local backup is corruption evidence, not a signature or complete
recovery authority. Coordinated restore, binding and lifecycle recovery,
backup rotation, and whole-product procedures remain release gates. The
detailed boundary is documented in
[OKF Knowledge operations](okf-knowledge-operations.md).

## Observability

Diagnostics should expose non-secret evidence sufficient to identify:

- operation/action/scope and plan digest;
- Registry/source and TUF role versions;
- package and surface generation;
- lifecycle checkpoint and retry status;
- provider selection and readiness;
- Registry generation/snapshot digest;
- Grant cutover and drain state; and
- actionable cleanup/reinstall instructions for unsupported state.

Logs and JSON errors must not echo secrets, full untrusted descriptor values, or
arbitrary package content.

## Security acceptance tests

Required gates include:

- current-schema canonical digest round trips;
- superseded-schema rejection;
- catalog/manifest/receipt provenance mismatch;
- dependency cycle, ambiguity, and search-bound failures;
- plan, confirmation, policy, scope, Grant, provider, and generation drift;
- path/link/archive attacks on Unix and Windows;
- interruption at every package/Grant/cutover/drain/removal checkpoint;
- mixed-generation and stale-route prevention;
- exact completed-result replay; and
- complete real-process lifecycle on each supported platform.

The final cross-platform and operational gates remain incomplete; therefore the
product remains a development preview.
