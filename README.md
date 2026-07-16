# A3S Use

<p align="center">
  <strong>Typed Application Capabilities for A3S</strong>
</p>

<p align="center">
  <em>Use browsers, Office documents, and independently shipped application domains through native CLI, standard MCP, and Skills</em>
</p>

<p align="center">
  <a href="#overview">Overview</a> •
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#browser">Browser</a> •
  <a href="#office">Office</a> •
  <a href="#external-extensions">Extensions</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Use** is the application-capability layer for A3S. Browser and Office are
first-party domains in the default distribution. Independently distributed
packages can add more domains without rebuilding Use by declaring native CLI,
standard MCP, and/or `SKILL.md` surfaces in an A3S ACL manifest.

The primary user entry point is `a3s use`; `a3s-use` is the standalone binary
used by the umbrella CLI and remains available for direct use, automation, and
diagnostics. A3S Search does not call either CLI: it depends directly on the
small `a3s-use-browser` Rust crate and injects `Arc<dyn PageRenderer>`.

Use is not a workflow engine, a universal operating-system package manager, or
a new RPC protocol. It owns typed capability contracts and the lifecycle of
its managed providers and extension packages. The umbrella A3S CLI owns the
top-level component catalog, release sources, and product installation policy.

### Basic usage

```bash
# Inspect built-in and installed capability domains.
a3s use capabilities --json
a3s use doctor --json

# Use Browser directly or preserve an interactive session across commands.
a3s use browser render https://example.com --output page.html
a3s use browser open https://example.com --session research
a3s use browser snapshot --session research --json
a3s use browser click @e1 --session research
a3s use browser close --session research

# Delegate Office operations to the supported OfficeCLI provider.
a3s use office get report.docx /body --json
a3s use office batch report.xlsx --input updates.json --json

# Start a standard MCP server for a supported domain.
a3s use mcp serve browser
a3s use mcp serve office
```

Every domain argument accepted by `a3s use ...` can also be passed directly to
`a3s-use ...`.

## Features

- **Built-In Browser and Office**: Keep stable first-party command routes while
  reporting provider readiness separately
- **Typed Rust Contracts**: Embed Browser rendering and Office operations
  without starting a CLI process or an MCP server
- **Agent Browser Compatibility**: Provide the locked 82-command vocabulary,
  151 MCP tools, six packaged Skills, Dashboard, and interactive runtime from
  `agent-browser` 0.31.2
- **Native OfficeCLI Delegation**: Preserve OfficeCLI's command and standard MCP
  contracts instead of reimplementing document formats or its resident transport
- **External Domains**: Install process-isolated packages that expose any useful
  combination of CLI, MCP, and Skill surfaces
- **Hot-Plug Discovery**: Publish immutable generation/revision snapshots so a
  resident host can add, replace, or remove live capabilities without restarting
- **Content-Bound Skills**: Project an absolute package path and lowercase
  SHA-256 for every `SKILL.md`, allowing consumers to verify the exact bytes
  before loading them
- **Managed Provider Safety**: Require explicit installation authority, bounded
  downloads, approved HTTPS origins, receipts, staging, and atomic activation
- **Structured Automation**: Return versioned `--json` documents and typed error
  codes while retaining native process status and streams for delegated commands
- **Component Ownership**: Remove only A3S-managed provider or package files;
  system browsers, user documents, profiles, and externally owned tools remain
  outside normal uninstall

### Capability matrix

| Domain | Origin | CLI | MCP | Skill | Runtime owner |
| --- | --- | --- | --- | --- | --- |
| Browser | Built in | Full Browser vocabulary | A3S Use standard MCP server | Six packaged Browser Skills | A3S Use |
| Office | Built in | Native OfficeCLI vocabulary | OfficeCLI standard MCP server | — | A3S Use manages the pinned provider |
| Box | Reserved built-in route | Native A3S Box vocabulary | — | — | Umbrella A3S CLI |
| External domain | Installed extension | Optional native executable | Optional standard MCP server | Optional `SKILL.md` | Extension package plus A3S Use lifecycle |

