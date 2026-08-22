# A3S Use Plugin Contract Reference

Status: development preview
Last updated: 2026-08-22

## Scope

This document describes the one cognitive-package contract line accepted by
the current A3S Use implementation. It is not a compatibility matrix.

The cognitive-package product has not shipped a supported release. Superseded
pre-release manifests, catalogs, receipts, plans, host inventories, manager
toolsets, and disk records are rejected. Negative test fixtures exist only to
prove rejection. Unsupported disk state must be removed and packages reinstalled.

Schema identifiers ending in `v1` below are not automatically “legacy.” They
are the current version of that specific contract when they appear in the
current set.

## Current contract set

| Contract | Current identifier |
| --- | --- |
| Package manifest | ACL `schema_version = 3` |
| Catalog record | `a3s.use.plugin-catalog.v3` |
| Package lock | `a3s.use.plugin-package-lock.v1` |
| Planning bundle | `a3s.use.plugin-planning-bundle.v1` |
| Operation plan draft | `a3s.use.plugin-operation-plan-draft.v3` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Operation confirmation | `a3s.use.plugin-operation-confirmation.v1` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v6` |
| Host protocol level | `6` |
| Host managed scope | `a3s.use.plugin-managed-scope.v2` |
| Host plan request/result | `a3s.use.plugin-host-plan-request/result.v1` |
| Host enablement plan request/result | `a3s.use.plugin-host-enablement-plan-request/result.v1` |
| Host apply request/result | `a3s.use.plugin-host-apply-request/result.v1` |
| Host observation request/result | `a3s.use.plugin-host-observation-request/result.v1` |
| Host operation observation request/result | `a3s.use.plugin-host-operation-observation-request/result.v1` |
| Host operation watch request | `a3s.use.plugin-host-operation-watch-request.v1` |
| Host cancellation request/result | `a3s.use.plugin-host-cancel-request/result.v1` |
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v4` |
| Installed receipt | numeric schema version `4` |
| Installed package graph | `a3s.use.installed-package-graph.v1` |
| Pending package graph | `a3s.use.pending-package-graph-operation.v4` |
| Pre-lock resolution attempt | `a3s.use.plugin-resolution-attempt.v1` |
| Pre-plan download attempt | `a3s.use.plugin-download-attempt.v1` |
| Lifecycle intent/operation | `a3s.use.plugin-lifecycle-intent/operation.v2` |
| Lifecycle diagnostic | `a3s.use.plugin-lifecycle-diagnostic.v1` |
| Operation diagnostic | `a3s.use.plugin-operation-diagnostic.v1` |
| Operation history | `a3s.use.plugin-operation-history.v1` |
| Operation history diagnostic | `a3s.use.plugin-operation-history-diagnostic.v1` |
| Pre-lock resolution diagnostic | `a3s.use.plugin-resolution-attempt-diagnostic.v1` |
| Pre-plan download diagnostic | `a3s.use.plugin-download-attempt-diagnostic.v1` |
| Enablement request/result | `a3s.use.cognitive-package-enablement-request/result.v1` |
| Enablement plan result | `a3s.use.cognitive-package-enablement-plan-result.v1` |
| Enablement state/operation | `a3s.use.cognitive-package-enablement-state/operation.v2` |
| Workspace Grant | `a3s.use.plugin-workspace-grant.v1` |
| Registry cutover | `a3s.use.registry-cutover.v1` |
| Runtime Task binding | `a3s.use.runtime-task-binding.v4` |
| Runtime Service provisioning | `a3s.use.runtime-service-provisioning.v1` |
| Runtime Service binding | `a3s.use.runtime-service-binding.v3` |
| OKF Knowledge binding | `a3s.use.okf-knowledge-binding.v2` |
| OKF Knowledge search | `a3s.use.okf-knowledge-search-request.v1` / `a3s.use.okf-knowledge-search-response.v1` |
| OKF Knowledge citation | `a3s.use.okf-knowledge-citation.v1` |
| OKF Knowledge read | `a3s.use.okf-knowledge-read-request.v1` / `a3s.use.okf-knowledge-read-response.v1` |
| OKF Knowledge backup | `a3s.use.okf-knowledge-backup.v1` |
| Coordinated Use state backup | `a3s.use.state-backup.v1` |
| Coordinated Use state backup retention plan | `a3s.use.state-backup-retention-plan.v1` |
| Coordinated Use state backup retention result | `a3s.use.state-backup-retention-result.v1` |
| Coordinated Use state restore plan | `a3s.use.state-restore-plan.v1` |
| Coordinated Use state restore operation | `a3s.use.state-restore-operation.v1` |
| Coordinated Use state restore result | `a3s.use.state-restore-result.v1` |
| Coordinated Use state restore diagnostic | `a3s.use.state-restore-diagnostic.v1` |
| OKF Knowledge backup retention plan | `a3s.use.okf-knowledge-backup-retention-plan.v1` |
| OKF Knowledge backup retention result | `a3s.use.okf-knowledge-backup-retention-result.v1` |
| OKF Knowledge restore plan | `a3s.use.okf-knowledge-restore-plan.v2` |
| OKF Knowledge restore operation | `a3s.use.okf-knowledge-restore-operation.v2` |
| OKF Knowledge restore result | `a3s.use.okf-knowledge-restore-result.v2` |
| OKF Knowledge restore diagnostic | `a3s.use.okf-knowledge-restore-diagnostic.v2` |

