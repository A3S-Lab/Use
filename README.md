# A3S Use

<p align="center">
  <strong>Typed Application Capabilities for A3S</strong>
</p>

<p align="center">
  <em>Use browsers, OCR, and independently shipped application domains through native CLI, standard MCP, and Skills</em>
</p>

<p align="center">
  <a href="#overview">Overview</a> •
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#browser">Browser</a> •
  <a href="#office">Office</a> •
  <a href="#ocr">OCR</a> •
  <a href="#external-extensions">Extensions</a> •
  <a href="#immutable-mcp-and-skill-releases">Releases</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Use** is the application-capability layer for A3S. Browser and OCR are
first-party domains in the default distribution, and Box is a component-backed
route. Independently distributed repositories, including A3S Office, add
domains without rebuilding Use by packaging native CLI, standard MCP, and/or
`SKILL.md` surfaces in an A3S ACL manifest.

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

# Install an independently built Office capability package.
a3s use component install a3s/office \
  --from ./a3s-use-office \
  --allow-unsigned \
  --json
a3s use office --help
a3s use mcp serve office

# Built-in local PP-OCRv6.
a3s use ocr doctor --json
a3s use ocr extract ./scan.png --json
a3s use mcp serve ocr
```

Every domain argument accepted by `a3s use ...` can also be passed directly to
`a3s-use ...`.

## Features

- **Standard external repositories**: Bind independently released packages to a canonical HTTPS source repository and a SemVer A3S Use compatibility range.
- **Native integration surfaces**: Preserve CLI argv and process status, standard MCP lifecycle, and existing `SKILL.md` packages without a private RPC envelope.
- **Atomic lifecycle**: Install, upgrade, enable, disable, watch, drain, and uninstall immutable package generations without restarting resident hosts.
- **Explicit trust**: Admit reviewed local packages, digest-bound release bundles, or TUF-verified registry targets; never clone and execute arbitrary source.
- **Route-based UX**: Invoke, diagnose, inspect, serve MCP, and uninstall by a unique route while keeping the package ID as the stable lifecycle identity.
- **Agent Browser compatibility**: Provide the locked Browser command, MCP, Skill, Dashboard, and interactive runtime surface.
- **First-party OCR**: Run pinned PP-OCRv6 detection and recognition locally through ONNX Runtime with bounded source evidence.
- **Content-bound discovery**: Publish generation and revision snapshots with hashes for Skills and workbench assets.
- **Immutable release contracts**: Canonicalize and digest MCP Runtime Service and Skill Agent-input descriptors with exact provenance and dependencies.
- **Component ownership**: Remove only A3S-managed provider or package files; system tools and user data remain outside normal uninstall.

### Capability matrix

| Domain | Origin | CLI | MCP | Skill | Runtime owner |
| --- | --- | --- | --- | --- | --- |
| Browser | Built in | Full Browser vocabulary with first-launch preparation | A3S Use standard MCP server with confirmed installer | Six packaged Browser Skills | A3S Use |
| Office | External `a3s/office` package | Native `a3s-office` vocabulary | Package-declared standard MCP server | Packaged `a3s-office` Skill | [A3S Office](https://github.com/A3S-Lab/Office) |
| Box | Reserved built-in route | Native A3S Box vocabulary | — | — | Umbrella A3S CLI |
| OCR | Built in | Doctor and typed image extraction | `ocr_doctor` and `ocr_extract` | One local PP-OCRv6 Skill | A3S Use process with ONNX Runtime |
| Science | External `a3s/science` package | Source-specific retrieval commands | 13 typed `science_*` tools | One research workflow Skill | Science extension process |
| External domain | Installed extension | Optional native executable | Optional standard MCP server | Optional `SKILL.md` | Extension package plus A3S Use lifecycle |

The Box route is component-backed. The umbrella CLI resolves its authoritative
Box executable and passes the canonical path to Use for one invocation. Use
does not copy Box, discover a replacement on `PATH`, or write a second receipt.

### Cargo feature matrix

Default features are `browser`, `ocr`, `extensions`, and `mcp`.

| Feature | Included capability |
| --- | --- |
| `browser` | Typed Browser library, stateless rendering, and full Browser driver delegation |
| `ocr` | Built-in typed PP-OCRv6 CLI/MCP with local ONNX inference |
| `extensions` | ACL manifests, package receipts, hot-plug registry, and external CLI/MCP/Skill routes |
| `mcp` | Standard MCP servers plus the managed Browser Streamable HTTP lifecycle |
| `lightpanda` | Explicit opt-in Lightpanda provider support in addition to Chrome |

A compiled command surface is not proof that its provider is installed. Use
`doctor`, `component status`, or `capabilities` to inspect runtime readiness.

### Crates

| Crate | Responsibility |
| --- | --- |
| `a3s-use-core` | Shared diagnostics, errors, artifacts, session IDs, risk classes, and immutable MCP/Skill release descriptors |
| `a3s-use-browser` | Object-safe rendering contract, providers, managed runtimes, and sessions |
| `a3s-use-browser-driver` | Complete interactive Browser CLI, MCP tools, Skills, Dashboard, and compatibility runtime |
| `a3s-use-extension` | A3S ACL manifest model, package registry, leases, and native surface descriptors |
| `a3s-use-ocr` | Local PP-OCRv6 engine, CLI, MCP tools, pinned models, and release-packaged Skill assets |
| `a3s-use-science` | Typed public life-science APIs, CLI, MCP tools, and extension package assets |
| `a3s-use` | Facade library, standalone CLI host, capability projection, and MCP entry points |

## Quick Start

### Installation

The preferred product installation goes through the umbrella CLI, which owns
release selection and the top-level component receipt:

```bash
a3s install use --source release
# Optional deterministic pre-warm; normal first use prepares these as needed.
a3s install use/browser
a3s use doctor --json
```

Prebuilt archives are also published on
[GitHub Releases](https://github.com/A3S-Lab/Use/releases). A complete archive
contains `a3s-use`, its sibling `a3s-use-browser-driver`, Browser Skills, the
Dashboard, and license/provenance notices. Keep those packaged assets together;
installing only the facade binary does not provide the complete Browser
surface. Office is released independently from its own repository.

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
a3s-use-browser = "0.2.0"
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

`BrowserPoolConfig::default()` remains non-installing for embedded callers such
as Search. Product commands validate their arguments and then prepare the same
shared managed runtime on the first local Browser launch when policy allows.

## Browser

Browser has two deliberately separate integration levels:

- `a3s-use-browser` is the small typed library used by Search and embedded Rust
  applications. `browser render` also uses this direct in-process path.
- `a3s-use-browser-driver` provides the complete interactive automation surface
  and is shipped as a sibling executable in release archives.

The compatibility driver tracks
[vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser)
`0.32.1` at commit
`2b202640ee89dc7aadb5e8c9d600e089e9056985`. Automated parity gates pin 82
accepted top-level commands, 151 typed MCP tools, and the `core`, `electron`,
`slack`, `dogfood`, `vercel-sandbox`, and `agentcore` Skills. Existing MCP
clients retain the `agent_browser_*` tool names.

With `--allowed-domains`, locally launched Chromium applies network controls
before paused pages, popups, workers, and out-of-process iframes resume. It
also blocks peer-connection constructors and non-proxied WebRTC UDP. Modes
that cannot guarantee early containment—including existing CDP sessions,
auto-connect, profiles, restore/state replay, direct-page providers, unsafe
startup arguments, iOS, and Safari—are rejected explicitly. Standalone
`wait --load load` and `wait --load domcontentloaded` also resolve immediately
when the active document has already reached the requested state.

`a3s-use mcp serve browser` exposes the A3S-owned standard MCP server over
stdio. `mcp start`, `mcp status`, and `mcp stop` manage its optional persistent
Streamable HTTP deployment. That deployment binds to an ephemeral loopback
port, requires a private bearer token, has bounded idle and maximum lifetimes,
and shares typed Browser session state. It is an MCP deployment, not an A3S
JSON-RPC service.

Provider selection stays typed. Embedded `Discovered*` providers never
download software. A direct local Browser launch is first-use authority for the
A3S product CLI; Code workers request the bounded installer through parent
confirmation. Both paths reuse a system browser or the shared A3S-managed
cache before downloading. Managed Chrome and Lightpanda installations use
bounded staging and atomic activation; Lightpanda assets require the publisher
SHA-256. Chrome for Testing does not publish an independent checksum in its
current version feed, so A3S records HTTPS provenance and the locally observed
digest without claiming publisher verification. Help, version, doctor, Skills,
profiles, and MCP server startup never install a browser.

See [Agent Browser Compatibility Baseline](docs/agent-browser-parity.md) for the
locked schemas, digests, runtime evidence, and promotion criteria.

## Office

A3S Office is developed and released independently at
[A3S-Lab/Office](https://github.com/A3S-Lab/Office). It is no longer compiled
into A3S Use. The repository publishes the `a3s-office` CLI, standard MCP
server, Office Skill, Rust OOXML engine, and `@a3s-lab/office` web component
library.

Office integrates through the same schema v2 external repository contract used
by every other independent domain. Its package ID is `a3s/office` and its route
is `office`:

```bash
# From an A3S Office checkout.
./scripts/package-a3s-use-extension.sh ./dist/a3s-use-office

