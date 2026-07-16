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

# Read and mutate OOXML in-process with no external Office provider.
a3s use office native create report.docx --json
a3s use office native view report.docx text --json
a3s use office native query report.docx 'p[style=Heading1]' --json
a3s use office native set report.docx /body/p[1] --text 'Updated' --json
a3s use office native add report.docx /body --type paragraph --text 'More' --json
a3s use office native add report.docx /body --type table --rows 2 --columns 3 --json
a3s use office native remove report.docx /body/p[2] --json
a3s use office native raw report.docx /word/document.xml --json
a3s use office native add-part report.docx / --type header --json
a3s use office native dump report.docx --output report.replay.json --json
a3s use office native merge template.docx report.docx --data @report.json --json

# Commands not yet promoted to native continue through the compatibility route.
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
- **A3S-Native Office Foundation**: Own safe OOXML package, XML, relationship,
  selector, semantic read, transactional mutation, and cross-format template
  merge layers while retaining the 0.1.x OfficeCLI compatibility backend for
  commands not yet promoted
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
| Office | Built in | Stable Office vocabulary | Standard MCP server | Planned Office Skills | A3S Use native engine; OfficeCLI compatibility in 0.1.x |
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

Office is moving to an A3S-owned Rust engine for Word, Spreadsheet, and
Presentation documents. The native engine now includes bounded package
admission, byte-preserving XML, content types, a safe relationship graph,
stable selectors, semantic `get`, `query`, `text`, `outline`, and `stats`
reads, safe blank-document creation, and loss-preserving text replacement for
existing Word, Spreadsheet, and Presentation nodes. It can safely inspect
existing XML parts and replace non-OPC-metadata XML parts while preserving the
root QName and validating the final document. Known chart, header, and footer
part carriers can be created together with their content type and owner
relationship. The typed mutation layer
also adds and removes Word paragraphs and basic table/row/cell structures,
upserts typed Spreadsheet text, number, boolean, and formula cells, removes
cells and bounded cell ranges, structurally inserts or deletes rows and columns,
and adds, removes, renames, reorders, or copies worksheets. Worksheet copy
clones the owned OPC relationship subgraph while preserving shared workbook
resources; removal garbage-collects only unshared descendants. Structural
Spreadsheet edits rewrite affected A1 formulas, defined names, worksheet
metadata, tables, comments, VML notes, drawing anchors, and chart references;
unsupported pivot and 3D-reference cases fail closed before save. Presentation
slides and text shapes also support native add/remove. Saves are atomic and
reject a changed source revision instead of overwriting another writer.
Cross-format template merge replaces `{{key}}` text in Word document and
auxiliary text parts, Spreadsheet string cells, and Presentation slides and
notes while preserving split-run formatting and reporting unresolved keys.

The native runtime does not require Microsoft Office, LibreOffice, OfficeCLI,
Python, Node.js, or .NET. LibreOffice may be used only by optional CI
interoperability checks and is never part of document execution.

The explicit `office native` CLI exposes in-process blank creation, reads,
typed add/set/remove operations, constrained raw XML access, known typed part
carriers, exact replay artifacts for a constrained canonical subset, and atomic
mutation batches, plus dependency-free template merge today. Other `0.1.x`
commands and the
Office MCP route still use a compatibility backend
pinned to OfficeCLI `1.0.136`. This is a migration boundary, not the target
architecture. The default command route will be promoted only after mutation,
fidelity, rendering, and cross-application interoperability gates pass.

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
a3s use office native remove workbook.xlsx /Data --json
a3s use office native remove deck.pptx '/slide[1]/shape[2]' --json

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

# Launch the current compatibility standard MCP server.
a3s use mcp serve office
```

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
    }
  ]
}
```

The whole batch rolls back if any mutation fails. Inputs are limited to 8 MiB
and 10,000 mutations. The version 1 mutation set is `set-text`,
`set-cell-value`, `add-paragraph`, `add-table`, `add-table-row`, `add-table-cell`,
`add-worksheet`, `insert-rows`, `delete-rows`, `insert-columns`,
`delete-columns`, `rename-worksheet`, `move-worksheet`, `copy-worksheet`,
`replace-xml-part`, `add-part`, `add-slide`, `add-shape`, and `remove`.

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
with plain one-run text shapes. Headers, notes, media, styles, rich text,
non-canonical package resources, and every other lossy case fail with
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

The batch result keeps the existing ordered `paths` ledger and adds
`createdParts` receipts containing `part`, `ownerPart`, `relationshipId`, and
`type`. Word supports chart, header, and footer carriers at `/`; Spreadsheet
supports chart carriers under a worksheet; Presentation supports chart carriers
under a slide. A carrier is a valid blank XML part with content type and owner
relationship. It is not visible in document layout until a typed operation or
explicit XML replacement references the returned relationship ID from the
owner XML.

Typed Spreadsheet batch values use an explicit nested type, for example:

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
    NativeOfficeDocument, NativeOfficeEditor, NativeOfficePackage,
    NativeOfficeReplayArtifact,
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
let headings = document.query("p[style=Heading1]")?;
println!("{} heading(s)", headings.len());

let mut editor = NativeOfficeEditor::open("report.docx").await?;
let raw = editor.raw_xml_part("/word/document.xml")?;
println!("{} {}", raw.part, raw.sha256);
let header = editor.add_part("/", a3s_use_office::NativeOfficePartType::Header)?;
println!("{} {}", header.part, header.relationship_id);
editor.set_text("/body/p[1]", "Updated")?;
let added = editor.add_paragraph("/body", "Summary")?;
editor.remove(added)?;
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

The native engine will not copy OfficeCLI's private resident protocol. It will
provide typed in-process sessions and its own standard MCP surface. Until that
promotion, a lost compatibility response can return
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
