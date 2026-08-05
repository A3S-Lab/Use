# A3S Use Plugin Platform Roadmap

- Status: in progress
- Planning baseline: 2026-07-30
- Product amendment: first-class OKF knowledge contribution accepted; M0K-A
  bundle contract frozen 2026-07-31 and M0K-B control-plane contracts frozen
  2026-08-01; package-level six-surface lifecycle and cognitive-package
  dependency/lock foundations accepted 2026-08-03; unified A3S Flow surface
  and exact-generation preflight binding foundation accepted 2026-08-04;
  stable-name Registry controls plus A3S Code TUI/Web Flow and hot-plug host
  integration accepted 2026-08-04; exact `flow.json` identity plus shared
  workspace-local durable execution/observation accepted 2026-08-04; bounded
  exact-generation package/route and Runtime N/N+1 storage accepted 2026-08-05
- Scope: A3S Use, the umbrella A3S CLI, A3S Code/Web/Knowledge, and plugin registries

This document is the source of truth for evolving A3S Use into a plugin
platform where a user or an authorized agent can discover, install, enable,
use, disable, and uninstall an immutable package that contributes executable
Tools, standard MCP servers, OKF knowledge, A3S Flow workflows, Skills, and
sandboxed UI.

The milestones are dependency ordered. They are not calendar commitments.
Effort ranges assume one primary engineer with review and cross-platform CI
support.

## Product Outcome

A complete user flow is:

```text
search catalog
  -> inspect signed metadata, permissions, provenance, and size
  -> resolve and review an immutable dependency lock plus install plan
  -> install and enable the selected package and required dependency closure
  -> use its CLI/HTTP Tool, MCP, cited OKF knowledge, Flow, Skill, or UI
  -> disable or uninstall it without restarting the host
```

An agent follows the same lifecycle through a standard MCP management surface.
Read-only discovery is available by default. Mutating operations require either
interactive confirmation or an explicit ACL policy that pre-authorizes the
exact registry, publisher, permission ceiling, and resource limits.

## Product Decisions

These decisions are part of the target contract:

1. The package is the unit of identity, trust, versioning, installation,
   upgrade, enablement, and removal.
2. The stable package identity remains `<publisher>/<name>`. The umbrella
   component identity remains `use/<publisher>/<name>`. A unique route is a
   presentation and dispatch alias, not an ownership identity.
3. "Plugin" is the user-facing product term. Existing extension manifests and
   commands remain compatible until a versioned migration is complete.
4. A plugin may contribute multiple named Tools, MCP servers, conformant OKF
   bundles, A3S Flow workflows, Skills, and sandboxed UIs.
5. A Plugin Tool is a real workload on which a Skill or UI may depend. A CLI
   Tool maps to a one-shot Runtime Task; an HTTP Tool maps to a Runtime Service.
   It is distinct from an MCP `tools/list` item and retains its native argv or
   HTTP API contract.
6. A3S Use binds and supervises Tools but does not translate them into a
   private tool protocol, universal action envelope, or generic
   `execute(plugin, action, payload)` RPC.
7. Catalog availability and active capability projection are separate.
   Uninstalled packages may appear in search results but never in the active
   Tool binding, MCP, OKF, Flow, Skill, or UI registry.
8. Registry metadata is fetched separately from package payloads. Browsing,
   searching, Code startup, and Skill matching never install a package.
9. Every install, upgrade, or uninstall uses plan/apply. Apply repeats
   resolution and fails closed when the plan digest changes.
10. Registry trust roots, unsigned local packages, secret grants, and user-data
   deletion remain user-owned authority.
11. Normal uninstall removes only receipt-owned package files. User data is
    retained unless a separate destructive purge is explicitly authorized.
12. OKF is a non-executable Open Knowledge Format contribution, not a Skill
    alias, Runtime workload, MCP server, or personal knowledge vault. The frozen
    content contract targets current OKF v0.2 while preserving an explicit v0.1
    compatibility path. A3S Use owns package-generation integrity; A3S
    Knowledge owns conformant atomic promotion, indexing, cited retrieval, and
    last-good-generation preservation.
13. Registry sources are named, replaceable host configuration. Installed
    receipts retain the exact source URL, trust root, channel, target, and
    digest reviewed for their generation; source replacement never rewrites
    package provenance silently.
14. A schema-v3 cognitive package may depend on other cognitive packages by
    canonical package ID and SemVer range. The host selects Registries, freezes
    exact versions and Registry/TUF provenance in a package lock, installs
    dependencies before dependents, and removes them in reverse without
    deleting shared retained dependencies.
15. A3S Flow is the single workflow engine. A package Flow uses a typed runtime
    adapter and explicit Tool/MCP/OKF dependencies. Code's `flow.json` is a
    design/deployment document mapped to the same identity, not a second
    package lifecycle or execution mechanism.

## Current Baseline

The following foundations are implemented:

- typed Browser, OCR, Box, component, and extension contracts;
- native CLI, standard MCP, Skill, and content-bound Activity Bar surfaces;
- schema v3 named Tool Task/Service, MCP, OKF, Flow, Skill, and UI surface contracts
  while retaining schema v1/v2 parsing compatibility;
- canonical MCP Service, Skill Agent-input, and Tool Task/Service release
  descriptors with stable JSON fixtures and package-level manifest binding
  validation;
- a real headless MCP conformance fixture whose Linux gate binds an ephemeral
  non-root OCI image by Registry manifest digest and proves health, standard
  initialization/request, bounded SIGTERM shutdown, cleanup, and restart;
- immutable package generations and receipt-owned installation roots;
- bounded, symlink-safe package and Runtime generation stores that preserve N
  while N+1 is prepared, resolve the exact snapshot-selected receipt, retain
  generation-specific route leases, and remove only receipt-owned N after
  cutover and drain;
- install, upgrade, enable, disable, uninstall, watch, and route draining;
- reviewed local packages, release bundles, and TUF-verified remote packages;
- exact registry target selection with version, channel, target, length, and
  SHA-256 provenance;
- bounded search and inspection over complete signed catalog records, with
  deterministic pagination and filesystem-only offline re-verification;
- umbrella CLI dry-run/apply plans protected by a plan digest;
- a Web Marketplace with catalog and installed views;
- host-owned named Registry configuration with explicit replacement,
  enable/disable, removal, refresh, and receipt-bound source identity;
- schema-v3 ACL package dependencies and required package README validation;
- deterministic bounded SemVer resolution with backtracking, conflict/cycle/
  Registry-ambiguity rejection, and canonical
  `a3s.use.plugin-package-lock.v1` host and provenance binding;
- complete dependency-forward Registry revalidation/download, exact retained
  dependency checks, reverse-dependent uninstall protection, and one atomic
  capability snapshot cutover for changed package graphs;
- standalone `a3s-use install` / `uninstall` graph commands plus compatible
  remote `component install` / `uninstall` dispatch for signed schema-v3
  records, with optional reviewed package-lock digests;
- durable installed-root locks and admitted pending graph operations carrying
  exact manifests and generations, including published-install journal repair
  and pending-only reverse-uninstall recovery;
- standalone cognitive-package authorization that injects trusted actor/policy
  authority before final plan binding, requires exact confirmation for ambient
  authority, persists replay-stable Grant snapshots/change sets/resolutions and
  signed ceilings in pending-v2 operations, and selects Grant-aware install,
  upgrade, and uninstall paths whenever the immutable graph needs Grants;
