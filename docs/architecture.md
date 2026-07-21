# Architecture

## Domain boundary

Browser and Office are typed libraries and reserved built-in command routes.
The default binary cannot omit their command and diagnostic surfaces, although
provider runtimes may be missing.

Office's target runtime is an A3S-owned Rust engine. Its lowest layer is a
bounded, loss-preserving OPC/OOXML package kernel; format semantics build above
that layer. The released 0.1.x CLI retains an explicitly managed OfficeCLI
compatibility backend until the native promotion gates in
[Native Office Engine](native-office.md) pass.

Search depends directly on the object-safe PageRenderer contract in
a3s-use-browser. It never executes the CLI or requires a background service.

Provider selection is explicit. `DiscoveredChrome` is the default and never
downloads software. Only a `Managed*` provider or an explicit component install
authorizes a download. Managed downloads are restricted to approved HTTPS
hosts and redirects, bounded by size, hashed into an installation receipt,
staged outside the active version, and atomically activated. Lightpanda assets
must match the publisher SHA-256 exposed by GitHub Releases. Chrome for Testing's
current version feed does not publish an independent SHA-256 value, so its
receipt records HTTPS provenance and locally observed hashes without claiming
publisher checksum verification.

## Native extension surfaces

An external package declares any useful combination of:

- CLI: argv, stdin, stdout, stderr, and process status;
- MCP: standard MCP tools, resources, prompts, and lifecycle;
- Skill: an existing SKILL.md package.

The package manifest is a3s-use-extension.acl and is parsed by a3s-acl. A3S Use
owns identity, routes, trust, activation, and lifecycle around the surfaces. It
does not define JSON-RPC methods or convert surfaces implicitly.

`a3s-use-science` is the reference multi-surface extension. It remains a
separate process and package even though its source is developed in this
repository. Its Rust API, native CLI, 13 standard MCP tools, and packaged Skill
share typed source-specific operations; the host sees only the declared
`a3s/science` CLI, MCP, and Skill surfaces. This demonstrates how a first-party
toolkit can ship without expanding the reserved built-in route set or adding a
generic action envelope.

`a3s-use-ocr` implements the reserved first-party `ocr` route in the default
Use build. The release packages its content-bound Skill and exposes the native
CLI plus standard stdio MCP without a separate extension install. The process
accepts bounded local image files and binds every result to the canonical
source digest. It runs the pinned `PP-OCRv6_small` detection and recognition
models locally through ONNX Runtime, without Python, PaddlePaddle, a remote OCR
endpoint, or an alternate backend. Model installation and repair are explicit
`use/ocr` component operations. Both MCP tools are closed-world and read-only.

## Hot-plug registry

Extension code remains behind native process boundaries. The registry is a
derived, immutable route projection with a schema version and monotonic
generation. Validated receipts are the source of truth. Each lifecycle commit
writes its receipt first and atomically publishes the resulting registry
snapshot; reconciliation repairs a missing publication after a crash.

Install and upgrade stage into a unique immutable package directory. The
active receipt switches atomically, while existing calls keep a shared package
lease and continue against the generation they accepted. Disable and uninstall
publish an invisible route before waiting for the exclusive drain lease. Each
binding includes the immutable package root, so every changed activation is
observable even when its version and manifest digest are unchanged. This
ordering gives the lifecycle three explicit guarantees:

1. no new call is admitted after the disable generation is visible;
2. an accepted CLI or MCP process retains its package until it exits;
3. a drain timeout returns an error while the route remains disabled.

Consumers read `extension snapshot` for the current projection or long-poll
`extension watch --after-generation <n>` for a later generation. No daemon,
custom RPC protocol, `dlopen`, or restart is required.

Explicit local sources may be directories or bounded `.tar.gz`, `.tgz`, and
`.zip` archives. Archive extraction runs off the async executor, accepts one
manifest-rooted package, preserves executable permissions, and rejects links,
path traversal, duplicate entries, unsupported file types, and expansion beyond
the package limits before lifecycle activation begins. Standard bounded macOS
AppleDouble sidecars are ignored rather than installed; they still count toward
the archive entry and expanded-byte limits.

### Unified capability projection