a3s-use component install a3s/office \
  --from ./dist/a3s-use-office \
  --allow-unsigned \
  --json

a3s-use office --help
a3s-use mcp serve office
```

See [External Repository Capabilities](docs/external-repositories.md) for the
package, trust, compatibility, route, and lifecycle contract. Office-specific
commands and APIs are documented in the Office repository.

## OCR

`a3s-use-ocr` implements the reserved built-in `ocr` route. The default Use
release packages its `a3s-use-ocr` Skill and exposes `ocr_doctor` plus
`ocr_extract` over standard MCP, so a resident A3S Code session receives
`mcp__use_ocr__*` without installing a separate extension.

OCR has one backend: the pinned `PP-OCRv6_small` detection and recognition
models running locally through ONNX Runtime. Release archives package those
models; `a3s install use/ocr` explicitly installs or repairs the same pinned
bundle when needed. Supported inputs are bounded local PNG, JPEG, WebP, GIF,
BMP, and TIFF files. The result binds the canonical source path, media type,
byte length, and SHA-256 alongside text, recognition/detection confidence,
polygons, and bounding boxes.

The pipeline decodes and normalizes the image, runs
`PP-OCRv6_small_det`, applies DB post-processing and reading-order sorting,
perspective-rectifies and rotates text crops, runs batched
`PP-OCRv6_small_rec`, and applies CTC decoding. It does not require Python or
PaddlePaddle, call a remote OCR API, or transfer source bytes off the device.

```bash
a3s use ocr doctor --json
a3s use ocr extract ./scan.png --json
a3s use mcp serve ocr
```

A3S Code may first-use install the verified parent Use release. A missing or
damaged managed model bundle is repaired explicitly with
`a3s install use/ocr`; the Code `use` worker never installs it implicitly.

See the [OCR crate](crates/ocr/README.md) for model resolution, the inference
workflow, and input boundaries.

## Science Toolkit

The repository includes `a3s-use-science` as a reference external extension,
not as another built-in route. Its process exposes one typed Rust client as 13
read-only MCP tools plus source-specific CLI commands for PubMed, ChEMBL,
ClinicalTrials.gov, bioRxiv, and Ensembl. The broader first-party catalog of
scientific Skills, MCP services, and compute workflows lives in
[A3S Science](https://github.com/A3S-Lab/Science). The package also contributes
a research brief view with declared HTML, CSS, and JavaScript assets bound to
its `a3s-use-science` Skill; A3S Web renders it in its own sandbox and review
flow.

Official A3S Use archives carry the complete platform-specific package under
`extensions/a3s/science`. It is only a trusted installation source: Science is
not registered or started until the user selects **Install** in A3S Web Market
or applies the reviewed umbrella CLI plan. The plan includes the exact expanded
package digest; A3S Use resolves the release-owned directory again, rechecks
that digest, and records `release-bundle` provenance. The installed package can
still be disabled, enabled, upgraded with a newer Use release, or uninstalled.
Remote TUF distribution remains available for packages not carried by a Use
release.

The release-owned catalog is inspectable without installing anything:

```bash
a3s-use extension catalog --json
```

Build a local package into a new directory and install it explicitly:

```bash
./crates/science/scripts/package.sh /tmp/a3s-use-science-package
a3s install use/a3s/science \
  --from /tmp/a3s-use-science-package \
  --allow-unsigned