- a public lifecycle factory that lets Code/Web hosts inject Runtime, Gateway,
  A3S Flow, Knowledge, Skill, and UI ownership while the standalone
  composition fails closed for unavailable Runtime Service, HTTP MCP, Flow,
  or OKF hosts;
- sandboxed plugin UI with verified HTML, CSS, and JavaScript assets;
- generation/revision capability snapshots consumed by A3S Code;
- live MCP and Skill projection into a dedicated A3S Use worker;
- M0K-A OKF bundle identity and bounded v0.2/v0.1 conformance;
- M0K-B named OKF manifests, recursive package validation, catalog-v3 evidence,
  Skill dependency closure, plan-v2 impact, projection receipts, scope-bound
  Knowledge observations, capability projections, last-good selection, and
  reconciliation gates;
- M0K-C-A injected Knowledge stage/promote/observe/remove contracts, byte-exact
  stage revalidation, adapter evidence checks, and a durable bounded
  exact-generation binding store with fail-closed last-good reconstruction;
- one canonical manifest surface graph shared by reconciliation and lifecycle;
- a package-level install/upgrade/enable/disable/uninstall intent with
  deterministic dependency-forward and reverse-removal checkpoints;
- a bounded atomic cross-process lifecycle journal with idempotent restart,
  optional-surface failure evidence, and tamper rejection; and
- typed Runtime Tool/MCP and Flow lifecycle ports, immutable Skill/UI evidence,
  and OKF Knowledge lifecycle adapters, proven by one content-addressed package
  containing all six kinds;
- first-class A3S Flow manifest, source integrity, Tool/MCP/OKF dependency
  edges, lifecycle ordering, reconciler ownership, and typed capability
  projection;
- a concrete `A3sFlowLifecycleHost` backed by the real `a3s-flow`
  `NativeTsRuntime::preflight`, plus symlink-safe exact-generation bindings,
  source/artifact reinspection, and production capability observation;
- additive host-capabilities v2 and manager-toolset v3 contracts that advertise
  Flow without rewriting frozen older schemas;
- standalone-host rejection before mutation when a required Flow lacks an
  injected compiler/runtime adapter;
- A3S Code lifecycle composition for executable Tool Tasks, stdio MCP,
  immutable Skill/UI, and real `a3s-flow` Native TypeScript preflight;
- one exact-generation watcher feeding Code TUI/Web plus the typed
  `GET /api/v1/plugins/flows` catalog; and
- exact `flow.json` identity resolution plus one cross-process-locked local
  `a3s-flow` event store shared by Code CLI, TUI, and Web; and
- detached-Web `install -> run -> upgrade -> run -> uninstall -> restart`
  coverage for Activity, Skill, Flow replacement, retained run history, and
  path-free observation.

The OKF control plane is implemented without creating another package manager,
Runtime route, or reconciler. It intentionally publishes no OKF capability
when promoted Knowledge evidence is absent. The injected port, Use-owned
evidence store, and package-lifecycle adapter are implemented; the production
A3S Knowledge backend and scope-aware host/session projection remain the next
implementation slice.

The main gaps are:

- the agent worker can discover plugins and create reviewed lifecycle plans,
  but is explicitly forbidden from applying them or toggling packages;
- package-level permission declarations are not precise enough to authorize
  native executable plugins without human review;
- schema v3 named surfaces, Tool release contracts, catalog-v3 executable
  planning targets, planning bundles, and typed Runtime lifecycle adapters are
  implemented, but the host Runtime Broker is not injected into the shared
  Plugin Manager yet;
- persisted Tool Task/Service bindings and Runtime observations feed the shared
  reconciler, and package-level package/capability plus
  Runtime/MCP/Flow/Skill/UI/OKF adapters exist, but the Plugin Manager still lacks
  complete host composition and scope-aware apply/observation wiring;
- the shared Plugin Manager now owns Marketplace, reviewed lifecycle
  orchestration, durable apply replay, and the first-class user CLI adapter;
  its bounded read-only management MCP is connected, while production host
  composition across package/capability and surface adapters remains to be
  connected;
- the dependency resolver, lock, remote closure downloader, graph lifecycle
  coordinator, and signed remote standalone CLI path are implemented in A3S
  Use; Code TUI/Web now inject their supported host set through the public
  lifecycle factory, while management MCP and managed-host mutation still need
  the same composition;
- package and Runtime storage now support exact N/N+1 coexistence, candidate
  commit replay, pre-cutover rollback, and exact prior-generation retirement;
  the dependency-closure coordinator binds prior/candidate locks in plan v3,
  plans Add/Replace/Remove/Retain, downloads only changed nodes, prepares N+1
  forward, publishes additions/replacements/removals once, preserves exact
  shared nodes, retires unreferenced N generations in reverse order, and
  replays both paths without generation inflation;
- A3S Use supplies the production `a3s-flow` preflight host and retained
  binding evidence, and A3S Code injects it and exposes the typed live Flow
  catalog; Code now resolves strict `flow.json` identities and provides local
  durable execution/observation through the same engine, while distributed
  workers, automatic suspended-work resumption, and production retention
  remain pending;
- the default Use release still carries an optional Science reference package
  payload instead of relying on on-demand registry delivery;
- the versioned OKF manager/search/selection contract, injected Knowledge port,
  and exact-generation binding store are frozen, but the umbrella host ACL
  policy and production A3S Knowledge atomic-index backend remain to be wired;
- official registry signing/key operations (distinct from replaceable source
  configuration) and Windows Browser parity are not yet at the final
  production gate.

## Development Plan

The [Plugin Platform Architecture](docs/plugin-platform-architecture.md)
defines the domain model, control/data planes, Tool workload semantics, surface
reconciliation, and Runtime bindings. Its
[Lifecycle and Security](docs/plugin-platform-lifecycle-and-security.md)
companion defines consistency, recovery, authorization, and storage. The
[Plugin Platform Development Plan](docs/plugin-platform-development-plan.md)
defines execution workstreams, validation, risks, and non-goals. Milestones
below are the delivery sequence for those documents.

### Delivery sequencing

The core critical path is `M0 -> M1 -> M2 -> M5 -> M6 -> M7`. The required OKF
lane is `M0K -> M2/M5 -> M6 -> M7` and must also finish before the cognitive
plugin platform is production-ready. Science and OKF package
splitting in M3 can proceed after the catalog and manager contracts stabilize.
The read-only management MCP in M4 can proceed alongside user UX after M2.
Runtime provider conformance work for M5 should begin after the M0 descriptor
fixtures, in parallel with M1 and M2 implementation.

The indicative total is 20–26 primary-engineer weeks after adding OKF. This is
an effort range, not a calendar promise; Runtime provider, Code/Web/Knowledge,
Science, release-security, and cross-platform CI work can run in parallel when
separately staffed.

## Milestones

### M0 — Contract freeze and fixtures (complete 2026-07-30)

Estimated effort: 2 weeks

Implementation status (2026-07-30):

- completed: architecture, lifecycle, security, and delivery-plan baselines;
- completed: schema v3 named Tool, MCP, Skill, and UI surfaces with v1/v2
  parsing compatibility and a stable ACL fixture digest;