The host capability inventory is exact: the current Host and managed-scope
schemas above, catalog v3, plan v4, and all six surface kinds. The separate
manager toolset accepts v4 only. Toolset v4 exposes optional canonical
`registryName` only on install planning; upgrade stays bound to installed
Registry provenance. A host advertising a different inventory is rejected.

## Package manifest

The package manifest is `a3s-use-extension.acl`. It is parsed with `a3s-acl`;
ACL is not HCL.

Required package-level fields:

- canonical `<publisher>/<name>` package ID;
- `schema_version = 3`;
- SemVer `version`;
- canonical route;
- `requires_use` that includes the current 0.3 host line and excludes pre-0.3
  hosts;
- sorted risk actions;
- repository URL plus immutable revision; and
- at least one named Tool, MCP, OKF, Flow, Skill, or UI surface.

The package root also requires a bounded, regular UTF-8 `README.md`.

Package dependencies are named `dependency` blocks containing only a canonical
package ID and SemVer requirement. A dependency cannot select a URL, Registry,
trust root, target, or mutable tag. Dependencies are sorted, unique, non-self,
bounded, and resolved as one acyclic graph.

### Surface contracts

| Surface | Required identity and dependency behavior |
| --- | --- |
| Tool | Named Task or Service; explicit interface, activation, timeout, and immutable executable/release evidence |
| MCP | Named standard MCP server; stdio or HTTP/release evidence; no A3S-specific RPC envelope |
| OKF | Named OKF v0.2 bundle; exact content digest, counts, byte limits, and Knowledge-host ownership |
| Flow | Named `a3s-flow` source/export; explicit Tool/MCP/OKF dependencies |
| Skill | Named `SKILL.md`; explicit Tool/MCP/OKF/Flow dependencies |
| UI | Named integrity-bound entry; explicit Skill/Tool/MCP/Flow bindings |

All package paths are relative, normalized, and package-owned. Absolute paths,
parent traversal, symlinks, hard-link ambiguity, archive link entries, duplicate
normalized paths, and size/count overflows fail before activation.

The surface dependency graph must be acyclic. Required surfaces publish only
when all required dependencies have exact ready evidence for the same package
generation. An optional surface failure may produce a degraded package but
cannot satisfy another required surface.

## OKF contract

Only OKF format `0.2` is accepted by the cognitive-package manifest and bundle
inspector. There is no 0.1 fallback.

Use verifies package ownership, byte/count bounds, content digest, concept
paths, frontmatter, and links. A Knowledge host owns stage, promote, observe,
cited search, bounded exact-document read, drain, and receipt-owned removal.
Search and read accept only an explicit promoted capability projection; the
host-side lease pins its lifecycle generation and installed package integrity
for the complete retrieval. Staged content is not live evidence. Only an exact
promoted binding may enter the capability snapshot.

Capability snapshot schema v2 applies the same rule to Runtime Tool Tasks. A
`toolTasks` entry is emitted only for a published, non-interactive,
release-backed Task whose v4 binding matches the exact scope, package digest,
surface, and lifecycle generation. The entry is invocation metadata, not a
provider fallback: hosts must still possess the named reviewed provider and
dispatch through the receipt-owned exact-generation lease.

