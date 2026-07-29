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
- umbrella CLI dry-run/apply plans protected by a plan digest;
- a Web Marketplace with catalog and installed views;
- sandboxed plugin UI with verified HTML, CSS, and JavaScript assets;
- generation/revision capability snapshots consumed by A3S Code;
- live MCP and Skill projection into a dedicated A3S Use worker.

The main gaps are:

- the agent worker is explicitly forbidden from installing extensions;
- no standard MCP management surface exposes plugin search or lifecycle plans;
- searchable registry metadata does not yet describe all surfaces,
  permissions, compatibility, and installed-size information needed for an
  informed autonomous choice;
- package-level permission declarations are not precise enough to authorize
  native executable plugins without human review;
- schema v3 named surfaces and Tool release contracts are implemented, but the
  catalog, manager, reconciler, and Runtime adapters do not consume them yet;
- Tool Task/Service deployment, binding, dependency readiness, and Runtime
  observation are not yet part of the package reconciler;
- the Web adapter owns marketplace orchestration that must be reusable by CLI,
  Web, and agent management surfaces;
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

### M1 — Signed searchable catalog

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
- in progress: Science registry-builder emission and end-to-end discovery of
  every Science catalog entry.

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

### M2 — Shared Plugin Manager application service

Estimated effort: 2–3 weeks

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

### M4 — Agent read-only plugin management

Estimated effort: 1 week

Deliverables:

- expose the Plugin Manager MCP server to the dedicated Use worker;
- implement search, inspect, installed list, status, and lifecycle plan tools;
- add accurate read-only/open-world annotations and bounded results;
- include verified provenance and permission summaries in agent-visible output;
- keep all apply and toggle operations unavailable in this milestone.

Exit criteria:

- an agent can find a verified package and produce an exact install plan;
- an agent cannot install, enable, disable, uninstall, add registries, or
  install from an arbitrary URL;
- catalog content cannot alter the worker policy or management MCP tool
  inventory;
- read-only operations work without a Code session rebuild.

### M5 — Permission policy and runtime enforcement

Estimated effort: 3–4 weeks

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
