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
native CLI, standard MCP, content-bound Skills, sandboxed host surfaces, and
exact-generation knowledge evidence.

It is an **A3S package manager**, not a replacement for `apt`, Homebrew, or
WinGet. A3S Use manages packages that participate in the A3S capability and
security model; operating-system package managers continue to own arbitrary
system software.

> [!IMPORTANT]
> The `v0.2` release line is the stable package-management foundation. The
> current `main` branch also implements the dependency foundation for npm-like
> cognitive packages: schema-v3 ACL dependencies, bounded deterministic SemVer
> resolution, an exact signed package lock, dependency-forward download and
> preparation, retained shared dependencies, reverse removal, and one atomic
> capability cutover for the changed closure. Tool, MCP, Skill, UI, and OKF
> remain package-owned surfaces. Umbrella CLI/Web host composition, grant
> composition, old-generation retirement, Gateway, and the real A3S Knowledge
> backend remain incomplete, so schema-v3 graph installation is not yet a GA
> CLI workflow. [ROADMAP.md](ROADMAP.md) is the source of truth.

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
└── cognitive plane    Tool · MCP · Skill · UI · OKF · agent context
```

The package is the only lifecycle unit. A Tool, Skill, MCP server, UI, or OKF
bundle can be selected and projected as a named contribution, but it cannot be
installed, upgraded, enabled, disabled, or uninstalled independently from its
owning package generation.

| Plane | Implemented foundation | Product direction |
| --- | --- | --- |
| Native | Built-in providers, external CLI/MCP executables, immutable generations | Production provider isolation and cross-platform release parity |
| Cognitive | Named Tool Task/Service, MCP, OKF, Skill, and UI contracts on `main` | Production Knowledge indexing, then Agents, prompts, hooks, and typed memory/context providers |
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

Prebuilt archives are published on
[GitHub Releases](https://github.com/A3S-Lab/Use/releases). Keep each archive's
binary, Skills, Dashboard, model assets, licenses, and provenance files
together; the facade binary alone is not the complete product surface.

## Package model

A cognitive package is an npm-like, versioned distribution unit. It has one
stable `<publisher>/<name>` identity, one ACL manifest, one required `README.md`,
zero or more Tool, MCP, Skill, UI, and OKF contributions, and optional SemVer
dependencies on other cognitive packages. A typical package is:

```text
acme-research/
├── a3s-use-extension.acl   identity · version · dependencies · surfaces
├── README.md               required schema-v3 package documentation
├── tools/                  native Task or Service artifacts
├── releases/               content-bound Tool/MCP descriptors
├── skills/                 SKILL.md files and supporting content
├── ui/                     integrity-bound static assets
└── okf/                    conformant knowledge bundles
```

Only the manifest and `README.md` names are fixed; contribution paths are
manifest-owned. The manifest is parsed by
[A3S ACL](https://github.com/A3S-Lab/ACL), the A3S Agent Configuration Language,
not HCL.

Schema v1 and v2 remain compatible. The schema-v3 baseline adds repeatable,
named Tool, MCP, OKF, Skill, and UI surfaces with an acyclic readiness graph:

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

  skill "review" {
    path          = "skills/review/SKILL.md"
    requires_tool = ["convert"]
    requires_mcp  = ["library"]
    requires_okf  = ["domain-knowledge"]
    optional      = false
  }

  ui "review" {
    entry     = "ui/review/index.html"
    skill     = "review"
    bind_mcp  = ["library"]
    optional  = false
  }
}
```

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
package proves that one admitted generation can contain Tool, MCP, OKF, Skill,
and UI contributions with dependency-bound lifecycle ordering.
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
schema-v3 activation through the new multi-host parent saga is still being
wired; until then the new lifecycle adapters fail closed when their required
host evidence is unavailable.

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
    → prepare grants, Runtime, static surfaces, and OKF
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

Registry sources are named host configuration, not compiled endpoints. The
official `a3s` source is only the default identity: an operator may bind it to a
mirror or private service, disable it without deleting trust state, or remove
the override and return to the unconfigured default hint. Release-bundled
packages remain an independent source and do not require any remote registry.

Add a source or replace a stable source name, then review the immutable
component plan and apply its exact digest:

```bash
a3s registry add https://packages.example.org/a3s/ \
  --name packages \
  --trust-root ./root.json \
  --yes
a3s registry refresh packages

a3s registry replace a3s https://mirror.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --yes
a3s registry disable a3s
a3s registry enable a3s

a3s --output json install use/acme/calendar --dry-run
a3s --output json install use/acme/calendar \
  --plan-digest <reviewed-plan-sha256>
```

Only enabled registries participate in catalog discovery and package
selection. If two enabled registries contain the same package, resolution fails
as ambiguous instead of choosing one silently. An installed receipt continues
to bind its source name, URL, TUF root, channel, and target digest; replacing or
disabling that source therefore blocks upgrades until the original identity is
restored or the package is explicitly migrated or reinstalled.

Catalog v1 adds bounded metadata-only search. Catalog v2 binds the manifest,
expanded package, permission ceiling, and surface dependencies. Catalog v3
adds the signed package dependency inventory, exact OKF bundle evidence and,
when executable Tool or MCP surfaces are present, a separately signed bounded
planning target so review does not require downloading the package archive. A
catalog dependency inventory must exactly match the admitted ACL manifest.

## Implementation status

### Stable package foundation

