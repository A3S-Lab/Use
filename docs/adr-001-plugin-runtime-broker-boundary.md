# ADR-001: Host-Owned Plugin Runtime Broker

- Status: accepted architecture; lifecycle integration in progress
- Decision date: 2026-07-30
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Lifecycle: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Roadmap: [A3S Use Plugin Platform Roadmap](../ROADMAP.md)

## Context

An A3S plugin may contribute Skills, CLI Tools, HTTP Tool Services, standard
MCP servers, and static UI. Tool means the real program or Service that
performs work; it does not mean an MCP `tools/list` entry.

The plugin package knows its workload contract and maximum required authority,
but it is not trusted to choose an execution backend. The host knows which
Runtime providers are configured, healthy, policy-approved, and capable on the
current machine. Those responsibilities must not be merged.

The architecture also needs executable plans to be reviewable without
downloading a potentially large package archive. Provider selection must
therefore operate on small signed evidence before package mutation.

## Decision

The umbrella A3S host owns a typed Plugin Runtime Broker. The broker is a
composition boundary, not a new execution protocol:

1. A3S Use validates the signed catalog-v3 planning bundle and converts it into
   provider-neutral Runtime Task or Service templates.
2. The host supplies explicit provider assignments and a
   `RuntimeClientRegistry`.
3. `RuntimeProviderSelector` connects only those provider IDs, reads their
   capabilities, and returns immutable provider evidence plus the exact
   connected clients.
4. Apply reopens the recorded provider and requires compatible build,
   capability, enforcement, and semantics evidence. It never falls back.

A plugin manifest, catalog record, planning bundle, Skill, Tool output, MCP
description, or UI message cannot register, name, prioritize, or replace a
Runtime provider.

`a3s-runtime` is the execution contract and provider abstraction. A component
named OCI Runtime is not automatically an `a3s-runtime` provider; it
participates only when the host explicitly adapts it through the required
provider factory/client contract and passes conformance tests.

## Surface Placement

The installed package store remains the single source of truth. Installation
does not scatter authoritative package files into unrelated roots.

| Surface | Canonical storage | Active projection or deployment |
| --- | --- | --- |
| Skill | Immutable plugin generation | Receipt-owned managed Skill projection for the authorized scope/session |
| Tool Task | Signed release/artifact reference plus immutable package metadata | Prepared Runtime Task launcher template; one exact Task per invocation |
| Tool Service | Signed release/artifact reference plus immutable package metadata | Private Runtime Service plus scoped Gateway binding |
| Streamable HTTP MCP | Signed release/artifact reference plus immutable package metadata | Runtime Service, health gate, then standard MCP initialize probe |
| stdio MCP | Immutable package generation | Supervised compatibility session until Runtime has a bidirectional session contract |
| UI | Integrity-bound static assets in the immutable generation | Receipt-owned Code/Web sandbox projection with declared backend bindings |

A managed projection may use a platform-appropriate link, mount, or copied
cache, but its receipt must bind the source generation and every owned path.
Deleting a projection must never delete mutable user data or an unrelated
Skill directory.

## Metadata and Payload Boundary

Search and inspect consume only verified catalog metadata.

Catalog v3 adds one exact TUF target:

```text
extensions/<package>/<version>/<channel>/<target>/planning-v1.json
```

The target is bounded to 512 KiB and binds:

- package, version, channel, and target;
- archive, expanded package, manifest, and permission-ceiling digests;
- every executable surface;
- Tool Task or Tool Service workload semantics;
- Streamable HTTP MCP Service semantics; and
- digest-pinned OCI artifact and release descriptors.

The planning target is fetched only when an install or upgrade needs
executable planning. The package archive is downloaded only after review and
apply authorization. Skill/UI-only catalog-v2 packages do not require the
planning target.

Optional surface selection narrows activation and authority, not archive
bytes. Avoiding unrelated Science downloads therefore requires independently
installable Science packages and dependency metadata. One large Science
archive cannot become selectively downloadable merely by marking surfaces
optional.

## Broker Planning Protocol

The broker consumes host-authenticated context and verified package evidence.
A conceptual input is:

```text
operation identity and expiry
actor and scope
host policy identity
capability generation and durable state revision
verified catalog-v3 record
verified planning bundle
selected exact package state
resolved permission ceiling
active grant snapshot
host-configured provider assignment per executable surface
```

It returns:

```text
canonical grant proposal and change-set evidence
provider-neutral Runtime templates
one provider/build/capability/enforcement/semantics proof per surface
exact provider clients retained for the apply saga
typed unsupported-capability or policy diagnostics
```

Provider clients and credentials are process-local handles. They are never
serialized into a plan, receipt, catalog result, log, or UI response.

### Two-pass provider selection

Planning has a deliberate two-pass structure to avoid a digest cycle between
policy, grant proposals, Runtime semantics, and provider evidence:

1. **Capability preflight**
   - derive the required Task/Service, artifact, isolation, network, health,
     mount, resource, output, secret-reference, and lifecycle profile;
   - query only the host-assigned provider;
   - capture provider ID, build, normalized capabilities, and enforcement;
   - reject an unsupported or changed provider.
2. **Host authorization**
   - evaluate package provenance, selected permissions, operation impact,
     workspace, and preflight enforcement through ACL policy;
   - allocate `allow`, `ask`, or `deny`;
   - construct the canonical pre-confirmation grant proposal and sorted grant
     change set.