The Box route is component-backed. The umbrella CLI resolves its authoritative
Box executable and passes the canonical path to Use for one invocation. Use
does not copy Box, discover a replacement on `PATH`, or write a second receipt.

### Cargo feature matrix

Default features are `browser`, `office`, `extensions`, and `mcp`.

| Feature | Included capability |
| --- | --- |
| `browser` | Typed Browser library, stateless rendering, and full Browser driver delegation |
| `office` | Typed Office contracts, managed OfficeCLI provider, and native delegation |
| `extensions` | ACL manifests, package receipts, hot-plug registry, and external CLI/MCP/Skill routes |
| `mcp` | Standard MCP servers plus the managed Browser Streamable HTTP lifecycle |
| `lightpanda` | Explicit opt-in Lightpanda provider support in addition to Chrome |

A compiled command surface is not proof that its provider is installed. Use
`doctor`, `component status`, or `capabilities` to inspect runtime readiness.

### Crates

| Crate | Responsibility |
| --- | --- |
| `a3s-use-core` | Shared diagnostics, errors, artifacts, session IDs, and risk classes |
| `a3s-use-browser` | Object-safe rendering contract, providers, managed runtimes, and sessions |
| `a3s-use-browser-driver` | Complete interactive Browser CLI, MCP tools, Skills, Dashboard, and compatibility runtime |
| `a3s-use-office` | Typed Office batches, pinned OfficeCLI lifecycle, and native delegation |
| `a3s-use-extension` | A3S ACL manifest model, package registry, leases, and native surface descriptors |
| `a3s-use` | Facade library, standalone CLI host, capability projection, and MCP entry points |

## Quick Start

### Installation

The preferred product installation goes through the umbrella CLI, which owns
release selection and the top-level component receipt:

```bash
a3s install use --source release
a3s install use/browser
a3s install use/office
a3s use doctor --json
```