Resident Code hosts do not need separate discovery paths for built-in and
external domains. `capability snapshot` projects Browser, native Office, OCR,
Box, and enabled extensions through one schema while preserving each binding's
`built-in` or `extension` origin. `capability watch` accepts both the extension
generation and a content revision. The generation advances for extension
lifecycle commits; the SHA-256 revision also detects built-in provider
readiness and packaged Skill changes when the extension generation remains
unchanged. Each Skill projection includes an absolute package path and its own
lowercase SHA-256, allowing a resident host to reject raced or modified bytes
before replacing its live Skill.
The default distribution projects the Browser, first-party `a3s-use-office`,
and first-party `a3s-use-ocr` Skills. `office skills list|get|path` exposes the
Office Skill as bounded local CLI reads; it never launches the OfficeCLI
compatibility provider. For resident hosts, `use/office` targets the built-in
`office-native` MCP server and is ready independently of OfficeCLI. A discovered
OfficeCLI provider is projected separately as `use/office-compat`, targeting
the standard compatibility server without carrying the native Skill. The
`use/ocr` route targets `ocr-native`; model readiness remains explicit and
never triggers a silent install.

The projection contains content-bound Skill references and an MCP launch target,
never executable extension code or a generic action payload. Consumers still
start `a3s-use mcp serve <target>` as a standard MCP server and load `SKILL.md`
through their native Skill registry. The capability commands are versioned JSON
CLI output, not a new RPC transport.

## Component-backed routes

`box` is a reserved Use route backed by the independently managed A3S Box
component. The umbrella CLI remains the only Box installer and receipt owner.
For `a3s use box ...`, it resolves or installs Box, canonicalizes its executable
path, and passes that path to Use for one child invocation. Use validates the
absolute regular executable and delegates argv, streams, working directory,
environment, and exit status. It does not discover Box on `PATH`, copy it, or
create a wrapper package. Other Use commands never auto-install Box.

## Persistent sessions

Browser exposes one typed session manager through the standard MCP tool set.
`mcp serve browser` provides stdio for MCP hosts. Separate Browser CLI
invocations connect to a managed standard MCP Streamable HTTP deployment so
that tabs, snapshots, and element references survive the invoking CLI process.
The deployment:

- binds only to an ephemeral `127.0.0.1` port;
- requires a random bearer token and validates `Origin` when present;
- records endpoint, token, PID, and ownership in a private generated receipt;
- shares one typed `BrowserSessions` instance across MCP client sessions;
- has bounded idle and maximum lifetimes;
- stops through a standard MCP tool and cleans up tabs, Chrome, and its receipt.

This is a deployment of the existing MCP server, not an A3S JSON-RPC service.
CLI session commands call MCP tools with their published schemas. The token is
never included in normal CLI output. `browser render` and Search remain direct
in-process Rust calls and never require the service.

