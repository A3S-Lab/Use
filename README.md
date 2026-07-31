<p align="center">
  <img
    src="assets/readme/hero.svg"
    width="1200"
    alt="A3S Use — AI Native Package Manager for native tools and cognitive plugins"
  />
</p>

<p align="center">
  <strong>One trusted package lifecycle for native tools and cognitive plugins on Linux, macOS, and Windows.</strong>
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#the-a3s-package-model">Package model</a> ·
  <a href="#trust-and-lifecycle">Trust</a> ·
  <a href="#current-capabilities">Capabilities</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#roadmap">Roadmap</a>
</p>

## What is A3S Use?

**A3S Use is the AI Native Package Manager for the A3S ecosystem.** It brings
platform-native executables and agent-facing cognition into one versioned,
verifiable package lifecycle, then projects installed capabilities through
native CLI, standard MCP, and content-bound Skills.

It is an **A3S package manager**, not a replacement for `apt`, Homebrew, or
WinGet. A3S Use manages packages that participate in the A3S capability and
security model; operating-system package managers continue to own arbitrary
system software.

> [!IMPORTANT]
> **Product direction and current implementation are deliberately separated.**
> The `v0.2` line is the package-management foundation: it manages built-in
> providers and external packages with one CLI, one MCP server, and/or one
> `SKILL.md`, backed by reviewed local sources, release digests, or TUF
> registries. The generalized native package graph and the full A3S cognitive
> plugin schema described in the [roadmap](#roadmap) are target architecture,
> not shipped claims.

The user-facing entry point is `a3s use`. The standalone `a3s-use` binary is
the package engine used by the umbrella CLI and remains available for direct
automation and diagnostics.

## Why AI Native?

Traditional package managers stop after placing binaries on disk. A3S Use
treats two forms as installable products:

- A **native package** delivers target-specific executables and runtime assets.
- A **cognitive plugin** delivers A3S-defined agent contributions. The current
  adapter installs MCP and `SKILL.md`; the target schema adds agents, prompts,
  hooks, knowledge, and typed memory/context providers.

They can ship separately or under one package identity. An AI-native package
therefore describes both what runs and what an agent may discover, understand,
invoke, and reload safely.

```text
A3S package
├── native plane       executable · runtime assets · target · provenance
└── cognitive plane    MCP · Skill · agent guidance · knowledge · providers
```

Today, A3S Use already binds native execution to standard MCP and content-bound
Skill discovery. The target model extends that same package identity to the
complete set of A3S cognitive contributions without inventing a private action
protocol.

| Plane | Available in `v0.2` | Target model |
| --- | --- | --- |
| Native | Built-in providers plus a package-declared CLI and MCP executable | Target-specific artifacts, dependencies, and transactional package graphs |
| Cognitive | Standard MCP, one content-bound `SKILL.md`, and integrity-bound Activity Bar assets | Multiple Skills, agents, prompts, hooks, knowledge, and typed memory/context providers |
| Control | ACL manifest, provenance, compatibility range, receipts, route leases, and capability snapshots | Unified lock state, rollback, garbage collection, and policy-aware contribution activation |

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

## The A3S package model

The current external package contract uses one
`a3s-use-extension.acl` manifest at the package root. ACL is the
[A3S Agent Configuration Language](https://github.com/A3S-Lab/ACL), not HCL.

```text
calendar-package/
├── a3s-use-extension.acl
├── bin/
│   └── acme-calendar
└── skills/
    └── calendar/
        └── SKILL.md
```

A schema `v2` package binds identity, compatibility, repository provenance,
risk classes, and native surfaces:

```acl
extension "acme/calendar" {
  schema_version = 2
  version        = "1.4.0"
  route          = "calendar"
  requires_use   = ">=0.2.0, <0.3.0"
  actions        = ["read", "mutate"]

  repository {
    url      = "https://github.com/acme/calendar"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  cli {
    executable  = "bin/acme-calendar"
    json_output = true
  }

  mcp {
    executable = "bin/acme-calendar"
    args       = ["mcp"]
    transport  = "stdio"
  }

  skill {
    path = "skills/calendar/SKILL.md"
  }
}
```

All paths are package-relative. Installation rejects missing or non-executable
surfaces, path traversal, links, invalid archives, route collisions, oversized
packages, identity drift, and incompatible `requires_use` ranges before
activation.

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

The package ID (`acme/calendar`) is the stable lifecycle identity. The route
(`calendar`) is the human-facing invocation alias. A3S Use preserves native
`argv`, stdin, stdout, stderr, and process status; MCP remains standard MCP;
Skills remain normal `SKILL.md` packages.

See [External Repository Capabilities](docs/external-repositories.md) for the
complete manifest, archive, route, compatibility, and lifecycle contract.

## Trust and lifecycle

Every install enters through an explicit trust path:

| Source | Trust decision | Intended use |
| --- | --- | --- |
| Local directory or archive | Human review plus `--allow-unsigned` | Development and private packages |
| Release-bundled package | Exact digest in a reviewed A3S component plan | First-party packages shipped with a Use release |
| Remote registry | Pinned TUF root, expiration and rollback checks, signed target metadata, and package digest | Production distribution |

The activation path is designed so untrusted bytes do not become live routes:

```text
resolve source
    → verify metadata and digest
    → validate archive and ACL
    → stage immutable generation
    → commit receipt
    → publish capability snapshot
    → drain superseded generation
```

Key guarantees available today:

- **No source execution:** Use installs built artifacts; it does not clone a
  repository, resolve a mutable branch, or run package build scripts.
- **Bounded extraction:** archives reject links, traversal, duplicate paths,
  unsupported entries, non-portable names, and excessive expansion.
- **Immutable activation:** install and upgrade stage a unique package
  generation before atomically switching the active receipt.
- **Safe hot updates:** accepted CLI and MCP calls retain a shared lease on
  their generation; disable and uninstall hide the route before draining it.
- **Recoverable publication:** receipts are authoritative, and reconciliation
  repairs a capability snapshot missed by a crash.
- **Content-bound cognition:** projected Skill and workbench assets carry
  absolute package paths and lowercase SHA-256 digests.

### TUF registry install

Enroll a registry with a trusted root, refresh signed metadata, review the
immutable plan, and apply that exact plan:

```bash
a3s registry add https://packages.example.org/a3s/ \
  --trust-root ./root.json \
  --yes
a3s registry refresh packages

a3s --output json install use/acme/calendar --dry-run
a3s --output json install use/acme/calendar \
  --plan-digest <reviewed-plan-sha256>
```

Registry targets are selected for `darwin-arm64`, `darwin-x86_64`,
`linux-arm64`, `linux-x86_64`, `windows-x86_64`, or the portable `any` target.
The reviewed plan binds the registry identity, bootstrap root, TUF metadata
versions, channel, target, archive length, and SHA-256.

## Current capabilities

### Package engine

| Capability | Status |
| --- | --- |
| Local directories and bounded archives | Available |
| Digest-reviewed release bundles | Available |
| TUF-verified remote registries | Available |
| Target-aware remote package selection | Available |
| ACL identity, repository provenance, and SemVer host compatibility | Available |
| Native CLI delegation | Available, one surface per package |
| Standard MCP launch | Available, one surface per package |
| Content-bound Skill installation | Available, one `SKILL.md` per package |
| Atomic install, upgrade, enable, disable, drain, and uninstall | Available |
| Generation/revision capability snapshots and long-poll watch | Available |
| General dependency/conflict solver and lock graph | Target architecture |
| Automatic generation rollback and package garbage collection | Target architecture |
| Full cognitive plugin contribution schema | Target architecture |

### Built-in and external domains

| Domain | Origin | Surfaces | Ownership |
| --- | --- | --- | --- |
| Browser | Reserved built-in route | Typed Rust API, CLI, standard MCP, Skills, Dashboard | Use + [A3S Browser](https://github.com/A3S-Lab/Browser) |
| OCR | Reserved built-in route | Local PP-OCRv6 CLI, standard MCP, Skill | Use + [A3S OCR](https://github.com/A3S-Lab/OCR) |
| Box | Component-backed route | Native CLI | Umbrella A3S CLI |
| Office | External `a3s/office` package | Package-declared CLI, MCP, Skill | [A3S Office](https://github.com/A3S-Lab/Office) |
| Science | External `a3s/science` reference package | Source-specific CLI, 13 MCP tools, Skill, Activity Bar assets | Use + [A3S Science](https://github.com/A3S-Lab/Science) |
| Any external domain | Installed package | Optional CLI, MCP, and/or Skill | Package repository + Use lifecycle |

A compiled route is not proof that its provider is installed. Inspect runtime
readiness with:

```bash
a3s use doctor --json
a3s use component list --json
a3s-use capability snapshot --json
```

Resident hosts can reload packages without restarting:

```bash
a3s-use capability watch \
  --after-generation 12 \
  --after-revision <sha256> \
  --timeout-ms 30000 \
  --json
```

The generation changes on package lifecycle commits. The revision also changes
when projected provider readiness, Skill content, or workbench assets change.

## Architecture

```text
     local package          release bundle          TUF registry
           └──────────────────────┬──────────────────────┘
                                  │
                       a3s umbrella CLI
              catalog · source policy · plan review · product receipt
                                  │ reviewed package input
                                  ▼
                         a3s-use package engine
       resolve · verify · validate · stage · receipt · activate · drain
                    ┌─────────────┴─────────────┐
                    │                           │
               native plane               cognitive plane
          executable · runtime             MCP · Skill · assets
                    │                           │
          OS process boundary        capability snapshot / watch
                    │                           │
                    └─────────────┬─────────────┘
                                  ▼
                       A3S Code · Web · agents
```

The boundaries are intentional:

- **The umbrella `a3s` CLI** currently owns product discovery, top-level
  component policy, release selection, reviewed plans, and the product receipt.
- **A3S Use** owns package validation, trust provenance, immutable generations,
  activation receipts, route leases, and the unified capability projection.
- **Package processes** own their domain behavior and MCP vocabulary. Use does
  not translate them into a universal `execute(action, payload)` envelope or
  load extension code with `dlopen`.
- **A3S hosts** own sandboxing, user confirmation, permissions, and rendering.
  A projected Skill supplies guidance; it cannot expand authority.
- **Embedded consumers** can depend on typed crates directly. For example,
  A3S Search uses `Arc<dyn PageRenderer>` from `a3s-use-browser` and does not
  require the facade CLI or an MCP process.

The target package-manager architecture keeps these boundaries while expanding
the manifest from today's external capability schema into a unified native and
cognitive contribution model. One package identity should bind target
artifacts, dependency state, permissions, cognition, provenance, and lifecycle
without conflating installation authority with runtime authority.

See [Architecture](docs/architecture.md) for route leases, registry
publication, persistent sessions, component ownership, and current
implementation details.

## Platform support

| Platform | Status | Current guarantee |
| --- | --- | --- |
| macOS arm64 / x86_64 | Supported | Release archives, managed providers, extension lifecycle, and complete Browser compatibility gates |
| Linux arm64 / x86_64 | Supported | Release archives, managed providers, extension lifecycle, and complete Browser compatibility gates |
| WSL | Supported through Linux | Linux runtime and filesystem contract |
| Windows x86_64 | Preview | Release archive, facade/package contract gates, Edge core-profile evidence, and local OCR process coverage |

Windows is a first-class target in the package format and release matrix, but
it is not yet part of the complete runtime parity claim. Promotion requires
persistent Browser sessions across invocations, advanced Browser profiles, and
the same full-workspace lifecycle evidence as Linux and macOS.

## Roadmap

### Foundation — available

- [x] ACL package identity, repository provenance, and host compatibility
- [x] Reviewed local, release-bundled, and TUF-verified sources
- [x] Immutable staging, atomic receipts, route leases, drain, and hot reload
- [x] Native CLI, standard MCP, content-bound Skill, and workbench projection
- [x] Cross-platform release artifacts for Linux, macOS, and Windows x86_64

### AI Native Package Manager — next

- [ ] Promote the external extension contract into a unified A3S package
      specification for native payloads and cognitive plugins
- [ ] Add dependency and conflict resolution, a deterministic lock graph, and
      multi-package transactions
- [ ] Model multiple Skills, agents, prompts, hooks, knowledge bundles, and
      typed memory/context providers as integrity-bound contributions
- [ ] Add permission declarations and policy evaluation before cognitive
      contribution activation
- [ ] Add explicit rollback, retained-generation policy, and garbage collection
- [ ] Promote Windows after complete lifecycle and Browser parity gates pass

## Workspace

| Crate | Responsibility |
| --- | --- |
| `a3s-use` | Facade library, standalone package engine, capability projection, and MCP entry points |
| `a3s-use-core` | Shared errors, diagnostics, artifacts, and immutable MCP/Skill release descriptors |
| `a3s-use-extension` | ACL package model, source trust, TUF registry, receipts, leases, and native surface descriptors |
| `a3s-use-science` | Reference multi-surface external package |

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

Focused package-engine validation:

```bash
cargo test -p a3s-use-extension --locked
cargo test -p a3s-use --test cli --locked
```

## Documentation

- [Architecture](docs/architecture.md)
- [External Repository Capabilities](docs/external-repositories.md)
- [Immutable MCP and Skill Release Descriptors](docs/release-descriptors.md)
- [Agent Browser Compatibility Baseline](docs/agent-browser-parity.md)
- [Science Reference Package](crates/science/README.md)
- [Third-Party Notices](THIRD_PARTY_NOTICES.md)

## License

A3S Use is licensed under the [MIT License](LICENSE). Release archives include
third-party components with their original licenses and provenance notices.