- completed: `a3s.use.mcp-release.v1` and `a3s.use.skill-release.v1`, exact
  cross-fixture dependency binding, canonical digest goldens, and a real
  digest-pinned Linux MCP lifecycle/restart conformance gate;
- completed: `a3s.use.tool-release.v1`, closed Task/CLI and Service/HTTP
  workload contracts, and canonical JSON digest fixtures;
- completed: catalog, verified TUF provenance, permission-ceiling,
  digest-bound plan/apply, and bounded manager MCP toolset contracts;
- completed: canonical catalog, permission, install-plan, and manager-toolset
  JSON fixtures with stable SHA-256 digests;
- completed: a complete v3 package fixture containing CLI/HTTP Tool,
  stdio/HTTP MCP, Skill, and UI surfaces, with deterministic expanded-package
  and archive digests;
- completed: deterministic signed TUF root, targets, snapshot, and timestamp
  metadata embedding the complete canonical catalog record.

Deliverables:

- record the package, route, surface, catalog, active-registry, and authority
  boundaries as versioned contracts;
- adopt "plugin" as the product term without removing extension compatibility;
- define named Skill, Tool Task, Tool Service, MCP, and UI surfaces plus their
  acyclic dependency graph;
- define the canonical Tool release descriptor and its Runtime Task/Service
  mapping;
- define the signed catalog record, permission ceiling, operation plan, and
  manager MCP schemas;
- add canonical ACL, JSON, and package fixtures with stable digests;
- document compatibility and schema evolution rules.

Exit criteria:

- existing extension packages continue to parse unchanged;
- new fixtures reject unknown privilege-bearing fields and noncanonical data;
- cross-SDK digest fixtures are deterministic;
- no lifecycle mutation is implemented before its plan schema is fixed.

### M0K — OKF contribution contract and fixtures (M0K-C-A lifecycle foundation complete 2026-08-03; M0K-C-B pending)

Estimated effort: 2–3 weeks

This is an additive product amendment. It does not reopen or mislabel the
completed Tool/MCP/Skill/UI M0 fixture set.

Implementation status (2026-08-03):

- completed M0K-A in `a3s-use-core`: the canonical
  `a3s.use.okf-bundle.v1` JSON descriptor binds format version, bundle root,
  exact content digest, concept/file counts, expanded bytes, and declared
  limits;
- completed M0K-A in `a3s-use-core`: one bounded inspector handles v0.2 and
  v0.1 fallback content, UTF-8 Markdown, YAML frontmatter, reserved files,
  canonical concept IDs, standard Markdown links, inert path-valued metadata,
  deterministic summaries, and non-fatal safe dangling-link diagnostics;
- completed M0K-A fixtures: canonical JSON and bundle digest goldens plus
  conformant, compatibility, drift, limit, malformed-content, and path-escape
  tests;
- completed M0K-B manifest/package integration: schema v3 accepts bounded named
  OKF surfaces, rejects executor-like authority, recursively validates every
  bundle byte, and lets a Skill require an OKF surface;
- completed M0K-B catalog/plan integration: catalog v3 binds exact OKF bundle
  evidence, OKF-only records omit executable planning targets, plan/draft v2
  derives exact `okfChanges`, and Runtime provider evidence remains Tool/MCP
  only;
- completed M0K-B host evidence: versioned projection receipts, Knowledge
  observations, capability projections, exact promoted-generation checks,
  staged/failed candidate rejection, and last-good selection;
- completed M0K-B reconciliation: OKF is owned by `KnowledgeHost`, remains
  unpublished without promoted evidence, and gates dependent Skills through
  the existing required closure;
- completed M0K-B canonical fixtures: OKF ACL and complete package digests,
  catalog-v3, plan-v2, manager-toolset-v2, projection receipt, Knowledge
  observation, and capability-projection JSON goldens. Existing v1/v2 and the
  original schema-v3 fixture remain byte-compatible;
- completed M0K-C-A adapter boundary: a public `Send + Sync`
  stage/promote/observe/remove port, an evidence-checking client, and a stage
  request that revalidates the exact borrowed OKF file bytes without a
  path-based time-of-check/time-of-use gap;
- completed M0K-C-A persistence: receipt plus observation records are stored
  atomically under hashed scope and validated package/surface paths, protected
  by a cross-process lock, bounded to 32 retained generations, and selected
  only through exact retained promoted evidence. Stale/conflicting writes,
  tampered JSON, symlinks, ownership drift, and removed-generation fallback
  fail closed; and
- completed M0K-C-A lifecycle foundation: the typed OKF host performs
  stage-store-promote-store, reuses exact promoted evidence after restart,
  hides capability without deleting Knowledge data on disable, and removes
  only receipt-owned projection evidence on uninstall.

M0K-C-B remains pending: implement the production A3S Knowledge backend behind
the injected port, bind it and the store to the parent operation journal and
scope-aware capability/session projection, and prove cited retrieval,
last-good rollback, retained-generation cleanup, and receipt-owned removal end
to end.

Deliverables:

- freeze a named OKF surface with a declared format version, current v0.2
  semantics, explicit v0.1 fallback behavior, bundle root, canonical content
  digest, concept/file count, expanded bytes, and limits;
- define bounded UTF-8 Markdown, properly delimited YAML frontmatter, required
  non-empty scalar `type` for non-reserved concepts, canonical concept IDs,
  reserved `index.md`/`log.md`, and standard-link handling;
- preserve unknown concept types and extension keys, treat safe dangling links
  as diagnostics rather than nonconformance, and reject only unsafe path or
  resource resolution at the package boundary;
- extend catalog, operation plan, dependency closure, permission policy,
  receipt, projection, capability snapshot, and host observation contracts;
- define the idempotent A3S Knowledge stage/promote/index boundary with exact
  package generation, index schema/build evidence, and last-good selection;
- add canonical ACL/JSON/package fixtures and stable SHA-256 digests; and
- freeze disable, upgrade, uninstall, retained-data, personal-vault isolation,
  and crash-replay semantics.

Exit criteria:

- schema v1/v2 and the existing schema-v3 fixture remain byte-compatible;
- the parser accepts `okf` only in schema v3 and validates the exact declared
  bundle during package admission;
- conformant v0.1/v0.2 and malicious OKF fixtures have deterministic cross-SDK
  results;
- an OKF plan binds the exact normalized bundle that Knowledge would promote;
- concept/frontmatter/link content cannot add authority;
- v0.2 Attested Computation metadata cannot implicitly select or invoke a Tool,
  executor, attester, Runtime provider, or secret; and
- the ownership contract forbids removing personal notes or another package's
  index; production cleanup and recovery proof remains part of M0K-C.

### M0F — Unified A3S Flow contribution (Code local execution/observation complete 2026-08-04; distributed runtime pending)

Implementation status:

- completed: schema-v3 named Flow with fixed `a3s-flow` engine, typed
  `native-ts` runtime, bounded source/export, optionality, and Tool/MCP/OKF
  dependencies;
- completed: catalog dependency closure, exact manifest/catalog inventory
  binding, package source validation/digest, lifecycle ordering, reverse
  stop/removal, reconciler ownership, and typed capability projection that
  withholds source-only readiness until host preflight evidence exists;
- completed: additive host-capabilities v2/protocol level 2 and manager-toolset
  v3, while v1/v2 canonical contracts remain frozen;
