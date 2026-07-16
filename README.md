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

# Load the packaged Office Skill or mutate OOXML with no external provider.
a3s use office skills list --json
a3s use office skills get a3s-use-office --full
a3s use office native create report.docx --json
a3s use office native view report.docx text --json
a3s use office native view report.docx annotated --limit 100 --json
a3s use office native view report.docx issues --type content --limit 50 --json
a3s use office native view report.docx html --output report.html --json
a3s use office native view deck.pptx svg --output deck.svg --json
a3s use office native view report.docx screenshot --output report.png --json
a3s use office native query report.docx 'p[style=Heading1]' --json
a3s use office native set report.docx /body/p[1] --text 'Updated' --json
a3s use office native set report.docx '/body/p[1]/r[1]' --bold true --font-family Aptos --font-size 14 --text-color 123456 --json
a3s use office native set report.docx '/body/p[1]' --align center --json
a3s use office native add report.docx '/body/p[1]' --type hyperlink --url https://example.com --display 'Open site' --tooltip 'A3S site' --json
a3s use office native add report.docx '/body/p[1]' --type comment --author Alice --initials AL --text 'Please review' --json
a3s use office native set report.docx '/comments/comment[1]' --author Bob --text 'Reviewed' --json
a3s use office native add report.docx /body --type paragraph --text 'More' --json
a3s use office native add report.docx /body --type table --rows 2 --columns 3 --json
a3s use office native add report.docx /body --type picture --input logo.png --alt 'A3S logo' --width 320 --json
a3s use office native remove report.docx /body/p[2] --json
a3s use office native move report.docx /body/p[2] --before /body/p[1] --json
a3s use office native copy workbook.xlsx /Sheet1 --name 'Sheet1 Copy' --json
a3s use office native swap deck.pptx '/slide[1]' '/slide[2]' --json
a3s use office native raw report.docx /word/document.xml --json
a3s use office native add-part report.docx / --type header --json
a3s use office native dump report.docx --output report.replay.json --json
a3s use office native merge template.docx report.docx --data @report.json --json

# Commands not yet promoted to native continue through the compatibility route.
a3s use office get report.docx /body --json
a3s use office batch report.xlsx --input updates.json --json

# Start a standard MCP server for Browser or the explicit native Office preview.
a3s use mcp serve browser
a3s use mcp serve office-native

# Keep using the pinned OfficeCLI compatibility MCP server where needed.
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
  `agent-browser` 0.32.1
- **A3S-Native Office Foundation**: Own safe OOXML package, XML, relationship,
  selector, semantic read, transactional add/set/remove/move/copy/swap,
  typed cross-format text formatting, inert hyperlinks, and legacy comments,
  native PNG/JPEG/GIF embedding, cross-format template merge, deterministic
  bounded all-format annotated views, all-format HTML/SVG semantic previews,
  bounded typed issue diagnostics, Browser-injected semantic PNG screenshots,
  authenticated loopback live watch, and an explicit typed standard MCP
  preview while retaining the 0.1.x
  OfficeCLI compatibility backend for surfaces not yet promoted
- **Packaged Office Guidance**: Ship one first-party `a3s-use-office` Skill for
  safe Word, Spreadsheet, Presentation, native MCP, and compatibility workflows
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
| Office | Built in | Stable Office vocabulary | Typed native preview plus OfficeCLI compatibility server | Packaged `a3s-use-office` Skill | A3S Use native engine; OfficeCLI compatibility in 0.1.x |
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
| `office` | Typed Office contracts, native OOXML read engine, and temporary OfficeCLI compatibility |
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
| `a3s-use-office` | Native OOXML foundation, typed Office operations, and compatibility lifecycle |
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
first-party Office Skill, the Dashboard, and license/provenance notices. Keep
those packaged assets together; installing only the facade binary does not
provide the complete Browser and Office Skill surfaces.

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

Provider selection stays explicit. Discovered providers never download
software. Managed Chrome and Lightpanda installations use bounded staging and
atomic activation; Lightpanda assets require the publisher SHA-256. Chrome for
Testing does not publish an independent checksum in its current version feed,
so A3S records HTTPS provenance and the locally observed digest without claiming
publisher verification.

See [Agent Browser Compatibility Baseline](docs/agent-browser-parity.md) for the
locked schemas, digests, runtime evidence, and promotion criteria.

## Office

