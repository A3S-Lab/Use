# A3S Use Plugin Platform Development Plan

- Status: implementation in progress
- Planning baseline: 2026-07-30
- Product amendment: first-class OKF knowledge contribution accepted; M0K-A
  bundle contract frozen 2026-07-31, M0K-B control plane frozen 2026-08-01,
  package-level six-surface lifecycle foundation accepted 2026-08-03, the
  cognitive-package dependency/lock foundation accepted 2026-08-03, and the
  unified A3S Flow surface and exact-generation preflight binding foundation
  accepted 2026-08-04; exact `flow.json` identity plus shared Code
  CLI/TUI/Web local durable execution and observation accepted 2026-08-04
- Roadmap: [A3S Use Plugin Platform Roadmap](../ROADMAP.md)
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Operations: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Contracts: [Plugin Contract Reference](plugin-contracts.md)

This document defines the technical execution plan for the milestones in the
plugin platform roadmap. The roadmap owns priority and completion status; this
plan owns execution workstreams, validation, delivery risks, and non-goals.
The architecture document owns domain and runtime boundaries.

## Target Architecture

```text
                   trusted, signed plugin registries
                     metadata first; payload on demand
                                  |
                         Plugin Catalog Service
                    search / inspect / resolve / lock
                                  |
                  +---------------+---------------+
                  |                               |
               user CLI/Web                 agent MCP client
                  |                               |
                  +-------- Plugin Manager -------+
                            plan / apply
                                  |
                    umbrella authorization broker
                  ACL policy / confirmation / grants
                                  |
                      host Plugin Runtime Broker
              signed templates / explicit provider evidence
                                  |
                      A3S Use package store
          revalidate / install forward / retain / remove reverse
                                  |
                 package lifecycle intent + journal
                 ordered typed hosts / idempotent replay
                                  |
                       capability snapshot/watch
       +----------+----------+----------+----------+----------+----------+
       |          |          |          |          |          |          |
   Tool Tasks Tool Services MCP servers OKF bundles A3S Flows  Skills   UI assets
    Runtime     Runtime      standard   Knowledge   durable   guidance  sandboxed
      Task      Service      protocol     index      engine               view
```

Ownership remains explicit:

- the umbrella A3S host owns configured registries, trust roots, install
  policy, user confirmation, and workspace authorization;
- A3S Use owns package validation, immutable activation, receipts, leases,
  surface reconciliation, provider/runtime bindings, and owned-file removal;
- each plugin repository owns its Tool CLI/HTTP and MCP vocabulary, A3S Flow
  source, Skill guidance, UI and OKF assets, version, license, and reproducible package;
- A3S Code/Web adapts the shared manager and capability registry, and composes
  the sole A3S Flow engine for workspace-local execution, without becoming a
  second package manager or workflow engine.

## Core Contracts

### Package surfaces

One package may declare multiple named surfaces in any compatible combination:

| Surface | Contract | Runtime authority |
| --- | --- | --- |
| Skill | Existing `SKILL.md` plus content digest | Guidance only; never grants permission |
| Tool Task | A non-interactive CLI program with native argv and exit semantics | One-shot A3S Runtime Task, or constrained legacy native runner |
| Tool Service | A private HTTP API with health and optional content-bound OpenAPI | Long-lived A3S Runtime Service behind a scoped binding |
| MCP | Standard stdio or Streamable HTTP server | Runtime Service for HTTP; supervised session for stdio |
| Flow | Durable workflow with explicit Tool/MCP/OKF dependencies | One injected `a3s-flow` engine with a typed runtime adapter |
| UI | Declared HTML, CSS, and JavaScript assets | Sandboxed view with scoped declared backend bindings |
| OKF | Conformant Open Knowledge Format Markdown concept bundle | Non-executable, exact-generation A3S Knowledge projection and local cited-search index |