- completed: concrete `a3s-flow` Native TypeScript preflight, symlink-safe
  exact-generation binding storage, source/artifact substitution rejection,
  blue/green retention, and production capability observation;
- completed: one six-surface content-addressed fixture and regression coverage
  for source corruption, missing dependencies, unavailable hosts, publication,
  and reverse teardown;
- completed: A3S Code lifecycle-factory injection, exact-generation TUI/Web
  watcher projection, typed live Flow catalog, and install/upgrade/uninstall
  hot-plug coverage;
- completed: strict path-free `flow.json` identity resolution for non-resident
  CLI, resident TUI, and Web; source revalidation and immutable staging before
  compiler/event mutation; workspace-local durable `a3s-flow` runs, idempotent
  run IDs, status/event APIs, upgrade/uninstall history retention, and Web
  process-restart recovery; and
- pending: distributed worker placement, automatic scheduling/resumption of
  suspended waits/retries/hooks, production retention/garbage collection, and
  a visible Web Flow run/history control surface over the completed APIs.

Exit criteria:

- installing one package resolves Flow with the same SemVer/Registry lock as
  every other contribution;
- required Tool/MCP/OKF capabilities are ready before Flow, while dependent
  Skill/UI appear only after Flow is ready;
- no Flow is published when source integrity, preflight, or host evidence is
  missing;
- install, disable, and uninstall need no host restart and retain explicit run
  history according to host policy; and
- local Native TypeScript, Code `flow.json`, and remote OS deployment resolve
  to one A3S Flow identity rather than independent workflow mechanisms.

### M1 – Signed searchable catalog (complete 2026-07-30)

Estimated effort: 1–2 weeks

Implementation status (2026-07-30):

- completed in Use: dual decoding for legacy target metadata and complete
  `a3s.use.plugin-catalog.v1` records;
- completed in Use: bounded local text search, exact filters, deterministic
  ordering, snapshot-bound pagination, and full provenance inspection;
- completed in Use: filesystem-only offline re-verification of the last exact
  online-verified TUF role bytes with cache age reporting;
- completed in Use: fail-closed compatibility, archive-evidence, cache
  tampering, expiration, cursor, and response-size coverage;
- completed in Science: registry-builder emission of complete catalog records
  for all 472 independently selectable package targets;
- completed end to end: discover all 472 records from a remote first page and
  filesystem-only cached pagination without archive downloads, then download
  and install only the selected `a3s/native-autodock` target;
- completed in Science CI validation: schema, surface honesty, permission
  ceiling, compatibility, archive binding, size bounds, provenance, and
  availability are checked for every published target.

Deliverables:

- extend TUF target metadata or a digest-bound signed index with search fields,
  surface IDs, Tool workload kinds, permission summary, compatibility, and
  size;
- implement bounded refresh, cached offline reads, text search, filters, stable
  sorting, and pagination;
- add inspect output that identifies exact registry provenance;
- keep package payload downloads out of search and inspect;
- update the Science registry builder to emit the new metadata.

Exit criteria:

- every Science catalog entry is discoverable without downloading an archive;
- tampered, expired, rolled-back, or incompatible metadata fails closed;
- offline search uses only the last verified snapshot and reports its age;
- catalog search has deterministic fixtures and output-size bounds.

### M2 — Shared Plugin Manager application service (in progress 2026-08-05)

Estimated effort: 2–3 weeks

Implementation status (2026-08-05):

- completed in `a3s-use-core`: one object-safe `PluginHostManager` port and
  canonical v1 managed-host capability, scope-fence, plan, digest-only apply,
  enablement, and observation contracts. They reuse the existing catalog,
  plan, confirmation, and Surface Reconciler states, reject mixed schemas and
  stale fences, expose bounded unavailable evidence, and contain no path,
  provider, endpoint, Secret value, or universal action payload;
- completed in the umbrella CLI: a reusable typed `plugin_manager` application
  service with one operation lock and centralized plan, apply, enable, and
  disable process boundaries;
- completed in the umbrella CLI: a bounded Marketplace read model joining
  release bundles, complete signed catalog records, legacy TUF records, and an
  immutable installed/enabled snapshot without package downloads;
- completed: exact catalog snapshot checks across cached pagination and legacy
  fallback, per-source verification errors, registry and item limits, stable
  latest-release selection, and full catalog provenance/permission/surface
  projection;
- completed in Code Web: the Plugins feature is a thin HTTP adapter over the
  shared manager and preserves the existing timeout, JSON-size, HTTP error, and
  reviewed-plan behavior;
- completed in the umbrella CLI: first-class `a3s plugin search`, `inspect`,
  and `list` commands are thin adapters over the shared manager, with canonical
  package identities, bounded filters, cached-only offline reads, typed errors,
  and stable human/JSON output;
- completed in the umbrella CLI: installed state comes from the bounded A3S Use
  capability snapshot and distinguishes desired enablement, current
  callability, readiness, and an unavailable observation;
- completed in the umbrella CLI: `install`, `upgrade`, explicit `apply`,
  `enable`, `disable`, and `uninstall` commands call only the shared manager;
  install, upgrade, and uninstall persist and review an immutable plan before
  applying its `operationId + canonicalPlanDigest`;
- completed in the umbrella CLI: interactive lifecycle commands render the
  exact terminal-safe plan and use a bounded asynchronous confirmation;
  non-interactive mutation requires `--yes`, while `--dry-run` persists no
  apply intent;
- completed in the shared manager: an immutable host policy selects cached-only
  catalog reads and propagates `--offline` into every delegated plan, apply, or
  toggle child process;
- completed in the umbrella CLI library: immutable one-hour reviewed plans
  receive cryptographically random operation IDs and are stored with
  append-only apply intents and seven-day replayable successful results;
- completed: apply accepts the frozen `operationId + planDigest` identity,
  retains a compatibility lookup for the current Web request shape, rejects
  expired or capability-drifted plans before first mutation, and resumes an
  existing intent through the umbrella component journal;
- completed: a cross-process manager mutation lock prevents two adapters from
  racing result publication. The component journal remains the cross-component
  boundary, while the A3S Use package journal owns canonical per-surface
  checkpoint ordering;
- completed: plans and results carry explicit A3S Use capability
  generation/revision evidence, including a bounded unavailable state that
  cannot turn a successful mutation into a false failure;
- completed in A3S Use: a deterministic, level-based schema v3 Surface
  Reconciler calculates required dependency closure, per-surface
  desired/observed state, aggregate ready/degraded/broken state, and atomic
  publication eligibility without starting new Runtime workloads;
- completed in A3S Use: capability snapshots expose the reconciliation
  evidence and project named Skills only after every required Tool, MCP, and
  OKF dependency is prepared or healthy; missing Runtime, MCP, UI, and
  Knowledge observations remain explicit `pending` evidence;
- completed in A3S Use: one canonical package-owned Tool/MCP/OKF/Flow/Skill/UI graph
  now drives both reconciliation and lifecycle, preventing independently
  installed surface state or divergent dependency order;
- completed in A3S Use: a versioned package-level intent, deterministic
  checkpoint schedule, durable atomic journal, and typed host coordinator
  prepare dependencies forward, publish once, and hide/drain/remove in reverse
  order with restart-safe idempotency evidence;