Office is moving to an A3S-owned Rust engine for Word, Spreadsheet, and
Presentation documents. The native engine now includes bounded package
admission, byte-preserving XML, content types, a safe relationship graph,
stable selectors, semantic `get`, `query`, `text`, `outline`, and `stats`
reads, bounded `issues` reports, safe blank-document creation, and
loss-preserving text replacement for existing Word, Spreadsheet, and
Presentation nodes. Typed rich-text mutation covers bold, italic, font family,
exact centipoint font size, RGB text color, and horizontal alignment. Word and
Presentation apply character properties to run paths and alignment to paragraph
paths; Spreadsheet applies the same contract to cells or bounded A1 ranges,
creating and deduplicating OOXML font and cell-style records when necessary. It
also creates, updates, reads, queries, and removes typed hyperlinks. Word owns
external HTTP/HTTPS/mailto links and internal bookmark targets with display
text and tooltips; Spreadsheet owns external links and internal cell locations
with display text and tooltips, auto-creating a missing linked cell;
Presentation owns external shape-wide links and tooltips. Presentation slide
jumps remain unsupported and fail closed. External targets reject embedded
credentials, active or relative schemes, and malformed URIs; semantic rendering
keeps every relationship inert and never fetches it. The same typed engine
creates, updates, reads, queries, and removes classic Office comments. Word
comments anchor to a main-document paragraph or run and expose stable
`/comments/comment[N]` paths. Spreadsheet comments are classic cell notes with
an author table, VML note drawing, and `/SheetName/A1/comment` paths, including
notes on otherwise blank cells. Presentation comments use legacy per-slide
comment and shared-author parts, optional EMU coordinates, and
`/slide[N]/comment[M]` paths. Removing an owning Word node, Spreadsheet cell or
range, or Presentation slide also removes its owned comment resources. This is
plain legacy-comment scope, not complete modern threaded-comment parity:
replies, resolved state, writable dates, rich bodies, Word header/footer
anchors, Spreadsheet threaded comments, and modern PowerPoint threaded
comments remain outside the typed contract. The engine can safely inspect
existing XML parts and replace non-OPC-metadata XML parts while preserving the
root QName and validating the final document. Known chart, header, and footer
part carriers can be created together with their content type and owner
relationship. The typed mutation layer also adds and removes Word paragraphs
and basic table/row/cell structures,
creates real Presentation DrawingML tables, appends grid-conformant rows,
fills underfull rows, edits cell text, and exposes table columns as stable
virtual paths. Columns can be inserted, resized in EMUs, removed, moved,
copied, or swapped while the grid, every row, and the graphic-frame width stay
in sync. Tables and structurally safe rows/cells can also be removed. It
upserts typed Spreadsheet text, number, boolean, and formula cells, removes
cells and bounded cell ranges, structurally inserts or deletes rows and columns,
and adds, removes, renames, reorders, or copies worksheets. It also exposes
typed `move`, `copy`, and `swap` mutations with zero-based or path-relative
placement. Word supports same-parent paragraph, table, row, cell, and run
moves and swaps plus identity-free paragraph, table, row, and run copies.
Spreadsheet supports worksheet arrangement and dense plain-row arrangement with
row/cell renumbering. Presentation supports slide arrangement, same-slide
top-level object movement/swaps, layout-only slide copies, and relationship-free
plain-shape copies with fresh non-visual identities. Cross-parent ownership
migration, formula-bearing or reference-rich row arrangement, identity-bearing
Word copies, table-cell copies, and relationship-owning Presentation copies fail
closed before save. Worksheet copy
clones the owned OPC relationship subgraph while preserving shared workbook
resources; removal garbage-collects only unshared descendants. Structural
Spreadsheet edits rewrite affected A1 formulas, defined names, worksheet
metadata, tables, comments, VML notes, drawing anchors, and chart references;
unsupported pivot and 3D-reference cases fail closed before save. Presentation
slides, text shapes, and basic tables also support native add/remove. PNG, JPEG,
and GIF can be embedded as real Word inline pictures, Spreadsheet one-cell
drawing anchors, and Presentation slide pictures; semantic reads and
reference-aware removal use the same cross-format `Picture` contract. Saves are
atomic and reject a changed source revision instead of overwriting another
writer.
Cross-format template merge replaces `{{key}}` text in Word document and
auxiliary text parts, Spreadsheet string cells, and Presentation slides and
notes while preserving split-run formatting and reporting unresolved keys.

The native document engine does not require Microsoft Office, LibreOffice,
OfficeCLI, Python, Node.js, or .NET. LibreOffice may be used only by optional
CI interoperability checks and is never part of document execution. Optional
PNG screenshot output requires the `browser` feature and a ready A3S Browser
provider because it captures the native semantic HTML through the existing
Browser contract.

