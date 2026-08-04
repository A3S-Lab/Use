<p align="center">
  <img
    src="assets/readme/hero.svg"
    width="100%"
    alt="A3S Use — AI Native Package Manager for native tools and cognitive plugins"
  />
</p>

<p align="center">
  <strong>One trusted package lifecycle for native tools and cognitive plugins on Linux, macOS, and Windows.</strong>
</p>

<p align="center">
  <a href="https://a3s-lab.github.io/Use/">Website</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#package-model">Package model</a> ·
  <a href="#trust-and-lifecycle">Trust</a> ·
  <a href="#implementation-status">Status</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#roadmap">Roadmap</a>
</p>

## What is A3S Use?

**A3S Use is the AI Native Package Manager for the A3S ecosystem.** It brings
platform-native executables and agent-facing cognition into one versioned,
verifiable package lifecycle, then projects installed capabilities through
native CLI, standard MCP, durable A3S Flow workflows, content-bound Skills,
sandboxed host surfaces, and exact-generation knowledge evidence.

It is an **A3S package manager**, not a replacement for `apt`, Homebrew, or
WinGet. A3S Use manages packages that participate in the A3S capability and
security model; operating-system package managers continue to own arbitrary
system software.

> [!IMPORTANT]
> The `v0.2` release line is the stable package-management foundation. The
> current `main` branch is the `v0.3` cognitive-package line. Signed remote
> schema-v3 packages now use the dependency graph from both `a3s-use install`
> and the compatible `component install` entry point: deterministic SemVer
> resolution, an exact Registry/TUF-bound lock, dependency-forward install,
> shared dependency retention, atomic publication, reverse uninstall, and
> crash replay are implemented. The standalone host supports executable Tool
> Tasks, stdio MCP, Skill, and UI; Runtime Service, HTTP MCP, and OKF packages
> require explicit Runtime/Gateway or A3S Knowledge adapters from an embedding
> host. A3S Flow is now a first-class package surface with typed lifecycle and
> catalog evidence; production compilation and execution require an injected
> `a3s-flow` host adapter. Grant composition, prior-generation retirement, and
> the complete Code/Web production E2E remain release gates.
> [ROADMAP.md](ROADMAP.md) is the source of truth.

The user-facing entry point is `a3s use`. The standalone `a3s-use` binary is
the delegated package engine and remains available for automation and
diagnostics. The umbrella host owns registry configuration, policy, user
confirmation, and Runtime provider composition; it must call the one shared
Plugin Manager rather than create a second lifecycle implementation.

## Why AI Native?

Traditional package managers stop after placing binaries on disk. A3S Use
treats two forms as installable products:

- A **native package** delivers target-specific executables and runtime assets.
- A **cognitive plugin** delivers A3S-defined agent contributions and the
  dependency, permission, and readiness evidence required to use them safely.

They may ship separately or under one immutable package identity:

```text
A3S package
├── native plane       executable · runtime assets · target · provenance
└── cognitive plane    Tool · MCP · OKF · Flow · Skill · UI · agent context
```

The package is the only lifecycle unit. A Tool, MCP server, OKF bundle, Flow,
Skill, or UI can be selected and projected as a named contribution, but it
cannot be installed, upgraded, enabled, disabled, or uninstalled independently
from its owning package generation.

| Plane | Implemented foundation | Product direction |
| --- | --- | --- |
| Native | Built-in providers, external CLI/MCP executables, immutable generations | Production provider isolation and cross-platform release parity |
| Cognitive | Named Tool Task/Service, MCP, OKF, A3S Flow, Skill, and UI contracts on `main` | Production Flow/Knowledge adapters, then Agents, prompts, hooks, and typed memory/context providers |
| Control | TUF provenance, SemVer dependency resolution, exact lock graphs, plan digests, receipts, grants, route leases, and atomic capability snapshots | Complete umbrella apply saga, rollback, garbage collection, and policy-aware activation |

## Quick start

Install the verified A3S Use release through the umbrella CLI:

```bash
a3s install use --source release
a3s use doctor --json
a3s use capabilities --json
```

Try the built-in Browser and local OCR capabilities:

```bash
a3s use browser render https://example.com --output page.html
a3s use ocr extract ./scan.png --json
```