- completed in A3S Use: P0 production package/capability hosts commit one
  deterministic immutable generation as receipt-schema-v3
  `installed-disabled`, atomically publish or hide its complete route binding,
  drain shared route leases, and remove only that exact generation; legacy
  schema-v1/v2 behavior remains compatible and cannot bypass lifecycle-managed
  ownership;
- completed in A3S Use: concrete Runtime Tool/MCP, immutable Skill/UI, and OKF
  Knowledge adapters validate exact package bytes and retain receipt-owned
  cleanup boundaries; an all-six-surface package fixture freezes the contract;
- completed in A3S Use: schema-v3 package dependency declarations, required
  README validation, bounded deterministic SemVer resolution, exact canonical
  lock/plan/host binding, and dependency-forward Registry closure download;
- completed in A3S Use: package graph install skips exact `Retain` generations,
  prepares changed nodes forward, verifies retained nodes against the current
  published snapshot, publishes the changed closure once, removes in reverse,
  rejects installed dependents, and recovers partial receipt writes without
  exposing a partial graph;
- completed in A3S Use: top-level `install`/`uninstall` and compatible remote
  `component` commands dispatch signed schema-v3 records through the graph
  manager; package-lock mismatches fail before archive download;
- completed in A3S Use: root dependency locks, admitted pending plans,
  per-package manifest/generation evidence, and symlink-safe stores survive
  restart; a published install completes unfinished journals, and uninstall
  can resume after both the root receipt and installed-root graph are gone;
- completed in A3S Use: operation plan v3 binds the exact prior/candidate lock
  union, host capabilities v3 advertises that schema without changing v1/v2,
  dependency removal shares the candidate cutover, and retained receipts let
  reverse-order GC resume after every cutover/drain/removal crash boundary;
- completed in A3S Use: grant-aware graph install, upgrade, and uninstall paths
  bind one reviewed plan to exact candidate and retirement Grants, persist
  candidate authorization before package preparation, and require the Registry
  host to return exact snapshot digest plus generation cutover evidence;
- completed in A3S Use: pre-cutover upgrade failure restores both package and
  Grant candidates, successful upgrade and uninstall drain accepted prior calls
  before revoking old Grants, and generation drift or cutover replay fails
  closed without inventing new evidence;
- completed in A3S Use: the standalone `CognitivePackageManager` injects a
  trusted authorization provider, binds actor and policy before final planning,
  confirms the exact immutable plan and canonical Grant changes, and persists
  confirmation, snapshot, change-set, resolved-Grant, and signed-ceiling
  evidence in pending-v2 records;
- completed in A3S Use: permission-bearing standalone install, upgrade, and
  uninstall operations cannot select the grant-free compatibility path;
  interrupted replay revalidates the original provider authority without
  reauthorization, and altered confirmation, Grant, ceiling, or legacy-v1
  evidence fails closed before package mutation;
- completed in A3S Use: `CognitivePackageLifecycleFactory` is the explicit
  embedding seam for Code/Web Runtime, Gateway, static projection, and A3S
  Knowledge adapters; the standalone factory does not invent fallback hosts;
- covered: typed complete-catalog mapping, lifecycle argument and digest
  validation, Use-owned JSON output, operation ID uniqueness, expiry,
  append-only replay, corruption rejection, cross-process locking, Web adapter
  compilation, deterministic surface graph/readiness fixtures, dependency-gated
  Skill projection, read-only and mutation CLI parser/authority/output
  contracts, offline child-policy propagation, a signed-registry CLI
  plan/apply/replay fixture, and a controlled Web Marketplace/invalid-plan
  smoke test;
- completed in A3S Code: the umbrella manager uses the public lifecycle factory
  for package/capability, executable Tool Task, stdio MCP, A3S Flow, and
  Skill/UI hosts; TUI watcher and detached-Web Marketplace lifecycle gates
  cover live generation replacement;
- pending: production Runtime Service, Gateway/HTTP MCP, and Knowledge host
  injection; umbrella and managed-host forwarding of the standalone Use plan
  authority, confirmation, and canonical Grant evidence; real signed
  cross-platform lifecycle E2E; and the published
  managed-host port required before a Cloud adapter can enable mutation. Hosts
  must not add a parallel implementation.

Deliverables:

- extract catalog, installed-state join, plan, apply, enable, and disable
  orchestration from Web-specific code into one shared application service;
- keep the umbrella component planner and A3S Use delegated lifecycle as the
  only mutation path;
- adapt CLI and Web to the shared service;
- preserve the existing operation lock, timeouts, JSON limits, and plan digest;
- make operation results idempotent and include capability generation/revision;
- add the level-based Surface Reconciler and per-surface desired/observed
  state, without enabling new Runtime workloads yet.

Exit criteria:

- CLI and Web produce equivalent plans and operation records;
- the existing Marketplace lifecycle E2E passes through the shared service;
- a plan changed between review and apply is rejected;
- simultaneous operations cannot publish conflicting package generations.
- remote managed scopes use the same manager, plan store, operation journal,
  package generations, grants, bindings, and capability publication as local
  adapters, with exactly one active mutation fence.

### M3 — User plugin UX and on-demand Science/OKF delivery

Estimated effort: 2 weeks

Deliverables:

- add the `a3s plugin` user vocabulary as an adapter over the shared manager;
- expose search, inspect, installed-only, and source-verification views in Web;
- display download size, installed size, surfaces, permissions, source, and
  digest before confirmation;
- stop embedding the `a3s/science` reference payload in every A3S Use release;
- publish independently useful Science capability groups through its signed
  registry, with an optional metadata-only collection package;
- publish at least one conformant, independently selectable OKF knowledge
  package and display its format version, concept count, expanded bytes,
  digest, provenance, and Knowledge-host compatibility before confirmation;
- retain explicit local-package and optional offline-pack workflows.

Exit criteria:

- a default A3S Use archive contains no Science executable, Skill, UI, or OKF
  payload;
- opening Code or Marketplace downloads no plugin archive;
- installing one Science entry downloads and activates only its selected TUF
  target and exact content-addressed dependency closure;
- installing one OKF entry promotes only its selected normalized bundle and
  produces cited search results without installing or executing a compiler;
- uninstall removes the package generation while retaining user data.

### M4 — Agent read-only plugin management (complete 2026-07-30)

Estimated effort: 1 week

Implementation status (2026-07-30):

- completed in the umbrella CLI: a host-owned standard MCP stdio adapter
  reuses the frozen M0 schemas and delegates every operation to the shared
  Plugin Manager;
- completed: the published inventory is exactly search, inspect, installed
  list, status, and install/upgrade/uninstall plan creation; apply, enable, and
  disable are absent at the protocol boundary and explicitly denied by the Use
  worker policy;
- completed: inputs reject unknown source fields, arbitrary URLs and paths,
  unsupported workspace scope, noncanonical package/version identities, and
  selective surfaces until their backend contract is implemented;
- completed: search and inspection include signed source provenance,
  compatibility, surface, archive digest, and permission-ceiling evidence;
  errors are typed and terminal-safe, pages use snapshot-bound cursors, and
  encoded results are capped at 4 MiB;
- completed in Code TUI and Web: the management server is hot-attached as
  `use_plugin_manager` to restored and new dedicated Use workers, preserves
  offline/no-auto-install policy in its child process, and requires no session
  rebuild;