The explicit `office native` CLI exposes in-process blank creation, reads,
typed add/set/remove/move/copy/swap, rich-text, hyperlink, and legacy-comment
operations,
constrained raw XML access,
known typed part carriers, exact replay artifacts for a constrained canonical
subset, visible PNG/JPEG/GIF pictures, and atomic mutation batches, plus
dependency-free template merge and semantic rendering today. HTML and SVG are
available for Word, Spreadsheet, and Presentation; bounded annotated and issue
views are available for all three formats; and Browser-injected PNG screenshots
are available for all three formats. An authenticated, loopback-only foreground
watch provides full saved-revision refresh for all three formats without a
resident pipe or mutation endpoint. `mcp serve office-native` exposes the same
editor, annotated/issue analysis, and screenshot composition through typed
standard MCP tools and bounded in-memory sessions.
The packaged `a3s-use-office` Skill exposes the same product boundaries to
agents without starting OfficeCLI. Discover its metadata with
`office skills list`, read only its `SKILL.md` with
`office skills get a3s-use-office`, append its four format/MCP references with
`--full`, or locate the installed directory with `office skills path`. The
capability snapshot binds the Skill path and lowercase SHA-256 so a resident
host can verify the bytes before loading them.
Other `0.1.x` commands and the default `mcp serve office` target still use a
compatibility backend pinned to OfficeCLI `1.0.136`. This is a migration
boundary, not a native-promotion claim. The default routes will be promoted
only after mutation, fidelity, rendering, compatibility, and cross-application
interoperability gates pass.

