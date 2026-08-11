# A3S Use Roadmap

Last updated: 2026-08-11

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
   receipt v3, plan v4, host protocol v4, manager toolset v4, pending graph v2,
   and enablement state/operation v2 are the only accepted baseline.
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
| Installed receipt | schema version 3 |
| Package lock | `a3s.use.plugin-package-lock.v1` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v4`, protocol 4 |
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v4` |
| Pending package graph | `a3s.use.pending-package-graph-operation.v2` |
| Lifecycle diagnostic | `a3s.use.plugin-lifecycle-diagnostic.v1` |
| Enablement state | `a3s.use.cognitive-package-enablement-state.v2` |
| Enablement operation | `a3s.use.cognitive-package-enablement-operation.v2` |
| Runtime Task binding | `a3s.use.runtime-task-binding.v4` |
| Runtime Service provisioning | `a3s.use.runtime-service-provisioning.v1` |
| Runtime Service binding | `a3s.use.runtime-service-binding.v3` |
| OKF Knowledge backup | `a3s.use.okf-knowledge-backup.v1` |

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
- [x] Expose one bounded, digest-pinned, immutable bootstrap-root admission API
  for managed hosts, returning its exact digest/version/size evidence; the
  admitted bytes still require the ordinary complete TUF refresh before
  catalog evidence is trusted. Standalone root imports and managed admission
  share that one public size bound.
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
  digest-bound database backup and offline verification, plus repair limited to
  rebuilding the derived search index from validated documents.
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
  revalidation, and expired-plan recovery only after Use-owned durable
  admission or completion evidence.

### Validation and documentation

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
- [x] Test-binary subprocess exit after all 14 Grant Store durable checkpoints
  in the canonical two-candidate/two-retirement lifecycle across forward
  prepare, cutover/retirement, and pre-cutover rollback, with exact
  candidate/prior convergence and terminal journal replay.
- [x] Real `a3s-use` process exit after an uninstall Registry hide cutover and
  before its package hide receipt, followed by exact-plan restart, an observed
  accepted-call drain, physical removal, and no capability-generation
  inflation. Missing generation state without the exact durable cutover is
  rejected without changing graph, pending-plan, or Registry evidence.
- [x] Signed standalone CLI Flow/OKF/Skill/UI install, process-restart
  observation, exact upgrade, uninstall, failed-preflight non-publication, and
  repaired exact replay coverage on Unix and Windows x86_64. The OKF fixture
  also exercises audit, backup, offline verification, and confirmed FTS repair.
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
- [ ] Consume the reviewed Runtime Task projection in Code CLI/TUI/Web agent
  tool discovery, then route invocation through the leased Use dispatcher.
- [ ] Compose HTTP/streamable MCP through Gateway with health, drain, and
  exact-generation retirement.
- [x] Complete bounded storage quota, projection retention, tombstone garbage
  collection, and physical compaction in the standalone Knowledge backend.
- [x] Complete managed Code/Web Knowledge Workspace/session carriers and prove
  leased prior-generation query semantics through those hosts.
- [x] Publish enabled Code Web Activity documents at exact Registry generation
  and revision URLs with opaque-origin CSP/security headers, restart stability,
  stale-generation `410 Gone`, and no managed-path disclosure.
- [x] Complete Code Web iframe adoption, dedicated v3 `MessagePort` brokering,
  ambient-message rejection, self-navigation termination, exact-document
  context binding, bounded state messaging, and active-generation frame/port
  replacement and drain.
- [x] Add bounded Code-owned durable UI state keyed by scope/package/surface,
  exact published-generation request leases, restart recovery, retained-surface
  upgrade/rollback preservation, and true-uninstall cleanup.
- [x] Complete Code Web failed-N+1 pre-cutover browser readiness, authority-free
  candidate delivery, failure rollback with N still callable, stale-plan
  non-replay, fresh-plan retry at the same N+1 lifecycle generation, and one
  successful Registry cutover.