- covered: frozen schema/annotation equality, bounded parsing and cursors,
  read-only worker permissions, hidden transport CLI, a cross-platform
  standard MCP process contract, and a Unix signed-registry search,
  inspection, exact-plan, no-download, and forbidden-apply E2E.

Deliverables:

- expose the Plugin Manager MCP server to the dedicated Use worker;
- implement search, inspect, installed list, status, and lifecycle plan tools;
- add accurate read-only/open-world annotations and bounded results;
- include verified provenance and permission summaries in agent-visible output;
- keep all apply and toggle operations unavailable in this milestone.

Exit criteria:

- an agent can find a verified package and produce an exact install plan;
- an agent cannot apply installation or uninstall plans, enable, disable, add
  registries, or install from an arbitrary URL;
- catalog content cannot alter the worker policy or management MCP tool
  inventory;
- read-only operations work without a Code session rebuild.

### M5 — Permission policy and runtime enforcement

Estimated effort: 3–4 weeks

Implementation status (in progress 2026-07-30):

- completed M5A: pin the typed `a3s-runtime` 0.2.0 contract at the monorepo
  compatibility revision and expose it through the plugin Runtime adapter;
- completed M5A: deterministically map release-backed CLI Tools to Runtime Task
  specs and HTTP Tools plus Streamable HTTP MCP to Runtime Service specs while
  preserving native argv, HTTP paths, ports, health, and protocol metadata;
- completed M5A: bind package, surface, scope, grant, descriptor, artifact, and
  non-secret Runtime spec evidence into a semantics-profile digest;
- completed M5A: re-read exact provider ID, build, normalized capability
  digest, enforcement profile, and required lifecycle features before prepare
  or apply, with no provider fallback;
- completed M5A: require immutable artifact digest/media matches, reject Task
  exit semantics that Runtime 0.2 cannot represent, and publish a Service
  activation only after its observation is running and healthy;
- completed M5A: separate Runtime convergence from the scoped Gateway binding
  and allow only an opaque non-secret `gateway:` endpoint reference in a
  Service binding receipt;
- completed M5B: make Task semantics an install-time launcher-template digest
  so invocation IDs and native argv change only the per-call Runtime spec,
  while the reviewed provider evidence remains stable;
- completed M5B: require matching standard MCP initialize evidence after
  Runtime health convergence before a Streamable HTTP MCP binding receipt can
  be created;
- completed M5B: add a bounded, atomic, cross-process-locked
  `state/bindings/runtime` store with hashed scope paths, monotonic generation
  and observation replacement, exact-ownership removal, symlink checks, and
  fail-closed receipt validation;
- completed M5C: invoke a prepared CLI Tool as a one-shot Runtime Task with
  exact native argv, revalidated binding/provider evidence, terminal success
  checks, and separately bounded stdout/stderr collection through Runtime
  logs;
- completed M5C: cap the current in-memory output adapter at 16 MiB per stream
  and reject a larger release capture contract before starting the Task;
- completed M5D: remove each terminal Task unit after output capture, attempt
  bounded stop/remove cleanup for ambiguous apply failures and invalid or
  non-terminal observations, and retain cleanup error evidence without
  replacing the primary invocation failure;
- completed M5D: live-inspect persisted bindings against the exact provider,
  build, capability digest, unit/generation/spec identity, health, and Runtime
  start identity; a restarted Service makes its old Gateway/MCP binding stale;
- completed M5D: drain and remove an exact Service unit with typed Runtime
  action requests while allowing cleanup on the same explicit provider after a
  provider build upgrade;
- completed M5D: version Task and Service binding receipts as v2 after adding
  enforcement and Runtime start identity; pre-v2 development receipts fail
  closed and must be prepared again instead of being interpreted under changed
  semantics;
- completed M5E: add an explicit-scope `RuntimeSurfaceObserver` that reads the
  exact package generation's receipts, resolves only receipt-selected
  providers through `RuntimeClientRegistry`, and observes release-backed Tool
  Tasks, Tool Services, and Streamable HTTP MCP Services without fallback;
- completed M5E: merge validated Runtime surface snapshots with disjoint stdio
  MCP, Skill, and UI host observations before named-surface reconciliation;
  unbound surfaces remain pending, stale bindings fail readiness, and adapter
  collisions are rejected;
- completed M5E: keep package-executable Tool Tasks and stdio MCP outside the
  Runtime observer so their supervised compatibility hosts remain the single
  observation owners;
- completed M5F: define the canonical package/scope-bound workspace grant
  contract with policy, actor, explicit confirmation, permission-ceiling,
  resolved-permission, lifetime, and digest evidence;
- completed M5F: implement independently testable permission-subset checks for
  filesystem scope/path/access, exact network hosts and ports, resources,
  native/child execution, secrets, private Services, and UI methods/paths;
- completed M5F: require explicit user confirmation for every secret-bearing
  grant and reject secret authority in agent grants;
- completed M5G: persist workspace grants as bounded, atomic,
  cross-process-locked, symlink-checked records outside package receipts,
  revalidating package generation, signed ceiling, and lifetime at active
  resolution;
- completed M5G: retain revisioned revocation tombstones that bind exact prior
  grant ownership, reject stale/conflicting transitions, and converge
  concurrent writes on the highest accepted revision;
- completed M5G: key authorization by scope, package, and immutable package
  digest so N and candidate N+1 grants can coexist during blue/green upgrade
  without prematurely deauthorizing N;
- completed M5H-A: define canonical grant-proposal and user-confirmation
  contracts that avoid circular plan/confirmation digests while binding the
  operation, exact plan, package generation, resolved permissions, policy,
  actor, and review lifetime;
- completed M5H-A: deterministically finalize `allow` proposals without
  confirmation and `ask` proposals only with exact user evidence, rejecting
  substitution, future/expired confirmation, secret-bearing agent proposals,
  and ceiling escalation;
- completed M5H-A: verify the proposal-to-final-grant-to-durable-store path in
  a cross-crate integration test;
- completed M5H-B: define canonical workspace grant snapshots and sorted
  multi-package change sets, binding them to the existing operation plan's
  `grantBeforeDigest` and `grantAfterDigest` workspace-impact evidence;
- completed M5H-B: derive required grant/revoke/no-op coverage from every root
  and dependency Add/Replace/Remove transition, reject missing or injected
  packages, state/receipt revision rollback, plan drift, and duplicate or
  unrelated confirmation;
- completed M5H-B: add a canonical operation-confirmation contract so every
  `ask` apply, including revoke-only uninstall, binds the exact operation plan;
- completed M5H-B: resolve ordered candidate grants and exact delayed
  revocations with one monotonic next state revision, preserving N until N+1
  capability cutover;
- completed M5I-A: build canonical scope grant snapshots directly from durable
  receipts under the cross-process store lock, with exact path ownership,
  deterministic package ordering, and bounds on publishers, packages, stored
  generations, and active plan entries;
- completed M5I-A: reject stale global revisions across both grants and
  revocation tombstones, moved or malformed records, unknown layout, and
  parallel granted generations for one package while safely ignoring
  non-authoritative abandoned atomic-write temporary files;
- completed M5I-B: extend resolved grant changes with immutable
  operation/plan/change-set identity, prior/next state revision, and prior/next
  capability generation, rejecting revision or generation exhaustion;