```bash
# Inspect without downloading anything.
a3s use office doctor --json

# Create by extension; an existing destination is never overwritten.
a3s use office native create report.docx --json
a3s use office native create workbook.xlsx --json
a3s use office native create deck.pptx --json

# Read without OfficeCLI, Microsoft Office, or LibreOffice.
a3s use office native get report.docx /body --depth 2 --json
a3s use office native query report.docx 'p[style=Heading1]' --json
a3s use office native view report.xlsx stats --json
a3s use office native view report.docx issues --json
a3s use office native view workbook.xlsx issues --type formula_not_evaluated --limit 20 --json
a3s use office native view report.docx html --output report.html --json
a3s use office native view workbook.xlsx html --output workbook.html --json
a3s use office native view report.docx svg --output report.svg --json
a3s use office native view workbook.xlsx svg --output workbook.svg --json
a3s use office native view deck.pptx svg --output deck.svg --json
a3s use office native view report.docx screenshot --output report.png --timeout-ms 30000 --json
a3s use office native watch deck.pptx --port 0
a3s use office native validate deck.pptx --json

# Inspect a safely parsed XML part inline or export its original bytes.
a3s use office native raw report.docx /word/document.xml --json
a3s use office native raw report.docx /word/document.xml --output document.xml --json

# Replace one existing XML part; --output is an optional Office save-as target.
a3s use office native raw-set report.docx /word/document.xml --input document.xml --output updated.docx --json

# Create known part carriers and receive their owner relationship IDs.
a3s use office native add-part report.docx / --type header --json
a3s use office native add-part report.docx / --type chart --json
a3s use office native add-part workbook.xlsx /Sheet1 --type chart --json
a3s use office native add-part deck.pptx '/slide[1]' --type chart --json

# Replace text in place or save to a separate OOXML document.
a3s use office native set report.docx /body/p[1] --text 'Updated' --json
a3s use office native set report.xlsx /Sheet1/B2 --text '42' --output updated.xlsx --json

# Apply typed text formatting. Word and Presentation character properties use
# run paths; their alignment uses paragraph paths. Word sizes must be exact
# half-point increments. Spreadsheet ranges may combine content and formatting.
a3s use office native set report.docx '/body/p[1]/r[1]' --bold true --italic false --font-family Aptos --font-size 14 --text-color 123456 --json
a3s use office native set report.docx '/body/p[1]' --align center --json
a3s use office native set workbook.xlsx /Sheet1/A1:C1 --bold true --font-size 11.5 --text-color 0066CC --align center --json
a3s use office native set deck.pptx '/slide[1]/shape[1]/paragraph[1]/run[1]' --italic true --font-family 'Aptos Display' --font-size 20 --json
a3s use office native set deck.pptx '/slide[1]/shape[1]/paragraph[1]' --align center --json

# Add or update inert hyperlinks. External targets accept only absolute
# HTTP/HTTPS/mailto URIs without credentials. Word internal targets are bookmark
# names; Spreadsheet internal targets are workbook locations. Presentation
# currently supports only external, shape-wide links without separate display text.
a3s use office native add report.docx '/body/p[1]' --type hyperlink --url https://example.com/report --display 'Open report' --tooltip 'A3S report' --json
a3s use office native set report.docx '/body/p[1]/hyperlink[1]' --location section_1 --display 'Jump to section' --json
a3s use office native set workbook.xlsx /Sheet1/A1 --location 'Sheet1!B2' --display B2 --json
a3s use office native set deck.pptx '/slide[1]/shape[1]' --url https://example.com/slides --tooltip 'Open slides' --json
a3s use office native query report.docx hyperlink --json
a3s use office native remove report.docx '/body/p[1]/hyperlink[1]' --json

# Add, update, discover, and remove classic Office comments. Word uses a body
# paragraph or run anchor; Spreadsheet uses one cell; Presentation uses a slide
# and optionally accepts a complete x/y EMU coordinate pair.
a3s use office native add report.docx '/body/p[1]' --type comment --author Alice --initials AL --text 'Please reword this' --json
a3s use office native set report.docx '/comments/comment[1]' --author Bob --initials BO --text 'Reviewed' --json
a3s use office native add workbook.xlsx /Sheet1/B2 --type comment --author Alice --text 'Check this formula' --json
a3s use office native add deck.pptx '/slide[1]' --type comment --author Alice --initials AL --text 'Rework this slide' --x-emu 914400 --y-emu 457200 --json
a3s use office native query deck.pptx comment --json
a3s use office native remove workbook.xlsx /Sheet1/B2/comment --json

# Preserve Spreadsheet value types; formula storage requests application recalculation.
a3s use office native set workbook.xlsx /Sheet1/A1 --number 42.5 --json
a3s use office native set workbook.xlsx /Sheet1/B1 --boolean true --json
a3s use office native set workbook.xlsx /Sheet1/C1 --formula 'SUM(A1:B1)' --json

# Set or remove a bounded rectangular range atomically.
a3s use office native set workbook.xlsx /Sheet1/A2:C4 --number 0 --json
a3s use office native remove workbook.xlsx /Sheet1/B3:C4 --json

# Edit worksheet structure and ordering without invoking OfficeCLI.
a3s use office native insert-rows workbook.xlsx /Sheet1 2 --count 3 --json
a3s use office native delete-columns workbook.xlsx /Sheet1 B --count 2 --json
a3s use office native rename-sheet workbook.xlsx /Sheet1 'Q1 Data' --json
a3s use office native move-sheet workbook.xlsx '/Q1 Data' 1 --json
a3s use office native copy-sheet workbook.xlsx '/Q1 Data' 'Q1 Copy' --position 2 --json

# Add and remove native document structures.
a3s use office native add report.docx /body --type paragraph --text 'Summary' --json
a3s use office native add report.docx /body --type table --rows 2 --columns 3 --json
a3s use office native add report.docx '/body/tbl[1]' --type row --columns 3 --json
a3s use office native add report.docx '/body/tbl[1]/tr[3]' --type cell --text 'Total' --json
a3s use office native add workbook.xlsx / --type sheet --name Data --json
a3s use office native add deck.pptx / --type slide --text 'Results' --json
a3s use office native add deck.pptx '/slide[1]' --type shape --text '42%' --json
a3s use office native add deck.pptx '/slide[1]' --type table --rows 3 --columns 2 --json
a3s use office native set deck.pptx '/slide[1]/table[1]/tr[1]/tc[1]' --text 'Metric' --json
a3s use office native add deck.pptx '/slide[1]/table[1]' --type row --columns 2 --json
a3s use office native add deck.pptx '/slide[1]/table[1]' --type column --index 1 --text 'Q2' --json
a3s use office native set deck.pptx '/slide[1]/table[1]/col[2]' --width-emu 2000000 --json
a3s use office native move deck.pptx '/slide[1]/table[1]/col[1]' --after '/slide[1]/table[1]/col[2]' --json
a3s use office native remove deck.pptx '/slide[1]/table[1]/col[3]' --json
a3s use office native remove deck.pptx '/slide[1]/table[1]/tr[4]' --json
a3s use office native remove workbook.xlsx /Data --json
a3s use office native remove deck.pptx '/slide[1]/shape[2]' --json

# Embed bounded PNG, JPEG, or GIF data as a real DrawingML picture. Supplying
# one dimension preserves the source aspect ratio; supplying both is explicit.
a3s use office native add report.docx /body --type picture --input logo.png --name Logo --alt 'A3S logo' --width 320 --json
a3s use office native add workbook.xlsx /Sheet1/B2 --type picture --input chart.jpeg --width 480 --height 270 --json
a3s use office native add deck.pptx '/slide[1]' --type picture --input photo.gif --json
a3s use office native remove deck.pptx '/slide[1]/picture[1]' --json

# Arrange supported semantic nodes. --index is zero-based; --before and --after
# resolve stable pre-mutation paths. A copy defaults to immediately after its
# source.
a3s use office native move report.docx '/body/p[3]' --before '/body/p[1]' --json
a3s use office native copy workbook.xlsx '/Q1 Data' --name 'Q1 Copy' --after '/Q1 Data' --json
a3s use office native swap deck.pptx '/slide[1]' '/slide[3]' --json

# Merge a template into a separate output. JSON may be inline, @file, or an
# existing .json path. Existing outputs require an explicit --force.
a3s use office native merge template.docx report.docx --data @report.json --json
a3s use office native merge template.xlsx report.xlsx --data '{"quarter":"Q3"}' --json

# Apply a bounded, versioned mutation document atomically.
a3s use office native batch deck.pptx --input mutations.json --json

# Dump the exactly replayable root subset, then replay it into a native blank.
a3s use office native dump report.docx --output report.replay.json --json
a3s use office native create restored.docx --json
a3s use office native batch restored.docx --input report.replay.json --json

# Install the current compatibility provider explicitly.
a3s install use/office
a3s use office get report.docx /body --json
a3s use office batch report.xlsx --input updates.json --json

# Launch the explicit native standard MCP preview. No OfficeCLI is consulted.
a3s use mcp serve office-native

# Launch the current compatibility standard MCP server.
a3s use mcp serve office
```

