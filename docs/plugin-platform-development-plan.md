# A3S Use Plugin Platform Development Plan

Status: active
Last updated: 2026-08-09

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
3. CLI, TUI, Web, and agent tools call one Plugin Manager.
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
- operation plan v4, confirmation, policy and host/provider evidence;
- manager toolset v4 with install-time Registry selection and plan/apply
  enablement;
- immutable package generations and dependency graph records;
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
- Implement exact-generation health, invocation, drain, and removal.
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

- Compose HTTP/streamable MCP with Gateway-owned endpoint lifecycle.
- [x] Define the typed bind, drain, and receipt-owned remove boundary used by
  Tool Services and Streamable HTTP MCP.
- Bind private service identity, MCP endpoint, health, and permission ceiling.
- Drain prior sessions and routes before removing the prior generation.
- Keep standard MCP transport; do not add an A3S RPC dialect.

Acceptance:

- stdio and HTTP MCP use the same package plan/cutover model; and
- endpoint or health failure keeps the required MCP and dependents unpublished.

### A3 Managed Knowledge

- [x] Enforce atomic receipt-accounted expanded-byte and projection quotas for
  each complete User/Workspace scope in the standalone SQLite backend.
- [x] Bound per-surface generations and scope-wide tombstones, reclaim removed
  index pages, truncate the WAL, and expose typed usage evidence.
- [x] Expose exact published-generation leases that participate in package
  route drain before Knowledge retirement.
- [x] Add Code/Web Workspace and session carriers for exact OKF projections.
- [x] Prove leased prior-generation query semantics through managed hosts.
- Preserve complete User/Workspace scope in every database, request, citation,
  and observation.

Acceptance:

- signed install/search/upgrade/search/uninstall/restart works through managed
  hosts with exact citations and no stale projection access.

### A4 Sandboxed UI

- [x] Publish enabled Code Web Activity documents at exact Registry generation
  and revision URLs. Inline only verified assets, enforce opaque-origin CSP and
  restrictive security headers, preserve the identity across restart, reject
  stale generations with `410 Gone`, and disclose no managed paths.
- [x] Complete browser-owned exact-URL iframe adoption, dedicated v3
  `MessagePort` brokering, ambient-message rejection, self-navigation
  termination, exact-document context binding, and bounded state messaging.
- [x] Add Code-owned persistent UI state keyed by scope/package/surface, exact
  published-generation request leases, restart recovery, retained-surface
  upgrade/rollback preservation, and true-uninstall cleanup.
- [ ] Add failed-N+1 readiness/cutover/rollback to keep the selected prior
  document available when a candidate cannot become ready.
- [ ] Bind UI to exact Skill/Tool/MCP/Flow readiness and Grant evidence.
- [x] Add active-document generation-aware hot replacement and port/frame
  drain in Code Web.
- [ ] Add equivalent generation-aware composition and drain in native hosts.

Acceptance:

- UI cannot access undeclared hosts, processes, files, secrets, or network;
- failed N+1 UI leaves N selected; and
- uninstall leaves no origin, storage, route, or binding.

## Workstream B — A3S Code CLI/TUI/Web convergence

Priority: P0
Status: in progress

- Run one shared Plugin Manager application service.
- Keep Registry source state, catalog cache, plan generation, policy, apply, and
  operation replay out of view-specific code.
- TUI `/packages`, Web marketplace, CLI, and agent MCP must display and apply
  the same operation ID and plan digest.
- Use one watcher keyed by capability generation plus revision.
- Preserve exact Flow and OKF history without repository-local paths.

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
Status: pending

### Linux

- Full workspace tests on x86_64 and arm64.
- Real-process Runtime/MCP/Flow/Knowledge/UI lifecycle.
- Container/release-bundle conformance and filesystem failure injection.

### macOS

- Full workspace and real-process tests on arm64 and x86_64.
- Quarantine, executable permission, browser/runtime, and filesystem cases.

### Windows

- [x] Run the complete current non-Science workspace suite and reject real
  directory junctions across package and durable state trust boundaries.