The native Office engine uses typed in-process sessions and exposes them through
the explicit `mcp serve office-native` standard MCP preview. It does not copy an
upstream private pipe protocol. The packaged `a3s-use-office` Skill selects
this native route first, loads format-specific references progressively, and
documents the explicit compatibility fallback without changing authority or
starting a provider. During the 0.1.x migration, Office blank creation, reads,
typed add/set/remove/move/copy/swap operations, constrained raw XML access,
bounded annotated and issue analysis, typed text formatting, typed Spreadsheet
cell presentation, data validation, conditional formatting, scoped defined
names, worksheet/table AutoFilters, ListObject tables, exact merged-cell
editing, typed scoped text
replacement, typed hyperlinks, and atomic mutation batches
are available explicitly under `office native`. Annotated views flatten the
shared semantic tree with stable paths and bounded observed formatting; the
same typed contract reads unsaved native MCP session state without a private
resident protocol. That route also owns bounded Spreadsheet
range mutation, row/column insertion and deletion, and worksheet
rename/reorder/copy with
loss-preserving OPC subgraph ownership. Its arrangement layer covers
same-parent Word structures, worksheets and dense plain rows, slides, and
same-slide top-level Presentation objects. Ownership- or reference-bearing
cases outside those bounds fail closed. Safe raw XML inspection and constrained
existing-part replacement use the same typed editor, post-mutation validation,
and rollback boundary. Known chart/header/footer part
carriers atomically update content types and owner relationships and return
typed creation receipts. Root-scoped native replay artifacts bind an exact
blank-template part-map fingerprint, typed mutations, and an expected result
fingerprint; `batch` rejects a wrong base and rolls back a wrong result.
Rich-text mutation is one closed Rust enum variant rather than a generic
property envelope. Bold, italic, typed underline, typed vertical script, font
family, centipoint size, RGB color, and horizontal alignment flow unchanged
through Rust, batch JSON, CLI parsing, standard MCP schemas, and the Office
Skill. Word and Spreadsheet also accept explicit single strikethrough. Word and
Presentation share typed display case, a portable highlight palette, and a
primary language tag; Word additionally accepts double strikethrough.
Format-specific unsupported combinations return stable errors. Word and
Presentation patch run or paragraph properties in place. Spreadsheet clones and deduplicates
`fonts` and `cellXfs`, retaining unknown style data and the document's strict
or transitional OOXML dialect.
Spreadsheet cell presentation is a second closed mutation variant instead of
an extension to the text-format object. Number format, solid/cleared fill,
typed cardinal/diagonal borders, vertical alignment, wrapping, rotation,
indentation, shrink-to-fit, and reading order flow through the same Rust,
versioned batch JSON, CLI, standard MCP, and Skill surfaces. The style layer
reuses built-in number-format IDs, deduplicates custom `numFmts`, `fills`,
`borders`, and `cellXfs`, merges only owned alignment and border properties,
and preserves unknown style data. Content, text format, cell format, and
hyperlink changes can share one editor transaction; validation and
post-mutation semantic reads occur before any atomic save. This remains a typed
Spreadsheet contract, not a generic property map or a private RPC method.
Spreadsheet data validation is another closed domain value, represented by
`add-data-validation` and `set-data-validation`; deletion reuses typed
`remove`. Seven rule families, eight comparison operators, three error styles,
disjoint A1 areas, formulas, messages, blank handling, and dropdown visibility
cross Rust, versioned batch JSON, CLI, standard MCP, replay, and Skill surfaces
without a property bag. The format layer normalizes inline lists, dates, times,
and A1 ranges, rejects every overlapping area, and bounds both rules and areas.
The loss-preserving writer retains the worksheet dialect and unknown rule
attributes, failing closed when replacement or final collection removal would
discard unknown children or collection data. Semantic projection adds stable
`dataValidation` nodes plus sparse observed/virtual-cell annotations; HTML and
SVG carry only inert metadata. This contract neither evaluates formulas nor
implies filter criteria, sort state, chart, or pivot support.
Spreadsheet conditional formatting is a separate closed domain value,
represented by `add-conditional-format` and `set-conditional-format`; deletion
reuses typed `remove`. Comparison/formula rules, text and statistical
predicates, blank/error/date predicates, data bars, two/three-color scales, and
standard three/four/five-icon sets cross Rust, versioned batch JSON, CLI,
standard MCP, replay, and Skill surfaces without a property bag. Thresholds,
operators, time periods, icon sets, RGB colors, and differential fill/font/bold
formatting are closed types. The writer allocates collision-free priorities and
deduplicates `dxfs`; canonical replay requires sequential priorities. It retains
strict/transitional SpreadsheetML and unknown
attributes, and fails closed when mutation would discard unsupported rule or
collection content. Semantic projection adds stable `conditionalFormatting`
nodes at `/Sheet/cf[N]` and supports typed `type` selectors. Formula storage is
not formula evaluation; x14-only data-bar axes/colors and Excel rendering
fidelity remain outside this contract.
Spreadsheet defined names are a separate closed value represented by
`add-named-range` and `set-named-range`; deletion reuses ordinary typed
`remove`. Workbook-global and worksheet-local scopes map explicitly to the
OOXML `localSheetId` identity. Stable paths include the percent-encoded name and
scope, while name-only and positional compatibility selectors remain available
when they are unambiguous. The escaped scope `worksheet:workbook` preserves the
distinction for a worksheet literally named `workbook`. Identifier, ref, comment, and collection limits are
enforced before mutation. Workbook-scoped bare A1 refs, leading `=`, unsupported
cross-workbook refs, duplicate `(name, scope)` identities, and collisions with
ListObject table names fail atomically. `_xlnm.*` and `Slicer_*` names remain
owned by their higher-level Office features and cannot be edited through this
contract. Semantic get/query, Rust, versioned batch JSON, CLI, standard MCP,
exact replay, and the Office Skill share the same typed value. The
loss-preserving writer retains strict/transitional SpreadsheetML and unknown
defined-name attributes and fails closed for unknown collection or child
content. This capability stores defined-name expressions and requests
recalculation; it neither evaluates formulas nor authors external links.
Spreadsheet AutoFilters are a separate closed value represented by
`add-spreadsheet-auto-filter` and `set-spreadsheet-auto-filter`; deletion reuses
ordinary typed `remove`. Worksheet filters own one A1 range, while ListObject
tables embed the same unique zero-based filter-column values. Exact value sets,
text/comparison predicates, ranges, blanks, top/bottom limits, and dynamic
average/date/month/quarter families cross Rust, versioned batch JSON, CLI,
standard MCP, exact replay, and the Office Skill without an action envelope or
property bag. Semantic projection exposes `AutoFilter`, `FilterColumn`, and
`FilterValue` nodes at stable worksheet and table-owned paths. The
loss-preserving writer retains strict/transitional SpreadsheetML and fails
closed for unknown attributes/comments, date-group/color/icon criteria,
extensions, and embedded sort state. Persisted physical row sorting remains a
separate typed contract.
Spreadsheet ListObject tables are another closed value represented by
`add-spreadsheet-table` and `set-spreadsheet-table`; deletion reuses ordinary
typed `remove`. The value owns workbook-wide name/display identity, one final A1
range, exact ordered column names, header/totals state, typed filter criteria, a
closed built-in style, and its style display flags. Identity, range width,
data-row, table overlap, merge overlap, worksheet-AutoFilter overlap, filter
column, and shared defined-name namespace invariants are checked before
mutation. The OPC layer owns table parts, content types, worksheet
relationships, `tableParts`, header stamping, and the table-owned AutoFilter
range. Semantic projection exposes stable `/Sheet/table[N]`, child column, and
table AutoFilter nodes. Rust, versioned batch JSON, CLI, standard MCP, exact
replay, and the Office Skill share the same value. The loss-preserving writer
retains strict/transitional dialects and supported unknown root/style content,
while calculated columns, totals functions, date-group/color/icon filters,
persisted sort state, custom styles, query tables, external data, and unsafe
relationship graphs fail closed rather than becoming generic properties.
Merged cells are another closed Spreadsheet mutation variant, represented by
`merge-cells` and `unmerge-cells`, rather than a style flag or universal action.
The editor normalizes A1 ranges, rejects geometric overlap and ListObject table
intersection, and makes exact repeated merges idempotent. Unmerge removes only
one exact range; non-exact intersecting requests return the valid exact ranges
instead of sweeping them. The loss-preserving XML patch keeps schema order,
OOXML dialect, and unknown merge collection data, failing closed when the final collection
cannot be removed losslessly. Semantic projection adds stable `mergeCell`
nodes and virtual blank covered cells without expanding the worksheet. Ordered
range sweeps keep validation and observed-cell annotation bounded at the
100,000-merge admission limit. The same variants cross versioned batch JSON,
CLI, standard MCP, replay dump, and Skill surfaces without a private protocol.
General text replacement is a separate closed mutation variant. A compiled
literal or Rust-regex matcher feeds the shared split-segment patch layer, which
maps every matched byte span back to its original OOXML text owner and assigns
inserted text to the first matched run. Format engines supply bounded semantic
scopes: Word document and auxiliary parts or narrower content paths,
Spreadsheet workbook/worksheet/cell/range string values, and Presentation
slides, objects, text descendants, or notes. Spreadsheet first computes shared
string reference multiplicity. A partial scope clones the lossless rich-string
item and rewrites only selected cell indexes; a whole-reference scope patches
the original once while counting matches per cell. This prevents alias leakage
without flattening rich runs, phonetic records, or unknown extensions. The same
receipt (`matchCount`, `changed`, and `changedParts`) crosses Rust, versioned
batch JSON, CLI, standard MCP, and the packaged Skill. Match, output, regex,
and cell-scope bounds are enforced before the editor's normal semantic
validation and atomic rollback boundary.
Hyperlink mutation is another closed typed contract shared by the Rust API,
versioned batch JSON, CLI, standard MCP schema, and Office Skill. Word owns
external HTTP/HTTPS/mailto relationships and internal bookmark anchors in body,
header, and footer parts; Spreadsheet owns external relationships and internal
locations on cells or bounded rectangular ranges; Presentation owns external
shape-wide click relationships and internal jumps to existing slides. URI
validation rejects credentials, active or relative schemes, controls, and malformed targets. External links
remain inert throughout semantic reads and rendering. Format engines allocate,
reuse, and garbage-collect hyperlink or slide relationship IDs without deleting
shared edges and preserve strict or transitional OOXML namespaces. Spreadsheet
rejects overlapping hyperlink ranges so semantic paths and updates remain
unambiguous.
Legacy-comment mutation is a third closed typed contract shared by the Rust
API, versioned batch JSON, CLI, standard MCP schema, and Office Skill. Word owns
main-document paragraph/run anchors and `word/comments.xml`; Spreadsheet owns
classic cell notes, author tables, and VML drawings; Presentation owns legacy
slide comments and the shared author list. Semantic paths remain format-native,
and ordinary `remove` plus owner removal garbage-collect only owned resources.
The XML patch layer preserves unknown attributes and extension nodes and keeps
strict/transitional dialects. Modern threaded comments, replies/resolution,
writable dates, rich bodies, and Word header/footer anchors remain explicit
future contracts rather than untyped properties.
Native template merge is a typed editor operation, not a generic action
envelope. The CLI boundary parses bounded JSON data, the format engines replace
text across Word document/auxiliary parts, Spreadsheet string cells, or
Presentation slides/notes, and the editor validates the complete result before
the package layer atomically creates a separate output. Split OOXML text runs
retain their original ownership; inserted values are never recursively
evaluated. The default output path is no-clobber, template/output identity is
rejected, and `--force` authorizes only destination replacement.