The standalone SQLite/FTS5 backend creates the one current database schema for
new state and rejects every unknown `user_version`. It does not migrate or
rewrite pre-release databases.

One scope-local backup file starts with the fixed A3S OKF backup header, a
bounded manifest length and SHA-256, the versioned JSON manifest, and the exact
compact SQLite snapshot bytes.
The manifest binds complete scope, creation time, database length and SHA-256,
storage accounting, and policy limits. Verification rejects unknown fields,
scope substitution, symlinks, length/digest mismatch, unsupported SQLite
schema, invalid receipts, inconsistent accounting, and FTS corruption.

`a3s.use.okf-knowledge-backup-retention-plan.v1` is the canonical review for
one owned directory, complete scope, bounded entry/byte policy, and exact
verified backup inventory. It contains only relative file names and digest,
timestamp, and byte evidence. Candidates are ordered by manifest creation time
and file name; the removal set is the oldest prefix required to satisfy both
limits. The plan never selects the last verified scope backup. Apply requires
the unchanged canonical plan digest and returns
`a3s.use.okf-knowledge-backup-retention-result.v1`. A changed directory fails
before removal; a partial filesystem failure reports the exact already-removed
entries as outcome-unknown. Backups for another complete scope and unrelated
files are never selected, while malformed managed candidates fail closed.

The backup digest detects corruption but is not a Registry signature. A
verified database cannot recreate package receipts, immutable package roots,
lifecycle journals, or Grants. Standalone restore therefore uses a separate
reviewed plan that validates those exact authorities together, binds the
current main/WAL/SHM evidence and current binding-inventory digest, and requires
the plan digest again at confirmed apply. It can replace or recreate the
scope-local database and create only binding files missing from an exact subset
of the backup inventory. Conflicting or newer binding evidence is never
overwritten. It cannot reconstruct missing Registry, package, lifecycle, or
Grant authority or perform clean-machine or whole-product recovery.

`a3s.use.okf-knowledge-restore-diagnostic.v2` is the bounded, path-free,
secret-free projection of the global active marker and one requested scope's
restore history. It reports exact plan/backup/authority digests, durable phase,
the reviewed binding-state digest and missing-binding count, timestamps,
retained prior-file count, physical operation-directory count, marker-handoff
directories without a journal, the fixed retention limit, and remaining
capacity. Reading it does not clean, rotate, or rewrite evidence.

Search-index repair is deliberately narrower than lifecycle repair. It first
validates core SQLite integrity, foreign keys, projection receipts, row
identity, scope, and accounting, then derives FTS rows from the retained
document table and repeats the complete audit. It never edits lifecycle
authority.

## Flow contract

The only engine identity is `a3s-flow`. A Flow surface binds:

- package ID, surface ID, and immutable package generation;
- `native-ts` source and export;
- exact source digest;
- declared Tool, MCP, and OKF edges; and
- host preflight/compiled-binding evidence.

`flow.json` is a host-facing visual design or deployment document. It does not
define another package format, lock, receipt, or lifecycle. Local Code and
remote OS placement must consume the same package-owned Flow identity.

Missing Flow ownership fails before publication. Source presence is not a
readiness fallback.

## Catalog and TUF provenance

Every signed Registry target carries one complete catalog-v3 record in
`custom.a3s`. The record contains:

- package identity, version, channel, target, display/search metadata, license,
  and repository identity;
- complete six-surface inventory and dependency edges;
- package dependencies and `requires_use`;
- archive target, length, and digest;
- expanded package bounds and exact package/manifest digests;
- planning target identity and digest;
- provider requirements; and
- the complete permission ceiling plus digest.

TUF verification binds Registry name and URL, root digest/version, timestamp,
snapshot, targets version, target path, length, digest, and catalog-record
digest. A partial `custom.a3s` record is invalid; there is no metadata fallback.

Prepared and downloaded remote packages always carry the verified catalog
record. A Registry/TUF receipt is invalid unless both resolved source
provenance and the complete verified catalog evidence are present and agree
with package ID and version.

Registry endpoints are replaceable host configuration. Replacement never
rewrites receipt provenance. The exact source must be restored or the package
must be reinstalled before an upgrade can proceed.

## Package resolution and lock

Resolution is bounded and deterministic. Candidate selection applies:

