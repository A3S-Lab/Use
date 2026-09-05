# ADR-003: Native A3S Code Harness and A3S Use Package Boundary

- Status: accepted architecture; implementation is staged
- Decision date: 2026-09-05
- Owners: A3S Code and A3S Use
- Related: [A3S Use Architecture](architecture.md), [A3S Use Roadmap](../ROADMAP.md), [Scoped Capability Architecture](../../code/manual/SCOPED_CAPABILITY_ARCHITECTURE.md)

## Context

A3S Code and A3S Use solve different problems. Code is the coding-agent
Harness: it owns the agent loop, model-facing context, governed tools,
workspace interaction, execution scopes, evidence, checkpoints, and
provider-local recovery. Use is the package manager: it owns package identity,
trust, dependency resolution, installation, Grants, generations, leases, and
retirement.

The two components currently have enough lifecycle vocabulary to be confused
with a generic plugin framework. That confusion would create two authorities
for package graphs, service readiness, execution loops, or recovery. It would
also make every Code release carry compatibility obligations for an unrelated
Harness implementation. A3S Code does not need to run or emulate DeepSeek
Harness (DSH), Cordis, or another foreign runtime. Concepts learned from those
systems may inform design reviews, but their APIs and semantic compatibility
are explicitly outside the product contract.

The repository also contains an independent zvec-grep retrieval integration.
Retrieval is a Code workspace data-plane concern. It must not become a hidden
package-management or capability-lifecycle authority.

## Decision

Build one native A3S Code Harness and one A3S Use package manager connected by
a narrow, versioned snapshot/lease contract. Do not add a DSH compatibility
surface, a DSH runtime bundle, a Cordis guest, or a JavaScript sidecar to Code
or Use.

The ownership boundary is:

| Boundary | Sole responsibility |
| --- | --- |
| A3S Use | Signed package/catalog input, SemVer closure, exact locks, Grants, provider-neutral plans, installation generations, capability snapshots, leases, cutover, drain, reverse retirement, and crash recovery for package state |
| A3S Code | Native Agent loop, Session/Run/Turn/Subtask scopes, model and Tool contracts, workspace/context/evidence, local policy, checkpoint serialization, and provider-local recovery |
| Runtime/Gateway/Flow/Knowledge/Sandbox/Box | Execution providers, network and workflow lifecycles, knowledge publication, command isolation, and workload isolation through their native contracts |
| Cloud | Cross-process or remote identity, authorization, durable checkpoint objects, placement, deployment, audit, and business lineage |

Use publishes an immutable, exact-generation capability snapshot. Code accepts
that snapshot as input to Session and Run admission and creates borrowed
leases; it never resolves a package, reads a package path, or asks Use for a
mutable `latest` value during execution. Code's catalog generation is a
derived identity and never aliases the Use generation.

## Native Code Harness architecture

The Code core is organized into five explicit planes. Each plane has one
source of truth and a typed seam to the next plane.

```text
Use snapshot + host policy
          │ exact admission
          ▼
Identity plane ──► Capability projection plane
      │                         │ immutable catalog + readiness DAG
      ▼                         ▼
Execution scopes ──► Provider seams ──► Evidence/time plane
Session/Run/Turn/       LLM · Workspace · Sandbox · Runtime ·
Subtask + leases        Gateway · Flow · Knowledge · UI
```

### Identity plane

The identity plane validates package, surface, provider, scope, catalog, and
generation identifiers. It stores the Use cursor, catalog digest, authority
ceiling, and source provenance as one canonical admission record. Mixed
generations, duplicate identities, and non-canonical encodings fail before an
`Arc` or provider handle can escape.

### Capability projection plane

The projection plane converts Use-owned surface descriptors into closed Code
values (`Tool`, `Mcp`, `Skill`, `Flow`, `Knowledge`, and `Ui` bindings). A
typestate contribution batch validates names, dependencies, limits, and
provider readiness, then publishes one immutable catalog by compare-and-swap.
The readiness DAG orders only published surface edges; Code does not resolve
package dependencies or provide general dependency injection.