Build the standalone engine from source:

```bash
git clone https://github.com/A3S-Lab/Use.git
cd Use
cargo build --workspace --bins --locked
./target/debug/a3s-use doctor --json
```

Install one signed cognitive package and its complete dependency closure from
a host-selected Registry:

```bash
a3s-use install acme/research \
  --registry-name packages \
  --registry-url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --version 2.0.0 \
  --json

a3s-use uninstall acme/research --json
```

For a separately reviewed resolution, add
`--package-lock-digest sha256:<64-hex-digits>`. A mismatch fails before any
package archive is downloaded.

Prebuilt archives are published on
[GitHub Releases](https://github.com/A3S-Lab/Use/releases). Keep each archive's
binary, Skills, Dashboard, model assets, licenses, and provenance files
together; the facade binary alone is not the complete product surface.

## Package model

A cognitive package is an npm-like, versioned distribution unit. It has one
stable `<publisher>/<name>` identity, one ACL manifest, one required `README.md`,
zero or more Tool, MCP, OKF, Flow, Skill, and UI contributions, and optional SemVer
dependencies on other cognitive packages. A typical package is:

```text
acme-research/
├── a3s-use-extension.acl   identity · version · dependencies · surfaces
├── README.md               required schema-v3 package documentation
├── tools/                  native Task or Service artifacts
├── releases/               content-bound Tool/MCP descriptors
├── flows/                  A3S Flow TypeScript workflow sources
├── skills/                 SKILL.md files and supporting content
├── ui/                     integrity-bound static assets
└── okf/                    conformant knowledge bundles
```

Only the manifest and `README.md` names are fixed; contribution paths are
manifest-owned. The manifest is parsed by
[A3S ACL](https://github.com/A3S-Lab/ACL), the A3S Agent Configuration Language,
not HCL.

Schema v1 and v2 remain compatible. The schema-v3 baseline adds repeatable,
named Tool, MCP, OKF, Flow, Skill, and UI surfaces with an acyclic readiness
graph:

```acl
extension "acme/research" {
  schema_version = 3
  version        = "2.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  dependency "acme/base" {
    version = "^1.4.0"
  }

  dependency "acme/vector-store" {
    version = ">=2.1.0, <3.0.0"
  }

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    executable  = "tools/convert/bin/convert"
    command     = "acme-research-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }

  mcp "library" {
    transport  = "streamable-http"
    release    = "releases/library-mcp-v1.json"
    activation = "eager"
    optional   = false
  }

  okf "domain-knowledge" {
    format_version         = "0.2"
    root                   = "okf/domain-knowledge"
    content_digest         = "sha256:bd85b0b63adb32bdf616384a619286af4c32401542655dd09e00450902ab478d"
    concept_count          = 4
    file_count             = 7
    expanded_bytes         = 2053
    max_files              = 256
    max_concepts           = 64
    max_expanded_bytes     = 67108864
    max_document_bytes     = 1048576
    max_links_per_document = 2048
    optional               = false
  }

  flow "review" {
    engine         = "a3s-flow"
    runtime        = "native-ts"
    source         = "flows/review.ts"
    export         = "run"
    requires_tool  = ["convert"]
    requires_mcp   = ["library"]
    requires_okf   = ["domain-knowledge"]
    optional       = false
  }

  skill "review" {
    path          = "skills/review/SKILL.md"
    requires_tool = ["convert"]
    requires_mcp  = ["library"]
    requires_okf  = ["domain-knowledge"]
    requires_flow = ["review"]
    optional      = false
  }

  ui "review" {
    entry     = "ui/review/index.html"
    skill     = "review"
    bind_mcp  = ["library"]
    bind_flow = ["review"]
    optional  = false
  }
}
```

### One A3S Flow model

A cognitive-package Flow is an `a3s-flow` workflow, not a second workflow
engine. The names below describe layers of one model rather than competing
mechanisms:

- **`a3s-use-extension.acl`** declares the packaged Flow identity, source,
  capability edges, and lifecycle metadata.
- **`flow.json`** stores A3S Code's visual design/deployment document for that
  same Flow identity.
- **`flows/*.ts`** supplies code-authored workflow and step handlers.
- **`native-ts`** adapts that TypeScript source to the A3S Flow runtime
  protocol.
- **`a3s-flow`** remains the only workflow engine: preflight, durable
  execution, event history, replay, scheduling, and observation.
- **A3S Code, Web, and OS** host or deploy the admitted Flow; they do not create
  another package journal or engine.

`engine = "a3s-flow"` is therefore fixed by the schema. Use validates and
content-binds the source, freezes its Tool/MCP/OKF capability edges, and
coordinates atomic prepare/publish/hide/remove. Source integrity alone never
marks a Flow ready: publication remains pending until the typed A3S Flow host
supplies successful preflight evidence for the same admitted package
generation. A typed import/deployment adapter must map `flow.json` to that
identity rather than creating another lifecycle.

### Package dependencies and lock graph

Dependency declarations contain only a canonical package ID and SemVer
requirement. They cannot select a URL, Registry, trust root, target, or mutable
tag. The host resolves the complete transitive closure from its enabled named
Registries and fails closed on missing releases, incompatible constraints,
cycles, search bounds, or the same dependency appearing in more than one
Registry.

The canonical `a3s.use.plugin-package-lock.v1` result freezes, for every node:

- the selected version and every satisfied dependency edge;
- archive, expanded-package, and manifest digests;
- host target and A3S Use compatibility;
- Registry name and URL, channel and target; and
- TUF root identity plus timestamp, snapshot, and targets versions.

The operation plan and apply request bind the exact lock digest. Apply
revalidates every Registry before downloading any archive, installs and
prepares dependencies before dependents, reuses only exact already-published
retained dependencies, and publishes all changed packages through one snapshot
cutover. Uninstall proceeds in reverse order and refuses to remove a package
while an installed dependent still requires it.

Registry selection is injected by the host. The public
`CognitivePackageManager` accepts one root Registry plus a bounded set of
dependency Registries; every resolved node records its own exact source and
TUF provenance. The standalone command accepts an explicit replaceable root
Registry, while umbrella Code/Web hosts can inject their enabled named source
set and lifecycle adapters without forking the resolver or package journal.

Schema v3 now accepts
**[OKF (Open Knowledge Format)](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)**
as a first-class, non-executable cognitive surface. The contract targets OKF
v0.2 with an explicit v0.1 compatibility path. An OKF contribution is a
shareable graph of UTF-8 Markdown concepts with YAML frontmatter; every
non-reserved concept requires a non-empty `type`, its bundle-relative path is
its identity, and standard Markdown links form the graph.

M0K-B implements the named manifest block, recursive package validation,
catalog-v3 bundle evidence, Skill → OKF dependency closure, plan-v2 impact,
projection receipts, scope-bound Knowledge observations, capability
projections, and last-good-aware reconciliation. M0K-C-A adds a public injected
`OkfKnowledgeAdapter` for stage/promote/observe/remove, revalidates the exact
in-memory OKF bytes at stage, checks all returned evidence, and persists
bounded exact-generation records under `bindings/knowledge`. Failed candidates
retain an exact promoted last-good generation; a latest removed generation
suppresses fallback. OKF never enters Runtime and cannot carry a runtime
permission ceiling. `OkfKnowledgeLifecycleHost` now supplies package-saga
stage/promote/hide/remove behavior and receipt-owned cleanup semantics.
Production A3S Knowledge indexing, umbrella-manager wiring, session projection,
and cited retrieval remain pending.

The executable and knowledge schema-v3 fixtures are
[`plugin-v3.acl`](crates/extension/fixtures/manifests/plugin-v3.acl) and
[`plugin-v3-okf.acl`](crates/extension/fixtures/manifests/plugin-v3-okf.acl).
The content-addressed
[`plugin-v3-cognitive`](crates/extension/fixtures/packages/plugin-v3-cognitive/)
package proves that one admitted generation can contain Tool, MCP, OKF, Flow,
Skill, and UI contributions with dependency-bound lifecycle ordering.
All paths are package-relative. Installation rejects missing or invalid
surfaces, path traversal, links, archive ambiguity, route collisions,
oversized packages, provenance drift, incompatible host ranges, or OKF bytes
that differ from the declared bundle contract before activation.

### Local package workflow

An unsigned local directory or `.tar.gz`, `.tgz`, or `.zip` archive requires
explicit trust:

```bash
a3s-use component install acme/calendar \
  --from ./calendar-package \
  --allow-unsigned \
  --json

a3s-use component status calendar --json
a3s-use calendar events list --json
a3s-use mcp serve calendar

a3s-use extension disable acme/calendar --json
a3s-use extension enable acme/calendar --json
a3s-use component uninstall calendar --json
```

These commands remain the compatible package-engine entry points. Production
schema-v3 local-package admission and multi-host parent-saga composition are
still being wired. Signed remote schema-v3 packages use the graph manager;
required Runtime Service, HTTP MCP, or OKF surfaces fail before publication
when their owning host adapter is unavailable. Required Flow surfaces likewise
fail before mutation unless the embedding host injects an `a3s-flow` adapter.

The package ID is the stable lifecycle identity. A route is a presentation and
dispatch alias. A3S Use preserves native `argv`, stdin, stdout, stderr, process
status, and standard MCP; it does not add a generic action envelope.

See [External Repository Capabilities](docs/external-repositories.md) for the
stable v2 package contract and
[Plugin Platform Architecture](docs/plugin-platform-architecture.md) for
schema-v3 surface semantics.

## Trust and lifecycle

Every install enters through an explicit trust path:

| Source | Trust decision | Intended use |
| --- | --- | --- |
| Local directory or archive | Human review plus `--allow-unsigned` | Development and private packages |
| Release-bundled package | Exact digest in a reviewed A3S component plan | First-party release content |
| Remote registry | Pinned TUF root, signed metadata, rollback checks, target digest | Production distribution |

The activation path keeps untrusted bytes away from live routes:

```text
resolve signed dependency graph
    → freeze exact package lock
    → build and review immutable plan
    → revalidate every Registry before payload download
    → verify and stage dependencies before dependents
    → prepare grants, Runtime, A3S Flow, static surfaces, and OKF
    → publish the changed closure in one capability generation
    → hide and drain superseded generation
    → remove unneeded packages in reverse dependency order
```

Available guarantees include bounded extraction, content-bound packages,
atomic receipts, route leases, crash reconciliation, TUF expiration and
rollback protection, and generation/revision capability snapshots. The
schema-v3 baseline additionally keeps package installation, workspace
authorization, Runtime binding, and surface readiness as separate evidence.

### TUF registry install

Registry endpoints and trust roots are host-injected, not compiled into the
package engine. `CognitivePackageManager` can therefore resolve the same
package format from a mirror, a private service, or another explicitly trusted
TUF Registry without changing the resolver or lifecycle journal.

The current umbrella CLI supports adding, removing, and refreshing additional
trusted sources. Stable-name replacement plus enable/disable controls,
including a production override for the unconfigured official `a3s` hint, are
still an integration item and are not presented here as shipped commands.
Release-bundled packages remain an independent source and do not require a
remote Registry.

Add and verify a source, then review the immutable component plan and apply its
exact digest:

```bash
a3s registry add https://packages.example.org/a3s/ \
  --trust-root ./root.json \
  --yes
a3s registry refresh packages

a3s --output json install use/acme/calendar --dry-run
a3s --output json install use/acme/calendar \
  --plan-digest <reviewed-plan-sha256>
```

Only configured registries participate in catalog discovery and package
selection. If two configured registries contain the same package, resolution
fails as ambiguous instead of choosing one silently. An installed receipt
continues to bind its source name, URL, TUF root, channel, and target digest;
changing or removing that source therefore blocks upgrades until the original
identity is restored or the package is explicitly migrated or reinstalled.

Catalog v1 adds bounded metadata-only search. Catalog v2 binds the manifest,
expanded package, permission ceiling, and surface dependencies. Catalog v3
adds the signed package dependency inventory, exact OKF bundle evidence, and
the complete Flow inventory/dependency graph. When executable Tool or MCP
surfaces are present, it also carries a separately signed bounded planning
target so review does not require downloading the package archive. Catalog
package and surface graphs must exactly match the admitted ACL manifest.

## Implementation status

### Stable package foundation

| Capability | Status |
| --- | --- |
| Local directories and bounded archives | Available |
| Digest-reviewed release bundles | Available |
| TUF-verified registries and target selection | Available |
| Host-injected Registry URL and trust root | Available in the package engine; no Registry endpoint is compiled in |
| Named registry add, remove, and refresh | Available in the umbrella host |
| Stable-name Registry replace, enable, and disable | Planned umbrella CLI integration |
| ACL identity, provenance, and SemVer host compatibility | Available |
| Digest-pinned MCP Runtime Service and immutable Skill Agent-input releases | Available with canonical cross-SDK fixtures and Linux OCI conformance |
| Native CLI, standard MCP, and content-bound Skill surfaces | Available |
| Schema-v1/v2 atomic install, upgrade, enable, disable, drain, and uninstall | Available |
| Capability snapshot and long-poll watch | Available |

### Plugin-platform baseline on `main`

| Capability | Status |
| --- | --- |
| Schema-v3 named Tool Task/Service, MCP, OKF, A3S Flow, Skill, and UI contracts | Implemented |
| Schema-v3 ACL package dependencies and required package README | Implemented with canonical SemVer requirements and bounded validation |
| Deterministic transitive resolver and `a3s.use.plugin-package-lock.v1` | Implemented with bounded backtracking, exact Registry/TUF provenance, host binding, cycle/conflict rejection, and canonical digest |
| Dependency-closure download and package graph lifecycle | Available for signed remote schema-v3 CLI installs: revalidate before download, install forward, retain exact published dependencies, publish changed nodes once, uninstall reverse |
| Canonical package-level graph, lifecycle intent, checkpoint journal, and crash replay | Implemented with durable plan-admission evidence, published-install convergence, and pending-only reverse-uninstall recovery |
| Schema-v3 package commit/removal and capability publish/hide/drain hosts | P0 implemented with generation-bound receipt schema v3, deterministic roots, atomic snapshot replacement, route leases, and legacy-bypass rejection |
| First-class OKF knowledge-package control plane | M0K-B implemented: bundle validation, manifest/catalog/plan, receipt/observation/projection, dependency-gated reconciliation, and canonical fixtures |
| Signed catalog v1–v3, offline verification, search, and planning target | Implemented |
| Immutable operation-plan, permission-ceiling, and provider-evidence contracts | Implemented |
| Exact-generation workspace grant store and recoverable grant journal | Implemented |
| Runtime Tool/MCP preparation, readiness, stop, and receipt-owned removal | Implemented as typed lifecycle adapters; prior-generation retirement remains pending |
| A3S Flow package surface, source integrity, lifecycle, and typed capability catalog | Control-plane contract implemented with the fixed `a3s-flow` engine and `native-ts` adapter; source-only readiness is withheld and the production compiler/execution host remains pending |
| Immutable Skill/UI validation and projection lifecycle | Implemented as typed static-surface adapters |
| Dependency-gated Surface Reconciler and planner evidence | Implemented |
| Injected OKF Knowledge port, durable binding store, and package lifecycle adapter | M0K-C-A foundation implemented with byte-exact stage validation, monotonic observations, last-good projection, and receipt-owned removal |
| Production A3S Knowledge backend and scoped cited retrieval | In progress; no production OKF publication without promoted evidence |
| Shared Manager parent saga across package, grants, Runtime, Gateway, and projection | Public `CognitivePackageLifecycleFactory` injection plus Use intent/journal/coordinator and package/capability hosts implemented; umbrella host and grant composition remain in progress |
| Production secret, egress, filesystem, child-process, Gateway, and stdio-MCP adapters | In progress |
| Automatic generation rollback and garbage collection | Target architecture |
| Agent, prompt, hook, memory, and context-provider contributions | Target architecture after OKF lifecycle integration |

The baseline intentionally fails closed while required host-owned Runtime,
Gateway, Knowledge, permission, or apply evidence is incomplete. Both
`a3s-use install` and compatible remote `component install` dispatch signed
schema-v3 records to the same graph manager; schema-v1/v2 and local package
paths remain compatible. Schema-v3 packages cannot use legacy extension
toggles, and upgrade remains rejected until two retained generations can be
retired safely. No path silently falls back to native execution, a different
provider, or a different Registry.

### Built-in and external domains

| Domain | Origin | Surfaces | Ownership |
| --- | --- | --- | --- |
| Browser | Reserved built-in route | Typed Rust API, CLI, standard MCP, Skills, Dashboard | Use + [A3S Browser](https://github.com/A3S-Lab/Browser) |
| OCR | Reserved built-in route | Local PP-OCRv6 CLI, standard MCP, Skill | Use + [A3S OCR](https://github.com/A3S-Lab/OCR) |
| Box | Component-backed route | Native CLI | Umbrella A3S CLI |
| Office | External `a3s/office` package | Package-declared CLI, MCP, Skill | [A3S Office](https://github.com/A3S-Lab/Office) |
| Science | External reference packages | Tool, MCP, Skill, and UI combinations | Use + [A3S Science](https://github.com/A3S-Lab/Science) |
| Cognitive fixture | Content-addressed schema-v3 package | Tool, MCP, OKF, Flow, Skill, and UI in one generation | Use contract and lifecycle tests |
| Any external domain | Installed package | Declared package surfaces | Package repository + Use lifecycle |

Inspect runtime readiness and immutable projection evidence with:

```bash
a3s use doctor --json
a3s use component list --json
a3s-use capability snapshot --json
```

Resident hosts can wait for package or content changes without restarting:

```bash
a3s-use capability watch \
  --after-generation 12 \
  --after-revision <sha256> \
  --timeout-ms 30000 \
  --json
```

## Architecture

```text
     local package          release bundle     named TUF registries
           └──────────────────────┬───────────────────────┘
                                  │
                    shared host Plugin Manager
       catalog · resolve · lock · policy · plan/apply · replay
                                  │ exact reviewed closure
                                  ▼
                         a3s-use package engine
    verify · install forward · bind · publish once · remove reverse
                    ┌─────────────┴─────────────┐
                    │                           │
               native plane               cognitive plane
          Runtime Task/Service       MCP · OKF · Flow · Skill · UI · context
                    │                           │
                    └─────────────┬─────────────┘
                                  ▼
                 A3S Code · Web · Knowledge · agents
```

The boundaries are intentional:

- **The umbrella host** owns named, replaceable registry configuration, trust
  roots, enabled state, ACL policy, confirmation, secrets, and explicit Runtime
  provider composition.
- **The shared Plugin Manager** is the only lifecycle application service used
  by CLI, Web, management MCP, and remote managed-host adapters.
- **A3S Use** owns package validation, immutable generations, receipts, grants,
  Runtime binding evidence, route leases, and capability reconciliation.
- **Package processes** own their CLI, HTTP, and MCP vocabulary. Use does not
  translate them into `execute(plugin, action, payload)` or load them through
  `dlopen`.
- **A3S hosts** own sandboxing, rendering, A3S Flow execution, and OKF
  indexing. Skill, UI, OKF, Flow source, Tool output, and remote content are
  data and cannot grant authority.

`a3s-use-core` publishes one versioned `PluginHostManager` port for remote
managed workspaces. Its distinct plan, digest-only apply, enablement, and
observation contracts reuse the canonical catalog, plan, confirmation, and
Surface Reconciler state types. Every request binds the exact host capability
digest and one host-derived `PluginManagedScope` fence. A managed scope has one
mutation authority: local CLI, Web, or management MCP adapters cannot race a
remote host adapter, while standalone scopes never become managed implicitly.
The port carries no filesystem path, executable, provider, public endpoint,
Secret value, generic action payload, package installer, or second lifecycle
state.

The host-owned Runtime Broker boundary and no-provider-fallback rule are
frozen in
[ADR-001](docs/adr-001-plugin-runtime-broker-boundary.md). The complete
multi-resource mutation is a durable, idempotent saga because package storage,
grants, Runtime, Gateway, and capability publication do not share a database
transaction. [ADR-002](docs/adr-002-cognitive-package-lifecycle-saga.md)
defines the package-owned Tool/MCP/OKF/Flow/Skill/UI checkpoint schedule. Its P0
package/capability hosts are implemented; host composition remains the next
production-wiring boundary.

## Platform support

| Platform | Status | Current guarantee |
| --- | --- | --- |
| macOS arm64 / x86_64 | Supported | Release archives, managed providers, package lifecycle, and complete Browser gates |
| Linux arm64 / x86_64 | Supported | Release archives, managed providers, package lifecycle, and complete Browser gates |
| WSL | Supported through Linux | Linux runtime and filesystem contract |
| Windows x86_64 | Preview | Release archive, facade/package contract gates, Edge core profile, and local OCR process coverage |

Windows is a package target and release-matrix member, but it is not yet part
of the complete runtime parity claim. Promotion requires the full plugin
lifecycle evidence plus the remaining persistent and advanced Browser gates.

## Roadmap

[ROADMAP.md](ROADMAP.md) is the dependency-ordered source of truth for the
plugin platform. The current critical path is:

1. have the umbrella Plugin Manager compose the implemented package,
   capability, Runtime, Gateway, static-surface, and Knowledge hosts into one
   coordinator;
2. join the workspace-grant sub-saga to package checkpoints and capability
   cutover, then wire CLI, Web, management MCP, and managed-host adapters to
   that single journaled mutation path;
3. add prior-generation retirement after blue/green capability cutover, then
   complete CLI/Web/agent lifecycle and crash-recovery E2E;
4. route Code Web, TUI, management MCP, and managed-host apply through the
   public lifecycle factory and the same persisted package graph, then add
   retained-generation rollback and garbage collection;
5. connect the M0K-C-A Knowledge port and lifecycle adapter to the production
   A3S Knowledge backend and scoped capability/session projection;
6. inject the production `a3s-flow` compiler/runtime adapter and make Code's
   `flow.json` import/deployment path resolve to the same installed Flow
   identity;
7. extend the remaining cognitive contribution model only after each host
   contract is frozen; and
8. complete official registry operations and Windows platform gates.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `a3s-use` | Facade, package engine, Runtime adapters, capability reconciliation, and MCP entry points |
| `a3s-use-core` | Canonical package/plugin, catalog, Flow graph, plan, permission, grant, release, OKF bundle, and Knowledge projection contracts |
| `a3s-use-extension` | ACL manifests, Flow source and recursive OKF/package validation, TUF catalog, package store, receipts, leases, and workspace grants |
| `a3s-use-mcp-release-fixture` | Non-published headless MCP process and digest-pinned OCI lifecycle conformance gate |
| `a3s-use-science` | Reference external package implementation |

Browser and OCR are maintained in independent repositories and pinned to exact
revisions for release assembly.

## Development

Run checks from this repository, not from the parent A3S monorepo:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

Focused package and plugin validation:

```bash
cargo test -p a3s-use-core --locked
cargo test -p a3s-use-extension --locked
cargo test -p a3s-use-mcp-release-fixture --locked
cargo test -p a3s-use --lib --bin a3s-use --locked
```

On x86_64 Linux with Docker and `musl-tools`, run the real release gate:

```bash
./scripts/mcp-release-container-conformance.sh
```

It builds a static non-root `scratch` image, pushes it to an ephemeral local
Registry, verifies the returned OCI manifest bytes, and renders
`a3s.use.mcp-release.v1` against their media type, digest, and exact size. It
runs that exact digest through health, MCP initialization,
`tools/list`, request, bounded SIGTERM shutdown, cleanup, and restart twice. No
image tag is used as release identity.

## Documentation

- [Official website](https://a3s-lab.github.io/Use/)
- [Plugin Platform Roadmap](ROADMAP.md)
- [Plugin Platform Architecture](docs/plugin-platform-architecture.md)
- [Plugin Lifecycle and Security](docs/plugin-platform-lifecycle-and-security.md)
- [Plugin Contract Reference](docs/plugin-contracts.md)
- [Immutable MCP, Skill, and Tool Releases](docs/release-descriptors.md)
- [Runtime Broker ADR](docs/adr-001-plugin-runtime-broker-boundary.md)
- [Cognitive Package Lifecycle Saga ADR](docs/adr-002-cognitive-package-lifecycle-saga.md)
- [Current Architecture](docs/architecture.md)
- [External Repository Capabilities](docs/external-repositories.md)
- [Immutable Release Descriptors](docs/release-descriptors.md)
- [Agent Browser Compatibility Baseline](docs/agent-browser-parity.md)
- [Third-Party Notices](THIRD_PARTY_NOTICES.md)

## License

A3S Use is licensed under the [MIT License](LICENSE). Release archives include
third-party components with their original licenses and provenance notices.
