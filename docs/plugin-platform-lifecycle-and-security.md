# A3S Use Plugin Lifecycle and Security

Status: development preview
Last updated: 2026-08-10

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
Before committing an immutable package generation, Use validates every
directory from its configured data root to the package namespace without
following links or reparse points. Under the same lock it removes only bounded,
physical `.lifecycle-staging-*` trees left by an interrupted commit. A staging
link, special file, unbounded inventory, or linked package parent fails closed.
The real-process recovery test terminates while a high-entry package is being
copied into that staging tree. Its exact pending plan and applying lifecycle
journal remain durable, while the receipt, installed graph, and route remain
absent. Explicit offline replay reclaims the physical partial tree, repeats the
same package-commit checkpoint, publishes one Registry generation, completes
the journal, and removes the pending operation without a network request.
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

The installed schema-v4 receipt retains the exact signed planning bundle after
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

The embedding host also owns UI state. A3S Code currently validates static UI
entry points and exact asset digests during lifecycle changes, and clears
receipt-owned UI state on true surface removal. CLI and TUI do not inject a
browser renderer or ambient state, context, Tool, MCP, Flow, or backend
authority. Any future renderer must supply those capabilities through an
explicit reviewed binding.

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

One shared host-metadata predicate rejects both ordinary symbolic links and the
Windows `FILE_ATTRIBUTE_REPARSE_POINT` class. Windows CI creates a real
directory junction and proves package copying fails before external content is
read or copied; the same predicate protects Registry/cache, Grant, lifecycle,
Runtime, Flow, and Knowledge state checks.

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

A real-process failure test terminates installation after the complete target
has entered the verified cache but while a high-entry archive is still being
extracted. At that boundary no package receipt, installed graph, pending graph
operation, or package root exists. An explicit offline retry revalidates the
cached target, performs no network request, and completes the ordinary package
preparation and publication path. The broader cross-platform extraction,
reboot, temporary-storage retention, and replacement-race matrix remains a
release gate.

## Crash recovery

Recovery loads the exact stored request, plan, confirmation, authorization,
locks, lifecycle intents, Grant operation, and cutover record. It resumes the
first incomplete checkpoint with the same idempotency key.

The package-host ambiguity window is covered by a test-binary subprocess
matrix for every canonical install, upgrade, enable, disable, and uninstall
checkpoint. Each child exits after syncing the host effect but before the
checkpoint receipt; restart recovery retries the same key, retains exactly one
durable effect, completes the journal, and makes no host call on terminal
replay.

The grant-bearing graph ambiguity window has a separate test-binary subprocess
matrix for install, upgrade, and uninstall. Each child exits after syncing the
atomic graph publish or hide effect but before package publication receipts and
Grant cutover evidence. Restart recovery reuses the same cutover key, retains
one graph effect, completes package and Grant journals, retires only the exact
prior grants, and does not publish or hide again on terminal replay.

Additional managed-host tests run the production package manager in real OS
child processes with one permission-bearing root and four dependencies for
install, upgrade, and uninstall. The parent holds a dependency lifecycle lock
after Registry publish/hide, so one package receipt is pending and the Grant
operation remains prepared. The parent kills each child externally. Restart
injects a provider that rejects any new authorization request; recovery instead
reuses the durable confirmation and resolved Grant, performs no network
request, preserves the exact candidate receipt, retires only the bound prior
receipt, completes package and Grant journals, retires the cutover, and does
not advance the Registry generation again. Actual Code/Runtime host processes
and the supported-platform matrix remain separate gates.

