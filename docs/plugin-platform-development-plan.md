# A3S Use Plugin Platform Development Plan

Status: active
Last updated: 2026-08-23

## Objective

Deliver a supported cross-platform AI Native Package Manager that installs one
signed cognitive package and its complete SemVer dependency closure, then
hot-publishes Tool, MCP, OKF, A3S Flow, Skill, and UI through one reviewed,
recoverable lifecycle.

The product is not released and not production-ready. This plan assumes one
current contract baseline and explicitly excludes pre-release compatibility
work.

## Engineering constraints

1. Work in the relevant Use crate/application, not the A3S monorepo root.
2. Human-authored package/configuration files use ACL and `a3s-acl`.
3. CLI, TUI, and agent tools call one Plugin Manager.
4. Planning is read-only; `plugin_apply_plan` is the only manager mutation.
5. Registry URLs and trust roots are host configuration.
6. Required missing provider/readiness evidence fails before publication.
7. Package, Grant, provider, and capability generations cut over together.
8. Recovery uses exact durable evidence and never infers deleted state.
9. Superseded preview contracts, APIs, and disk layouts are deleted, not
   migrated.
10. Modified behavior receives focused and integration tests.

## Current baseline

Completed in the Use repository:

- manifest v3 and all six named surface contracts;
- catalog v3, complete TUF metadata, verified provenance, and exact receipts;
- content-addressed verified archive/planning-target caching plus explicit
  zero-network install and upgrade with full cached-evidence revalidation;
- canonical ACL Registry source state with a bounded enabled set, default
  selection, revision-bound confirmed mutations, managed trusted-root import,
  and identity-isolated TUF/cache state reused by exact source restoration;
- typed per-Registry cache byte/entry/free-space bounds, oldest-first
  retention, stale-write cleanup, usage evidence, and confirmed zero-network
  garbage collection;
- separately signed package-native Tool/stdio MCP planning targets rebound to
  the digest-bound manifest after download;
- bounded SemVer resolver, package lock, prior/candidate upgrade binding;
- process-resilient pre-lock Registry/TUF resolution attempts with path-free
  refreshed/cached per-source diagnostics and exact handoff to download state;
- operation plan v4, confirmation, policy and host/provider evidence;
- manager toolset v4 with install-time Registry selection and plan/apply
  enablement;
- immutable package generations and one unified installation snapshot;
- dependency-forward install, one cutover, reverse uninstall and upgrade GC;
- durable Registry cutover replay and exact lifecycle journals;
- Workspace Grant composition, joint rollback, and drain-before-revoke;
- authorization-safe two-pass provider binding from an unbound draft through
  assigned-provider preflight, host policy, canonical Grant semantics, and a
  drift-checked final immutable plan;
- exact Add/Replace/Retain/Enable lifecycle-generation derivation plus
  apply-time Grant and provider-evidence reconstruction contracts;
- standalone executable Task, stdio MCP, Skill/UI, SQLite/FTS5 OKF, and
  explicitly configured real `a3s-flow` Native TypeScript lifecycle hosts;
- typed Runtime Service endpoint consumption and an injected Gateway lifecycle
  port that drains routes before Runtime stop, removes routes before Runtime
  removal, and retains the exact binding receipt until both complete;
- whole-scope OKF expanded-byte/projection quotas, per-surface generation
  limits, globally bounded tombstones, post-removal SQLite/WAL compaction, and
  exact-scope storage usage diagnostics;
- versioned, package-scoped latest/previous lifecycle checkpoint diagnostics
  that exclude idempotency keys, credentials, tokens, secret values, and
  package-authored error text, while distinguishing same-operation candidate
  and retirement phases by exact intent digest;
- bounded per-scope/package terminal operation history with validated outcome,
  replay-safe `(operationId, planDigest)` identity, and zero-network CLI
  observation after package removal;
- CLI diagnostics, package graph commands, Knowledge search/usage, and watchers;
- current-schema fixtures, digest goldens, remote Registry tests, recovery
  tests, and GitHub Pages site.