1. enabled source set;
2. exact package identity;
3. requested release channel/version;
4. host target and `requires_use`;
5. provider capabilities; and
6. all accumulated SemVer constraints.

The resolver rejects cycles, missing releases, incompatible constraints,
source ambiguity, duplicate equal-priority candidates, and configured search
limits.

The package lock freezes each selected catalog record and its provenance.
Install order is dependency-forward; removal order is its exact reverse.
Retained packages are accepted only when their installed receipt and verified
catalog evidence match the locked node exactly.

Receipt v4 binds the exact canonical non-empty surface set selected by the
reviewed lifecycle plan. Missing, empty, duplicated, unsorted, unknown, or
dependency-incomplete selections fail closed; the loader never expands absent
evidence to the manifest inventory. Receipt v3 is unsupported preview state
and must be removed and reinstalled rather than migrated implicitly.

Upgrade always carries two locks:

- the **prior lock**, which proves the installed dependency graph and reverse
  retirement order; and
- the **candidate lock**, which proves the graph to publish.

The candidate lock alone cannot authorize retirement.

## Immutable operation plan

Plan v4 is used for install, upgrade, uninstall, enable, and disable. A plan
contains:

- operation ID, action, actor, authority, scope, expiry, and policy decision;
- exact host capability evidence;
- exact prior/candidate package-lock bindings where applicable;
- sorted Add/Replace/Remove/Retain package transitions;
- selected surface closure and before/after state evidence;
- provider, permission, OKF, Flow, byte, download, and process impact;
- immutable current-state revision; and
- canonical plan digest.

Apply accepts the operation ID, plan digest, and exact confirmation. It
re-derives plan semantics and rejects any catalog, lock, scope, policy,
provider, receipt, generation, or state drift before mutation.

Enablement planning returns either:

- `Planned`, with the exact plan-v4 envelope; or
- `NoChange`, a terminal read-only result without a synthetic mutation plan.

Completed operation reads return the durable terminal result. Recovery resumes
the exact stored plan and authorization. It never generates a replacement plan
or asks a child operation to invent authority.

## Host protocol

Host capabilities v6 advertise protocol level 6 and exact current schema
inventories. Managed hosts expose separate methods for:

1. capability discovery;
2. install/upgrade/uninstall planning;
3. enable/disable planning;
4. exact reviewed apply;
5. package observation;
6. exact operation observation;
7. revision-bound operation watch; and
8. explicit-user cancellation.

Operation requests bind the assignment generation, capability digest, exact
managed scope, package ID, operation ID, and plan digest. The status phases are
`Planned`, `AwaitingConfirmation`, `Denied`, `Preparing`, `Publishing`,
`Finalizing`, `Completed`, `Failed`, and `Cancelled`. They are projections of
durable Host outcomes, pending graph or enablement records, and lifecycle
journals; they are never inferred from elapsed time. Progress is expressed as
bounded completed/total checkpoints without percentages. A graph-wide status
aggregates every changed package generation and omits `currentSurface` when a
single package cannot be identified unambiguously.

The status revision is the digest of the complete status. A watch returns as
soon as that revision changes or after its bounded timeout of at most 30
seconds. `Completed` requires the durable Host outcome. A rolled-back lifecycle
is `Failed`, but graph failure is not terminal while any matching journal is
still applying or rolling back.

Cancellation requires explicit user authority and is accepted only before
durable graph or enablement admission. The accepted cancellation is persisted,
observes as terminal `Cancelled`, and replays as `AlreadyCancelled`. Once
admission or publication begins, cancellation returns `TooLate`; a completed
operation returns `AlreadyCompleted`.

Managed scope v2 includes both kind (`user` or `workspace`) and ID. Equal
textual IDs in different kinds produce different descriptor digests and cannot
alias a Host fence, request replay store, authorization, plan, or observation.
The v1 scope and Host v5 inventory are rejection fixtures only.

The manager toolset contains exactly:

```text
plugin_search
plugin_inspect
plugin_list_installed
plugin_status
plugin_plan_install
plugin_plan_upgrade
plugin_plan_uninstall
plugin_apply_plan
plugin_plan_enable
plugin_plan_disable
```

There is no `plugin_enable` or `plugin_disable` mutation tool. Enablement uses
plan then `plugin_apply_plan` like every other mutation.

