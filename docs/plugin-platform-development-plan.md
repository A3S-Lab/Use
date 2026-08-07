# A3S Use Plugin Platform Development Plan

Status: active
Last updated: 2026-08-08

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
- separately signed package-native Tool/stdio MCP planning targets rebound to
  the digest-bound manifest after download;
- bounded SemVer resolver, package lock, prior/candidate upgrade binding;
- operation plan v4, confirmation, policy and host/provider evidence;
- manager toolset v3 with plan/apply enablement;
- immutable package generations and dependency graph records;
- dependency-forward install, one cutover, reverse uninstall and upgrade GC;
- durable Registry cutover replay and exact lifecycle journals;
- Workspace Grant composition, joint rollback, and drain-before-revoke;
- standalone executable Task, stdio MCP, Skill/UI, SQLite/FTS5 OKF, and
  explicitly configured real `a3s-flow` Native TypeScript lifecycle hosts;
- typed Runtime Service endpoint consumption and an injected Gateway lifecycle
  port that drains routes before Runtime stop, removes routes before Runtime
  removal, and retains the exact binding receipt until both complete;
- whole-scope OKF expanded-byte/projection quotas, per-surface generation
  limits, globally bounded tombstones, post-removal SQLite/WAL compaction, and
  exact-scope storage usage diagnostics;
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
- Reconstruct the exact selected provider evidence in the CLI apply path.
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

- Define per-package origin, CSP, navigation, storage, and backend bindings.
- Bind UI to exact Skill/Tool/MCP/Flow readiness and Grant evidence.
- Add generation-aware hot replacement and drain.

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

- Expand beyond compile/Core/facade/CLI coverage.
- Test Registry/package/Grant state, reparse points, file locks, antivirus
  contention, process groups, named resources, and reboot recovery.
- Run the complete signed six-surface lifecycle and failure matrix.

Acceptance: every advertised platform passes the same contract, lifecycle,
recovery, and residue assertions.

## Workstream E — Registry and supply-chain operations

Priority: P0 release gate
Status: pending

- Operate a documented TUF Registry with root rotation, expiry, rollback
  protection, mirror replacement, cached offline reads, and incident recovery.
- Publish complete catalog-v3 records and planning targets only.
- Define source replacement and exact-provenance restoration workflows.
- Produce reproducible archives, checksums, signatures, SBOMs, and provenance.
- Verify release archives in clean Linux/macOS/Windows environments.
- Add bounded download resume, disk-space/quota, and cache retention tests.

Acceptance: an operator can bootstrap trust, install, rotate/replace sources,
recover offline, audit provenance, and remove the product using published
instructions only.

## Workstream F — Operations and support

Priority: P0 release gate
Status: pending

- Define retention and garbage collection for packages, prior generations,
  TUF cache, Grants, Flow history, OKF indexes, UI storage, and journals.
- Treat the implemented standalone OKF scope quota/GC, integrity audit,
  non-overwriting verified database backup, and derived FTS repair as bounded
  storage controls, not as completion of cross-product restore, authority
  recovery, backup rotation, or retention operations.
- Add diagnostics for plan, download, prepare, cutover, drain, rollback, and
  recovery using non-secret evidence.
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
- no-op terminal result replay.

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