## Workstream A — Managed provider completion

Priority: P0
Status: in progress

### A1 Runtime Service

- [x] Compose native and managed provider-neutral package plans through typed
  `RuntimeClientRegistry` objects, including mixed-surface packages.
- [x] Bind canonical pre-confirmation Grant proposals into final
  provider/build/capability/enforcement/semantics evidence in A3S Use.
- [x] Implement the unbound → provisional Grant/provider preflight → host
  authority → canonical Grant/final provider protocol, including a final
  policy fixed-point check.
- [x] Derive exact managed generations for Add, Replace, Retain, and Enable;
  reject missing prior generations and counter exhaustion.
- [x] Expose apply-time Grant reconstruction and exact reviewed-provider
  evidence verification without persisting process-local Runtime clients.
- [x] Wire two-pass planning and exact apply-time reconstruction into the
  shared CLI install and reviewed enable/disable paths, with durable planning
  bundles, Grant snapshots, and provider generations across restart.
- [x] Implement restart-safe exact-generation Runtime Task invocation from a
  self-contained argument-free binding template, with receipt-owned provider
  reconnection, stale-generation rejection, and an accepted-call Registry
  lease held through capture and cleanup.
- [x] Publish exact installation/package/generation-matched Runtime Tool Tasks
  through capability snapshot schema v5; omit missing or mismatched bindings.
- [x] Require explicit owner evidence for every required Tool, MCP, Flow, OKF,
  Skill, and UI surface before atomically publishing the capability generation.
- [x] Persist `a3s.use.runtime-service-provisioning.v1` before apply, retain
  exact plan/provider/request identity across Tool and HTTP MCP Gateway bind
  failures, commit the final binding without an unowned gap, and make
  candidate rollback converge without duplicate Runtime effects or residue.
- [x] Exercise Tool and HTTP MCP in real test subprocesses across requested,
  Runtime-effect, runtime-applied, Gateway-effect, gateway-ready, and
  final-binding commit windows; require one effect, exact replay, and complete
  route/unit/binding cleanup.
- Complete production Runtime Service provider
  composition without weakening exact-generation drain and removal.
- [x] Recover a confirmed same-generation provider resource loss by draining
  and removing only the stale Gateway route, retaining the Runtime unit,
  replaying apply with a new exact request key, and publishing a fresh binding
  receipt for the newly allocated Gateway endpoint. Interrupted route removal
  retains the prior receipt for exact replay without a Runtime stop or remove
  effect.
- [x] Consume only Runtime-published generation-bound loopback Service
  endpoints and enforce Gateway drain/remove ordering in the shared lifecycle
  adapter.
- [x] Prove no native fallback when the selected provider disappears during
  Use-owned selection.

Acceptance:

- signed Tool Task and Service packages install, invoke, upgrade, and uninstall;
- N remains callable only through leases accepted before N+1 cutover; and
- changed provider evidence fails before archive download or mutation.

### A2 HTTP MCP and Gateway

- [x] Compose HTTP/streamable MCP with Gateway-owned endpoint lifecycle.
- [x] Define the typed bind, drain, and receipt-owned remove boundary used by
  Tool Services and Streamable HTTP MCP.
- [x] Bind private service identity, MCP endpoint, health, and permission
  ceiling.
- [x] Drain prior sessions and routes before removing the prior generation.
- [x] Keep standard MCP transport; do not add an A3S RPC dialect.

A3S CLI `main` commit `563e7e139740e845369f9102a2d47026733797a8`
qualifies four real Linux Tool and MCP processes across retained N/N+1
routing, standard MCP initialize, Gateway and lifecycle-host restart,
stop/drain, exact receipt-owned removal, and zero residual routes, Runtime
units, receipts, or PIDs. Runtime provider-process loss/rebinding, non-Linux
providers, and the complete cross-platform recovery matrix remain open.

Acceptance:

- stdio and HTTP MCP use the same package plan/cutover model; and
- endpoint or health failure keeps the required MCP and dependents unpublished.