`CognitivePackageHostManager` is the production adapter for this port. It is
bound to one exact `PluginManagedScope` fence and advertises one immutable
capability digest per manager build. Its private durable records map a Host
request ID to the exact Use-produced plan, map the operation ID back to that
request, and retain the terminal Host projection for idempotent replay. Those
records do not replace the package graph store, enablement operation store,
Grant store, lifecycle journal, Registry receipt, or capability snapshot.

Plan lifetime is an admission boundary, not a recovery deadline. An apply for
a merely planned operation must still be inside the original plan window. If
that window has elapsed, the adapter permits only exact replay after loading
one of the existing Use-owned proofs: an admitted pending graph, matching
completed lifecycle journal, active/completed enablement operation, or the
already persisted Host terminal result. The confirmation is revalidated in
the original plan window. Missing or mismatched evidence fails closed and
never extends the plan lifetime.

Pending package graph v4 is the only accepted graph-operation record. It
requires the reviewed envelope, explicit `planned`, `admitted`, or `cancelled`
phase, `plannedAtMs`, `admittedAtMs`, and durable authorization evidence.
Cancellation additionally requires its timestamp and request identity. V2 and
v3 records are unsupported preview state and are rejected rather than upgraded
or interpreted with implicit defaults.

## Workspace Grants

The permission ceiling is signed catalog input. A reviewed Workspace Grant is
a policy-approved subset bound to scope, package, surfaces, immutable package
generation, operation identity, and expiry.

Planning snapshots existing grants and emits canonical proposed and resolved
changes. Apply re-derives the set and rejects scope, revision, authority,
confirmation, ceiling, or resolution drift.

The graph saga orders Grant changes as follows:

```text
persist candidate grants
→ prepare package candidates
→ publish exact Registry snapshot
→ checkpoint Grant cutover
→ drain calls admitted by the prior generation
→ revoke exact prior grants
→ remove prior package generations
```

A failure before publication rolls package and Grant candidates back together.
After publication, recovery completes retirement; it does not restore the old
visible graph.

## Lifecycle and storage

The lifecycle intent and journal contain the complete package generation,
scope, action, surface schedule, and idempotency keys. Host traits must return
canonical SHA-256 evidence for each checkpoint.

Capability visibility methods always return exact immutable snapshot cutover
evidence. Host traits have no fallback publication API. A separate retirement
method may mark a prior receipt hidden only after the exact Registry route is
already absent; otherwise it fails before mutation.

Current disk records are bounded, canonical, path-owned, and atomically
replaced. Loading rejects:

- unknown schema/version;
- unknown fields;
- missing catalog or provenance evidence;
- path traversal, symlink, or ownership drift;
- changed package/manifest digest;
- generation or scope mismatch; and
- a missing exact journal required for recovery.

Deletion of recovery evidence is corruption, not permission to infer state.

Persistent Tool and Streamable HTTP MCP Services add one generation-scoped
provisioning record before the Runtime apply request. The record advances only
`requested` → `runtime-applied` → `gateway-ready` and binds the lifecycle key,
apply request, scope, package generation, Grant/descriptor/spec digests,
provider evidence, Runtime observation, and opaque Gateway endpoint. The final
v3 binding is synced before provisioning is deleted. If both records survive a
crash they must be identical in binding evidence; conflicting or missing
ownership fails closed. Candidate rollback inspects a `requested` unit before
dropping a pre-apply marker, while later phases replay the original bind key,
drain, and receipt-owned removal.

The lifecycle diagnostic is a read-only JSON projection, not a mutable disk
record or recovery input. It reports latest/previous operation identity,
action, status, generation, digests, checkpoint progress, bounded failure
codes, timings, and rollback evidence. It omits checkpoint idempotency keys,
credentials, tokens, secret values, package-authored error text, and package
content. Consumers must not treat the projection as authority to recreate a
missing journal.
Candidate and retirement phase intents may share the reviewed graph
`operationId`. `intentDigest` is the exact phase identity; latest and previous
records with the same intent digest are invalid.

The operation diagnostic is a read-only, computed JSON projection for one exact
retained install, upgrade, or uninstall graph, one active admitted enable/
disable operation, or the newest Host-reviewed enable/disable plan that has not
been admitted in one explicit User or Workspace scope. It is not persisted
apply or recovery authority. The projection reports:

- exact operation ID, action, phase, reviewed plan digest, lock digests,
  timing, impact, and authority/confirmation status;