The production `CognitivePackageHostManager` path has a separate five-operation
real-process matrix. Install/upgrade planning verifies and caches the complete
five-node graph; uninstall planning binds the exact installed lock; disable and
enable planning bind the exact package state generation. Every plan persists
the Host request/plan binding and returns the digest-only review boundary before
the Registry server is stopped. Install, upgrade, and uninstall children are
externally killed after atomic graph publication/hide while the Grant operation
and one dependency receipt are incomplete. Disable is killed after root hide
and Grant cutover while an accepted-call lease blocks drain. Enable is killed
after Registry publication while its candidate Grant remains prepared. A
second child uses only the stored plan and confirmation, plus the verified
planning cache for install/upgrade, to complete drain and lifecycle/Grant
journals, converge the exact candidate/prior Grant or enablement
regrant/revocation, and persist the terminal Host outcome. A later digest-only
apply is an exact outcome replay and does not authorize, access the network, or
advance the Registry generation.

The Grant Store has an additional test-binary subprocess matrix over all 14
durable checkpoints in the canonical two-candidate/two-retirement lifecycle.
It exits after each phase journal, candidate receipt, prior revocation, and
candidate restoration write across preparation, cutover/retirement, and
pre-cutover rollback. Restart converges to the exact completed or rolled-back
journal, preserves or revokes only the bound receipts, and terminal replay is
identical. Remaining real CLI, provider, and cross-platform failure injection
stay separate release gates.

Runtime Services close their nested effect/receipt window before the package
surface checkpoint returns. A generation-scoped provisioning receipt is synced
before apply, then advanced after exact healthy Runtime observation and after
idempotent Gateway bind/MCP initialize. The final binding is synced before
provisioning is deleted. Restart reuses the original lifecycle and apply keys;
a pre-apply marker is removed only after an exact Runtime inspect proves the
unit absent, while later phases are completed and receipt-owned cleanup drains
Gateway before Runtime removal. Unit tests cover Tool and HTTP MCP bind
interruption plus pre-apply and post-apply candidate rollback. A test-binary
subprocess matrix exits at requested sync, ambiguous Runtime effect,
runtime-applied sync, ambiguous Gateway effect, gateway-ready sync, and the
final-binding-plus-provisioning window for both Service kinds. Recovery keeps
one Runtime and Gateway effect, terminal replay is side-effect free, and
receipt-owned removal leaves no route, unit, binding, or provisioning residue.
Real managed-provider and CLI process-kill qualification across the supported
platform matrix remains a release gate.

The standalone CLI binary additionally has a deterministic real-process
multi-node install case. After the first dependency is fully prepared, the
parent holds that dependency's journal lock while the child atomically
publishes all nine package routes and the durable cutover. The child is killed
before that dependency's publication receipt and the parent graph record are
written. Explicit offline restart performs no network request, reuses the same
cutover, completes every package journal and the exact installed graph, clears
the cutover acknowledgement, and leaves the Registry at generation 1.

The standalone CLI also has a deterministic real-process uninstall case. The
parent holds the package journal lock while the child
durably hides the Registry graph. This proves the child can be killed before
its package hide receipt. On restart, the same pending plan and cutover request
are replayed; a held prior-generation route lease proves recovery stops at
accepted-call drain before it removes the retained generation. Releasing the
lease completes the journal and physical removal without advancing the
Registry generation again.

A second real-process uninstall case uses a high-entry immutable generation
and terminates while its directory is physically being removed. By that point
the route is hidden and both selected and retained receipts are absent, while
the exact pending graph and applying journal remain. Restart uses the original
lifecycle identity to finish the partial directory, complete the package
checkpoint and cutover acknowledgement, and remove pending state without
advancing the Registry generation again.
A paired negative case deletes selected state with no matching durable cutover
and verifies that Registry, graph, and pending-plan evidence remain unchanged.

`replayed = false` means an interrupted operation resumed work. `replayed =
true` is reserved for returning a previously completed terminal result.

Recovery rejects:

- changed plan/lock/catalog/receipt/provider/policy evidence;
- missing required Grant journal;
- missing package state without exact durable Registry cutover evidence;
- conflicting operation ownership;
- cutover key reused for another request;
- unknown schema or unknown fields; and
- deleted graph evidence needed to determine the closure.