Raster image mutation uses a shared media ownership layer below the three
format engines. PNG, JPEG, and GIF bytes are validated and bounded before any
package mutation. Word owns an inline drawing relationship from its main part;
Spreadsheet owns the image relationship from a worksheet drawing part and
anchors the picture to a cell; Presentation owns it from the slide part. Each
format exposes the result as a semantic `Picture` node with stable path,
relationship ID, name, alternative text, and pixel dimensions. Removal first
edits the owner XML, then drops an unreferenced relationship, and garbage
collects media only when the package relationship graph has no remaining target
edge. The media changes share the editor's atomic rollback boundary. SVG is a
separate future package representation because OOXML requires a raster
fallback; this is distinct from the read-only all-format SVG semantic view.

Semantic rendering is a read-only layer over the same document tree and OPC
relationship graph. It produces deterministic standalone HTML and SVG for all
three formats, carries stable `data-path` metadata, and embeds only validated
internal raster parts as `data:` URLs. Word SVG stacks semantic regions and
blocks; Spreadsheet SVG projects observed cells sparsely; Presentation SVG
uses its slide geometry. External
relationships are never fetched. Render composition has a 16 MiB bound; CLI
artifact publication is atomic and no-clobber, while standard MCP applies its
stricter 8 MiB result bound. The Office crate remains browser-independent. At
the root facade, screenshot composition stages that HTML privately and injects
its `file://` URL plus a temporary PNG destination into the existing typed
Browser `PageRenderer`. The facade validates one regular PNG and its provider
size/SHA-256 receipt, applies a 64 MiB artifact bound, and publishes the caller's
destination atomically without clobbering. It does not add another browser
runtime to Office, and the result remains a semantic preview rather than a
layout-fidelity render.