- package counts plus path-free Registry names, TUF role versions, and verified
  catalog/archive digests;
- current Registry generation/snapshot digest and the exact graph cutover
  status;
- selected provider identities, builds, capability evidence, enforcement, and
  readiness;
- Grant authorization/journal phase and cutover or rollback evidence; and
- bounded lifecycle publication, accepted-call drain, checkpoint, rollback,
  and recovery guidance.

`a3s-use extension diagnose <publisher/name> --json` reads this evidence under
the shared maintenance fence without network access, reconciliation, recovery,
or writes. Retained graph evidence covers planned, admitted, and cancelled
install/upgrade/uninstall operations, including a reviewed graph before
installation. Active Use-owned enablement evidence takes precedence. When no
active operation exists, an observation-only digest-bound index resolves the
newest exact Host-reviewed plan by `(plannedAtMs, requestId)` and projects it as
`planned`, or as `cancelled` when its exact Host cancellation record exists.
The projection binds the installed receipt, current state generation, selected
surfaces, plan action/digest, Registry source/cutover, selected providers,
awaiting-admission or cancelled Grant state, and a deterministic expected
lifecycle-unit count reconstructed from the installed manifest. It never
persists that reconstructed intent. A completed Use enablement operation or
durable Host outcome suppresses the pre-admission plan, while state/desire drift
causes it to be ignored. The index retains the complete managed scope only for
private request lookup; Host/request/authority/fence identities do not enter
the public result. The contract is bounded to 2 MiB, rejects unknown fields and
inconsistent counts/digests/phases, and omits paths, Registry URLs, idempotency
keys, credentials, tokens, secret names and values, package content, and
arbitrary package-authored text. Invalid backing evidence fails closed with a
path-free cleanup/reinstall instruction instead of echoing the damaged state.
For each Registry-backed Add or Replace transition in a retained install or
upgrade graph, the operation reports signed expected bytes, currently retained
bytes, and an exact-target `missing`, `partial`, or `complete` cache status.
The aggregate is `missing`, `in-progress`, or `complete`; non-download actions
are `not-required`, while a graph containing a source that cannot be observed
is `unavailable`. The datastore identity is derived from the exact historical
`VerifiedCatalogProvenance`, so replacing the named Registry source cannot
redirect an old operation to new cache state. Observation is lock-independent,
zero-network, read-only, and path-free. `complete` means an exact-length regular
entry exists where only verified promotion normally writes; observation does
not rehash it. A partial or complete observation is never download, apply, or
recovery authority.

The same retained graph and download-attempt projections observe every
separately signed executable-planning target selected by the exact package
lock. `planningBytes`, `planningRetainedBytes`, `planningTargetCount`, and
aggregate `planning` accompany a canonical `planningTargets` inventory. Each
entry contains only package ID, Registry name, target digest, expected/retained
bytes, and `missing`, `partial`, or `complete` state. A package without a
planning target contributes no entry; an entirely static operation reports
`not-required`. Historical Registry provenance selects the datastore exactly
as it does for archives. Observation never waits for the cache lock, rehashes
content, or turns a partial/complete entry into planning, apply, or recovery
authority.

Before Registry/TUF metadata access and before an exact package lock exists,
Use writes `a3s.use.plugin-resolution-attempt.v1` under the same process-held
per-package lock used by the later download phase. It binds the explicit User
or Workspace scope, install/upgrade action, root package, requested version and
channel, refreshed/cached access, and start time. Each configured root or
dependency Registry advances through `pending`, `verifying`, `verified`, or
`failed` with only its path-free name, source-identity and trust-root digests,
verified TUF role versions, bounded package-target count, observation time, and
stable bounded error code. The record cannot contain a Registry URL, path, raw
transport error, credential, or metadata byte. Terminal success additionally
binds the exact package-lock digest and package count.

When no retained graph or download attempt exists, `extension diagnose` falls
back to `a3s.use.plugin-resolution-attempt-diagnostic.v1`. Its phase is
`pre-lock`; access is `refreshed` or `cached`; and overall state is
`resolving`, `resolved`, or `failed`. Reads are bounded, zero-network,
read-only, and never wait for or acquire the package lock. Resolution failure
or process exit retains the record. A successful handoff writes the matching
download attempt before deleting it, so diagnosis has no intentional gap.
Damaged state fails closed without echoing its fields. Real CLI tests cover an
externally killed online resolution, a terminal Registry verification failure,
and an offline cache-missing failure that constructs no network transport.

