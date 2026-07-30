# A3S Use Plugin Platform Development Plan

- Status: implementation in progress
- Planning baseline: 2026-07-30
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
                         A3S Use package store
                 stage / verify / activate / receipt
                                  |
                       capability snapshot/watch
          +-----------+-----------+-----------+-----------+
          |           |           |           |           |
        Skills   Tool Tasks  Tool Services  MCP servers  UI assets
       guidance   Runtime      Runtime       standard    sandboxed
                    Task       Service       protocol      view
```

Ownership remains explicit:

- the umbrella A3S host owns configured registries, trust roots, install
  policy, user confirmation, and workspace authorization;
- A3S Use owns package validation, immutable activation, receipts, leases,
  surface reconciliation, provider/runtime bindings, and owned-file removal;
- each plugin repository owns its Tool CLI/HTTP and MCP vocabulary, Skill
  guidance, UI assets, version, license, and reproducible package;
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
prepared or healthy.

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

### Searchable catalog metadata

Signed catalog records must be sufficient to search and review a plugin without
downloading its archive:

- package ID, display name, description, publisher, keywords, and categories;
- semantic version, channel, host compatibility, and target;
- declared surface IDs, Tool Task/Service kinds, and MCP tool count when
  publisher-generated;
- compressed bytes, expanded bytes, and file count;
- package-level permission summary;
- license and canonical source repository;
- registry identity, TUF role versions, archive target, and SHA-256;
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
- Skill/UI dependency changes and selected Runtime provider evidence;
- permission and secret-grant diff;
- download and installed-size estimates;
- affected workspace grants;
- whether calls must drain;
- the canonical plan digest and expiration time.

Apply accepts the digest, repeats resolution, and rejects any changed target,
metadata version, permission set, package content, or ownership state.

## Authorization Model

The default agent policy is `ask`, not `allow`.

```acl
plugins {
  agent_install   = "ask"
  agent_uninstall = "ask"

  trusted_registries = ["a3s"]
  trusted_publishers = ["a3s"]

  max_download_bytes = 52428800

  allow {
    read_only       = true
    network_read    = true
    workspace_write = false
    secrets         = false
    child_process   = false
  }
}
```

The final ACL schema may refine these names, but it must preserve typed values
and the following decisions:

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
policy may be more restrictive but never more permissive. Skill text, UI
messages, Tool output, MCP descriptions, API documentation, and remote content
cannot modify policy or authorize an install.

Native process isolation is not equivalent to a sandbox. Until filesystem,
environment, process, and network restrictions are enforced on a platform, a
native executable package is reported as `native-unconfined` and cannot use
the unattended `allow` path.

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
- named surface dependency graphs and Tool release descriptors.

### Registry and package tests

- metadata tampering, expiry, rollback, and root mismatch;
- deterministic search and pagination;
- target length and SHA-256 mismatch;
- archive traversal, links, devices, duplicate paths, and expansion limits;
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
- uninstall ownership and retained user data.

### Agent safety tests

- search and plan without mutation;
- default confirmation and explicit pre-authorization;
- denial of trust-root, unsigned-package, secret, and purge operations;
- prompt-injection text in catalog, Skill, Tool output/API documents, MCP
  descriptions, and UI messages;
- permission escalation during upgrade;
- native-unconfined unattended-install rejection;
- dynamic CLI/HTTP Tool binding and removal without arbitrary-path execution.

### Release tests

- no Science payload in the default Use archive;
- one selected Science package and its exact dependency closure downloaded per
  install;
- installed archive smoke through CLI, Web, and manager MCP;
- Skills, CLI/HTTP Tools, MCP capabilities, and UI share one package identity
  and generation;
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
| Science catalog and packages | A3S Science registry builder and package sources |
| Release and compatibility evidence | Use, Browser, OCR, CLI, Web, and Science CI workflows |

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| Signed native code is treated as safe code | Require permission review and enforced sandbox for unattended install |
| Registry changes after agent review | Digest-bound plan/apply with complete re-resolution |
| Skill or UI attempts to authorize itself | Treat content as guidance/data; authorization remains host-owned |
| Search downloads or installs the catalog | Separate signed metadata from payload and active capabilities |
| Multiple adapters diverge | One shared Plugin Manager application service |
| Upgrade silently expands privilege | Signed permission metadata plus explicit permission diff |
| Skill is published before its executable dependency | Dependency-gated surface reconciliation |
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
