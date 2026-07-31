# A3S Use Plugin Platform Development Plan

- Status: implementation in progress
- Planning baseline: 2026-07-30
- Product amendment: first-class OKF knowledge contribution accepted and M0K-A
  bundle contract frozen 2026-07-31
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
                          search / inspect / list
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
                 stage / verify / activate / receipt
                                  |
                       capability snapshot/watch
       +----------+----------+----------+----------+----------+
       |          |          |          |          |          |
     Skills   Tool Tasks Tool Services MCP servers UI assets OKF bundles
    guidance   Runtime     Runtime      standard   sandboxed Knowledge
                 Task      Service      protocol     view      index
```

Ownership remains explicit:

- the umbrella A3S host owns configured registries, trust roots, install
  policy, user confirmation, and workspace authorization;
- A3S Use owns package validation, immutable activation, receipts, leases,
  surface reconciliation, provider/runtime bindings, and owned-file removal;
- each plugin repository owns its Tool CLI/HTTP and MCP vocabulary, Skill
  guidance, UI and OKF assets, version, license, and reproducible package;
- A3S Code/Web adapts the shared manager and capability registry without
  becoming a second package manager.

## Core Contracts

### Package surfaces

One package may declare multiple named surfaces in any compatible combination:

| Surface | Contract | Runtime authority |
| --- | --- | --- |
| Skill | Existing `SKILL.md` plus content digest | Guidance only; never grants permission |
| Tool Task | A non-interactive CLI program with native argv and exit semantics | One-shot A3S Runtime Task, or constrained legacy native runner |
| Tool Service | A private HTTP API with health and optional content-bound OpenAPI | Long-lived A3S Runtime Service behind a scoped binding |
| MCP | Standard stdio or Streamable HTTP server | Runtime Service for HTTP; supervised session for stdio |
| UI | Declared HTML, CSS, and JavaScript assets | Sandboxed view with scoped declared backend bindings |
| OKF | Conformant Open Knowledge Format Markdown concept bundle | Non-executable, exact-generation A3S Knowledge projection and local cited-search index |

The checked-in schema-v3 baseline implements Tool, MCP, Skill, and UI only.
OKF is an accepted target contribution and must not be added to production
manifests until its additive schema, canonical fixtures, conformance limits,
receipt/projection contract, and host observation are frozen.

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
states. A Skill is ready only after its required Tool and MCP bindings are
prepared or healthy and, in the target model, any required OKF generation is
conformant and atomically promoted.

This sequence is a user-facing phase view derived from separate desired and
observed state. It is not persisted as one mutable linear enum.

Install and upgrade commit a new immutable generation. Disable and uninstall
first remove the route from new callers, then acquire the exclusive drain
lease. Existing calls retain the exact generation they accepted.

The M2 implementation projects this model into schema v3 capability bindings.
Its deterministic Surface Reconciler calculates dependency levels, required
closure, host ownership, desired/observed surface state, aggregate readiness,
and publication eligibility. It does not claim deployment: missing Runtime,
MCP, and UI adapters remain explicit `pending` evidence, while a Skill can be
projected only when its required dependency closure is already usable.
The target OKF adapter enters this same reconciler; it may not publish an
independent knowledge generation outside the package operation.

### Searchable catalog metadata

Signed catalog records must be sufficient to search and review a plugin without
downloading its archive:

- package ID, display name, description, publisher, keywords, and categories;
- semantic version, channel, host compatibility, and target;
- declared surface IDs, Tool Task/Service kinds, and MCP tool count when
  publisher-generated;
- declared OKF surface IDs, format version, concept/file counts, expanded
  bytes, and content digest after the additive contract is implemented;
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
- assemble workspace grant proposals/change sets before final draft binding;
- coordinate package, grant, Runtime, Gateway, projection, capability, and
  drain checkpoints in the parent saga; and
- pass CLI/Web/agent install-use-upgrade-uninstall E2E with production
  providers.

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
  allowed_surfaces   = ["mcp", "skill", "tool", "ui"]

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
content cannot modify policy or authorize an install.

The policy example above reflects the implemented v1 surface enum. The OKF
workstream must add a canonical `okf` value through a versioned or explicitly
compatible schema change before policy can authorize an OKF-bearing package.
Unknown surface values continue to fail closed.

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

1. **Contract and fixture:** freeze the manifest-local surface identity,
   declared format version, v0.2/v0.1 compatibility behavior, bundle root,
   canonical digest, size/file limits, optional dependencies, catalog fields,
   plan impact, policy enum, receipt, projection, and observation schemas. Add
   stable ACL/JSON digest fixtures.
2. **Bounded conformance:** validate UTF-8 Markdown, properly delimited YAML
   frontmatter, one non-empty scalar `type` on non-reserved concepts,
   canonical concept IDs, reserved `index.md`/`log.md`, standard Markdown link
   syntax, file/node limits, and expanded-package ownership. Preserve unknown
   types and extension keys; report safe dangling links without rejecting the
   bundle. Reject raw compiler inputs as active OKF authority.
3. **Knowledge host adapter:** stage a candidate under an exact package
   generation, call the A3S Knowledge promotion/index boundary idempotently,
   record a non-secret observation digest, and retain the last good searchable
   generation when validation or indexing fails.
4. **Reconciliation and lifecycle:** include OKF in dependency closure,
   capability snapshots, plan/apply replay, upgrade cutover, disable, drain-free
   removal, and receipt-owned uninstall without touching personal notes or
   another package's index.
5. **Product E2E:** search a signed OKF-bearing package without archive
   download; review provenance, concept count, bytes, and permission impact;
   install and query cited concepts; upgrade atomically; disable/uninstall; and
   recover each checkpoint after injected crashes.

Implementation status (2026-07-31): M0K-A has completed the shared
`a3s.use.okf-bundle.v1` descriptor, deterministic content identity, bounded
v0.2/v0.1 inspector, and canonical/malicious fixtures in `a3s-use-core`. This
is the single conformance implementation intended for slices 1–4. Production
manifest acceptance remains fail-closed until the rest of slice 1 and the
Knowledge observation/lifecycle schemas are implemented.

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
and retires prior receipts with crash-safe replay. The remaining integration is
for the Plugin Manager's parent saga to coordinate these checkpoints with
package commit, Runtime health, capability publication, route switching, and
lease drain.

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
- target OKF manifest, policy, catalog, receipt, projection, and observation
  fixtures once their additive contracts are frozen.

### Registry and package tests

- metadata tampering, expiry, rollback, and root mismatch;
- deterministic search and pagination;
- target length and SHA-256 mismatch;
- archive traversal, links, devices, duplicate paths, and expansion limits;
- malformed OKF frontmatter, unsafe out-of-root references, path collisions,
  oversized concept graphs, nondeterministic digests, raw-source promotion,
  and non-fatal diagnostics for safe dangling links;
- incompatible host and unsupported surface declarations;
- reproducible package digest and provenance.

### Lifecycle tests

- plan/apply mismatch;
- install and upgrade atomicity;
- enable, disable, and watch generation changes;
- concurrent install convergence;
- route conflict and stale lookup rejection;
- in-flight drain, timeout, retry, and crash reconciliation;
- Runtime Task invocation and private Service health/binding behavior;
- provider capability mismatch and no-fallback behavior;
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
- Skills, CLI/HTTP Tools, MCP capabilities, UI, and OKF share one package
  identity and generation;
- macOS, Linux, and Windows evidence remains aligned with platform claims.

## Workstream Map

| Workstream | Primary locations |
| --- | --- |
| Package, catalog, TUF, receipts, grants, leases | `crates/extension/`, `src/release_bundles.rs` |
| Surface reconciliation and bindings | `src/capability_registry.rs`, `src/extension_host.rs` |
| Tool/MCP Runtime deployment | A3S Runtime adapters, `src/mcp/`, release descriptors |
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
| Skill, UI, or OKF attempts to authorize itself | Treat content as guidance/data; authorization remains host-owned |
| Search downloads or installs the catalog | Separate signed metadata from payload and active capabilities |
| Multiple adapters diverge | One shared Plugin Manager application service |
| Upgrade silently expands privilege | Signed permission metadata plus explicit permission diff |
| Skill is published before its executable dependency | Dependency-gated surface reconciliation |
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
