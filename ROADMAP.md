# A3S Use Roadmap

Last updated: 2026-08-23

## Product status

A3S Use is a development preview. The cognitive-package platform has not
shipped a supported product release and is not production-ready.

This roadmap is deliberately release-oriented. Completed internal contracts
or green unit tests are evidence of implementation progress, not a release
claim. A product release requires the cross-repository, cross-platform, supply
chain, recovery, and operational gates in this document.

## Product outcome

A3S Use will be the AI Native Package Manager for A3S hosts on Linux, macOS,
and Windows. It must install platform-native capabilities and versioned
cognitive packages whose dependency graph can contribute:

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

1. **The package is the lifecycle unit.** A surface cannot be installed,
   upgraded, enabled, disabled, or removed independently of its owning package.
2. **There is one current cognitive-package format.** Manifest v3, catalog v3,
   receipt v4, plan v4, host protocol v6, managed scope v2, manager toolset v4,
   pending graph v4, pre-lock resolution attempt/diagnostic v1, pre-plan
   download attempt/diagnostic v1, and enablement state/operation v2 are the
   only accepted baseline.
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

## Current protocol baseline

| Contract | Accepted version |
| --- | --- |
| Cognitive-package manifest | schema version 3 |
| Signed catalog record | `a3s.use.plugin-catalog.v3` |
| Installed receipt | schema version 4 |
| Package lock | `a3s.use.plugin-package-lock.v1` |
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
| Enablement state | `a3s.use.cognitive-package-enablement-state.v2` |
| Enablement operation | `a3s.use.cognitive-package-enablement-operation.v2` |
| Runtime Task binding | `a3s.use.runtime-task-binding.v4` |
| Runtime Service provisioning | `a3s.use.runtime-service-provisioning.v1` |
| Runtime Service binding | `a3s.use.runtime-service-binding.v3` |
| Capability snapshot cursor | `a3s.use.capability-snapshot-cursor.v1` |
| Extension snapshot cursor | `a3s.use.extension-snapshot-cursor.v1` |
| Coordinated Use state backup | `a3s.use.state-backup.v1` |
| Coordinated Use state backup retention plan | `a3s.use.state-backup-retention-plan.v1` |
| Coordinated Use state backup retention result | `a3s.use.state-backup-retention-result.v1` |
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
- [x] Prior-generation retirement fails unless the graph route is already
  absent.
- [x] Hosts can acquire an exact currently published lifecycle generation by
  package, manifest, and generation identity; the lease participates in the
  same accepted-call drain as route dispatch.
- [x] Hosts can derive a typed capability/Registry cursor and atomically lease
  every callable package generation in canonical order. Publication is
  rechecked after the complete batch is held; stale, hidden, mixed,
  digest-mismatched, contended, or non-lifecycle routes fail closed without a
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
- [x] Capability snapshot v2 projection for exact scope/package/generation
  matched release-backed Runtime Tool Task bindings.
- [x] Workspace Grant proposal/change/resolution/ceiling binding.
- [x] Candidate Grant persistence before prepare, cutover checkpointing,
  drain-before-revoke, and joint pre-cutover rollback.
- [x] Manager MCP toolset v4 with explicit install-time Registry selection,
  read-only planning, and one apply tool.
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
  cutover but before dependency journal and parent graph completion, followed
  by zero-network exact replay with one complete visible closure and no
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
- [ ] Consume the reviewed Runtime Task projection in Code CLI/TUI and agent
  tool discovery, then route invocation through the leased Use dispatcher.
- [ ] Compose HTTP/streamable MCP through Gateway with health, drain, and
  exact-generation retirement.
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

- [ ] Run one shared Plugin Manager service across CLI, TUI, and manager
  MCP without a second catalog, plan, or mutation implementation.
- [ ] Verify TUI `/packages` and CLI output show the exact plan, package graph,
  source, permission ceiling, and confirmation boundary.
- [ ] Prove install → invoke → exact-generation upgrade → invoke → uninstall
  → process restart for Tool, MCP, Flow, Skill, UI, and OKF.
- [ ] Prove watcher resumption, no duplicate side effects, and path-free
  retained history after process restart.