Prebuilt archives are also published on
[GitHub Releases](https://github.com/A3S-Lab/Use/releases). A complete archive
contains `a3s-use`, its sibling `a3s-use-browser-driver`, Browser Skills, the
Dashboard, and license/provenance notices. Keep those packaged assets together;
installing only the facade binary does not provide the complete Browser surface.

Build all binaries from source with:

```bash
git clone https://github.com/A3S-Lab/Use.git
cd Use
cargo build --workspace --bins --locked
./target/debug/a3s-use doctor --json
```

### Embed Browser rendering

Applications that only need page rendering should depend on the Browser crate,
not the facade binary:

```toml
[dependencies]
a3s-use-browser = "0.1.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
url = "2"
```

```rust,no_run
use std::sync::Arc;

use a3s_use_browser::{BrowserPool, BrowserPoolConfig, PageRenderer, RenderRequest};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let browser = Arc::new(BrowserPool::new(BrowserPoolConfig::default()));
    let page = browser
        .render(RenderRequest::new(Url::parse("https://example.com")?))
        .await?;

    println!("{}", page.html);
    browser.shutdown().await;
    Ok(())
}
```

`BrowserPoolConfig::default()` discovers an existing Chrome-compatible browser
and never authorizes a download. Select a managed provider or run an explicit
component install when A3S should own the runtime.

## Browser

Browser has two deliberately separate integration levels:

- `a3s-use-browser` is the small typed library used by Search and embedded Rust
  applications. `browser render` also uses this direct in-process path.
- `a3s-use-browser-driver` provides the complete interactive automation surface
  and is shipped as a sibling executable in release archives.

The compatibility driver tracks
[vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser)
`0.31.2` at commit
`3591f0f4b719c94bcb9aec83ebe811c5dd7f587a`. Automated parity gates pin 82
accepted top-level commands, 151 typed MCP tools, and the `core`, `electron`,
`slack`, `dogfood`, `vercel-sandbox`, and `agentcore` Skills. Existing MCP
clients retain the `agent_browser_*` tool names.

`a3s-use mcp serve browser` exposes the A3S-owned standard MCP server over
stdio. `mcp start`, `mcp status`, and `mcp stop` manage its optional persistent
Streamable HTTP deployment. That deployment binds to an ephemeral loopback
port, requires a private bearer token, has bounded idle and maximum lifetimes,
and shares typed Browser session state. It is an MCP deployment, not an A3S
JSON-RPC service.

Provider selection stays explicit. Discovered providers never download
software. Managed Chrome and Lightpanda installations use bounded staging and
atomic activation; Lightpanda assets require the publisher SHA-256. Chrome for
Testing does not publish an independent checksum in its current version feed,
so A3S records HTTPS provenance and the locally observed digest without claiming
publisher verification.

See [Agent Browser Compatibility Baseline](docs/agent-browser-parity.md) for the
locked schemas, digests, runtime evidence, and promotion criteria.

## Office

Office delegates commands to
[iOfficeAI/OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) rather than
reimplementing Word, spreadsheet, or presentation formats. The managed provider
is pinned to reviewed OfficeCLI version `1.0.136`.

```bash
# Inspect without downloading anything.
a3s use office doctor --json

# Install the reviewed provider explicitly, then use its native vocabulary.
a3s install use/office
a3s use office get report.docx /body --json
a3s use office batch report.xlsx --input updates.json --json

# Launch OfficeCLI's own standard MCP server.
a3s use mcp serve office
```

Managed installation accepts only approved HTTPS release origins, bounds the
download, verifies the publisher SHA-256, stages outside the active version,
and activates atomically. Native execution sets `OFFICECLI_SKIP_UPDATE=1` so an
upgrade remains an explicit A3S component operation.

OfficeCLI may reuse its own resident process internally. Its pipe and framing
are private implementation details: A3S Use neither speaks nor reimplements
that transport. A lost response to a mutation can therefore return
`use.office.outcome_unknown`; callers must report that the operation may have
been applied and must not retry it automatically.

## External Extensions

External Use domains stay behind process boundaries. A package contains an
`a3s-use-extension.acl` manifest parsed by `a3s-acl` and any declared native
executables or Skill files. ACL is the A3S Agent Configuration Language; it is
not HCL and is not parsed with an HCL parser.

```acl
extension "acme/slack" {
  schema_version = 1
  version        = "1.0.0"
  route          = "slack"
  actions        = ["read", "mutate"]

  cli {
    executable  = "bin/a3s-use-acme-slack"
    json_output = true
  }

  mcp {
    executable = "bin/a3s-use-acme-slack"
    args       = ["serve", "--mcp"]
    transport  = "stdio"
  }

  skill {
    path = "skills/slack/SKILL.md"
  }
}
```

Install an explicitly trusted local package and invoke its route with:

```bash
a3s install use/acme/slack --from ./slack-extension --allow-unsigned
a3s use slack channels list
a3s use mcp serve acme/slack

a3s use extension disable acme/slack --json
a3s use extension enable acme/slack --json
a3s uninstall use/acme/slack
```

The current extension source is an explicit local directory. It must pass
manifest, route, path, package-size, and executable validation, and unsigned
content requires `--allow-unsigned`. A signed remote publisher channel is
roadmap work; Use does not silently install arbitrary Homebrew, npm, Cargo,
system, or `PATH` packages.

Built-in and management routes are reserved. Extensions cannot shadow
`browser`, `office`, `box`, `component`, `capability`, or other host commands.

## Live Host Integration

Resident hosts consume `capability snapshot` and `capability watch`. The
projection presents Browser, Office, Box, and enabled extensions through one
read-only schema while preserving each binding's `built-in` or `extension`
origin. The extension generation advances on receipt mutations; a content
revision also changes when built-in readiness or packaged Skill content changes.

```bash
a3s-use capability snapshot --json
a3s-use capability watch \
  --after-generation 3 \
  --after-revision <sha256> \
  --timeout-ms 30000 \
  --json
```

A3S Code uses this contract to maintain a dedicated `use` worker for TUI and
Web sessions. The worker is default-deny and receives only `mcp__use_*` tools;
it cannot use the workspace, shell, unrelated MCP servers, or recursive task
tools. Projected Skills provide guidance only and cannot expand permissions or
authorize installation. Code verifies their projected SHA-256 before loading
the exact bytes.

A capability becomes callable only after its MCP connection is ready. A
removed or replaced route leaves the worker catalog before its old connection
drains. Starting Code never installs Use: component installation remains an
explicit umbrella CLI action.

## Protocol and Lifecycle Boundaries

Use preserves each native integration contract:

| Surface | Contract |
| --- | --- |
| CLI | `argv`, stdin, stdout, stderr, process status, and optional versioned `--json` output |
| MCP | Standard MCP client/server lifecycle and the package's declared transport |
| Skill | Existing `SKILL.md` package convention with a content digest in registry projections |

`--json` is structured CLI output, not JSON-RPC. Use does not define a generic
`execute(domain, action, payload)` envelope, translate one MCP vocabulary into
another, load Rust dynamic libraries, or aggregate every domain into one server.

Extension install and upgrade stage a unique immutable package directory, then
commit a receipt and atomically publish a new registry snapshot. Accepted CLI
or MCP invocations hold a shared route lease until their child process exits.
Disable and uninstall first hide the route from new callers, then wait for an
exclusive drain lease before deleting owned files. A drain timeout leaves the
route disabled, so retrying the lifecycle operation converges safely.

The receipt is authoritative. Reconciliation rebuilds a snapshot missed by a
crash, and in-flight calls retain the exact package generation they accepted.

## Architecture

```text
                         user / script / agent
                                  │
                          a3s umbrella CLI
                     catalog, sources, receipts
                                  │
                              a3s use
                                  │
                              a3s-use host
                    ┌─────────────┼──────────────┐
                    │             │              │
                Browser         Office       extension registry
             typed + driver    OfficeCLI      CLI / MCP / Skill
                    │             │              │
                    └──────── capability snapshot/watch ───────► A3S Code

  a3s-search ── Arc<dyn PageRenderer> ──► a3s-use-browser

  a3s use box ── canonical executable supplied by a3s ──► A3S Box
```

The dependency arrows are intentional. Search links only the Browser contract,
so rendering does not require `a3s-use`, MCP, or a resident process. Office and
external domains keep their native process boundaries. A3S Code consumes the
read-only projection and connects standard MCP/Skill surfaces; it does not gain
component installation authority.

Source is split between the facade under `src/` and focused workspace crates
under `crates/`. See [Architecture](docs/architecture.md) for package leases,
registry publication, persistent sessions, component ownership, and roadmap
details.

## Platform Support

| Platform | Status | Current guarantee |
| --- | --- | --- |
| macOS arm64 / x86_64 | Supported | Managed providers, extension lifecycle, complete Browser compatibility gates, and release archives |
| Linux arm64 / x86_64 | Supported | Managed providers, extension lifecycle, complete Browser compatibility gates, and release archives |
| WSL | Supported through Linux | Follows the Linux runtime and filesystem contract |
| Windows x86_64 | Preview / roadmap | Compile, command, MCP schema, Skill, packaging, and non-Browser-runtime checks only |

Windows is not currently part of the Browser runtime compatibility claim. It
will be promoted after real-Chrome sessions persist across separate
`a3s use browser` invocations with the same bounded startup, cleanup, and
lifecycle guarantees as macOS and Linux.

## Development

Run checks from the A3S Use repository directory:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

The suite covers typed contracts, provider discovery and installation,
extension validation and route draining, capability snapshots, native
delegation, MCP schemas, packaged Skills, and the locked agent-browser
compatibility surface. Real-Chrome integration gates run serially with isolated
home and runtime directories on supported platforms.

## License

A3S Use is licensed under the [MIT License](LICENSE). The Browser compatibility
driver contains work derived from `vercel-labs/agent-browser` under Apache-2.0;
see [Third-Party Notices](THIRD_PARTY_NOTICES.md) and
[Upstream Provenance](crates/browser-driver/UPSTREAM.md).