The engine does not reconstruct missing authority or guess a cleanup graph.
An `applying` or `rolling-back` package journal remains active ownership: a
different operation cannot replace it. Only a terminal `completed` or
`rolled-back` record may be moved to previous history when the next reviewed
intent begins.

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

`knowledge backup-retention` scans at most 4,096 entries in one owned,
non-linked directory and fully verifies every managed backup candidate before
selecting anything. One complete scope, an entry ceiling, and a byte ceiling
produce a canonical oldest-first plan. Apply requires both `--yes` and the
unchanged plan digest under the same cross-process directory lock used by
backup publication. It preserves unrelated files, other scopes, and at least
one verified backup; stale plans and linked or malformed candidates fail
closed. A partial deletion reports outcome-unknown with the exact removed
entries instead of guessing.

The scope-local backup is corruption evidence, not a signature or complete
recovery authority. Standalone authority-bound database restore is implemented
as a path-free reviewed plan plus digest-only confirmed apply. Planning binds
the verified backup, live main/WAL/SHM evidence, and complete
Registry/package/lifecycle/Grant authority plus the current exact-subset
binding digest. Apply revalidates that authority under the exclusive
maintenance fence, creates only missing exact backup binding files, retains
the exact prior database files, and replays the durable six-state operation
after process exit. Conflicting or newer binding evidence fails closed.
Missing Registry/package/lifecycle/Grant authority recovery, clean-machine
recovery, cross-platform operational drills, and whole-product rollback
procedures remain release gates. The detailed boundary is
documented in
[OKF Knowledge operations](okf-knowledge-operations.md).

`a3s-use knowledge restore-status --json` acquires the exclusive maintenance
fence only long enough to read a coherent global marker and requested-scope
inventory. Its `a3s.use.okf-knowledge-restore-diagnostic.v2` result contains no
paths or package content and reports at most 32 validated operation summaries,
the active durable phase, reviewed binding-state digest and missing-binding
count, unrecorded marker-handoff directory count, and remaining capacity. It
never rotates or deletes rollback evidence.

### Coordinated whole-installation backup

`a3s-use state backup <path> --json` acquires the same exclusive maintenance
fence used by restore and creates one `a3s.use.state-backup.v1` archive outside
the live data/state roots. The manifest binds portable relative data/state
paths, exact file lengths and SHA-256 digests, read-only and Unix-mode evidence,
per-family accounting, the Registry generation/projection digest, and sorted
installed-receipt digests. No clock value or source root enters the manifest,
so unchanged state produces identical archive bytes on the same platform.

Creation inventories the allowlisted roots, copies each regular file while
rechecking its length and digest, then rebuilds the complete inventory before
non-overwriting publication. Lock files are excluded. Active restore markers,
pending Registry cutovers, nonterminal lifecycle/Grant/package/enablement or
Runtime provisioning evidence, resumable partials, atomic-write leftovers,
lifecycle package staging, unknown top-level families, non-portable names,
links/reparse points, and special files fail closed. `state verify-backup`
checks the canonical manifest, header digest, exact archive length, and every
concatenated payload digest without extraction, network access, or local state.

The archive contains raw package and state bytes and is sensitive operational
data. Its hashes detect corruption; they do not authenticate a publisher or
recreate missing authority. `state backup-retention` fully verifies each managed
archive under the publication directory lock, returns a path-free oldest-first
canonical plan, requires its unchanged digest plus explicit confirmation, and
retains at least two verified recovery generations. `state plan-restore` and
confirmed `state restore` implement same-version/OS/architecture recovery with
exact live Registry and Grant authority, an explicit external rollback archive,
link/reparse-safe candidates, seven durable phases, bounded history, and
read-only path-free status. Missing Registry/package/lifecycle/Grant authority
recovery, cross-platform drills, and clean-machine disaster-recovery exercises
remain release gates. The operator format and procedure are documented in
[Coordinated state backup operations](state-backup-operations.md).

## Observability