- completed M5I-B: persist an atomic bounded grant-operation intent before
  side effects, including the locked observed before snapshot, exact candidate
  receipts plus signed ceilings, and exact prior receipts for retirement;
- completed M5I-B: implement idempotent intent-recorded -> preparing ->
  prepared -> cutover-committed -> retiring -> completed phases, with
  non-future exact capability-cutover evidence and retirement of N only after
  N+1 cutover;
- completed M5I-B: recover partial prepare and partial retirement across store
  instances, reject stale snapshots, operation-ID conflict, ceiling
  substitution, candidate drift, and unknown journal fields, and preserve
  same-generation grant replacement instead of tombstoning the new grant;
- completed M5I-C: add a plan-bound `PluginGrantLifecycleUnit` and grant-aware
  package-graph install, upgrade, and uninstall entry points that persist Grant
  candidates before package/Runtime prepare and accept only exact Registry
  snapshot digest plus N -> N+1 generation evidence, including the initial
  0 -> 1 cutover;
- completed M5I-C: persist rolling-back/rolled-back evidence and exact prior
  Grant records so a pre-cutover failure restores overwritten records or
  removes only the operation-owned candidate while package rollback converges
  independently;
- completed M5I-C: checkpoint cutover before retirement, drain calls admitted
  by the prior capability generation before revoking its Grants, reuse durable
  cutover/rollback timestamps on replay, and reject generation drift without
  retiring prior authorization;
- completed M5J-A in the umbrella CLI: define and strictly parse the host-owned
  `a3s.plugin-policy.v1` ACL contract with normalized registry, publisher,
  source, package-size, surface, workspace, filesystem, network, resource,
  execution, and UI ceilings plus a stable policy digest;
- completed M5J-A in the umbrella CLI: deterministically evaluate complete
  immutable Use operation plans, downgrade an out-of-ceiling `allow` to
  `ask`, deny agent secret grants, block `native-unconfined` unattended use,
  and recheck exact policy authority during apply;
- completed M5J-B in the umbrella CLI: load authorization through a bounded
  read from an explicit operator-selected ACL or the existing user-level ACL,
  while excluding automatically discovered workspace configuration from
  pre-authorization;
- completed M5J-B in the umbrella CLI: inject one immutable authorization
  policy into the shared Plugin Manager and expose common complete-plan
  evaluation and apply-time verification APIs to CLI, Web, and management MCP
  adapters; Web remains on the conservative default-`ask` policy until it has
  a trusted host policy source;
- completed M5J-C-A in the umbrella CLI: bind every reviewed plan to a
  host-selected actor, with CLI/Web producing user plans and management MCP
  producing agent plans, while package and request content cannot choose the
  principal; persist and return the actor with the frozen `user/current`
  lifecycle scope;
- completed M5J-C-B in the umbrella CLI: accept an optional complete Use plan
  draft from the delegated planner, bind host identity/lifetime/actor/scope,
  requested release and verified capability generation, evaluate policy, and
  persist the strict `PluginOperationPlanEnvelope`;
- completed M5J-C-B in the umbrella CLI: separate the user-reviewed full-plan
  digest from the upstream component mutation digest, recheck current policy
  before first intent, require and persist exact confirmation for `ask`, and
  resume existing intent from recorded authority without stranding partial
  side effects;
- completed M5J-C-C-A in `a3s-use-core`: define the strict planner-owned
  `a3s.use.plugin-operation-plan-draft.v1` contract without operation identity,
  lifetime, scope, actor, policy, or confirmation authority; bind those fields
  only in the host to produce a validated final operation plan;
- completed M5J-C-C-A in `a3s-use-core`: derive package surface changes and
  plan secret changes from exact before/after states, and reject incomplete
  Runtime provider evidence before a draft can be emitted;
- completed M5J-C-C-B in `a3s-use-core`: add the backward-compatible
  `a3s.use.plugin-catalog.v2` contract with a mandatory signed manifest digest
  and strict Skill/UI dependency edges while preserving catalog-v1 canonical
  bytes and digests;
- completed M5J-C-C-B in `a3s-use-core`: deterministically resolve all
  mandatory surfaces plus only the explicitly selected optional surface
  closure, rejecting missing, duplicate, cyclic, or kind-invalid dependencies;
- completed M5J-C-C-C-A in `a3s-use-core`: derive a validated registry install
  transition from verified catalog-v2 evidence, preserving TUF/archive
  provenance, binding manifest and expanded-package digests, narrowing only
  selected surface/permission evidence, and deriving the exact surface delta;
- completed M5J-C-C-C-B in `a3s-use-extension`: carry the selected verified
  catalog-v2 record through TUF target download into receipt v2, then
  revalidate catalog provenance, exact target resolution, raw manifest digest,
  and expanded-package digest whenever that receipt is loaded; retain receipt
  v1 compatibility for catalog-v1, local, and release-bundle installations;
- completed M5J-C-C-C-C in `a3s-use-core` and `a3s-use-extension`: resolve an
  exact selected package state and derive remove or registry-replace
  transitions from plan-ready installed evidence plus caller-supplied active
  surfaces; receipts remain immutable release evidence and never infer the
  live activation set;
- completed M5J-C-C-D-A in `a3s-use::plugin_runtime`: resolve explicit
  per-surface Runtime provider assignments through `RuntimeClientRegistry`,
  bind provider/build/capability/enforcement/semantics evidence, return the
  exact connected clients, sort evidence for plan construction, and reject
  duplicate, unavailable, or incapable assignments without fallback;
- completed M5J-C-C-D-B in `a3s-use`: add a canonical digest for complete
  extension receipts and publish strict capability `plannerEvidence` only for
  plan-ready schema-v3 registry packages, binding the receipt, catalog,
  manifest, expanded package, desired enabled state, and exact dependency-
  closed named-surface inventory;
- completed M5J-C-C-E-A in the umbrella CLI: retain the verified catalog-v2
  record in the component plan and its digest, join it to the exact registry
  target, verified installation/capability evidence, and a durable monotonic
  planner-state revision, then emit and host-bind a complete live install draft
  for packages containing only permission-free Skill and UI surfaces;
- completed M5J-C-C-E-A in the umbrella CLI: recheck planner-state evidence
  before apply, advance it atomically and idempotently after successful child
  mutation, preserve catalog-v1 component-plan compatibility, and fail closed
  instead of fabricating provider or grant evidence for executable or
  permission-bearing packages;
- completed M5J-C-C-E-B in `a3s-use-core` and `a3s-use`: define and emit strict
  package-specific installed planning evidence that joins a freshly validated
  receipt and complete catalog-v2 record to the same capability generation,
  revision, desired state, and dependency-closed active-surface inventory;
- completed M5J-C-C-E-C in the umbrella CLI: consume that evidence for
  permission-free Skill/UI registry upgrades and uninstalls, match it to the
  compact capability snapshot and umbrella current version, derive exact
  replace/remove transitions and impact, and prevent catalog-v2 upgrades from
  falling back when installed evidence is missing or drifted;
- completed M5J-C-C-D-C in `a3s-use-core`: add backward-compatible
  `a3s.use.plugin-catalog.v3` with one exact bounded `planning-v1.json` target,
  preserving catalog-v1/v2 canonical bytes and requiring target name, length,
  and SHA-256 for v3;
