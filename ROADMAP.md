# A3S Use Plugin Platform Roadmap

- Status: in progress
- Planning baseline: 2026-07-30
- Scope: A3S Use, the umbrella A3S CLI, A3S Code/Web, and plugin registries

This document is the source of truth for evolving A3S Use into a plugin
platform where a user or an authorized agent can discover, install, enable,
use, disable, and uninstall an immutable package that contributes Skills,
executable Tools, standard MCP servers, and sandboxed UI.

The milestones are dependency ordered. They are not calendar commitments.
Effort ranges assume one primary engineer with review and cross-platform CI
support.

## Product Outcome

A complete user flow is:

```text
search catalog
  -> inspect signed metadata, permissions, provenance, and size
  -> review an immutable install plan
  -> install and enable one selected package
  -> use its Skill, CLI/HTTP Tool, MCP capabilities, or UI
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
4. A plugin may contribute multiple named Skills, executable Tools, standard
   MCP servers, and sandboxed UIs.
5. A Plugin Tool is a real workload on which a Skill or UI may depend. A CLI
   Tool maps to a one-shot Runtime Task; an HTTP Tool maps to a Runtime Service.
   It is distinct from an MCP `tools/list` item and retains its native argv or
   HTTP API contract.
6. A3S Use binds and supervises Tools but does not translate them into a
   private tool protocol, universal action envelope, or generic
   `execute(plugin, action, payload)` RPC.
7. Catalog availability and active capability projection are separate.
   Uninstalled packages may appear in search results but never in the active
   Skill, Tool binding, MCP, or UI registry.
8. Registry metadata is fetched separately from package payloads. Browsing,
   searching, Code startup, and Skill matching never install a package.
9. Every install, upgrade, or uninstall uses plan/apply. Apply repeats
   resolution and fails closed when the plan digest changes.
10. Registry trust roots, unsigned local packages, secret grants, and user-data
   deletion remain user-owned authority.
11. Normal uninstall removes only receipt-owned package files. User data is
    retained unless a separate destructive purge is explicitly authorized.

## Current Baseline

The following foundations are implemented:

- typed Browser, OCR, Box, component, and extension contracts;
- native CLI, standard MCP, Skill, and content-bound Activity Bar surfaces;
- schema v3 named Tool Task/Service, MCP, Skill, and UI surface contracts while
  retaining schema v1/v2 parsing compatibility;
- canonical Tool Task/Service release descriptors with stable JSON fixtures
  and package-level manifest binding validation;
- immutable package generations and receipt-owned installation roots;
- install, upgrade, enable, disable, uninstall, watch, and route draining;
- reviewed local packages, release bundles, and TUF-verified remote packages;
- exact registry target selection with version, channel, target, length, and
  SHA-256 provenance;
- bounded search and inspection over complete signed catalog records, with
  deterministic pagination and filesystem-only offline re-verification;
- umbrella CLI dry-run/apply plans protected by a plan digest;
- a Web Marketplace with catalog and installed views;
- sandboxed plugin UI with verified HTML, CSS, and JavaScript assets;
- generation/revision capability snapshots consumed by A3S Code;
- live MCP and Skill projection into a dedicated A3S Use worker.

The main gaps are:

- the agent worker can discover plugins and create reviewed lifecycle plans,
  but is explicitly forbidden from applying them or toggling packages;
- package-level permission declarations are not precise enough to authorize
  native executable plugins without human review;
- schema v3 named surfaces and Tool release contracts are implemented and the
  manager exposes their signed catalog projection; the first typed Runtime
  adapter can now plan release-backed Tasks and Services and health-gate a
  directly invoked Service apply, but lifecycle apply is not connected yet;
- persisted Tool Task/Service bindings, MCP protocol probes, dependency
  readiness, and Runtime observations are not yet part of the package
  reconciler;
- the shared Plugin Manager now owns Marketplace, reviewed lifecycle
  orchestration, durable apply replay, and the first-class user CLI adapter;
  its bounded read-only management MCP is connected, while Runtime/MCP/UI apply
  adapters remain to be connected;
- the default Use release still carries an optional Science reference package
  payload instead of relying on on-demand registry delivery;
- official registry key operations and Windows Browser parity are not yet at
  the final production gate.

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

The critical path is `M0 -> M1 -> M2 -> M5 -> M6 -> M7`. Science package
splitting in M3 can proceed after the catalog and manager contracts stabilize.
The read-only management MCP in M4 can proceed alongside user UX after M2.
Runtime provider conformance work for M5 should begin after the M0 descriptor
fixtures, in parallel with M1 and M2 implementation.

The indicative total is 16–21 primary-engineer weeks. This is an effort range,
not a calendar promise; Runtime provider, Code/Web, Science, release-security,
and cross-platform CI work can run in parallel when separately staffed.

## Milestones

### M0 — Contract freeze and fixtures (complete 2026-07-30)

Estimated effort: 2 weeks

Implementation status (2026-07-30):

- completed: architecture, lifecycle, security, and delivery-plan baselines;
- completed: schema v3 named Tool, MCP, Skill, and UI surfaces with v1/v2
  parsing compatibility and a stable ACL fixture digest;
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

### M2 — Shared Plugin Manager application service (in progress 2026-07-30)

Estimated effort: 2–3 weeks

Implementation status (2026-07-30):

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
  racing result publication, while the existing component journal remains the
  sole per-side-effect checkpoint journal;
- completed: plans and results carry explicit A3S Use capability
  generation/revision evidence, including a bounded unavailable state that
  cannot turn a successful mutation into a false failure;
- completed in A3S Use: a deterministic, level-based schema v3 Surface
  Reconciler calculates required dependency closure, per-surface
  desired/observed state, aggregate ready/degraded/broken state, and atomic
  publication eligibility without starting new Runtime workloads;
- completed in A3S Use: capability snapshots expose the reconciliation
  evidence and project named Skills only after every required Tool and MCP
  dependency is prepared or healthy; missing Runtime, MCP, and UI adapters
  remain explicit `pending` observations;
- covered: typed complete-catalog mapping, lifecycle argument and digest
  validation, Use-owned JSON output, operation ID uniqueness, expiry,
  append-only replay, corruption rejection, cross-process locking, Web adapter
  compilation, deterministic surface graph/readiness fixtures, dependency-gated
  Skill projection, read-only and mutation CLI parser/authority/output
  contracts, offline child-policy propagation, a signed-registry CLI
  plan/apply/replay fixture, and a controlled Web Marketplace/invalid-plan
  smoke test;
- pending: Runtime/MCP/UI observation and apply adapters, and the complete Unix
  Marketplace lifecycle E2E through the shared service.

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

### M3 — User plugin UX and on-demand Science delivery

Estimated effort: 2 weeks

Deliverables:

- add the `a3s plugin` user vocabulary as an adapter over the shared manager;
- expose search, inspect, installed-only, and source-verification views in Web;
- display download size, installed size, surfaces, permissions, source, and
  digest before confirmation;
- stop embedding the `a3s/science` reference payload in every A3S Use release;
- publish independently useful Science capability groups through its signed
  registry, with an optional metadata-only collection package;
- retain explicit local-package and optional offline-pack workflows.

Exit criteria:

- a default A3S Use archive contains no Science executable, Skill, or UI
  payload;
- opening Code or Marketplace downloads no plugin archive;
- installing one Science entry downloads and activates only its selected TUF
  target and exact content-addressed dependency closure;
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
- pending: join verified registry/package records, installed receipts,
  selected Runtime provider evidence, and current capability state into the
  draft and emit it from the live umbrella plan,
  package/Runtime/capability saga wiring for the durable grant journal,
  secret-reference adapters,
  filesystem/network/child-process enforcement, durable binding
  orchestration, streaming/file-backed large Task output, the actual MCP
  initialize client adapter, stdio supervision, Gateway route revocation,
  binding-store cleanup orchestration, and scope-aware capability/session
  snapshot wiring.

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

Exit criteria:

- policy evaluation is deterministic and independently testable;
- a Skill, UI, Tool output/API document, or MCP description cannot expand
  package permissions;
- upgrades that add permission fail pending a new grant;
- unattended native installation is impossible without an enforced sandbox;
- an HTTP Tool cannot become ready on a provider without Service networking
  and health-check support;
- secret values never enter plans, receipts, logs, catalog output, or UI.

### M6 — Authorized agent lifecycle and hot use

Estimated effort: 3 weeks

Deliverables:

- expose apply, enable, disable, and uninstall management MCP tools with
  correct annotations;
- inherit parent confirmation for `ask` decisions;
- support unattended apply only when every policy ceiling passes;
- refresh the active capability registry after successful mutation;
- attach new Tool bindings, MCP, Skill, and UI surfaces to active sessions
  without restart;
- publish a Skill only after every required Tool and MCP binding is usable;
- hide routes before drain and remove package files only after lease release;
- report partial readiness and typed provider failures without fallback.

Exit criteria:

- an E2E agent can search, inspect, plan, obtain confirmation, install, invoke
  one CLI Tool, one HTTP Tool, and one plugin MCP capability, then disable,
  re-enable, and uninstall the package;
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
4. Skills, CLI/HTTP Tools, MCP capabilities, and UI remain bound to one
   immutable package identity and generation;
5. authorization, secrets, sandboxing, plan integrity, route draining, and
   owned-file removal fail closed;
6. active sessions observe installation and removal without restart;
7. official registry and platform claims are backed by reproducible release
   and end-to-end evidence.