`a3s-use extension inspect <publisher/name> --json` currently exposes
`a3s.use.plugin-lifecycle-diagnostic.v1` for the default User scope. The
projection reads the latest and previous records under the package-scoped
journal lock and includes:

- operation, action, scope, generation, and plan/intent/artifact digests;
- total and completed checkpoint counts;
- per-checkpoint kind, surface, required flag, bounded state, evidence digest,
  bounded error code, and observation time; and
- terminal completion time and rollback evidence digest when present.

The projection never contains checkpoint idempotency keys, provider
credentials, endpoint tokens, secret values, arbitrary package-authored error
text, or package content. It is read-only lifecycle evidence, not a telemetry
backend and not recovery authority.
One reviewed graph operation may create consecutive candidate and retirement
phase intents for a package. They share the graph `operationId` by design and
remain distinct through `intentDigest`, action, generation, and artifact
digests. Duplicate latest/previous intent digests fail closed.

`a3s-use extension diagnose <publisher/name> --json` exposes
`a3s.use.plugin-operation-diagnostic.v1` for one exact retained install,
upgrade, or uninstall graph, active admitted enable/disable operation, or
newest Host-reviewed enable/disable plan that has not been admitted in the
selected User or Workspace scope. It reads durable graph, enablement, Host
request-index, Registry snapshot/cutover, lifecycle, and Grant evidence under
the shared maintenance fence without network access, reconciliation, recovery,
or writes. Retained graphs cover planned, admitted, and cancelled operations;
active Use-owned enablement evidence always takes precedence.
The bounded projection includes:

- exact operation/plan/lock identity, action, phase, timing, counts, impact,
  authority, and confirmation state;
- path-free Registry names, TUF role versions, verified catalog/archive
  digests, and current Registry generation/snapshot/cutover evidence;
- provider identity/build/capability evidence and readiness;
- Grant authorization/journal/cutover/rollback evidence;
- lifecycle publication, accepted-call drain, checkpoint, and rollback
  summaries; and
- stable resume, review, cancellation-observation, or operator-review guidance.

The 2 MiB contract rejects unknown fields and internally inconsistent evidence.
It never contains paths, Registry URLs, idempotency keys, credentials, tokens,
secret names or values, package content, or arbitrary package-authored text.
Damaged backing state returns a path-free cleanup/reinstall diagnostic without
echoing the rejected bytes. For pre-admission enablement, a digest-bound index
selects the newest exact Host request by `(plannedAtMs, requestId)` and exposes
only the public plan scope/package plus `planned` or exact `cancelled` state.
The projection reconstructs the expected lifecycle schedule from the exact
installed receipt, manifest, selected surfaces, and reviewed plan without
persisting it. It reports selected providers, awaiting-admission or cancelled
Grant state, current cutover evidence, and review/cancellation guidance. The
private index retains the managed scope only for request lookup; Host ID,
authority, fence, Host request/cancellation IDs, and private paths are excluded.
A durable Use completion or Host outcome suppresses the stale plan.

For retained Registry-backed install/upgrade graphs and pre-plan download
attempts, exact historical provenance selects the observed datastore and the
projection reports expected/retained archive and separately signed executable-
planning-target bytes plus per-target missing/partial/complete state without
taking the cache lock, contacting the Registry, writing, or exposing paths.
Complete is an exact-length observation at the verified-promotion location,
not a content rehash; no observed target or partial is planning, apply, or
recovery authority. Static packages report planning as `not-required`.

Before Registry/TUF access begins, the process-held package lock protects a
bounded `a3s.use.plugin-resolution-attempt.v1` record. It binds scope, action,
requested version/channel, refreshed/cached access, and path-free root and
dependency Registry states. Each Registry exposes only source-identity and
trust-root digests, pending/verifying/verified/failed state, verified TUF role
versions, bounded package-target count, observation time, and a stable bounded
error code. Success adds the exact package-lock digest/count. The
`a3s.use.plugin-resolution-attempt-diagnostic.v1` projection has phase
`pre-lock`, makes no network request or write, and does not wait for the package
lock. Failure or process exit retains it; success durably creates the download
attempt before deleting it. URLs, paths, raw transport errors, credentials, and
metadata bytes are excluded.