The checked-in schema-v3 baseline implements Tool, MCP, OKF, Flow, Skill, and UI.
M0K-B freezes the OKF manifest, package validation, catalog, plan,
receipt/projection, and host-observation contracts. M0K-C-A adds the injected
Knowledge port, exact-byte stage request, checked adapter client, and durable
generation store. A package-level intent/journal/coordinator, typed Runtime,
Flow/Skill/UI, and OKF adapters, and P0 package/capability hosts now provide the
in-crate lifecycle foundation. Grant-aware graph paths additionally compose
durable Grant prepare, exact cutover, pre-cutover rollback, drain, and
retirement around that package graph. Production publication still requires
umbrella and managed-host forwarding of the standalone manager's
policy/confirmation-driven Grant authority, typed provider composition, and a
real A3S Knowledge backend.

Flow uses one product model. `engine = "a3s-flow"` is fixed; `native-ts` is the
first admitted execution adapter. Use owns package integrity, dependency
ordering, and catalog projection. Code/Web or another embedding host owns
compiler preflight and durable execution. Code now resolves one strict,
path-free installed identity from `flow.json`, revalidates and stages the exact
source before mutation, and shares one workspace-local event store across CLI,
TUI, and Web. Existing `flow.json` documents are therefore executed or deployed
through a typed adapter to that same Flow identity rather than retained as a
parallel runtime. Distributed workers and automatic suspended-work resumption
remain a later host concern. Web currently exposes the run/history API without
a Marketplace Flow control surface; CLI and TUI provide the interactive local
execution UX.

The dependency foundation is also implemented: schema-v3 ACL package
dependencies, a required bounded `README.md`, bounded deterministic SemVer
resolution, canonical `a3s.use.plugin-package-lock.v1`, lock-bound plans and
host requests, dependency-forward remote download, retained-generation checks,
reverse removal, and one Registry snapshot cutover for changed packages. This
is an in-crate foundation until P1/P4 route the umbrella CLI and Web through it.

A Plugin Tool is not an MCP `tools/list` item. It is the real executable
workload on which a Skill or UI may depend. A3S Use manages its lifecycle and
binding but preserves its CLI or HTTP vocabulary. It does not introduce a
private action schema.

UI continues to have no generic execute message. It may reach only
manifest-declared Tool Service bindings through an origin-scoped reverse proxy
or use the existing reviewed MCP bridge. It never receives an A3S OS token,
registry key, host filesystem path, ambient network access, or direct Runtime
or MCP bearer token.

### Lifecycle state

The manager exposes installation and activation as separate state:

```text
available
  -> resolved
  -> planned
  -> staged
  -> installed
  -> enabled
  -> ready
  -> draining
  -> removed
```

`incompatible`, `broken`, `degraded`, and `disabled` are explicit diagnosable
states. A Flow is ready only after its required Tool/MCP bindings and OKF
generation are usable. A Skill is ready only after every required Flow or
direct Tool/MCP/OKF dependency is ready.

This sequence is a user-facing phase view derived from separate desired and
observed state. It is not persisted as one mutable linear enum.

Install and upgrade commit a new immutable generation. Disable and uninstall
first remove the route from new callers, then acquire the exclusive drain
lease. Existing calls retain the exact generation they accepted.

The M2 implementation projects this model into schema v3 capability bindings.
Its deterministic Surface Reconciler calculates dependency levels, required
closure, host ownership, desired/observed surface state, aggregate readiness,
and publication eligibility. The package coordinator consumes the same graph,
persists deterministic checkpoints, prepares surfaces forward, and removes
them in reverse. Concrete Runtime, immutable Flow/Skill/UI evidence, and OKF adapters exist,
but production host injection is not connected: missing observations remain
explicit `pending` evidence, while a Skill can be projected only when its
required dependency closure is already usable. OKF enters this same operation
through exact receipt/observation evidence; it may not publish an independent
knowledge generation outside the package operation.

### Searchable catalog metadata

Signed catalog records must be sufficient to search and review a plugin without
downloading its archive:

- package ID, display name, description, publisher, keywords, and categories;
- semantic version, channel, host compatibility, and target;
- normalized schema-v3 package dependencies with canonical SemVer ranges;
- declared surface IDs, Tool Task/Service kinds, and MCP tool count when
  publisher-generated;
- declared OKF surface IDs, format version, concept/file counts, expanded
  bytes, limits, and content digest in catalog v3;