3. **Authorized template selection**
   - convert the signed bundle into exact Runtime templates;
   - bind the canonical grant-proposal digest into Runtime semantics;
   - validate the templates against the same provider/build/capability
     evidence;
   - emit final `PlannedProviderEvidence`.
4. **Immutable plan**
   - bind package transitions, grant-change digest, provider evidence, impact,
     operation identity, actor, scope, expiry, capability generation, and
     state revision into one canonical plan digest.

If any provider observation changes between preflight and final selection,
planning fails. It does not silently restart with another provider.

## Apply and Reconciliation Protocol

Apply accepts only the stored operation identity and reviewed plan digest:

1. load the immutable plan and terminal result, if any;
2. return an existing terminal result without repeating side effects;
3. revalidate trust metadata, package target, planning target, installed
   evidence, grants, host policy, provider evidence, and state revision;
4. persist durable operation intent;
5. download, stage, and verify the exact package archive;
6. commit an immutable installed-disabled candidate receipt;
7. prepare candidate grants without revoking the active generation;
8. prepare or apply exact Runtime Task/Service bindings;
9. health-gate Services and complete MCP initialization;
10. commit Skill and UI projections only when their dependency closure is
    usable;
11. atomically publish one capability generation;
12. hide and drain the superseded generation;
13. revoke old grants and remove old Runtime units, routes, projections, and
    unreferenced immutable package files; and
14. persist an append-only terminal result.

Runtime, Gateway, package storage, grants, and capability publication do not
share a database transaction. The lifecycle is therefore a durable,
idempotent saga. Every external mutation uses an operation/surface/generation
idempotency key.

## Tool and MCP Semantics

### CLI Tool

- The install plan binds the immutable launcher template.
- Each invocation supplies native argv and a unique invocation identity.
- The Runtime Task is finite and preserves stdout, stderr, timeout, and exit
  semantics.
- Arbitrary package paths and interactive shells are not generic plugin APIs.

### HTTP Tool Service

- The Service is private and health-gated.
- The plugin retains its HTTP API vocabulary.
- The host exposes only a scoped opaque Gateway binding.
- A UI can call only declared method/path bindings.

### MCP

- MCP remains the standard protocol.
- Streamable HTTP MCP uses a Runtime Service and is not ready until both
  Service health and MCP initialization succeed.
- stdio MCP remains in a supervised compatibility host until Runtime exposes a
  portable bidirectional session contract.
- No Tool is automatically wrapped as MCP and no MCP tool is treated as a
  package lifecycle Tool.

## Failure Rules

The broker and lifecycle fail closed when:

- catalog-v3 omits its planning target;
- the TUF target name, length, digest, or custom metadata drifts;
- the bundle does not cover every executable catalog surface;
- an artifact is mutable or does not match its release descriptor;
- a provider is absent, duplicated, incapable, or changes build/capabilities;
- requested authority cannot be represented by the locked Runtime contract;
- policy, grant snapshot, capability generation, or state revision changes;
- a Service lacks private networking or declared health support;
- MCP initialization evidence is missing;
- required dependency readiness fails; or
- apply observes a plan digest mismatch.

There is no default provider, best-effort downgrade, native escape hatch,
automatic unsigned source, or archive download to “see if it works.”

## Implementation State

Implemented:

- catalog-v3 exact planning-target contract;
- strict executable planning-bundle contract;
- TUF target-only loading and catalog binding;
- provider-neutral Runtime templates for release-backed Tool Tasks, HTTP Tool
  Services, and Streamable HTTP MCP Services;
- explicit `RuntimeClientRegistry` selection and immutable provider evidence;
- CLI component-plan transport of the verified planning bundle without package
  archive download;
- a package-lifecycle Tool/MCP adapter that revalidates immutable files, uses
  explicit selected clients, health-gates Tool Services, requires Gateway plus
  standard initialize evidence for HTTP MCP, persists bindings, and performs
  idempotent receipt-owned stop/removal; and
- fail-closed rejection while complete provider/grant lifecycle evidence is
  unavailable.

Remaining:

- host broker integration into the shared Plugin Manager's final draft;
- two-pass preflight/authorization/final selection;
- workspace-aware grant-change and package-journal coordination;
- parent-saga coordination of the implemented exact Runtime N/N+1 store and
  prior-generation retirement after capability cutover;
- production provider injection for every CLI/Web/Cloud host;
- secret, filesystem, egress, and child-process enforcement adapters;
- Gateway route and MCP initialization orchestration;
- stdio MCP supervision; and
- cross-platform lifecycle E2E.

## Consequences

Benefits:

- executable plans are reviewable without downloading the package archive;
- package content cannot choose its executor or expand authority;
- CLI Tools and HTTP Services retain their native contracts;
- provider drift is visible before side effects;
- Cloud, Desktop, and CLI can inject different providers behind one contract;
- the shared Plugin Manager remains the single lifecycle application service.

Costs:

- provider selection is a host integration requirement;
- planning performs two bounded capability checks;
- Runtime features that cannot represent a permission fail closed;
- stdio MCP needs a separate compatibility lifecycle until the Runtime
  contract grows a session primitive; and
- capability publication requires a multi-resource saga rather than a simple
  directory copy.