export A3S_SCIENCE_CONTACT_EMAIL=researcher@example.org
a3s use science pubmed search "single-cell atlas" --limit 10 --json
a3s use science ensembl lookup homo_sapiens TP53 --json
a3s use mcp serve a3s/science
```

The same package can be installed from a local archive:

```bash
COPYFILE_DISABLE=1 tar -czf /tmp/a3s-use-science-package.tar.gz \
  -C /tmp/a3s-use-science-package .
a3s install use/a3s/science \
  --from /tmp/a3s-use-science-package.tar.gz \
  --allow-unsigned
```

Developer-provided local directories, `.tar.gz`, `.tgz`, and `.zip` packages
require `--allow-unsigned`; use them only after explicit review. A signed remote
distribution can publish the same package through a configured TUF registry;
the umbrella CLI then installs it without `--from` or `--allow-unsigned`.
PubMed requires the contact email, while `NCBI_API_KEY` is optional. See the
[Science crate](crates/science/README.md), its
[data-source notice](crates/science/DATA_SOURCES.md), and
[A3S Science repository boundary](crates/science/UPSTREAM.md) for the full
command set, data egress, limits, and interpretation boundaries.

## External Extensions

External Use domains stay behind process boundaries. A package contains an
`a3s-use-extension.acl` manifest parsed by `a3s-acl` and any declared native
executables or Skill files. ACL is the A3S Agent Configuration Language; it is
not HCL and is not parsed with an HCL parser.

```acl
extension "acme/slack" {
  schema_version = 2
  version        = "1.0.0"
  route          = "slack"
  requires_use   = ">=0.2.0, <0.3.0"
  actions        = ["read", "mutate"]