The native MCP process exposes 12 typed tools: `office_validate`,
`office_create`, `office_open`, `office_list`, `office_get`, `office_query`,
`office_view`, `office_raw_xml`, `office_apply_batch`,
`office_merge_template`, `office_save`, and `office_close`. It accepts no shell
command string and defines no A3S RPC dialect; stdio carries only standard MCP.
Each process owns at most 64 sessions. Mutation batches are atomic in memory,
limited to 10,000 mutations and 8 MiB of JSON, and remain unsaved until
`office_save`. Results are limited to 8 MiB, raw XML responses to 1 MiB, and
queries to at most 1,000 returned nodes. `office_close` rejects dirty sessions
unless the caller saves or explicitly sets `discard=true`. `office_view`
accepts `html` and `svg` for all three formats in addition to text, annotated,
outline, and statistics. The typed `annotated` view flattens stable semantic
paths, node types, text, styles, and observed formatting; its `limit` is 1
through 1,000 and defaults to 200. The `issues` view accepts an optional typed
`issueType` and a `limit` from 1 through 1,000, defaulting to 200. It also
accepts `screenshot` for all three formats; that mode requires a no-clobber
local `output` ending in `.png` and accepts an optional `timeoutMs` from 1
through 120,000.

Native issue analysis is conservative and read-only. It currently reports
missing picture alternative text, missing or incompatible internal part
relationships, formulas without cached results, formulas that reference a
missing worksheet, cached or explicit formula errors, and low contrast between
explicit RGB run text and its own shape fill. Reports include stable category,
subtype, severity, path, context, and suggestion fields; filtering happens
before the bounded result window, and `count`, `returned`, and `truncated`
remain explicit. It does not infer text overflow, object overlap, theme or
inherited colors, or Microsoft Office layout behavior. A clean report is not a
full fidelity or interoperability certification.

Native render artifacts are deterministic, standalone, and network-free. They
contain no timestamp or source path, escape document text and attributes, carry
stable semantic paths as `data-path`, and embed only validated internal
PNG/JPEG/GIF parts as `data:` URLs. HTML declares a restrictive CSP and uses a
sparse observed-cell representation instead of expanding large Spreadsheet
gaps. Each render is bounded to 16 MiB while it is composed. CLI `--output`
publishes through an atomic no-clobber file operation; inline MCP output remains
subject to the stricter 8 MiB structured-result limit. These are semantic
previews, not a Microsoft Office layout-fidelity claim. Screenshot mode stages
the same deterministic HTML privately, opens its `file://` URL through the
existing `PageRenderer`, and validates one regular PNG plus its size and
SHA-256 receipt before atomic no-clobber publication. It defaults to a 30-second
deadline, caps the deadline at 120 seconds, and caps the PNG at 64 MiB. It does
not fetch external relationships or consult OfficeCLI.

`office native watch <file>` renders the same bounded all-format HTML, binds
only `127.0.0.1`, selects an ephemeral port by default, and prints a URL with a
fresh 256-bit capability token. Every page, status response, and standard SSE
stream requires that token or its HttpOnly same-site cookie and validates the
exact loopback `Host`. The wrapper runs its own fixed script while the document
preview stays in a sandboxed iframe under the renderer's script-free CSP.
Atomic saves from another `office native` process trigger a full refresh.
Transient missing or invalid revisions leave the last valid preview visible,
publish a typed error state, and retry until recovery. The server is read-only:
it has no mutation/RPC endpoint, never opens an external relationship, and
does not observe unsaved `office-native` MCP sessions until `office_save`.
`--timeout-ms` bounds automated runs; otherwise Ctrl+C stops the foreground
server. Interactive editing, selection/mark overlays, and layout goldens remain
open.

Native batch input is an ordinary JSON document, not an RPC protocol. The
current schema is:

```json
{
  "schemaVersion": 1,
  "mutations": [
    {
      "operation": "set-text",
      "path": "/body/p[1]",
      "text": "Updated"
    },
    {
      "operation": "add-paragraph",
      "parent": "/body",
      "text": "Summary"
    },
    {
      "operation": "remove",
      "path": "/body/p[2]"
    },
    {
      "operation": "move",
      "path": "/body/p[3]",
      "position": {
        "kind": "before",
        "path": "/body/p[1]"
      }
    }
  ]
}
```

The whole batch rolls back if any mutation fails. Inputs are limited to 8 MiB
and 10,000 mutations. The version 1 mutation set is `set-text`,
`set-text-format`, `set-hyperlink`, `set-comment`, `set-table-column-width`,
`set-cell-value`, `add-paragraph`,
`add-table`, `add-table-row`, `add-table-column`, `add-table-cell`,
`add-comment`, `add-worksheet`, `insert-rows`, `delete-rows`, `insert-columns`,
`delete-columns`, `rename-worksheet`, `move-worksheet`, `copy-worksheet`,
`move`, `copy`, `swap`, `replace-xml-part`, `add-part`, `add-slide`, `add-shape`,
`add-image`, and `remove`.