- completed M5J-C-C-D-C in `a3s-use-core`: define the strict
  `a3s.use.plugin-planning-bundle.v1` contract binding package/archive/
  manifest/permission identities, complete executable surface coverage,
  release descriptors, and digest-pinned artifacts for Tool Tasks, HTTP Tool
  Services, and Streamable HTTP MCP Services;
- completed M5J-C-C-D-D in `a3s-use-extension`: load only the exact signed TUF
  planning target, compare TUF and catalog identity, reject package custom
  metadata and raw-byte drift, and rebind the typed bundle without downloading
  the package archive;
- completed M5J-C-C-D-E in `a3s-use::plugin_runtime`: convert the signed bundle
  plus exact package state and canonical grant proposal into provider-neutral
  Runtime templates, binding proposal authority into semantics and failing
  closed on permission shapes not representable by Runtime 0.2;
- completed M5J-C-C-E-D in the umbrella CLI: carry the TUF-verified planning
  bundle through registry resolution into the component plan and canonical
  component digest, accept catalog v3 as plan-ready, and require exact
  catalog/bundle binding before executable planning proceeds;
- completed architecture decision: freeze the host-owned two-pass Plugin
  Runtime Broker boundary in
  `docs/adr-001-plugin-runtime-broker-boundary.md`; packages cannot register
  providers and a provider failure has no fallback;
- completed in `a3s-use`: a typed lifecycle adapter now distinguishes native
  Tool launchers, Runtime Tasks, Runtime Tool Services, stdio MCP launchers,
  and Streamable HTTP MCP Services; it requires explicit provider selections,
  Gateway endpoint evidence, MCP initialize evidence, durable bindings, and
  idempotent stop/removal. Its bounded exact-generation store retains N and
  N+1 concurrently, observes and removes the intent-selected generation, and
  rejects moved, tampered, symlinked, or over-limit receipts;
- completed in standalone `a3s-use`: assemble canonical workspace Grant
  changes after trusted authority binding, persist exact confirmation and
  resolved Grant evidence, construct `PluginGrantLifecycleUnit`, and select
  the grant-aware graph apply path for permission-bearing operations;
- pending: inject the Runtime Broker into the shared umbrella Plugin Manager,
  forward exact Use Grant authority and confirmation through CLI/Web/management
  MCP/managed-host entry points, and add secret-reference adapters,
  filesystem/network/child-process enforcement, production-provider-backed
  prior-generation Runtime retirement, streaming/file-backed large Task
  output, the production MCP initialize client adapter, stdio supervision,
  Gateway route revocation, and scope-aware capability/session snapshot wiring.

Deliverables:

- add validated ACL policy for registry, publisher, size, surface, permission,
  and workspace ceilings;
- add package permission declarations and upgrade permission diffs;
- define secret-name requests without exposing secret values;
- map CLI Tools to Runtime Tasks and HTTP Tools plus Streamable HTTP MCP to
  Runtime Services through an injected typed `RuntimeClient`;
- keep stdio MCP on the supervised compatibility host until Runtime has a
  bidirectional session contract;
- launch workloads with a sanitized environment and package-owned working/data
  roots;
- choose and record an explicit compatible provider during plan, with no
  silent fallback during apply;
- enforce available filesystem, network, and child-process restrictions;
- classify unsupported native confinement as `native-unconfined`;
- persist workspace grants separately from package receipts.
- adopt manager-toolset v2 and the canonical `okf` surface in the umbrella host
  ACL policy; the shared bounded OKF validator and core selector are complete;
- connect the completed injected Knowledge port and exact-generation store to
  the production A3S Knowledge backend, atomic index cutover, parent saga, and
  receipt-owned cleanup.

Exit criteria:

- policy evaluation is deterministic and independently testable;
- a Skill, UI, OKF concept/frontmatter, Tool output/API document, or MCP
  description cannot expand package permissions;
- upgrades that add permission fail pending a new grant;
- unattended native installation is impossible without an enforced sandbox;
- an HTTP Tool cannot become ready on a provider without Service networking
  and health-check support;
- secret values never enter plans, receipts, logs, catalog output, or UI.
- malformed or failed candidate OKF content never replaces the last good
  searchable generation or mutates a personal knowledge vault.

### M6 — Authorized agent lifecycle and hot use

Estimated effort: 3 weeks

Deliverables:

- expose apply, enable, disable, and uninstall management MCP tools with
  correct annotations;
- inherit parent confirmation for `ask` decisions;
- support unattended apply only when every policy ceiling passes;
- refresh the active capability registry after successful mutation;
- attach new Tool bindings, MCP, OKF, Flow, Skill, and UI surfaces to active sessions
  without restart;
- publish a Skill only after every required Flow or direct Tool/MCP/OKF
  binding is usable;
- hide routes before drain and remove package files only after lease release;
- report partial readiness and typed provider failures without fallback.

Exit criteria:

- an E2E agent can search, inspect, plan, obtain confirmation, install, invoke
  one CLI Tool, one HTTP Tool, and one plugin MCP capability, then disable,
  re-enable, and uninstall the package;
- an E2E user and agent can install a signed OKF-bearing package, retrieve a
  line-cited concept from its exact generation, upgrade atomically, and retain
  the prior searchable generation after an injected index failure;
- an E2E user and agent can install a signed Flow-bearing package from a
  replaceable Registry, run it through A3S Code, observe the same identity in
  TUI/Web, then disable and uninstall it without restart;
- the same E2E succeeds without a prompt only under an explicit matching ACL
  policy;
- denial, cancellation, timeout, plan drift, permission drift, and drain
  timeout all fail closed;
- uninstall during an in-flight call preserves that exact generation and blocks
  new calls.

### M7 — Production supply chain and platform gates

Estimated effort: 2–4 weeks

Deliverables:

- establish official registry offline root-key operations, delegated signing
  roles, rotation, expiry, rollback protection, and recovery procedures;
- publish reproducible package provenance and release attestations;
- add security withdrawal and deprecation metadata;
- verify installed release archives through CLI, Web, and agent lifecycle E2E;
- verify reproducible OKF bundle/catalog digests and A3S Knowledge conformance
  evidence in release automation;
- complete the Windows persistent-session and advanced Browser compatibility
  gates required for supported status;
- document incident response and registry disable behavior.

Exit criteria:

- official registry publication does not depend on a long-lived online root
  key;
- release automation verifies every package digest and compatibility claim;
- a withdrawn target cannot be newly installed and remains diagnosable;
- macOS and Linux pass complete lifecycle E2E;
- Windows is either promoted with equivalent evidence or remains explicitly
  preview with no unsupported claim.

## Completion Definition

The plugin-platform objective is complete when:

1. a user can search, inspect, install, enable, use, disable, and uninstall a
   signed multi-surface plugin through CLI and Web;
2. an agent can perform the same lifecycle through standard MCP, with default
   confirmation and policy-bounded unattended operation;
3. installing one plugin downloads only its metadata-selected payload;
4. Skills, CLI/HTTP Tools, MCP capabilities, UI, and OKF remain bound to one
   immutable package identity and generation;
5. authorization, secrets, sandboxing, plan integrity, route draining, and
   owned-file removal fail closed;
6. active sessions observe installation and removal without restart;
7. official registry and platform claims are backed by reproducible release
   and end-to-end evidence.