- compressed bytes, expanded bytes, and file count;
- package-level permission summary;
- license and canonical source repository;
- registry identity, TUF role versions, archive target, and SHA-256;
- for catalog v3, one exact bounded `planning-v1.json` target name, length,
  and SHA-256 for pre-archive executable planning;
- deprecation, replacement, or security-withdrawal state.

Search operates locally over verified metadata after a bounded refresh. Results
retain registry provenance. A browser or model must not invent an installable
identity that was not returned by a verified catalog.

### Immutable operation plan

Install, upgrade, and uninstall plans include:

- action, package ID, component ID, selected version, channel, and target;
- source registry and trust-root identity;
- the exact transitive package lock and its canonical digest when dependencies
  are present;
- exact archive length and SHA-256;
- expanded package digest when known;
- surfaces added or removed;
- Skill/UI/OKF dependency changes and selected Runtime provider evidence;
- permission and secret-grant diff;
- download and installed-size estimates;
- affected workspace grants;
- whether calls must drain;
- the canonical plan digest and expiration time.

Apply accepts the digest, repeats resolution, and rejects any changed target,
metadata version, permission set, package content, or ownership state.

Implemented dependency-plan guarantees include deterministic bounded
backtracking, highest-compatible selection, distinct missing/conflict/cycle/
ambiguity errors, exact host target/version binding, package-ID-sorted canonical
lock bytes, and plan reconstruction from every locked catalog record. Remote
apply revalidates all locked Registry/TUF/catalog evidence before downloading
the first archive.

The executable planning boundary is
[ADR-001](adr-001-plugin-runtime-broker-boundary.md). Catalog-v3 resolution
downloads only the small signed planning target. A3S Use derives
provider-neutral Task/Service templates; the host supplies explicit provider
assignments and clients. Provider preflight, policy/grant resolution, and
final semantics selection are two deterministic, side-effect-free passes.
The package archive is not downloaded until an authorized apply.

Implemented as of the planning baseline:

- strict catalog-v3 planning-target and executable planning-bundle contracts;
- exact TUF target-only loading and catalog rebinding;
- provider-neutral Tool Task, Tool Service, and Streamable HTTP MCP templates;
- explicit `RuntimeClientRegistry` provider selection and evidence; and
- verified planning-bundle transport in the umbrella CLI component plan.

Remaining on the critical path:

- inject the host Runtime Broker into the shared Plugin Manager;
- forward workspace Grant authority and exact confirmation from the umbrella
  reviewed plan into the standalone Use operation without changing identity;
- inject the implemented package/capability and typed surface hosts into the
  package-level coordinator through one umbrella Plugin Manager composition;
- transport and apply the implemented dependency lock from the standalone CLI
  through Code Web, TUI, management MCP, and managed-host adapters using the
  public lifecycle factory and the same durable graph records;
- select the implemented grant-aware graph path from umbrella and managed-host
  adapters without leaving an authorized grant-free bypass;
- coordinate the implemented exact package/Runtime N/N+1 storage, cutover,
  rollback, drain, and removal primitives after blue/green cutover; and
- pass CLI/Web/agent install-use-upgrade-uninstall E2E with production
  providers.

### Next implementation slices

The remaining work is dependency ordered. A later slice must not bypass an
earlier ownership or durability gate.