An image mutation uses the same versioned batch boundary:

```json
{
  "operation": "add-image",
  "parent": "/Sheet1/B2",
  "image": {
    "format": "png",
    "data": "<base64>",
    "name": "Logo",
    "altText": "A3S logo",
    "widthPx": 320
  }
}
```

The native engine validates decoded format signatures and structure, bounds
bytes and pixel dimensions, infers a missing axis from the source aspect ratio,
and inserts a visible OOXML picture object. Word uses an inline DrawingML run,
Spreadsheet uses a one-cell drawing anchor, and Presentation uses a slide
picture. Removal deletes the XML object and an unused image relationship, then
deletes the media part and content-type declaration only when no relationship
anywhere in the package still targets it. Direct CLI image inputs must be
regular, non-symlink files no larger than 64 MiB. Normal CLI output never
contains image data; `createdImage` and batch `createdImages` receipts contain
only paths, owner/media parts, relationship ID, format, and final dimensions.

OOXML SVG image embedding is not implemented yet; the all-format SVG semantic
preview is an output format and does not alter the package. Correct OOXML SVG
image support requires an SVG part plus a raster fallback rather than treating
SVG as an ordinary bitmap.

Image replacement, crop, rotation, effects, compression controls, floating
Word wrapping, Spreadsheet two-cell sizing, and rich layout rendering also
remain outside this bounded add/read/remove milestone.

`office native dump` produces a stricter versioned batch artifact, also as
ordinary JSON:

```json
{
  "format": "a3s.office.native-replay",
  "schemaVersion": 1,
  "documentKind": "word",
  "scope": "/",
  "base": "blank",
  "baseSha256": "<sha256-of-the-uncompressed-blank-part-map>",
  "resultSha256": "<sha256-of-the-uncompressed-result-part-map>",
  "mutations": []
}
```

The first dump scope is the complete document (`/`). It accepts only content
that current typed mutations can reproduce byte-for-byte at the OOXML part-map
level: plain Word paragraphs and rectangular tables, Spreadsheet worksheets and
typed cells without styles or cached formula results, and Presentation slides
with plain one-run text shapes and canonical basic tables. Headers, notes,
media, non-canonical table styling, rich text, non-canonical package resources,
and every other lossy case fail with
`use.office.dump_unsupported`; nothing is silently flattened or omitted.

Replay requires the exact A3S blank template identified by `baseSha256`.
`batch` checks that precondition before mutation and checks `resultSha256`
afterward. A failed result check restores the original in-memory package before
any save. Dump files are limited to 8 MiB and 10,000 mutations, refuse to
overwrite an existing path, and use a 1 MiB inline-output limit. This is a
portable Office batch artifact, not RPC and not a universal action envelope.

`office native merge` opens a `.docx`, `.xlsx`, or `.pptx` template, performs a
single-pass replacement, validates the resulting OPC/semantic document, and
atomically writes a separate output. The template and output may not identify
the same file. The output is no-clobber by default; `--force` is the only way to
replace an existing destination, and it never authorizes modifying the template
in place.

Merge data must be a JSON object. Literal top-level keys take precedence over
flattened nested paths: `{"user.name":"literal","user":{"name":"nested"}}`
resolves `{{user.name}}` to `literal`. Nested objects use dot paths and arrays
use bracket paths such as `{{items[0].name}}`. Replacement is deliberately
single pass, so a value containing `{{another.key}}` remains literal and is
reported as unresolved rather than recursively substituted. Results include the
replacement count, sorted used keys, sorted unresolved placeholders, and sorted
changed OOXML parts.

Word merge covers the main document, headers, footers, footnotes, endnotes, and
comments. Presentation merge covers slides and notes. Spreadsheet merge covers
inline strings, direct `t="str"` values, and referenced shared rich strings;
shared-string replacements are counted per referencing cell and phonetic runs
are left untouched. A resolved placeholder in a numeric, boolean, error, or
otherwise unsupported cell fails closed instead of coercing the cell type.

Data files are limited to 8 MiB and must be regular, non-symlink files. Flattened
data is additionally bounded by entry count, nesting depth, key length, and
total bytes. XML-forbidden replacement characters fail the whole in-memory
transaction before any output is created. This native path does not invoke
OfficeCLI, Microsoft Office, LibreOffice, Python, Node.js, or .NET.

Raw replacement is also available inside the same atomic batch:

```json
{
  "operation": "replace-xml-part",
  "part": "/word/document.xml",
  "xml": "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p/></w:body></w:document>"
}
```