- [ ] Complete reviewed UI backend bindings and equivalent sandbox/generation
  composition in CLI, TUI, and native hosts. Those hosts remain
  static-integrity-only until they inject an equivalent renderer.
- [x] Prove that every required surface remains unpublished when its owner or
  evidence is missing.

Exit gate: a six-surface signed package completes install, enable, upgrade,
disable, and uninstall through the same reviewed plan/apply service in each
supported managed host.

### M2 — Finish A3S Code TUI/Web hot-plug qualification

Status: in progress

- [ ] Run one shared Plugin Manager service across CLI, TUI, Web, and manager
  MCP without a second catalog, plan, or mutation implementation.
- [ ] Verify TUI `/packages` and Web marketplace show the exact plan, package
  graph, source, permission ceiling, and confirmation boundary.
- [ ] Prove install → invoke → exact-generation upgrade → invoke → uninstall
  → process restart for Tool, MCP, Flow, Skill, UI, and OKF.
- [ ] Prove watcher resumption, no duplicate side effects, and path-free
  retained history after process restart.
- [ ] Run the same scenarios for User and Workspace scope and reject scope-kind
  substitution under the same textual ID.

Exit gate: Code CLI/TUI/Web and agent tools produce the same plan digest and
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

Status: pending

- [ ] Run full workspace and real-process package lifecycle tests on Linux
  x86_64/arm64 and macOS arm64/x86_64.
- [x] Run signed Registry trust/lock, dependency-graph install/upgrade/uninstall,
  Grant, standalone Flow preflight/lifecycle, and OKF cutover scenarios through
  real `a3s-use` processes on Windows x86_64, including killed-process replay
  of removed-dependency cleanup without capability-generation inflation.
- [x] Run the complete current non-Science workspace suite on Windows x86_64
  and reject directory junctions across package, Registry/cache, Grant,
  lifecycle, Runtime, Flow, and Knowledge trust boundaries.
- [x] Run the Runtime Service provisioning subprocess-exit matrix for Tool and
  HTTP MCP on the configured platform CI jobs. Real managed-provider and CLI
  process-kill qualification remains open.
- [ ] Expand the remaining Windows gate to the complete filesystem, Runtime,
  MCP, watcher, failure-injection, and crash-recovery matrix.
- [x] Test real-process uninstall interruption between durable Registry cutover
  and its package receipt, then hold the prior generation lease through restart
  to prove drain-before-removal and exact generation replay.
- [ ] Test the remaining interrupted download, archive extraction, graph/Grant
  cutover, drain, removal, process crash, reboot, antivirus contention, and
  reparse-point replacement races.
- [ ] Verify release archives install and run without repository-local paths.

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
- [x] Rebuild every shipped native executable for all five targets on a second
  clean runner without a compiled-artifact cache, require a byte match with the
  primary archive, and publish deterministic attested rebuild evidence.
- [ ] Add an externally operated witness for the complete staged tree and final
  archive digest, and retain verification evidence outside the Release asset
  trust boundary.
- [ ] Define storage retention, quota, garbage collection, backup, and repair
  procedures for packages, cutover evidence, Grants, Flow history, UI state,
  and OKF projections. Scope-local OKF database audit, verified backup, and
  derived-index repair are implemented, but restore, binding/authority
  recovery, backup rotation, and whole-product procedures remain open.
- [x] Expose bounded, secret-free latest/previous lifecycle checkpoint
  diagnostics through `extension inspect --json`.
- [ ] Add broader telemetry and diagnostics for plan, download, provider
  readiness, cutover, drain, rollback, and recovery without exposing secrets.
- [ ] Complete threat model review, privilege boundaries, security response,
  upgrade policy, and support runbooks.

Exit gate: a release candidate can be installed, upgraded, recovered, audited,
and removed by an operator using only published artifacts and documentation.

## Release blockers

The first supported product release is blocked until all of the following are
green:

1. One reviewed Plugin Manager serves CLI, TUI, Web, and agent management MCP.
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