### Execution scope plane

The native loop is structured as `Session -> Run -> Turn -> Subtask`. Child
scopes borrow their parent's capability lease and can only reduce authority
ceilings. Cancellation flows downward, effects are supervised by the owning
scope, and close is reverse-order, idempotent, and bounded. A foreground
child, stream bridge, background task, or memory operation must settle before
its owning Run releases the exact Use lease.

### Provider seam plane

Providers are injected by the host through typed contracts. Code contains no
implicit local fallback and no provider discovery by package content. LLM,
workspace/retrieval, Sandbox, Runtime Task/Service, Gateway, Flow, Knowledge,
and UI providers report versioned readiness and enforcement evidence. A
provider may be replaced only through a new validated capability generation.

The zvec-grep integration remains behind the workspace retrieval provider
seam. It contributes indexed data and query evidence, not package identity,
authority, or lifecycle behavior.

### Evidence and time plane

Every model input, Tool request/result, capability binding, usage record,
checkpoint, and recovery decision is bound to the Run journal's logical time
and exact catalog identity. Evidence is bounded and digest-addressed; raw
secrets and unrestricted package paths never enter the portable contract.
Checkpoint import pins the complete historical Code catalog and Use cursor
before target-Run admission. Cloud may add external revision fencing, but
Code does not create a second business checkpoint authority.

## Use package-manager architecture

Use remains a control plane, not an agent loop or runtime framework. Its
transaction is:

```text
discover verified catalog
→ resolve SemVer closure and freeze lock
→ review immutable plan
→ preflight host providers and Grants
→ prepare package/surface effects dependency-forward
→ publish one capability generation
→ lease and drain prior generation
→ retire effects and bytes dependency-reverse
→ persist exact terminal/recovery evidence
```

The package store and transactional Control Store are authoritative. Provider
projections, Code catalogs, launchers, and UI assets are derived and receipt-
owned. A failed prepare leaves the current generation untouched; a failed
post-cutover operation completes retirement or rolls back only according to
the recorded checkpoint. No operation infers state from wall-clock time or a
second journal.

Use package surfaces stay deliberately small and native:

| Surface | Host owner | Code relationship |
| --- | --- | --- |
| Tool Task/Service | Runtime | Code receives a typed invocation binding and exact readiness evidence |
| MCP | Runtime/Gateway | Code uses the standard MCP contract through a generation-bound client |
| OKF Knowledge | Knowledge | Code receives a digest-bound knowledge binding; publication remains outside the agent loop |
| A3S Flow | Flow | Code receives an immutable workflow binding and host-owned engine |
| Skill | Code/host | Code composes a Skill as a bounded Subtask/Turn contribution |
| UI | Desktop/Web host | Code receives only reviewed, integrity-bound presentation metadata |

There is intentionally no DSH or generic “plugin runtime” surface. A package
may expose more than one of the six native surfaces, but every surface still
belongs to one package generation and one host provider.

## Code–Use contract

The contract is additive to the existing Use snapshot and lease records. The
next implementation slice should publish a small, generated declaration for
the Code projection:

```text
a3s.use.capability-snapshot-cursor.v1       # existing Use authority
a3s.use.runtime-task-binding.v4             # existing Runtime binding
a3s.use.runtime-service-binding.v3          # existing Runtime binding
a3s.code.use-capability-projection.v1       # Code's normalized projection
```

`a3s.code.use-capability-projection.v1` contains only canonical, path-free
identities, surface descriptors, provider evidence, dependency edges, scope
limits, Use cursor/digest, and the Code catalog digest. It excludes package
source, credentials, executable paths, arbitrary JSON service lookup, and
foreign runtime ABI details. The projection is accepted atomically or not at
all.

Admission and execution rules are invariant:

1. Use verifies and leases generation `N` before Code admits a Session or Run.
2. Code records `N` and its derived catalog digest before the first provider
   call.