| Capability | Status |
| --- | --- |
| Local directories and bounded archives | Available |
| Digest-reviewed release bundles | Available |
| TUF-verified registries and target selection | Available |
| Named registry add, replace, enable, disable, remove, and refresh | Available in the umbrella host |
| ACL identity, provenance, and SemVer host compatibility | Available |
| Digest-pinned MCP Runtime Service and immutable Skill Agent-input releases | Available with canonical cross-SDK fixtures and Linux OCI conformance |
| Native CLI, standard MCP, and content-bound Skill surfaces | Available |
| Schema-v1/v2 atomic install, upgrade, enable, disable, drain, and uninstall | Available |
| Capability snapshot and long-poll watch | Available |

### Plugin-platform baseline on `main`

| Capability | Status |
| --- | --- |
| Schema-v3 named Tool Task/Service, MCP, OKF, Skill, and UI contracts | Implemented |
| Schema-v3 ACL package dependencies and required package README | Implemented with canonical SemVer requirements and bounded validation |
| Deterministic transitive resolver and `a3s.use.plugin-package-lock.v1` | Implemented with bounded backtracking, exact Registry/TUF provenance, host binding, cycle/conflict rejection, and canonical digest |
| Dependency-closure download and package graph lifecycle | Implemented foundation: revalidate before download, install forward, retain exact published dependencies, publish changed nodes once, uninstall reverse |
| Canonical package-level graph, lifecycle intent, checkpoint journal, and crash replay | Implemented foundation; all five contribution kinds share one operation |
| Schema-v3 package commit/removal and capability publish/hide/drain hosts | P0 implemented with generation-bound receipt schema v3, deterministic roots, atomic snapshot replacement, route leases, and legacy-bypass rejection |
| First-class OKF knowledge-package control plane | M0K-B implemented: bundle validation, manifest/catalog/plan, receipt/observation/projection, dependency-gated reconciliation, and canonical fixtures |
| Signed catalog v1–v3, offline verification, search, and planning target | Implemented |
| Immutable operation-plan, permission-ceiling, and provider-evidence contracts | Implemented |
| Exact-generation workspace grant store and recoverable grant journal | Implemented |
| Runtime Tool/MCP preparation, readiness, stop, and receipt-owned removal | Implemented as typed lifecycle adapters; prior-generation retirement remains pending |
| Immutable Skill/UI validation and projection lifecycle | Implemented as typed static-surface adapters |
| Dependency-gated Surface Reconciler and planner evidence | Implemented |
| Injected OKF Knowledge port, durable binding store, and package lifecycle adapter | M0K-C-A foundation implemented with byte-exact stage validation, monotonic observations, last-good projection, and receipt-owned removal |
| Production A3S Knowledge backend and scoped cited retrieval | In progress; no production OKF publication without promoted evidence |
| Shared Manager parent saga across package, grants, Runtime, Gateway, and projection | Use intent/journal/coordinator plus package/capability hosts implemented; umbrella host and grant composition remain in progress |
| Production secret, egress, filesystem, child-process, Gateway, and stdio-MCP adapters | In progress |
| Automatic generation rollback and garbage collection | Target architecture |
| Agent, prompt, hook, memory, and context-provider contributions | Target architecture after OKF lifecycle integration |

The baseline intentionally fails closed while host-owned Runtime, permission,
and apply evidence is incomplete. The compatible `component install` command
still uses the legacy installer; it does not yet expose schema-v3 dependency
resolution or the graph coordinator. Schema-v3 packages cannot use legacy
extension toggles, and upgrade remains rejected until two retained generations
can be retired safely. No path silently falls back to native execution, a
different provider, or a different Registry.

### Built-in and external domains

| Domain | Origin | Surfaces | Ownership |
| --- | --- | --- | --- |
| Browser | Reserved built-in route | Typed Rust API, CLI, standard MCP, Skills, Dashboard | Use + [A3S Browser](https://github.com/A3S-Lab/Browser) |
| OCR | Reserved built-in route | Local PP-OCRv6 CLI, standard MCP, Skill | Use + [A3S OCR](https://github.com/A3S-Lab/OCR) |
| Box | Component-backed route | Native CLI | Umbrella A3S CLI |
| Office | External `a3s/office` package | Package-declared CLI, MCP, Skill | [A3S Office](https://github.com/A3S-Lab/Office) |
| Science | External reference packages | Tool, MCP, Skill, and UI combinations | Use + [A3S Science](https://github.com/A3S-Lab/Science) |
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
          Runtime Task/Service       MCP · Skill · UI · OKF · context
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
- **A3S hosts** own sandboxing, rendering, and OKF indexing. Skill, UI, OKF,
  Tool, and remote content are data and cannot grant authority.

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
defines the package-owned Tool/MCP/OKF/Skill/UI checkpoint schedule. Its P0
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
4. wire the implemented dependency lock and graph coordinator into the shared
   CLI/Web/management-MCP apply path, then add retained-generation rollback and
   garbage collection;
5. connect the M0K-C-A Knowledge port and lifecycle adapter to the production
   A3S Knowledge backend and scoped capability/session projection;
6. extend the remaining cognitive contribution model only after each host
   contract is frozen; and
7. complete official registry operations and Windows platform gates.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `a3s-use` | Facade, package engine, Runtime adapters, capability reconciliation, and MCP entry points |
| `a3s-use-core` | Canonical package/plugin, catalog, plan, permission, grant, release, OKF bundle, and Knowledge projection contracts |
| `a3s-use-extension` | ACL manifests, recursive OKF/package validation, TUF catalog, package store, receipts, leases, and workspace grants |
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