- Expand filesystem coverage to replacement races, file locks, antivirus
  contention, process groups, named resources, and reboot recovery.
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
- [x] Rebuild every shipped native executable on a second cache-free clean
  runner for all release targets, require a byte match with the primary
  archive, and publish deterministic attested evidence.
- Add an externally operated full-tree/final-archive witness and retain its
  evidence outside the Release asset trust boundary.
- Verify release archives in clean Linux/macOS/Windows environments.

Acceptance: an operator can bootstrap trust, install, rotate/replace sources,
recover offline, audit provenance, and remove the product using published
instructions only.

## Workstream F — Operations and support

Priority: P0 release gate
Status: pending

- Define retention and garbage collection for packages, prior generations,
  TUF metadata, Grants, Flow history, OKF indexes, UI storage, and journals.
- [x] Define verified target-payload cache byte/entry/free-space bounds,
  deterministic retention, stale-write cleanup, usage, and confirmed GC.
- Treat the implemented standalone OKF scope quota/GC, integrity audit,
  non-overwriting verified database backup, and derived FTS repair as bounded
  storage controls, not as completion of cross-product restore, authority
  recovery, backup rotation, or retention operations.
- [x] Expose latest/previous package lifecycle checkpoint status, bounded
  failure codes, digests, timings, and rollback evidence through
  `extension inspect --json` without secret-bearing fields.
- Add broader diagnostics for plan, download, provider readiness, cutover,
  drain, rollback, and recovery using non-secret evidence.
- [x] Define and implement the standalone OKF repair boundary: only FTS rows
  derived from validated documents may be rebuilt; receipt, scope, projection,
  binding, and lifecycle evidence remain immutable and fail closed.
- [ ] Define coordinated backup/restore and repair boundaries for every state
  family. Missing exact evidence must remain fail-closed; restore or repair
  cannot invent authority.
- Complete threat-model review, security response, upgrade policy, rollback
  policy, and support runbooks.
- Establish performance budgets for catalog refresh, resolution, install,
  startup, watcher latency, and storage growth.

## Test matrix

### Contract tests

- current schema canonical round trip and SHA-256 golden;
- unknown-field and superseded-schema rejection;
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
- route absence before retirement;
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
- test-binary subprocess exit after all 14 Grant Store durable phase,
  candidate-receipt, prior-revocation, and candidate-restoration checkpoints
  in the canonical two-candidate/two-retirement lifecycle, followed by exact
  state convergence;
- real CLI exit after the durable uninstall Registry hide and before the
  package hide receipt, followed by exact-plan restart, observed blocking at
  accepted-call drain, physical generation removal, and no second Registry
  generation; absence without the exact cutover remains fail-closed;
- no-op terminal result replay without another host call;
- completed graph replay without another atomic publish or hide;
- latest/previous checkpoint diagnostics remain bounded and omit idempotency
  keys and secret-bearing fields;
- a rolling-back operation rejects a conflicting new intent.

### Security tests

- path traversal, symlink/reparse point, archive link, duplicate path, and size
  attacks;
- plan, policy, scope kind, confirmation, provider, Grant, and generation drift;
- missing recovery evidence and tampered journal rejection;
- static UI/Skill ambient-authority denial;
- secret-safe error diagnostics.

### Product E2E

- one real signed six-surface package through CLI, TUI, Web, and manager MCP;
- install/invoke/upgrade/invoke/uninstall/restart;
- interruption at every durable checkpoint;
- Linux, macOS, and Windows; and
- clean release archive with no checkout-local dependencies.

## Delivery sequence

1. Finish managed Runtime/Gateway/Knowledge/UI composition.
2. Converge A3S Code CLI/TUI/Web and agent MCP on the same service.
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
- Guessing recovery state after exact journals or graph records were deleted.
- Building a second workflow lifecycle around `flow.json`.

## Release definition

The platform is releasable only when the same signed package graph can be
reviewed, installed, hot-used, upgraded, recovered, audited, and removed across
all advertised hosts and platforms without mixed generations, duplicated side
effects, ambient authority, or leftover owned state.