Native live watch is a root-facade deployment of the same deterministic HTML,
not another Office engine. It validates and renders before binding an ephemeral
`127.0.0.1` port, authenticates every standard HTTP/SSE request with a fresh
256-bit capability token or HttpOnly same-site cookie, validates the exact
loopback Host, and keeps document HTML inside a sandboxed iframe. A bounded
poller detects saved file revisions, reopens them through the package kernel,
and atomically swaps the in-memory preview only after a complete render. Failed
or partial revisions retain the last valid bytes and emit a typed error state
until recovery. There is no write endpoint, resident pipe, custom RPC dialect,
external relationship fetch, or OfficeCLI/LibreOffice dependency. Unsaved MCP
session state becomes visible only after `office_save`.

Issue analysis is another read-only projection over the same semantic tree and
relationship graph. It returns bounded typed records with stable category,
subtype, severity, semantic path, context, and suggestion fields. The initial
rules cover missing image alternative text, broken typed relationships,
uncached formulas, missing-sheet formula references, formula errors, and
explicit shape-fill/text RGB contrast. Filtering precedes the 200-default,
1,000-maximum result window. The scanner deliberately avoids inferred layout
diagnostics such as text overflow, overlap, pagination, theme resolution, or
application-specific rendering; a clean result is not a fidelity claim.