  repository {
    url = "https://github.com/acme/slack"
  }

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

  contributes {
    activity_bar "channels" {
      title       = "Slack"
      description = "Prepare a reviewed Slack context."
      icon        = "messages-square"
      entry       = "web/activity.html"
      skill       = "slack"
      order       = 140
    }
  }
}
```

Install an explicitly trusted local package and invoke its route with:

```bash
a3s install use/acme/slack --from ./slack-extension --allow-unsigned
a3s use slack channels list
a3s use doctor slack --json
a3s use mcp serve slack

a3s use extension disable acme/slack --json
a3s use extension enable acme/slack --json
a3s uninstall use/acme/slack
```

The current extension source is an explicit local directory or a `.tar.gz`,
`.tgz`, or `.zip` archive. Archives must contain exactly one package manifest;
every entry must belong to that manifest's package root. Installation rejects
links, traversal, duplicate paths, unsupported entries, excessive expansion,
and non-portable paths before validating the manifest, route, executable, and
Skill surfaces. Unsigned content requires `--allow-unsigned`. Use does not
silently install arbitrary Homebrew, npm, Cargo, system, or `PATH` packages.

A release-bundled source is a separate first-party provenance. It must live
under the installed A3S Use release's `extensions/<publisher>/<name>` tree,
appear in `extension catalog`, and match the digest in the reviewed umbrella
plan. It never uses `--allow-unsigned` and cannot be combined with registry or
local-package options.

### Signed extension registries

Remote extensions use TUF metadata and a separately established bootstrap-root
digest. Enroll a registry with either a root file or its SHA-256, verify it,
review the immutable component plan, and apply that exact plan:

```bash
a3s registry add https://packages.example.org/a3s/ \
  --trust-root ./root.json \
  --yes
a3s registry refresh packages

a3s --output json install use/a3s/science --dry-run
a3s --output json install use/a3s/science \
  --plan-digest <reviewed-plan-sha256>

a3s --output json upgrade use/a3s/science --dry-run
a3s --output json upgrade use/a3s/science \
  --plan-digest <reviewed-upgrade-sha256>