Only existing XML parts may be replaced. `[Content_Types].xml`, root and
part-level relationship files, binary parts, DTDs, external entities, non-UTF-8
input, and replacement roots with a different local name or namespace are
rejected. Direct `raw-set` input is limited to 8 MiB. Inline `raw` output is
limited to 1 MiB; use its `--output` option to export larger original part bytes
without modifying the Office package. Raw export refuses to overwrite an
existing destination. Every replacement runs through the normal semantic and
OPC post-mutation validation, and any failure rolls back the whole batch.

Typed part creation is also batchable:

```json
{
  "operation": "add-part",
  "parent": "/slide[1]",
  "type": "chart"
}
```

The batch result keeps the existing ordered `paths` ledger, adds `swaps`
receipts containing the post-mutation `first` and `second` paths, and adds
`createdParts` receipts containing `part`, `ownerPart`, `relationshipId`, and
`type`. Word supports chart, header, and footer carriers at `/`; Spreadsheet
supports chart carriers under a worksheet; Presentation supports chart carriers
under a slide. A carrier is a valid blank XML part with content type and owner
relationship. It is not visible in document layout until a typed operation or
explicit XML replacement references the returned relationship ID from the
owner XML.

Typed formatting is batchable through the same public mutation contract:

```json
{
  "operation": "set-text-format",
  "path": "/Sheet1/A1:C1",
  "format": {
    "bold": true,
    "fontFamily": "Aptos",
    "fontSizeCentipoints": 1150,
    "textColor": { "red": 0, "green": 102, "blue": 204 },
    "alignment": "center"
  }
}
```

`fontSizeCentipoints` is an integer count of 1/100 point; the CLI accepts the
equivalent point value through `--font-size`. An empty `format` object, unknown
properties, invalid RGB components, and unsupported target/property
combinations fail the whole batch. Spreadsheet style records are cloned and
deduplicated without replacing unrelated style children or attributes.

Hyperlinks use the same typed batch contract and remain inert data:

```json
{
  "operation": "set-hyperlink",
  "path": "/body/p[1]",
  "hyperlink": {
    "target": {
      "kind": "external",
      "uri": "https://example.com/report"
    },
    "display": "Open report",
    "tooltip": "A3S report"
  }
}
```

Use `{ "kind": "internal", "location": "section_1" }` for a Word
bookmark or a Spreadsheet location such as `Sheet1!B2`. Word accepts a body
paragraph path when adding and the returned hyperlink path when updating.
Spreadsheet accepts one cell and creates it when absent. Presentation accepts a
shape or its hyperlink path, does not accept separate display text, and returns
`use.office.hyperlink_target_unsupported` for an internal slide jump. Removing
a hyperlink or its owning paragraph, cell, shape, or slide garbage-collects only
an unused hyperlink relationship. Strict and transitional OOXML namespaces are
preserved.

Legacy comments use their own typed batch variants rather than generic
properties:

```json
{
  "operation": "add-comment",
  "parent": "/slide[1]",
  "comment": {
    "author": "Alice",
    "text": "Review this slide",
    "initials": "AL",
    "position": { "xEmu": 914400, "yEmu": 457200 }
  }
}
```

Use `set-comment` with a partial `update` object and the stable returned path.
Word accepts author, initials, and plain text on main-document comments;
Spreadsheet accepts author and plain text for classic cell notes; Presentation
also accepts a complete `position`. Removing a comment uses the ordinary typed
`remove` mutation. Unknown OOXML attributes and extension nodes survive these
updates, and strict/transitional relationship and root dialects are retained.
Modern threaded comments and replies, resolved state, writable comment dates,
rich comment bodies, Word header/footer anchors, and Spreadsheet threaded
comments are intentionally not represented yet.

Typed Spreadsheet content values use an explicit nested type, for example:

```json
{
  "operation": "set-cell-value",
  "path": "/Sheet1/C1",
  "value": {
    "type": "formula",
    "expression": "SUM(A1:B1)"
  }
}
```

Formula mutation stores OOXML formula text and marks the workbook for a full
recalculation when opened. Structural edits rewrite supported A1 references
without evaluating formulas. A complete formula parser, dependency graph, and
evaluator remain a separate rich-Spreadsheet delivery gate.

The native package, semantic, and editor APIs are available directly to Rust
callers:

```rust,no_run
use a3s_use_office::{
    NativeOfficeComment, NativeOfficeCommentUpdate, NativeOfficeDocument,
    NativeOfficeEditor, NativeOfficeHorizontalAlignment, NativeOfficeHyperlink,
    NativeOfficeInsertPosition, NativeOfficePackage, NativeOfficeRenderFormat,
    NativeOfficeReplayArtifact, NativeOfficeRgbColor, NativeOfficeTextFormat,
};

# async fn inspect() -> Result<(), Box<dyn std::error::Error>> {
let mut package = NativeOfficePackage::open("report.docx").await?;
let document_xml = package.part("/word/document.xml")?;
println!("{} bytes", document_xml.len());

// Format engines will use this API without dropping unknown OOXML parts.
package.save().await?;

let blank = NativeOfficePackage::create("blank.xlsx").await?;
println!("created {:?}", blank.kind());

let document = NativeOfficeDocument::open("report.docx").await?;
println!("{}", document.text_view().text);
let html = document.render(NativeOfficeRenderFormat::Html)?;
println!("{} {} bytes", html.sha256, html.byte_length);
let headings = document.query("p[style=Heading1]")?;
println!("{} heading(s)", headings.len());

let mut editor = NativeOfficeEditor::open("report.docx").await?;
let raw = editor.raw_xml_part("/word/document.xml")?;
println!("{} {}", raw.part, raw.sha256);
let header = editor.add_part("/", a3s_use_office::NativeOfficePartType::Header)?;
println!("{} {}", header.part, header.relationship_id);
editor.set_text("/body/p[1]", "Updated")?;
editor.set_text_format(
    "/body/p[1]/r[1]",
    NativeOfficeTextFormat {
        bold: Some(true),
        font_family: Some("Aptos".into()),
        font_size_centipoints: Some(1400),
        text_color: Some(NativeOfficeRgbColor::new(0x12, 0x34, 0x56)),
        ..NativeOfficeTextFormat::default()
    },
)?;
editor.set_text_format(
    "/body/p[1]",
    NativeOfficeTextFormat {
        alignment: Some(NativeOfficeHorizontalAlignment::Center),
        ..NativeOfficeTextFormat::default()
    },
)?;
let hyperlink = NativeOfficeHyperlink::external("https://example.com/report")?
    .with_display("Open report")
    .with_tooltip("A3S report");
let hyperlink_path = editor.set_hyperlink("/body/p[1]", hyperlink)?;
println!("created {hyperlink_path}");
let comment_path = editor.add_comment(
    "/body/p[1]",
    NativeOfficeComment::new("Alice", "Please review this paragraph")?
        .with_initials("AL"),
)?;
editor.set_comment(
    &comment_path,
    NativeOfficeCommentUpdate {
        text: Some("Reviewed".into()),
        ..NativeOfficeCommentUpdate::default()
    },
)?;
let added = editor.add_paragraph("/body", "Summary")?;
let moved = editor.move_node(
    added,
    None,
    Some(NativeOfficeInsertPosition::at_index(0)),
)?;
let copied = editor.copy_node(&moved, None, None, None)?;
let swapped = editor.swap_nodes(moved, copied)?;
editor.remove(swapped.second)?;
let table = editor.add_table("/body", 2, 3)?;
editor.set_text(format!("{table}/tr[1]/tc[1]"), "Name")?;
editor.save().await?;

let mut template = NativeOfficeEditor::open("template.docx").await?;
let merge = template.merge_template(&serde_json::json!({
    "customer": {"name": "A3S Lab"}
}))?;
println!("{} replacement(s)", merge.replaced_count);
template.save_as_new("merged.docx").await?;

let replay = NativeOfficeReplayArtifact::dump(&editor.snapshot()?, "/")?;
let mut restored = NativeOfficeEditor::create("restored.docx").await?;
restored.apply_replay(&replay)?;
restored.save().await?;
# Ok(())
# }
```

Managed compatibility installation accepts only approved HTTPS release
origins, bounds the download, verifies the publisher SHA-256, stages outside
the active version, and activates atomically. Compatibility execution sets
`OFFICECLI_SKIP_UPDATE=1` so upgrades remain explicit A3S operations.

The native engine does not copy OfficeCLI's private resident protocol. The
explicit `office-native` target now provides typed in-process sessions over its
own standard MCP surface. Until the default-route promotion, a lost
compatibility response can return
`use.office.outcome_unknown`; callers must not retry it automatically.

See [Native Office Engine](docs/native-office.md) for the complete requirements,
compatibility scope, safety invariants, delivery gates, and migration plan.

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
             typed + driver  native OOXML     CLI / MCP / Skill
                              + 0.1 compat
                    │             │              │
                    └──────── capability snapshot/watch ───────► A3S Code

  a3s-search ── Arc<dyn PageRenderer> ──► a3s-use-browser

  a3s use box ── canonical executable supplied by a3s ──► A3S Box
```

The dependency arrows are intentional. Search links only the Browser contract,
so rendering does not require `a3s-use`, MCP, or a resident process. Office is
an in-process typed engine with a temporary 0.1.x compatibility process;
external domains retain their process boundaries. A3S Code consumes the
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
extension validation and route draining, capability snapshots, the native
OOXML package kernel, compatibility delegation, MCP schemas, packaged Skills,
and the locked agent-browser compatibility surface. Real-Chrome integration
gates run serially with isolated home and runtime directories on supported
platforms.

## License

A3S Use is licensed under the [MIT License](LICENSE). The Browser compatibility
driver contains work derived from `vercel-labs/agent-browser` under Apache-2.0;
see [Third-Party Notices](THIRD_PARTY_NOTICES.md) and
[Upstream Provenance](crates/browser-driver/UPSTREAM.md).