### A3 Managed Knowledge

- [x] Enforce atomic receipt-accounted expanded-byte and projection quotas for
  each complete User/Workspace scope in the standalone SQLite backend.
- [x] Bound per-surface generations and scope-wide tombstones, reclaim removed
  index pages, truncate the WAL, and expose typed usage evidence.
- [x] Expose exact published-generation leases that participate in package
  generation drain before Knowledge retirement.
- [x] Add A3S Code Workspace and session carriers for exact OKF projections.
- [x] Prove leased prior-generation query semantics through managed hosts.
- Preserve complete User/Workspace scope in every database, request, citation,
  and observation.

Acceptance:

- signed install/search/upgrade/search/uninstall/restart works through managed
  hosts with exact citations and no stale projection access.

### A4 Sandboxed UI

- [x] Validate exact UI entry points and declared asset digests during package
  lifecycle changes.
- [x] Clear receipt-owned UI state on true surface removal.
- [ ] Bind UI to exact Skill/Tool/MCP/Flow readiness and Grant evidence.
- [ ] Add sandboxed, generation-aware rendering in supported hosts; CLI and TUI
  remain static-integrity-only until a reviewed renderer is injected.

Acceptance:

- UI cannot access undeclared hosts, processes, files, secrets, or network;
- uninstall leaves no receipt-owned projection, state, package publication, or binding.

## Workstream B — A3S Code CLI/TUI convergence

Priority: P0
Status: in progress

- [x] Expose the Use-owned typed capability/Registry cursor and an injected,
  all-or-nothing exact-generation snapshot lease. Keep CLI snapshot schema v5
  unchanged and leave Run scope ownership to Code.
- [x] Implement one shared typed Plugin Manager application service over the
  production Host Manager, including stable listing/search identities, all
  frozen planning operations, durable reviewed-plan reopening, digest-only
  apply, and a standard MCP adapter with injected trusted confirmation.
- [x] Migrate the standalone CLI to that service. The `plugin` command maps the
  four reads, five planning operations, and digest-only apply directly to the
  manager-v4 inputs and typed results. Plans do not mutate, apply requires the
  exact durable operation ID and plan digest plus explicit `--yes`, `Ask` plans
  receive no implicit confirmation, and exact replay remains zero-network.
  Registry-backed compatibility install, upgrade, and uninstall fields remain
  unchanged.
- [x] Migrate the A3S Code TUI and compose the manager MCP in Code on that
  service without a second presentation-owned plan, confirmation, or mutation
  path. A3S CLI commit `ce1240891d6926c132aed8212efabaf6c925f4db`
  composes CLI, TUI `/packages`, and the exact manager-v4 MCP over one service.
- Keep Registry source state, catalog cache, plan generation, policy, apply, and
  operation replay out of view-specific code.
- TUI `/packages`, CLI, and agent MCP must display and apply the same operation
  ID and plan digest.
- Use one watcher keyed by capability generation plus revision.
- Preserve exact Flow and OKF history without repository-local paths.

The managed Host process-recovery gate now preserves an operation status
revision from the reviewed pre-admission state through an externally killed
apply and offline recovery. The resumed watcher observes one completed change,
terminal replay remains unchanged, the Registry generation does not inflate,
and retained history exposes neither paths nor Host request identities.

Required E2E:

```text
search
→ inspect catalog/source/permissions
→ plan install
→ confirm/apply
→ invoke Tool + MCP + Flow + OKF + UI/Skill
→ plan/apply exact-generation upgrade
→ invoke N+1 and inspect retained N history
→ plan/apply uninstall
→ restart all processes and verify terminal replay/no residue
```

Run the sequence for User and Workspace scopes, permission-free and
permission-bearing packages, and interrupted operations at every durable
checkpoint.

## Workstream C — Distributed A3S Flow

Priority: P1
Status: pending

- Bind package Flow source/export/generation to remote scheduling without a
  second package lifecycle.