3. A cutover to `N+1` affects new admissions only; existing Runs retain `N`.
4. A stale, revoked, mixed, or digest-mismatched call fails closed.
5. Code releases the lease after all Run-owned effects settle; Use then drains
   and retires `N` in reverse dependency order.
6. Recovery rehydrates the exact historical projection. It never substitutes
   `latest`, silently downgrades a surface, or reconstructs a missing Grant.

## Delivery roadmap

The roadmap is intentionally native and release-oriented. DSH compatibility
work is deleted from the plan rather than deferred behind a feature flag.

| Phase | Outcome | Exit evidence |
| --- | --- | --- |
| C0 Contract freeze | Publish this boundary and remove foreign-runtime terminology from active Code/Use plans | Architecture review, ownership table, non-goals, and generated contract names agree |
| C1 Code core hardening | Finish `CAR-01`, `CAR-03`, `CAR-04`, and `CAR-05`; keep Session/Run/Turn/Subtask and evidence paths single-source | Focused Rust tests, strict Clippy/rustdoc, checkpoint/restart/cancellation suites, Cloud/Box qualification |
| C2 Capability kernel GA | Close `CAP-GA1`; remove legacy shadow registries and piecemeal reconciliation | Official hosts and Rust/Node/Python/Go SDKs use atomic projections and exact Use leases |
| C3 Provider seams | Normalize LLM, Workspace/retrieval, Sandbox, Runtime, Gateway, Flow, Knowledge, and UI adapters | Provider conformance fixtures, no hidden fallback, readiness and enforcement evidence at admission |
| C4 Temporal recovery | Make checkpoint export/import, generation cutover, lease drain, and crash takeover deterministic | Kill/restart/fault-injection matrix, external CAS/fencing through Cloud, zero residual tasks/files/leases |
| C5 Resource and observability | Add bounded budgets, backpressure, logical-time telemetry, and long-horizon leak tests | p95/p99 SLO reports, memory/file/socket ceilings, cancellation and overload evidence |
| U1 Use scoped authority | Complete A1 scoped Installation and A2 transactional Control Store | User/Workspace isolation, one serial mutation order, exact plan/receipt/recovery replay |
| U2 Use registry and hosts | Qualify signed Registry, provider preflight, Capability MCP Gateway, and manager APIs | Offline install, cross-platform supply-chain checks, host/provider parity, release runbook |
| U3 Package ecosystem | Publish reviewed Tool/MCP/OKF/Flow/Skill/UI packages and SDK helpers | Reproducible artifacts, dependency closure, upgrade/rollback/drain matrix, security review |

The dependency order is:

```text
C0 → C1 → C2 → C3 → C4 → C5
          ╲              ╱
           U1 → U2 → U3
```

Code and Use may develop in parallel after C0, but neither may introduce a
second authority to unblock the other. Retrieval benchmark work may proceed
inside C3/C5 without changing the package or capability contracts.

## Non-goals

- DSH/Cordis API compatibility, differential compatibility tests, or a DSH
  runtime/sidecar/guest process.
- A generic dependency-injection, event-bus, HMR, or JavaScript plugin kernel
  in Code.
- Making A3S Use an Agent Loop, model context manager, or execution scheduler.
- Allowing packages to select providers, read local package paths, receive
  credentials, or mutate the current generation directly.
- Turning retrieval, Flow, Knowledge, or MCP into alternate Code authorities.
- Preserving superseded preview schemas or adding compatibility fallbacks to
  the current Use contract line.

## Verification and release evidence

Every phase must provide machine-readable evidence for identity, generation,
authorization, readiness, logical time, cleanup, and recovery. The minimum
release matrix covers:

- Rust format, Clippy, unit/integration tests, and generated SDK parity;
- exact Use install/upgrade/enable/disable/uninstall and crash replay;
- Code model/tool/stream/checkpoint/recovery and provider-conformance tests;
- Linux, macOS, and Windows with offline and constrained-resource profiles;
- Cloud compatibility-lock and Box real-workload qualification; and
- no mixed generations, stale invocations, leaked leases, residual files, or
  unowned provider effects after close.