| Slice | Scope | Required proof |
| --- | --- | --- |
| P0 — Package/capability hosts (complete 2026-08-03) | Installed-disabled generation commit/removal and atomic publish/hide/drain over receipt schema v3, snapshots, and route leases | Root/receipt/snapshot replay, exact removal, drain timeout, tamper rejection, and unchanged v1/v2 suites pass |
| P0-D — Package dependency graph (complete 2026-08-03) | ACL SemVer dependencies, exact lock, Registry-set resolution/download, standalone CLI dispatch, retained dependency verification, forward install, atomic graph publication, reverse uninstall, and durable graph replay | Backtracking/conflict/cycle/ambiguity fixtures, lock drift rejection before download, cross-Registry TUF closure, retained-node checks, reverse-dependent guard, published-install repair, pending-only uninstall recovery, and symlink ownership tests pass |
| P0-F — Unified Flow contract and Code local runtime (complete 2026-08-04) | First-class Flow inventory, `a3s-flow` identity, Native TypeScript integrity, Tool/MCP/OKF edges, lifecycle/reconciliation, exact `flow.json` resolution, immutable source staging, and one Code CLI/TUI/Web durable local store | Manifest/catalog/source drift fails before mutation; fixed run IDs are idempotent; histories survive host recreation and package upgrade/uninstall; public responses leak no managed path; no injected host means no publication |
| P0-U — Dual-generation storage (complete 2026-08-05) | Bounded exact package receipts, snapshot-selected route leases, Runtime generation directories, exact observation, pre-cutover rollback, and receipt-owned removal | N remains callable while N+1 is staged; commit replay preserves N; moved/tampered/symlinked/over-limit state fails closed; Runtime cleanup removes only N |
| P0-G — Dependency-graph upgrade and GC core (complete 2026-08-05) | Plan-v3 prior/candidate lock binding, Add/Replace/Remove/Retain, selected downloads, shared-node retention, dependency-forward N+1 preparation, one capability cutover including removed routes, automatic unpublished-candidate rollback, reverse N retirement, exact graph CAS, and durable replay | Preparation/publication failure restores N and removes additions; successful cutover retires replacements and unreferenced dependencies exactly once; retained-receipt and applying/rolling-back crash fixtures converge without generation inflation |
| P1 — Host composition (Code baseline complete; production providers pending) | Keep Code's public lifecycle-factory composition for executable Tool Tasks, stdio MCP, A3S Flow, and Skill/UI; add explicit Runtime Service, Gateway/HTTP MCP, and A3S Knowledge adapters | TUI and Web produce the same intent and supported host set today; unavailable production hosts continue to fail before publication |
| P2-A — Grant graph saga (complete 2026-08-05) | Compose durable Grant prepare, exact Registry cutover, joint pre-cutover rollback, accepted-call drain, and prior-Grant retirement with package graph operations | Candidate Grant survives restart; failed publication restores package and Grant candidates; prior Grant survives until exact cutover and drain |
| P2-B — Product Grant wiring (standalone complete 2026-08-06; umbrella pending) | Standalone derives canonical proposals/change sets/resolved Grants/ceilings after injected policy authority and exact confirmation, persists pending-v2 replay evidence, and invokes only the Grant-aware path when required; umbrella and managed hosts must forward the same authority | Standalone install/upgrade/uninstall and crash replay cannot bypass or regenerate authorization; CLI, Web, management MCP, and managed hosts still need identical end-to-end evidence |
| P3 — Production blue/green composition (core complete; providers pending) | Compose the implemented graph cutover, rollback, dependency GC, and retirement coordinator with Gateway/Knowledge/grant receipts | Failed N+1 leaves N callable across every production provider; successful N+1 leaks no old Runtime unit or route; provider receipts preserve the core's crash-safe removed-node behavior |
| P4 — Product adapters (Code baseline complete; MCP/managed hosts pending) | Preserve the shared CLI/Web Marketplace journal, Code snapshot watcher, and local Flow runtime; add visible Web Flow controls and route management MCP/managed-host mutations through the same path | Detached Web API covers install/run/upgrade/run/uninstall/restart with retained Flow history and TUI covers local routing plus watcher readiness; visible Web UX and production hosts must extend the same no-restart proof |
| P5 — Production E2E | Exercise signed and replaceable registries, policy/confirmation, crash replay, retained data, and all six surfaces on supported platforms | macOS/Linux gates pass; Windows claims remain preview until equivalent evidence exists |

P1, the umbrella/managed-host remainder of P2-B, and the remaining
provider-composition work in P3 remain release blockers for calling schema-v3
cognitive-package lifecycle production-ready.
P4 must include dependency-bearing graph operations
through CLI and Web and is the hot-plug product gate. P5 is the release
promotion gate.

## Authorization Model

The default agent policy is `ask`, not `allow`.