- [ ] Run the same scenarios for User and Workspace scope and reject scope-kind
  substitution under the same textual ID.

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
  passed the current non-Science workspace suite on all four targets from exact
  `main` commit `40bc5593cbf58ca2da171d85ba578c2d6bd911c8` while the
  matching Windows job and general release gates also passed.
- [x] Run signed Registry trust/lock, dependency-graph install/upgrade/uninstall,
  Grant, standalone Flow preflight/lifecycle, and OKF cutover scenarios through
  real `a3s-use` processes on Windows x86_64, including killed-process replay
  of removed-dependency cleanup without capability-generation inflation.
- [x] Run the complete current non-Science workspace suite on Windows x86_64
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
  now use one final-component no-follow handle from discovery through append,
  checkpoint, and final pre-promotion verification. Promotion reopens the
  final path without following it, rehashes it, and retains that exact handle
  through staging; a deterministic post-verification replacement fails commit.
  A live Windows partial or verified-target handle permits readers but denies
  external writes, removal, and replacement, while Unix staging remains bound
  to the verified handle after a path replacement. Windows-native scanner tests
  now prove a transient read-only handle that denies delete sharing converges
  into promotion within the two-second retry bound. A persistent handle stops at
  the bound without publishing or deleting the complete partial; after release,
  the next transaction rehashes, promotes, and stages the exact bytes without a
  network transfer. Reboot recovery, antivirus contention beyond this exact
  promotion boundary, product-host contention, and the remaining platform
  scenarios stay open.
- [x] Test real-process uninstall interruption between durable Registry cutover
  and its package receipt, then hold the prior generation lease through restart
  to prove drain-before-removal and exact generation replay.
- [ ] Complete the interrupted download, archive extraction, graph/Grant
  cutover, drain, removal, process crash, reboot, remaining antivirus contention
  outside verified-target promotion, and reparse-point replacement matrix. A
  real `a3s-use` process-kill test now
  proves digest-bound target download resume without partial publication. A
  second real-process test kills installation while a verified high-entry
  archive is being extracted, proves no receipt, graph, pending operation, or
  package root was published, and completes an exact zero-network retry from
  the verified cache. A third real-process test kills the following immutable
  package copy after its pending plan and applying journal are durable, then
  proves retry reclaims the actual bounded crash-staging tree, publishes only
  the exact generation once, and removes the pending operation. Package commit
  also rejects staging or package-parent links/reparse points. A fourth
  real-process test kills uninstall during physical generation deletion after
  route hiding and receipt removal; exact replay finishes the partial directory,
  completes the journal, and does not inflate the Registry generation. A fifth
  real-process test kills a nine-node install after the complete atomic graph
  is visible but before one dependency journal and the installed parent graph
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

- [ ] Publish and operate at least one documented Registry with root rotation,
  expiry, mirror replacement, offline recovery, and incident procedures.
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
  and OKF projections. A deterministic `a3s.use.state-backup.v1` whole-install
  inventory now snapshots all allowlisted Use-owned families under one
  exclusive maintenance fence, binds Registry generation/snapshot and installed
  receipt digests, excludes locks, rejects nonterminal or unknown state, and
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

1. One reviewed Plugin Manager serves CLI, TUI, and agent management MCP.
2. All six surfaces have production provider composition in declared hosts.
3. Exact graph and Grant recovery passes failure injection at every checkpoint.
4. Linux, macOS, and Windows pass the declared real-process matrix.
5. Registry operations, signing, provenance, and release installation are
   independently reproducible.
6. Storage retention, repair, observability, incident response, and support
   procedures are documented and exercised.
7. Website and README examples pass against the release candidate.

Until then, README and website copy must say **development preview** and must
not advertise production readiness or a stable cognitive-package contract.

## Completion definition

A3S Use is ready to publish only when a user can select a trusted Registry,
review one exact plan, install a signed dependency graph, hot-use its six
surface types in a supported host, recover from interruption without guessing,
upgrade without exposing mixed generations, and uninstall without leaving
routes, grants, processes, projections, or package-owned state behind.