- Treat `flow.json` only as a visual design/deployment carrier.
- Define execution placement, resume, cancellation, retry, and observation
  ownership between Code, OS, Flow, Runtime, and Use.
- Preserve the same plan, Grant, dependency edges, and package receipt for local
  and remote execution.

Acceptance:

- changing placement does not change package identity or create another lock,
  receipt, or journal;
- local and remote observations bind the same compiled Flow generation; and
- process/network interruption resumes without duplicate workflow effects.

## Workstream D — Cross-platform hardening

Priority: P0 release gate
Status: in progress

- A killed real-process Registry target download resumes from its exact
  persisted prefix without publishing a receipt, installation snapshot,
  operation, or package root.
- A killed real-process verified archive extraction leaves no receipt,
  installation snapshot,
  pending operation, or package root; an explicit offline retry revalidates the
  cached target and completes without a network request.
- A killed real-process immutable package copy leaves its exact pending plan
  and applying lifecycle journal but no receipt, installation snapshot, or
  package binding. Offline replay reclaims the actual bounded crash-staging tree,
  publishes one generation, and removes the pending operation.
- Uninstall retires the exact scoped graph, receipt, and package-binding authority while
  retaining global expanded-package bytes. Replay finishes cutover
  acknowledgement without advancing the Registry generation again, even when
  another process holds the shared artifact open for reading.
- A killed real-process nine-node install retains one complete atomically
  published Registry graph and its pending cutover before a dependency journal
  and the installation snapshot complete. Explicit offline replay performs no
  network request, completes every package journal and the exact snapshot,
  retires the cutover, and does not advance the Registry generation again.
- Externally killed managed-host processes cover five-node permission-bearing
  install, upgrade, and uninstall after Registry publish/hide while one
  dependency publication receipt is pending and the Grant operation remains
  prepared. Restart disables reauthorization, performs no network request,
  preserves the exact candidate Grant, retires only the bound prior Grant,
  completes package and Grant journals, and does not inflate the Registry
  generation.
- Five real `CognitivePackageHostManager` protocol children cover the complete
  reviewed mutation set after the Registry server is stopped. Install, upgrade,
  and uninstall stop at the five-node graph publish/hide boundaries. Disable
  stops after root hide and Grant cutover while accepted-call drain is blocked;
  enable stops after publication while its candidate Grant remains prepared.
  Digest-only apply recovers from the durable reviewed plan and confirmation,
  uses only the exact planning cache for install/upgrade, completes drain and
  lifecycle/Grant journals, converges the exact candidate/prior Grant or
  enablement regrant/revocation, and remains terminally replayable without
  reauthorization, network access, or Registry generation inflation.