```

When a root file is supplied, the umbrella CLI copies it into registry-owned
configuration and records its digest. With a digest-only enrollment, Use may
fetch `<registry>/metadata/root.json`, but it caches the file only after the
bytes match the pinned SHA-256. Subsequent root rotation, timestamp, snapshot,
and targets metadata are verified by TUF with expiration and rollback
enforcement. Registry URLs require HTTPS; loopback HTTP is accepted only for
tests and local development.

A dry-run verifies metadata but does not download the target archive. Its outer
component digest includes the exact `ResolvedRemotePackage`: registry identity,
bootstrap root, every TUF metadata version, package version and channel,
platform target, archive path, length, and SHA-256. Apply resolves again and
fails before target download if that plan changed. It then passes the resolved
package's own digest to `a3s-use`, which repeats TUF verification immediately
before downloading and activating the archive. The installed receipt records
`registry-tuf` trust and the complete signed provenance. Registry installs
reject `--allow-unsigned`; local `--from` installs cannot provide registry
options.

Registry upgrades reuse the registry identity and channel recorded in that
signed provenance instead of searching every configured source again. A
missing registry, changed URL or bootstrap root, and semantic-version downgrade
are rejected before payload download. Plain `a3s upgrade` reports newer signed
targets, while `a3s upgrade --all` includes them in the selected batch. If the
verified target is identical to the installed target, `a3s-use` validates and
reconciles the receipt and registry snapshot without downloading or
reactivating the package.

Publish metadata below `<registry>/metadata/` and payloads below
`<registry>/targets/`. An extension target uses this canonical path:

```text
extensions/<publisher>/<name>/<version>/<channel>/<target>/<archive>
```

Its TUF target `custom.a3s` object must contain `schemaVersion`, `packageId`,
`version`, `channel` (`stable`, `beta`, or `nightly`), and `target` (an A3S host
target or `any`). Duplicate identities, mismatched paths, unsupported archives,
and oversized targets are rejected before payload download.

Hosts can call `a3s_use_extension::list_remote_packages` to obtain a fully
verified, host-compatible package catalog without downloading archives. The
returned catalog includes the verified metadata versions and immutable
`ResolvedRemotePackage` entries required for review and subsequent installation.

Built-in and management routes are reserved. Extensions cannot shadow
`browser`, `ocr`, `box`, `component`, `capability`, or other host commands.
See [External Repository Capabilities](docs/external-repositories.md) for the
schema v2 repository, compatibility, route-resolution, and lifecycle contract.

## Live Host Integration

Resident hosts consume `capability snapshot` and `capability watch`. The
projection presents Browser, OCR, Box, and installed extensions through one
read-only schema while preserving each binding's `built-in` or `extension`
origin. External bindings include their repository identity and required A3S
Use range. The extension generation advances on receipt mutations; a content
revision also changes when built-in readiness or projected content changes.

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
the exact bytes. Ordinary built-in Browser and OCR operations may run inside
that worker, while bounded provider installers and newly projected extension
tools retain the parent host's interactive confirmation boundary.

An installed `a3s/office` package appears as an external `office` route with
the CLI, MCP, Skill, repository, and compatibility metadata declared by the
Office release. A3S Use does not duplicate or replace those surfaces.

A capability becomes callable only after its MCP connection is ready. A
removed or replaced route leaves the worker catalog before its old connection
drains. Code TUI resolves the catalogued Use component on first launch and may
install its verified release before terminal takeover. Offline mode and
`A3S_NO_AUTO_INSTALL=1` remain strict no-mutation boundaries; setup failure is
non-fatal and stays visible through `/use`.

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
                ┌──────────┬──────────┬─────────────────────────┐
                │          │          │                         │
             Browser      OCR     extension registry ──► Office, Science, …
          typed + driver  ONNX      CLI / MCP / Skill
                │          │          │
                └──── capability snapshot/watch ───────────────► A3S Code

  a3s-search ── Arc<dyn PageRenderer> ──► a3s-use-browser

  a3s use box ── canonical executable supplied by a3s ──► A3S Box
```

The dependency arrows are intentional. Search links only the Browser contract,
so rendering does not require `a3s-use`, MCP, or a resident process. OCR runs
the pinned PP-OCRv6 models locally through ONNX Runtime; model installation is
an explicit component operation. Office and other external domains retain
their repository and process boundaries. A3S Code consumes the read-only
projection and connects standard MCP/Skill surfaces; bounded component
installation requests still require the parent TUI's authority.

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
| Windows x86_64 | Preview / roadmap | Release packaging plus real Microsoft Edge core-profile and local OCR process coverage |

Windows is not yet part of the complete Browser compatibility claim. The
default 31-tool core profile now has real Microsoft Edge evidence through Code
TUI and standard MCP, including bounded Doctor, namespaced daemon startup,
navigation, interaction, screenshots, reads, tabs, and cleanup. Promotion still
requires persistent sessions across separate invocations plus the advanced
Browser profiles and the same lifecycle guarantees as macOS and Linux.

## Development

Run checks from the A3S Use repository directory:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

The suite covers typed contracts, provider discovery and installation,
extension validation, repository compatibility, route draining, capability
snapshots, MCP schemas, packaged Skills, and the locked agent-browser
compatibility surface. Real-Chrome integration gates run serially with isolated
home and runtime directories on supported platforms. Office engine and editor
tests run in the independent A3S Office repository.

## License

A3S Use is licensed under the [MIT License](LICENSE). The Browser compatibility
driver contains work derived from `vercel-labs/agent-browser` under Apache-2.0;
see [Third-Party Notices](THIRD_PARTY_NOTICES.md) and
[Upstream Provenance](crates/browser-driver/UPSTREAM.md).