```acl
plugins {
  schema = "a3s.plugin-policy.v1"

  agent_install   = "allow"
  agent_upgrade   = "ask"
  agent_uninstall = "ask"

  trusted_registries = ["a3s"]
  trusted_publishers = ["a3s"]
  allowed_surfaces   = ["flow", "mcp", "okf", "skill", "tool", "ui"]

  max_download_bytes  = 52428800
  max_installed_bytes = 268435456
  max_packages        = 16
  max_surfaces        = 64

  allow_release_bundles = true
  allow_user_scope      = false
  workspace_ids         = ["workspace:research"]
  max_workspaces        = 1

  permissions {
    plugin_data = "read-write"
    temporary   = "read-write"

    native_execution = false
    child_process     = false
    private_service   = true
    secrets           = false

    max_cpu_millis               = 2000
    max_memory_bytes             = 1073741824
    max_pids                     = 256
    max_ephemeral_storage_bytes  = 2147483648

    network "api.example.com" {
      ports = [443]
    }

    workspace "inputs" {
      access = "read"
    }
  }
}
```

Omitted ceilings are zero, empty, or false. Exact lists and rules are
normalized before digesting; duplicates, unknown fields, broad network
patterns, and unattended secret grants fail closed. The policy preserves the
following decisions:

| Operation | Default agent decision | May be pre-authorized |
| --- | --- | --- |
| Search verified metadata | Allow | Yes |
| Inspect or list local state | Allow | Yes |
| Build an immutable plan | Allow | Yes |
| Install signed declarative-only package | Ask | Yes, within all policy ceilings |
| Install digest-pinned Runtime Tool/MCP workload | Ask | Yes, with a compatible enforced provider |
| Install content-bound A3S Flow workflow | Ask | Yes, with an explicit compatible `a3s-flow` host adapter |
| Install native Tool or MCP executable | Ask | Only with an enforced sandbox profile |
| Enable or disable installed package | Ask | Yes, per workspace |
| Uninstall receipt-owned files | Ask | Yes, when no protected grant depends on it |
| Add or rotate a trust root | Deny | No; user only |
| Install unsigned/local package | Deny | No; user only |
| Grant a secret | Deny | No; user only |
| Purge plugin user data | Deny | No; user only |

Package permissions form a ceiling. Individual MCP annotations or HTTP route
policy may be more restrictive but never more permissive. Skill text, UI or
OKF content, Tool output, MCP descriptions, API documentation, and remote
content cannot modify policy or authorize an install. Flow source is data and
cannot create ambient authority; its executable and knowledge access comes
only from explicit Tool/MCP/OKF dependency grants.

Core surface selection and manager-toolset v2 now define the canonical `okf`
value. The umbrella host must adopt it through a versioned or explicitly
compatible ACL policy change before policy can authorize an OKF-bearing
package. Unknown surface values continue to fail closed.

Native process isolation is not equivalent to a sandbox. Until filesystem,
environment, process, and network restrictions are enforced on a platform, a
native executable package is reported as `native-unconfined` and cannot use
the unattended `allow` path.

## OKF Contribution Workstream

OKF is a non-executable cognitive surface, not a Skill alias, MCP server,
Runtime workload, or personal knowledge vault. The first delivery targets
current Open Knowledge Format v0.2 with an explicit v0.1 compatibility path and
reuses the same immutable package generation and parent saga as every other
surface.

The dependency-ordered slices are:

1. **Contract and fixture (complete M0K-B):** freeze the manifest-local surface identity,
   declared format version, v0.2/v0.1 compatibility behavior, bundle root,
   canonical digest, size/file limits, optional dependencies, catalog fields,
   plan impact, policy enum, receipt, projection, and observation schemas. Add
   stable ACL/JSON digest fixtures.
2. **Bounded conformance (complete M0K-A/M0K-B):** validate UTF-8 Markdown, properly delimited YAML
   frontmatter, one non-empty scalar `type` on non-reserved concepts,
   canonical concept IDs, reserved `index.md`/`log.md`, standard Markdown link
   syntax, file/node limits, and expanded-package ownership. Preserve unknown
   types and extension keys; report safe dangling links without rejecting the
   bundle. Reject raw compiler inputs as active OKF authority.