Basic Presentation table mutation is a format-owned structural layer over the
same loss-preserving XML editor. It inserts a real graphic frame and DrawingML
table into the slide shape tree, allocates a collision-free non-visual ID,
keeps row width aligned with `a:tblGrid`, and updates graphic-frame height after
row changes. Empty cell text insertion preserves DrawingML child ordering.
Columns are virtual semantic nodes backed by one grid column plus one cell per
row. Insert, remove, same-table move/copy/swap, and positive-EMU width mutation
update those physical elements in lockstep and keep graphic-frame width equal
to the grid-width sum. Operations that would underfill a normal row or require
merged-span rewriting fail before save; Presentation table merge editing
remains outside this bounded milestone. These mutations use the existing typed
batch transaction and do not introduce another protocol or runtime.

Unpromoted commands are delegated to OfficeCLI and
`mcp serve office-compat` launches its standard MCP server; `mcp serve office`
remains a legacy alias. That compatibility process remains isolated from the
native engine. `mcp serve office-native` instead runs the A3S-owned server in
process and never discovers or starts OfficeCLI. Resident capability projection
uses the native target canonically and advertises compatibility as a distinct
optional route.

The preview MCP adapter has an explicit typed vocabulary rather than a command
string passthrough. It supports validate, create/open/list, semantic get/query,
bounded annotated plus text/outline/statistics views, all-format HTML and SVG,
all-format bounded issue views, Browser-injected semantic PNG screenshots,
constrained raw XML inspection, atomic typed mutation batches, including typed
Spreadsheet data-validation and conditional formatting, scoped defined names,
worksheet/table AutoFilters, ListObject tables, and exact merge/unmerge,
immutable-template merge, save, and close. A screenshot requires an explicit
no-clobber `.png` output and
releases the Office session lock before Browser rendering. A server process
owns at most 64 sessions. Batches and structured results are limited to 8 MiB,
batches to 10,000 mutations, query, annotated, and issue output to 1,000
records, and inline raw XML to 1 MiB. Mutations remain in memory until an
explicit save, while
close fails on dirty state unless discard is explicit. These are MCP deployment
rules around the same editor types, not a second Office domain model or an A3S
RPC protocol.

External MCP packages are launched from their declared executable, arguments,
and transport. A3S Use owns package identity and activation, not the package's
MCP tool vocabulary. The managed Browser deployment does not aggregate,
translate, or proxy Office and extension tool vocabularies.

## Component CLI contract

The umbrella CLI delegates runtime lifecycle through ordinary commands:

    a3s-use component list --json
    a3s-use component status browser --json
    a3s-use component install browser --json
    a3s-use component install office --json
    a3s-use component uninstall office --json

Each invocation accepts argv and returns one versioned JSON document plus an
exit status. This is CLI automation, not JSON-RPC.

In 0.1.x, managed Office installation means the reviewed OfficeCLI compatibility
release. It is fetched only by an explicit install or repair command, restricted
to approved HTTPS hosts, bounded by size, and checked against the publisher's
SHA-256 before atomic activation. Compatibility execution sets
`OFFICECLI_SKIP_UPDATE=1`; A3S upgrades are explicit component operations.

After native promotion, Office is built in and this component command no longer
downloads an engine. The compatibility backend moves to an explicitly named
component for one deprecation cycle before removal.

## Roadmap

Implemented:

1. Core, Browser, Office, OCR, extension, and component contracts.
2. Chrome and Lightpanda extraction from Search.
3. Search injection through `Arc<dyn PageRenderer>`.
4. Typed Browser rendering and session tools over standard MCP stdio.
5. Native OfficeCLI delegation, pinned installation, and non-retryable
   ambiguous-write handling.
6. Direct standard-MCP launch for Office and external packages.
7. Authenticated loopback standard MCP Streamable HTTP for persistent Browser
   CLI sessions.
8. The complete locked agent-browser `0.32.1` command, MCP, Skill, Dashboard,
   lifecycle, and interactive Browser surface behind `a3s use browser`.
9. Generation-based extension hot plug with enable, disable, watch, graceful
   route draining, and crash reconciliation.