After exact lock resolution and disposition selection but before package
archive transfer, Use writes `a3s.use.plugin-download-attempt.v1`. It binds the
explicit User or Workspace scope, install/upgrade action, exact package lock and
digest, selected package IDs, and start time. A process-held per-package lock
prevents a live attempt from being replaced. Failure or process exit leaves the
record available for diagnosis; a later process may replace it only after the
lock is released. Use deletes it only after a matching reviewed pending graph is
durable. The record is observation state, not a plan, receipt, apply input, or
recovery authority.

When no graph, active enablement operation, or diagnosable Host-reviewed plan
exists, `extension diagnose` falls back to
`a3s.use.plugin-download-attempt-diagnostic.v1`. It reports the action,
`pre-plan` phase, start time, package-lock digest, package/target counts, and
exact current archive and planning-target byte evidence. Unknown or
inconsistent attempt fields fail closed through the same path-free diagnostic
error. Real killed-process tests observe active archive and planning-target
partials without waiting for the target-cache lock, reobserve retained bytes
after exit, resume the exact Range, and prove the attempt disappears after the
reviewed graph becomes durable.

`extension diagnose --history --json` exposes
`a3s.use.plugin-operation-history-diagnostic.v1` for the explicit package and
User or Workspace scope. Its underlying
`a3s.use.plugin-operation-history.v1` record contains only already validated
public operation diagnostics. It keeps the newest 16 operation occurrences
oldest-first on disk and returns them newest-first within an exact 8 MiB stored
byte limit. Every entry wraps the immutable point-in-time diagnostic with its
retention time and a `completed`/`rolled-back` operation or `cancelled` graph-
plan outcome. The
outcome is accepted only when lifecycle terminal receipts, Grant phase, and
Registry cutover evidence agree; historical `recovery` inside the embedded
point-in-time diagnostic is not current recovery authority.

Successful graph and enablement paths durably append history before deleting
pending graph or active enablement recovery authority. A process exit between
those writes leaves both records; exact replay deduplicates the occurrence by
`(operationId, planDigest)` and then completes cleanup. Graph operation IDs are
lock-derived and can legitimately recur after reinstall, so textual ID alone
is not a history key. Oldest entries are removed before appending would exceed
the count or byte limit. History remains queryable after uninstall. Reading it
performs no network access, reconciliation, recovery, or write. Unknown fields,
duplicate occurrence identities, inconsistent outcomes, links/reparse points,
non-regular files, and oversized records fail closed through the path-free
operation diagnostic error without echoing retained content.

Real Host/CLI tests additionally prove planned and cancelled pre-admission
enablement projections perform no network request, authorization, admission,
or lifecycle write, expose no Host fence or path, and disappear during the
completed-Use/unfinished-Host-outcome window rather than presenting stale
apply guidance.

## Canonicalization and limits

Machine-owned JSON uses deterministic canonical serialization before digesting.
Collections that represent sets are sorted and unique. Unknown fields fail
closed. Contract, plan, package, dependency, Registry, target, and journal
sizes are bounded by checked constants.

Human-authored configuration remains ACL. JSON is used for signed/canonical
machine records, receipts, locks, plans, backup manifests, and command output.

## Golden fixtures

Current canonical fixtures live under:

- `crates/core/fixtures/plugins/`;
- `crates/extension/fixtures/manifests/`;
- `crates/extension/fixtures/packages/`; and
- `crates/extension/fixtures/registry/plugin-v3/`.

Each canonical JSON fixture has an adjacent SHA-256 golden where applicable.
Tests verify round trips, unknown-field rejection, digest stability, signed
catalog/manifest binding, current host inventory, operation confirmation, and
fail-closed rejection of superseded inputs.

## Evolution rule before first release

Until the first supported product release:

1. change the current schema when the design requires it;
2. update all producers, consumers, fixtures, tests, and documentation in the
   same change;
3. remove the superseded decode/API/storage path;
4. reject stale state with a cleanup and reinstall instruction; and
5. retain only negative fixtures needed to prove rejection.

After the first supported release, compatibility and migration policy must be
an explicit product decision. That future policy must not be implemented
preemptively in the current preview.