3. **Knowledge adapter foundation (complete M0K-C-A):** validate the exact
   borrowed OKF bytes in the stage request, expose an injected
   stage/promote/observe/remove port, check returned receipt/observation
   evidence, and durably retain bounded exact-generation records with
   last-good projection.
4. **Production Knowledge and lifecycle (in progress M0K-C-B):** the package
   adapter now supplies idempotent stage/promote/hide/receipt-remove behavior.
   Implement the real A3S Knowledge index backend behind the port and wire it
   into capability snapshots, plan/apply replay, upgrade cutover, and scoped
   sessions without touching personal notes or another package's index.
5. **Product E2E (pending M0K-C-B/M6):** search a signed OKF-bearing package without archive
   download; review provenance, concept count, bytes, and permission impact;
   install and query cited concepts; upgrade atomically; disable/uninstall; and
   recover each checkpoint after injected crashes.

Implementation status (2026-08-02): M0K-A completed the shared
`a3s.use.okf-bundle.v1` descriptor, deterministic content identity, bounded
v0.2/v0.1 inspector, and canonical/malicious fixtures. M0K-B completed schema
v3 manifest and full-package admission, catalog-v3 bundle evidence,
Skill-to-OKF closure, plan/draft v2 impact, manager-toolset v2, projection
receipt, Knowledge observation, capability projection, and Knowledge-owned
reconciliation. Golden ACL, JSON, and complete-package digests freeze the
slice. M0K-C-A completed the byte-exact injected adapter boundary, durable
last-good binding store, and package-lifecycle adapter foundation. M0K-C-B must
now connect a real Knowledge backend and the scope-aware production hosts;
absent promoted evidence remains unpublished.

The workstream does not compile PDFs, Office files, images, archives, or web
pages during install. Independent compilers produce normalized OKF before
packaging. The package payload reviewed by the plan is the payload promoted by
the Knowledge host.

OKF v0.2 Attested Computation fields remain inert metadata at this boundary.
They cannot implicitly select or invoke a Tool, executor, attester, Runtime
provider, or secret. Any executable binding must be declared and authorized
through the existing Tool and host contracts.

## User And Agent Surfaces

### User commands

The intended product vocabulary is:

```text
a3s plugin search <query>
a3s plugin inspect <publisher/name>
a3s plugin list
a3s plugin install <publisher/name>
a3s plugin enable <publisher/name>
a3s plugin disable <publisher/name>
a3s plugin uninstall <publisher/name>
```

Existing commands such as `a3s install use/<publisher>/<name>` and
`a3s use extension ...` remain compatibility routes and call the same manager.
There is one implementation and one receipt format.

### Agent management MCP

The target host exposes one standard MCP management server with:

```text
plugin_search
plugin_inspect
plugin_list_installed
plugin_status
plugin_plan_install
plugin_plan_upgrade
plugin_plan_uninstall
plugin_apply_plan
plugin_enable
plugin_disable
```

Read-only tools carry correct MCP annotations. Apply, enable, and disable are
mutating. Uninstall is destructive. Tools return typed failures and never fall
back to shell, workspace edits, arbitrary URLs, or unsigned packages.

The completed M4 adapter publishes only the first seven tools, ending at
`plugin_plan_uninstall`. Plan creation may persist an immutable reviewed plan
but cannot apply it or change active capabilities. `plugin_apply_plan`,
`plugin_enable`, and `plugin_disable` remain absent from `tools/list` and are
also explicitly denied by the dedicated Use worker. M6 adds them only after
typed ACL policy, provider enforcement, and inherited parent confirmation are
available.

There is deliberately no `plugin_execute` management tool. After activation,
the capability watcher projects Skills, managed CLI Tool shims, scoped HTTP
Tool bindings, and MCP capabilities into the authorized session. The agent
uses a Tool through its native CLI or HTTP vocabulary described by the Skill,
and uses MCP through standard MCP.

## Storage And Scope

The target storage model is:

- immutable package generations and archives are user-wide and reusable;
- activation and grants may be workspace-scoped;
- exact package generations have separate grant records so N and candidate
  N+1 can coexist until the capability snapshot switches;
- secrets remain in the host secret store and are injected only for an
  approved package, operation, and workspace;