10. The component-backed `a3s use box` route with one Box binary and receipt.
11. A unified generation/revision capability projection for built-in and
    external MCP and Skill surfaces.
12. The native Office OPC/OOXML package kernel with bounded admission,
    document-kind verification, unknown-part preservation, and atomic save.
13. Native content-type and relationship graphs, safe loss-preserving XML,
    common selectors, semantic Word/Spreadsheet/Presentation reads, safe blank
    creation, scoped cross-format literal/regex replacement with split-run and
    shared-string safety, typed cross-format underline and vertical-script
    formatting, Word/Presentation highlight, text case, and language, plus
    format-bounded strikethrough, typed Spreadsheet number/fill/border/alignment
    and cell-presentation formatting, typed Spreadsheet data-validation
    add/set/remove with sparse semantic readback, typed Spreadsheet
    conditional-format add/set/remove with stable semantic readback, typed
    scoped Spreadsheet defined-name add/set/remove with stable semantic
    selectors, typed worksheet/table Spreadsheet AutoFilters with closed
    criteria, stable filter-column paths, and exact replay, typed Spreadsheet
    ListObject add/set/remove with stable table and column paths, exact
    Spreadsheet merged-cell editing,
    and typed Spreadsheet text/number/boolean/formula cell and range mutation,
    typed Word/Spreadsheet/Presentation hyperlink
    read/add/update/remove with inert external targets, typed legacy comment
    read/add/update/remove with format-owned anchors, authors, positions, and
    resource cleanup, Word paragraph and
    bounded table/row/cell mutation,
    worksheet add/remove/rename/reorder/copy with owned OPC-subgraph cloning and
    cleanup, bounded cross-format move/copy/swap arrangement, Spreadsheet
    row/column structural edits with formula and related-part reference
    rewriting, Presentation slide/shape and DrawingML table row/cell/column
    mutation,
    core node removal, constrained raw XML inspection/replacement, typed
    chart/header/footer part carriers, exact root replay artifacts for the
    canonical typed subset including basic Presentation tables, cross-format
    native template merge with bounded JSON
    and immutable templates, native PNG/JPEG/GIF add/read/remove with
    reference-aware media cleanup, deterministic all-format HTML and SVG
    semantic rendering, bounded all-format annotated views, bounded
    conservative all-format issue analysis, Browser-injected all-format
    semantic PNG screenshots with
    validated receipts and no-clobber publication, authenticated loopback
    all-format live watch with saved-revision refresh, atomic batches,
    changed-file conflict detection, and the dependency-free `office native`
    CLI.
14. An explicit native Office standard MCP preview with bounded typed tools,
    in-process sessions, deferred atomic save, dirty-close protection, and
    process-level evidence that OfficeCLI is not consulted.
15. A packaged first-party `a3s-use-office` Skill with progressive
    Word/Spreadsheet/Presentation/MCP references, bounded local discovery,
    release-archive smoke checks, and content-bound capability projection.
16. TUF-verified remote extension registries with pinned bootstrap roots,
    expiration and rollback enforcement, exact review/apply plans, and signed
    provenance receipts. Registry upgrades restore the recorded source and
    channel, reject identity drift and version downgrades, and converge before
    payload download when the installed signed target is already current.
17. A first-party built-in OCR route with typed PP-OCRv6 diagnostics, bounded
    image admission, source SHA-256 evidence, native ONNX Runtime detection and
    recognition, standard closed-world MCP annotations/output schemas, pinned
    release models, and a content-bound Skill that projects to
    `mcp__use_ocr__*` in A3S Code.

Next:

1. Complete native read interoperability and repair-dialog evidence against
   Microsoft Office and the optional CI LibreOffice oracle.
2. Native Office mutation, formula, rich-format, interactive-watch parity,
   layout goldens, MCP promotion, and compatibility gates defined in
   `docs/native-office.md`.
3. Windows real-Chrome persistent sessions. Windows remains a preview build
   until separate `a3s use browser` invocations can open and reuse a session
   with the same runtime guarantees as macOS and Linux. Windows compilation,
   CLI/MCP schemas, packaged assets, and non-runtime tests remain continuously
   checked in CI meanwhile.
4. Production publication for the official A3S extension registry, including
   an offline-held root-key policy and release automation. The client does not
   substitute a placeholder or generated key for that operational trust root.