Before a reviewed graph exists, an exact package lock and selected archive set
are retained as `a3s.use.plugin-download-attempt.v1` under a process-held
per-package lock. The record survives failure/exit, can be superseded only after
that lock is released, and is removed only after the pending graph is durable.
The path-free `a3s.use.plugin-download-attempt-diagnostic.v1` projection exposes
the same archive and planning-target byte evidence with phase `pre-plan`;
malformed records fail closed without echoing their fields.

Completed and rolled-back operation observations plus cancelled graph plans are retained in
`a3s.use.plugin-operation-history.v1` and exposed newest-first through
`extension diagnose --history --json` as
`a3s.use.plugin-operation-history-diagnostic.v1`. Each explicit User or
Workspace scope/package inventory is limited to 16 occurrences and 8 MiB.
Retention precedes deletion of pending graph or active enablement recovery
authority; a crash in between is replay-safe because occurrence identity is
the pair `(operationId, planDigest)`. The plan digest is required because an
exact reinstall can legitimately recreate the same lock-derived textual graph
operation ID. Terminal outcome validation correlates lifecycle status and
completion receipts, Grant terminal phase, and Registry cutover status.
Historical embedded recovery guidance records the pre-cleanup observation and
is never current authority. Unknown fields, conflicting identities or
outcomes, links/reparse points, and oversized state fail closed without echoing
paths or content. Real CLI install/uninstall/reinstall, pre-admission graph
cancellation, and killed managed Workspace Host recovery tests prove
zero-network history, package-removal survival, exact scope isolation, and
replay deduplication.

Real-process tests kill an executable-planning-target transfer, observe its
active and retained partial bytes, resume the exact Range, and prove gap-free
handoff to the reviewed graph. Host/CLI tests prove planned and cancelled
pre-admission enablement diagnosis has zero network, authorization, admission,
or lifecycle side effects; exposes no Host/fence/path evidence; and suppresses
the stale plan after Use completion even before the Host outcome can commit.

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
- package-host effect/receipt interruption at every canonical lifecycle
  checkpoint in a test-binary subprocess;
- grant-bearing graph publish/hide effect interruption before package and
  Grant cutover receipts in a test-binary subprocess;
- externally killed managed-host processes for install, upgrade, and uninstall
  after multi-node Registry publish/hide but before one dependency receipt and
  Grant cutover/retirement, followed by zero-network recovery with
  reauthorization disabled and exact candidate/prior Grant convergence;
- externally killed `CognitivePackageHostManager` protocol applies for all five
  reviewed mutations with the Registry unavailable: multi-node
  install/upgrade publication or uninstall hide, disable after root hide and
  Grant cutover while drain is blocked, and enable after publication with its
  candidate Grant prepared; recovery converges exact candidate/prior Grants or
  enablement regrant/revocation, completes drain and lifecycle/Grant journals,
  and replays the terminal Host outcome without generation inflation; install
  and upgrade consume only the reviewed planning cache;
- Grant Store interruption after every durable phase, candidate, revocation,
  and restoration write in a test-binary subprocess;
- real CLI uninstall interruption after durable Registry hide but before its
  package receipt, followed by restart drain and exact removal;
- real CLI multi-node install interruption after durable Registry publication
  but before dependency journal and parent graph completion, followed by exact
  zero-network replay without another Registry generation;
- real-process interruption at every graph/Grant/cutover/drain/removal
  checkpoint;
- mixed-generation and stale-route prevention;
- exact completed-result replay; and
- complete real-process lifecycle on each supported platform.

The final cross-platform and operational gates remain incomplete; therefore the
product remains a development preview.