- plugin data is separate from executable package files;
- Tool and MCP Runtime bindings are non-secret receipts, not copied payloads;
- uninstall removes package files after route drain but retains data;
- cache eviction is separate from uninstall and never changes capability
  receipts;
- concurrent installs for the same package serialize through one lifecycle
  lock and converge idempotently.

Workspace grant writes additionally serialize under a dedicated store lock,
atomically replace only the same scope/package/digest record, and preserve
revocation tombstones. Reading a record is observational; use-time authority
requires revalidation against the current package digest, signed ceiling, and
clock.

Plan construction must bind canonical pre-confirmation grant proposals, not
invent a final grant digest before an `ask` decision exists. The subsequent
user confirmation binds both plan and proposal digests; only then may apply
finalize and persist a grant.

The workspace impact's before digest identifies a sorted active-grant snapshot;
its after digest identifies a sorted change set covering root and dependency
packages. Resolution derives required Add/Replace/Remove entries from the
package plan and returns prepare-grant plus delayed-revoke phases. The
before snapshot is now built by a bounded, locked traversal of exact durable
grant generations; stale tombstone/grant revisions and ambiguous parallel
grants fail closed. The grant lifecycle adapter now records immutable intent,
applies candidate receipts idempotently, checkpoints exact capability cutover,
and retires prior receipts with crash-safe replay. The graph coordinator now
persists candidate Grants before package preparation, jointly rolls back
package and Grant candidates before cutover, requires exact Registry snapshot
and generation evidence, drains prior accepted calls, and only then retires old
Grants. The standalone manager now derives and durably binds canonical Grant
inputs after trusted authority and exact confirmation, then selects this path
for every permission-bearing operation. Remaining integration must forward the
same authority through umbrella and managed-host Plugin Managers while
composing production Runtime, Gateway, and Knowledge hosts.

Workspace-scoped activation must not duplicate the package payload. Global
uninstall refuses to proceed while another protected workspace grant still
requires the package unless the user explicitly reviews that impact.

## Validation Matrix

Every milestone adds focused tests at the owning layer.

### Contract tests

- ACL manifest and policy parsing;
- canonical JSON and plan digests;
- permission ceiling and permission-diff fixtures;
- MCP schemas and annotations;
- UI asset paths, media types, sizes, and digests;
- named surface dependency graphs and Tool release descriptors;
- A3S Flow engine/runtime/source/export validation, content digests, and host
  capabilities v1/v2 compatibility;
- canonical package dependency ranges, bounded resolver behavior, exact lock
  bytes/digest, and plan/host-request lock binding;
- canonical OKF manifest, complete package, catalog-v3, plan-v2,
  manager-toolset-v2, receipt, projection, and observation fixtures.

### Registry and package tests

- metadata tampering, expiry, rollback, and root mismatch;
- deterministic search and pagination;
- target length and SHA-256 mismatch;
- archive traversal, links, devices, duplicate paths, and expansion limits;
- malformed OKF frontmatter, unsafe out-of-root references, path collisions,
  oversized concept graphs, nondeterministic digests, raw-source promotion,
  and non-fatal diagnostics for safe dangling links;
- incompatible host and unsupported surface declarations;
- Catalog/manifest package-dependency mismatch, Registry ambiguity, Registry
  identity drift, and dependency-forward multi-target download;
- reproducible package digest and provenance.

### Lifecycle tests

- plan/apply mismatch;
- canonical all-six-surface forward preparation and reverse removal;
- retained shared-dependency reuse, exact published-generation verification,
  reverse-dependent uninstall protection, atomic graph cutover, and recovery
  from partial receipt writes;
- journal restart, concurrent same-key replay, optional failure, required
  failure, tampered records, and symlinked paths;
- install and upgrade atomicity;
- enable, disable, and watch generation changes;
- concurrent install convergence;
- route conflict and stale lookup rejection;
- in-flight drain, timeout, retry, and crash reconciliation;
- Runtime Task invocation and private Service health/binding behavior;
- provider capability mismatch and no-fallback behavior;
- Flow preflight, dependency-gated publication, reverse stop/remove, source
  corruption, and unavailable-host rejection;