- Reject package-parent or staging links/reparse points without following them.
  Native CI run
  [32604181662](https://github.com/A3S-Lab/Use/actions/runs/32604181662)
  passed the current Use-owned suite on all five release targets from exact
  `main` commit `40bc5593cbf58ca2da171d85ba578c2d6bd911c8`; product-host,
  reboot, external-contention, and replacement-race qualification remains
  open.

### Linux

- [x] Run the current Use-owned workspace and real-process package lifecycle
  tests on native x86_64 and arm64 runners in CI run 32604181662.
- Container/release-bundle conformance and filesystem failure injection.

### macOS

- [x] Run the current Use-owned workspace and real-process package lifecycle
  tests on native arm64 and x86_64 runners in CI run 32604181662.
- Quarantine, executable permission, browser/runtime, and filesystem cases.

### Windows

- [x] Run the complete current Use-owned workspace suite and reject real
  directory junctions across package and durable state trust boundaries. CI
  run 32604181662 provides the initial native Windows evidence. The Flow
  Runtime subset additionally runs exact-generation retention, artifact
  substitution, tampered or moved binding records, same-text scope-kind
  isolation, and directory-junction rejection on Windows. Shared link tests
  also exercise Registry target-cache state, lifecycle receipts, package graph
  and diagnostic stores, enablement locks, lifecycle and Runtime records,
  whole-state backup/restore, and OKF database/binding/backup/restore paths
  through real Windows directory junctions without following external content.
  Native Windows tests also prove single-package and graph cutover-capacity
  rejection happens before lifecycle receipt replacement, and that Box CLI
  delegation preserves arguments, output, and exit status through a `.cmd`
  component.
- [x] Route every production temporary-file publication for Registry
  state/cache, Workspace Grants, package and Host records, lifecycle, Runtime,
  Flow, Knowledge, enablement, backup, restore evidence, and diagnostics
  through bounded blocking primitives while preserving replace versus
  no-clobber semantics. Restore and Knowledge recovery moves retain their
  replay source on bounded rename failure, and lifecycle/restore-history
  directory moves use the same retry bound. Retry only Windows access, sharing,
  and lock violations for at most two seconds; prove released file/directory
  lock convergence, persistent-lock bounds, existing-target rejection,
  unchanged prior content, source retention, and temporary cleanup.
- [x] Open retained resumable Registry partials once without following their
  final path and keep that exact handle through admission, append, and
  checkpoint. Verify complete bytes through that handle, then copy and rehash
  into a digest-locked, no-clobber global blob before publishing canonical
  source observation metadata. Retain the no-follow blob handle through staging.
  A Unix partial-path replacement cannot redirect commit away from the held
  source handle, and Windows live partial/blob handles permit readers while
  denying external writes, removal, and replacement. Windows-native scanner
  tests prove transient no-delete-share contention converges within the
  two-second bound. If final partial cleanup remains locked, the durable global
  blob and source observation survive together with a redundant complete
  partial for a later zero-network cleanup retry. Invalid-partial cleanup and
  stale/partial/source-observation reclamation use the same bounded blocking
  delete; native tests prove transient convergence and persistent selected-file
  preservation followed by an exact rescan retry. Source cleanup never deletes
  a global blob.
  Lifecycle receipts and bounded abandoned `.artifact-staging-*` trees also use
  blocking deletion with the same Windows retry. Native tests prove transient
  receipt and nested-file release lets authority retirement or commit continue,
  while a persistent reader of a complete global artifact never delays
  uninstall. Native tests additionally hold the active artifact-staging
  directory at its content rename: transient contention completes the same
  commit, while a persistent lock fails before receipt or Registry-snapshot
  mutation, preserves residual staging, and permits exact commit replay after
  release. Selected upgrade-receipt replacement also has native scanner
  coverage: transient contention completes the same upgrade, while a persistent
  lock retains the valid global candidate artifact, removes retained-receipt
  candidate state, preserves the byte-exact prior published generation, leaves
  no temporary receipt, and permits exact replay after release.
- Expand filesystem coverage to externally raced targets, antivirus contention
  outside global blob publication/source cleanup, active package commit,
  upgrade-receipt replacement, lifecycle removal, and the shared publication
  paths, process groups, named resources, and reboot recovery.
- Run the complete signed six-surface lifecycle and failure matrix.

Acceptance: every advertised platform passes the same contract, lifecycle,
recovery, and residue assertions.

## Workstream E — Registry and supply-chain operations

Priority: P0 release gate
Status: in progress

- Operate a documented TUF Registry with root rotation, expiry, rollback
  protection, mirror replacement, and incident recovery.
- [x] Persist verified archives and signed planning targets by SHA-256 and
  support explicit fail-closed offline install/upgrade without network
  fallback.
- [x] Enforce bounded cache bytes/entries and disk-space admission; expose
  source-bound zero-network usage and confirmed oldest-first garbage
  collection with stale-write cleanup.
- [x] Add bounded, integrity-preserving download resume with durable
  digest-bound partials, exact Range validation, full signed-byte
  re-verification, and partial-aware retention.
- [x] Publish user-scoped Linux/macOS and Windows installers that select the
  exact platform archive, enforce its release checksum, reject unsafe
  extraction and command conflicts, bind packaged OCR/Skill resources, and
  activate one version atomically.
- Publish complete catalog-v3 records and planning targets only.
- [x] Define and implement revision-reviewed source replacement, disable/remove
  without evidence deletion, and exact-identity provenance restoration.
- [x] Deterministically serialize archives and publish per-platform SPDX SBOMs,
  GitHub OIDC provenance/SBOM attestations, and a keyless Sigstore checksum
  bundle from a workflow with pinned Actions and release tools.
- [x] Require both platform installers to authenticate the checksum manifest
  with Cosign against the exact tag workflow identity before archive download,
  fail closed, and retain the verified evidence with the installed version.
- [x] Pass byte-for-byte rebuilds for every shipped native executable on a
  second cache-free clean runner for all release targets. Non-publishing
  qualification run
  [32600882148](https://github.com/A3S-Lab/Use/actions/runs/32600882148)
  byte-matched all five primary archives from exact `main` commit
  `e3d5f955a63cc136dbb07e9419a32760328df320`.
- Add an externally operated full-tree/final-archive witness and retain its
  evidence outside the Release asset trust boundary.
- [x] Verify release archives in clean Linux/macOS/Windows environments.
  Qualification run 32600882148 scanned every target archive for checkout
  paths and passed isolated-home/working-directory execution on all five
  targets.

Acceptance: an operator can bootstrap trust, install, rotate/replace sources,
recover offline, audit provenance, and remove the product using published
instructions only.

## Workstream F — Operations and support

Priority: P0 release gate
Status: pending

- Define retention and garbage collection for packages, prior generations,
  TUF metadata, Grants, Flow history, OKF indexes, UI storage, and journals.
- [x] Define Registry source-observation/partial logical byte, entry, and
  free-space bounds, deterministic retention, stale-write cleanup, usage, and
  confirmed source cleanup. This never deletes a global blob.
- [x] Add a guarded, bounded, path-free physical inventory for raw blobs,
  expanded trees, and abandoned staging; reject unknown or unsafe layout.
- Define cross-state reachability, quota, audit, quarantine, verified
  rehydration, and confirmed GC across every source, installation, and durable
  operation. The physical inventory alone grants no deletion authority.
- Treat the implemented standalone OKF scope quota/GC, integrity audit,
  non-overwriting verified database backup, exact-plan oldest-first backup
  rotation, derived FTS repair, and authority-bound database plus
  exact-subset missing-binding restore as bounded storage controls, not as
  completion of missing Registry/package/lifecycle/Grant authority recovery,
  clean-machine recovery, cross-platform recovery drills, or whole-product
  retention operations.
- Treat the implemented `a3s.use.state-backup.v2` coordinated inventory as the
  corruption-detection input for one explicit installation. It acquires that
  installation's exclusive maintenance fence, binds its Registry and
  installed-receipt authority,
  copies every allowlisted state family with exact hashes, rescans before
  publication, and verifies offline without extraction. Exact-plan retention
  now fully verifies every archive under one external-directory lock, binds a
  path-free oldest-first inventory, rejects stale review, and retains at least
  two recovery generations. Reviewed same-version/OS/architecture restore/apply
  now uses exact independent authority, an external rollback archive, durable
  crash replay, and bounded read-only diagnostics. Missing-authority recovery,
  cross-platform drills, and clean-machine disaster recovery remain open.
- [x] Expose latest/previous package lifecycle checkpoint status, bounded
  failure codes, digests, timings, and rollback evidence through
  `extension inspect --json` without secret-bearing fields.
- [x] Add broader diagnostics for plan, download, provider readiness, cutover,
  drain, rollback, and recovery using non-secret evidence. One exact retained
  planned/admitted/cancelled install/upgrade/uninstall graph or active admitted
  enable/disable operation now has a bounded, path-free cross-product
  projection through `extension diagnose --json`, including reviewed-plan,
  Registry/TUF, provider, Grant, cutover, lifecycle publication/drain/rollback,
  recovery, and retained download-cache evidence. Retained Registry-backed
  install/upgrade graphs and pre-plan download attempts report expected/
  retained archive and executable-planning-target bytes plus exact-target
  missing/partial/complete state from historical provenance without network
  I/O, writes, cache-lock acquisition, or paths. The attempt survives process
  exit and is removed only after the reviewed graph is durable. Standalone Knowledge recovery exposes a path-free
  active/history/capacity projection through `knowledge restore-status --json`.
  `extension diagnose --history --json` additionally retains the newest 16
  completed or rolled-back operations and cancelled graph plans within 8 MiB
  per scope/package. Retention precedes recovery-record cleanup, exact replay is
  deduplicated by `(operationId, planDigest)`, history survives uninstall, and
  corrupt/link/oversize evidence fails closed. Pre-lock Registry/TUF
  resolution now records requested version/channel, per-Registry verification
  state, source/trust digests, role versions, bounded failure, and terminal lock
  evidence before handing off without a diagnostic gap to the download
  attempt. Killed online, terminal verification-failure, and zero-network
  offline-failure CLI tests cover the phase. The newest exact Host-reviewed
  pre-admission enable/disable plan is selected by a digest-bound
  `(plannedAtMs, requestId)` index and projected as `planned` or `cancelled`
  with installed source, selected provider, awaiting/cancelled Grant, exact
  expected lifecycle-unit count, Registry cutover, and stable guidance. Active
  Use evidence takes precedence, and completed Use or Host outcomes suppress
  stale plans. Real CLI tests prove zero network, authorization, admission, and
  Host/fence/path leakage; a killed planning-target transfer proves partial
  observation, retained bytes, exact Range resume, and gap-free graph handoff.
- [x] Define and implement the standalone OKF repair boundary: only FTS rows
  derived from validated documents may be rebuilt; receipt, scope, projection,
  binding, and lifecycle evidence remain immutable and fail closed.
- [ ] Define coordinated backup/restore and repair boundaries for every state
  family. Coordinated path-free inventory backup and offline payload
  verification plus reviewed same-version/OS/architecture restore are
  implemented with exact independent authority, explicit rollback evidence,
  and crash replay. Missing-authority recovery, clean-machine exercises, and
  non-restore repair boundaries remain open. Missing exact evidence must remain
  fail-closed; restore or repair cannot invent authority.
- Complete threat-model review, security response, upgrade policy, rollback
  policy, and support runbooks.
- Establish performance budgets for catalog refresh, resolution, install,
  startup, watcher latency, and storage growth.

## Test matrix

### Contract tests

- current schema canonical round trip and SHA-256 golden;
- unknown-field and superseded-schema rejection;
- pending-operation, pre-plan download, and operation-history diagnostic round
  trips; unknown-field rejection; 2 MiB operation and 8 MiB retained-history
  bounds; exact byte consistency; outcome correlation; replay deduplication;
  oldest-first retention; and one-based final-checkpoint validation;
- catalog/manifest/package/receipt binding;
- host capability and manager toolset exact inventory;
- plan/confirmation/lock/Grant digest stability.

### Registry and resolver tests

- TUF expiry, rollback, root mismatch, target length/digest mismatch;
- incomplete `custom.a3s` rejection;
- missing verified catalog receipt rejection;
- source ambiguity, cycles, incompatible constraints, and search bounds;
- replaceable source configuration without receipt rewrite.

### Lifecycle tests

- forward install and reverse removal order;
- retained dependency exactness;
- Add/Replace/Remove/Retain upgrade;
- cutover replay, completion acknowledgement, and key conflict;
- package-binding absence before retirement;
- drain before Grant/package removal;
- pre-cutover joint rollback and post-cutover retirement recovery;
- failed Flow compiler preflight leaves no published binding or active named
  capability, and a repaired retry resumes the exact durable candidate
  generation;
- test-binary subprocess exit after a durable host effect but before receipt
  persistence at every canonical install, upgrade, enable, disable, and
  uninstall checkpoint, followed by exact-key recovery with one durable
  effect;
- test-binary subprocess exit after a grant-bearing install, upgrade, or
  uninstall atomic graph publish/hide effect but before package publication
  receipts and Grant cutover evidence, followed by exact-key recovery with one
  graph effect and completed package/Grant journals;
- externally killed managed-host processes for five-node install, upgrade, and
  uninstall after Registry publish/hide but before one dependency publication
  receipt and Grant cutover/retirement, followed by zero-network replay with
  reauthorization disabled, the exact candidate/prior Grant transition,
  completed journals, and no second generation for the same cutover;
- externally killed `CognitivePackageHostManager` protocol applies for all five
  reviewed mutations with the Registry unavailable: five-node install/upgrade
  publication or uninstall hide before one dependency receipt and Grant
  cutover/retirement, disable after root hide and Grant cutover while drain is
  blocked, and enable after publication with its candidate Grant prepared;
  replay converges the exact candidate/prior Grant or enablement
  regrant/revocation and the terminal Host outcome without another Registry
  generation; install and upgrade consume only their exact planning caches;
- test-binary subprocess exit after all 14 Grant Store durable phase,
  candidate-receipt, prior-revocation, and candidate-restoration checkpoints
  in the canonical two-candidate/two-retirement lifecycle, followed by exact
  state convergence;
- real CLI exit after the durable uninstall Registry hide and before the
  package hide receipt, followed by exact-plan restart, observed blocking at
  accepted-call drain, physical generation removal, and no second Registry
  generation; absence without the exact cutover remains fail-closed;
- real CLI exit after a complete multi-node Registry publish but before one
  dependency publication receipt and installation snapshot persistence,
  followed by zero-network replay of the exact cutover with no second Registry
  generation;
- no-op terminal result replay without another host call;
- completed graph replay without another atomic publish or hide;
- latest/previous checkpoint diagnostics remain bounded and omit idempotency
  keys and secret-bearing fields;
- pending graph diagnostics remain zero-network and omit paths, Registry URLs,
  idempotency keys, credentials, package content, and untrusted package text at
  reviewed-plan, graph-cutover, Grant-prepared, and accepted-call-drain crash
  windows; invalid backing JSON fails closed without echoing injected secrets;
- completed install/uninstall/reinstall history remains newest-first and
  zero-network after package removal, distinguishes repeated textual operation
  IDs by plan digest, and rejects injected secret-bearing state without
  disclosure; managed Workspace Host kill/recovery retains one exact entry;
- a rolling-back operation rejects a conflicting new intent.

### Security tests

- path traversal, symlink/reparse point, archive link, duplicate path, and size
  attacks;
- plan, policy, scope kind, confirmation, provider, Grant, and generation drift;
- missing recovery evidence and tampered journal rejection;
- static UI/Skill ambient-authority denial;
- secret-safe error diagnostics.

### Product E2E

- one real signed six-surface package through CLI, TUI, and manager MCP;
- install/invoke/upgrade/invoke/uninstall/restart;
- interruption at every durable checkpoint;
- Linux, macOS, and Windows; and
- clean release archive with no checkout-local dependencies.

## Delivery sequence

1. Finish managed Runtime/Gateway/Knowledge/UI composition.
2. Converge A3S Code CLI/TUI and agent MCP on the same service.
3. Complete distributed Flow placement without a second lifecycle.
4. Run cross-platform real-process and failure-injection qualification.
5. Complete Registry, distribution, retention, security, and support operations.
6. Freeze a release candidate only after all documentation examples pass.

## Explicit non-goals

- Replacing general-purpose OS package managers.
- Translating every Tool into MCP or a universal action envelope.
- Letting packages choose providers, Registries, trust roots, or permissions.
- Publishing required surfaces with missing evidence.
- Keeping multiple preview schema/API/storage implementations alive.
- Guessing recovery state after exact journals or the installation snapshot
  was deleted.
- Building a second workflow lifecycle around `flow.json`.

## Release definition

The platform is releasable only when the same signed package graph can be
reviewed, installed, hot-used, upgraded, recovered, audited, and removed across
all advertised hosts and platforms without mixed generations, duplicated side
effects, ambient authority, or leftover owned state.