- OKF last-good-generation preservation across conformance and index failure;
- uninstall ownership and retained user data.

### Agent safety tests

- search and plan without mutation;
- default confirmation and explicit pre-authorization;
- denial of trust-root, unsigned-package, secret, and purge operations;
- prompt-injection text in catalog, Skill, OKF, Tool output/API documents, MCP
  descriptions, and UI messages;
- permission escalation during upgrade;
- native-unconfined unattended-install rejection;
- dynamic CLI/HTTP Tool binding and removal without arbitrary-path execution.

### Release tests

- no Science payload in the default Use archive;
- one selected Science package and its exact dependency closure downloaded per
  install;
- installed archive smoke through CLI, Web, and manager MCP;
- CLI/HTTP Tools, MCP capabilities, OKF, Flow, Skills, and UI share one package
  identity and generation;
- macOS, Linux, and Windows evidence remains aligned with platform claims.

## Workstream Map

| Workstream | Primary locations |
| --- | --- |
| Package, catalog, TUF, receipts, grants, leases | `crates/extension/`, `src/release_bundles.rs` |
| Package-level lifecycle intent, journal, and typed hosts | `src/plugin_lifecycle/` |
| Surface reconciliation and bindings | `src/capability_registry.rs`, `src/extension_host.rs` |
| Tool/MCP Runtime deployment | A3S Runtime adapters, `src/mcp/`, release descriptors |
| A3S Flow preflight, local execution/history, distributed replay, and Code/OS adapter | A3S Flow, A3S Code `/flow` plus Web Flow API, Use lifecycle host |
| Umbrella plan, policy, and lifecycle | A3S CLI `components/`, registry store, configuration |
| Agent worker and manager MCP adapter | A3S CLI `use_registry.rs` and Code session adapters |
| User Marketplace and sandboxed UI | A3S Web Plugins feature and Code Web plugin API |
| OKF conformance, projection, and cited search | A3S Knowledge service, Code/Web OKF adapters, Use reconciler |
| Science catalog and packages | A3S Science registry builder and package sources |
| Release and compatibility evidence | Use, Browser, OCR, CLI, Web, and Science CI workflows |

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| Signed native code is treated as safe code | Require permission review and enforced sandbox for unattended install |
| Registry changes after agent review | Digest-bound plan/apply with complete re-resolution |
| Skill, UI, OKF, or Flow source attempts to authorize itself | Treat content as guidance/data; authorization remains host-owned |
| Search downloads or installs the catalog | Separate signed metadata from payload and active capabilities |
| Multiple adapters diverge | One shared Plugin Manager application service |
| Upgrade silently expands privilege | Signed permission metadata plus explicit permission diff |
| Skill is published before its executable dependency | Dependency-gated surface reconciliation |
| `flow.json` and package Flow become divergent runtimes | One exact installed identity and one A3S Flow engine; the typed adapter is shared by CLI, TUI, Web, and OS deployment metadata |
| Candidate OKF indexing replaces valid knowledge before it is complete | Stage and validate, atomically promote, retain the last good generation |
| Runtime provider cannot honor Service isolation | Capability negotiation in plan and no silent fallback |
| Uninstall breaks active calls | Hide, acquire drain lease, then remove owned files |
| Uninstall destroys user data | Separate executable package roots from retained data and purge |
| Registry compromise affects every user | Pinned roots, delegated roles, expiry, rollback protection, and withdrawal |
| Cross-platform sandbox semantics differ | Report enforced profiles precisely and fail unattended native install closed |

## Non-Goals

This plan does not turn A3S Use into:

- a universal operating-system package manager;
- a frontend for arbitrary npm, pip, Cargo, Homebrew, Winget, APT, or source
  repository installs;
- an arbitrary URL downloader or Git clone-and-execute service;
- an in-process native dynamic-library host;
- a new A3S JSON-RPC dialect;
- a universal tool/action schema layered over MCP;
- a translation layer that rewrites native Tool CLI or HTTP operations;
- a browser UI runtime with host DOM, ambient network, or secret access;
- an authority for an agent to add trust roots or install unsigned code.
