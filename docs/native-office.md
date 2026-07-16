# Native Office Engine

## Decision

A3S Use will own its Office document engine. The steady-state runtime must read,
create, modify, validate, and render `.docx`, `.xlsx`, and `.pptx` without an
OfficeCLI, Microsoft Office, LibreOffice, Python, Node.js, or .NET dependency.

LibreOffice is not a fallback engine, renderer, formula evaluator, conversion
service, or installation prerequisite. CI may invoke it only as an optional
external interoperability oracle; shipped binaries and normal tests must work
without it.

The `a3s use office` route, typed Rust API, standard MCP server, and packaged
Skills remain stable product surfaces. OfficeCLI `1.0.136` is a temporary
compatibility backend and a black-box behavior baseline while native coverage
is built. It is not the target architecture.

The native implementation is written in Rust from the OOXML/OPC specifications
and public compatibility behavior. Do not mechanically translate upstream C#
source. Synthetic fixtures and differential black-box tests may be used to
measure compatibility.

## Product scope

The native engine must provide these common capabilities across all three
formats:

- safe OPC/ZIP package loading and atomic persistence;
- loss-preserving round trips for unknown parts, relationships, elements, and
  attributes;
- create, semantic view, get, query, set, add, remove, move, copy, and swap;
- typed selectors with stable, one-based document paths;
- transactional batch mutation and explicit partial-apply compatibility mode;
- template merge and replayable dump/batch documents;
- raw part access, constrained XML mutation, part creation, and validation;
- open, save, close, revision tracking, and conflict detection;
- text, outline, statistics, issue, HTML, SVG, and screenshot views;
- standard MCP tools and Office-specific Skills backed by the same typed engine.

The compatibility command baseline is OfficeCLI `1.0.136`, commit
`4ba79f0b984e141f57f58d4398ba2df29e8187e8`. Product parity covers document
operations, not OfficeCLI's installer, updater, agent-configuration writers, or
private resident-pipe protocol.

### Word

Word coverage includes paragraphs, runs, styles, numbering, tables, sections,
headers and footers, hyperlinks, bookmarks, images, shapes, text boxes, fields,
TOC, equations, comments, footnotes and endnotes, content controls, form fields,
revisions, charts, OLE preservation, document properties, international fonts,
BCP-47 language tags, and RTL layout.

### Spreadsheet

Spreadsheet coverage includes workbooks, worksheets, cells, ranges, row and
column mutation, formulas, defined names, styles and number formats, tables,
sorting, filters, validation, conditional formatting, drawings, images, charts,
pivot tables and caches, slicers, sparklines, comments, OLE preservation, and
CSV/TSV import.

The formula subsystem requires a real parser, dependency graph, recalculation
engine, dynamic-array spilling, reference rewriting, and a typed function
registry. Formula values are never evaluated by a shell or general-purpose
script runtime.

### Presentation

Presentation coverage includes slides, masters, layouts, themes, placeholders,
shapes, text, groups, connectors, tables, images, charts, audio and video, OLE
preservation, 3D model parts, equations, diagrams, notes, comments, animations,
transitions, morph metadata, and slide zoom.

## Architecture

```text
CLI / standard MCP / Skill / typed Rust API
                    |
             Office command layer
                    |
       session + transaction coordinator
                    |
   selector / semantic model / validation
        /              |              \
     Word          Spreadsheet     Presentation
                        |
             formula + pivot engines
        \              |              /
       relationships / media / charts / themes
                    |
        loss-preserving XML part model
                    |
         safe OPC/ZIP package kernel
                    |
              .docx/.xlsx/.pptx
```

The package kernel owns archive safety, canonical part names, document-kind
identification, bounded memory admission, unknown-part preservation, and atomic
save. It does not contain format-specific selectors or mutations.

Format engines expose typed operations. CLI and MCP payloads are boundary
adapters and must parse into those operations; they are not the domain model.
There is no cross-domain action envelope and no A3S JSON-RPC protocol.

Rendering is separate from document mutation. Semantic HTML/SVG renderers
produce deterministic artifacts. Screenshot generation injects the existing
`a3s-use-browser` rendering contract instead of embedding a second browser
runtime in the Office engine.

## Safety and fidelity invariants

1. Opening a document never downloads a relationship target or executes a
   macro, OLE payload, formula, field, or embedded script.
2. Archive entry count, archive bytes, expanded bytes, part bytes, and
   compression ratio are bounded before semantic parsing.
3. Absolute, traversal, control-character, symbolic-link, encrypted, and
   case-ambiguous part names are rejected.
4. DTDs and external XML entities are rejected. Namespace prefixes, unknown
   attributes, `mc:AlternateContent`, and untouched parts survive round trips.
5. External relationships are data. Network access requires a separate,
   explicit policy and remains disabled by default.
6. Mutation batches are atomic by default. A compatibility caller must opt in
   to continue-on-error behavior and receives an applied-operation ledger.
7. Save writes a synchronized temporary package in the destination directory
   and atomically replaces the target. A changed source revision causes a
   conflict instead of silently overwriting another writer.
8. Ambiguous mutation outcomes retain `use.office.outcome_unknown` and are
   never retried automatically.
9. Replay dump never emits a lossy approximation. A dump is accepted only when
   replaying its typed mutations from the recorded blank-template fingerprint
   reproduces the complete uncompressed OOXML part map byte-for-byte.
10. Template merge never modifies its template in place. It validates all
    replacements transactionally and creates a distinct no-clobber output unless
    the caller explicitly authorizes destination replacement with `--force`.

## Delivery gates

Native Office is promoted by evidence, not by the presence of command names.

### Gate 0 — Package kernel

- Detect Word, Spreadsheet, and Presentation packages.
- Enforce archive and part safety limits.
- Preserve every unknown part byte-for-byte while a known part changes.
- Save atomically and remain `Send + Sync`.

Status: implemented in the native engine; it is a public foundation API and is
not yet the default CLI provider.

### Gate 1 — Native read

- Loss-preserving XML and relationship graph.
- Common selector parser and stable paths.
- Text, outline, statistics, get, and query for all three formats.
- Fixtures open without repair in Microsoft Office and LibreOffice.

Status: implementation in progress. Loss-preserving XML, UTF-8/UTF-16 safety,
content types, relationship resolution, common selectors, stable `get` paths,
and semantic text/outline/statistics reads are implemented for all three
formats. The explicit `a3s use office native` CLI exercises them without an
external provider. Gate 1 remains unpromoted until the cross-application
repair-dialog corpus passes and remaining rich read nodes are covered.

### Gate 2 — Native basic mutation

- Create documents and perform core text, table, cell, sheet, slide, shape, and
  image mutations.
- Atomic batch, save conflict detection, raw access, validation, merge, and dump.
- Untouched-part and untouched-subtree fidelity gates.

Status: implementation in progress. Native creation now writes minimal Word,
Spreadsheet, and Presentation packages selected by extension with atomic,
no-clobber persistence. Native `set-text` edits existing Word
paragraph/run/cell content and Presentation shape text; Spreadsheet cell paths
upsert missing ordered rows and cells and maintain worksheet dimensions.
Spreadsheet writes preserve explicit text, finite-number, boolean, and formula
types. Formula writes strip an optional leading `=`, enforce Excel's 8192
character bound, and mark the workbook for full recalculation; they do not yet
parse or evaluate formulas. Cell set/remove accepts normalized A1 rectangular
ranges of at most 100,000 cells and rolls back the whole operation on error.
Native Spreadsheet structure operations insert or delete at most 10,000 rows or
columns, rename worksheets, reorder worksheets, and copy a worksheet after its
source or at an explicit one-based position. Copy assigns new worksheet and
table identities, clones owned table/comment/VML/drawing/chart/media OPC
subgraphs, preserves explicitly shared workbook resources, rewrites copied
self-references and local defined names, and leaves every source part unchanged.
Worksheet removal deletes only unshared descendants, removes local definitions,
shifts workbook indexes, and changes surviving formulas that target the deleted
sheet to `#REF!`. Formula and structural mutations discard stale calculation
chains and request a full application recalculation.

Row and column edits update cell and row references, dimensions, column
definitions, defined names, workbook view state, merges, filters, selections,
validation, conditional formatting, hyperlinks, sort state, ignored errors,
tables, comments, VML note anchors, drawing anchors, and chart formulas.
Supported local and cross-sheet A1 formula references, including absolute,
rectangular, whole-row, and whole-column references, are rewritten and their
cached values are invalidated; external references and string literals are
preserved. Unsafe 3D-reference and pivot-table structural or copy edits fail
closed and roll back.

Native add supports Word paragraphs and bounded table/row/cell structures,
while remove supports Word paragraphs, runs, tables, rows, and cells with
structural last-child invariants and table-grid maintenance. Spreadsheet cells
and worksheets and Presentation slides, text shapes, and basic DrawingML tables
also support native add/remove. Presentation table creation emits a real
`p:graphicFrame` and `a:tbl`; row creation follows the existing `a:tblGrid`,
blank cells accept native text replacement, and row removal updates the frame
height. Slide and worksheet removal updates their OPC relationships,
content types, and owned parts. The typed editor and `office native batch`
provide all-or-nothing in-memory rollback, bounded versioned inputs, atomic
save/save-as, revision-conflict detection, and byte preservation for untouched
package parts and XML subtrees. Safe raw XML inspection and replacement are now
implemented for existing parts. Raw reads validate XML before returning
normalized UTF-8 text and can export the original bytes. Raw replacement accepts
bounded UTF-8 input, rejects content-type and relationship parts, requires the
root local name and namespace to remain unchanged, and participates in the same
semantic/OPC validation and atomic rollback as every other native mutation.
Known part creation is implemented for Word chart/header/footer carriers and
Spreadsheet/Presentation chart carriers. It allocates collision-free part
names, writes content-type overrides, creates owner relationships, returns typed
relationship receipts, handles transitional and strict OOXML namespaces, and
rolls back every package change on failure. It does not yet insert a visible
chart frame or Word section reference.

Bounded raster image add/read/remove is implemented natively for PNG, JPEG, and
GIF across all three formats. Input bytes are base64 only at the typed batch
boundary and are never returned by normal CLI output. The decoder validates the
declared format against the decoded signature and basic image structure, reads
source dimensions, enforces byte/dimension/pixel bounds, and preserves aspect
ratio when only width or height is requested. Word inserts a real inline
DrawingML picture under `/body`, a paragraph, or a table cell. Spreadsheet
requires an anchor cell such as `/Sheet1/A1`, creates or reuses the worksheet
drawing, and inserts a one-cell anchor. Presentation inserts a real picture in
the selected slide shape tree. Every operation allocates a collision-free media
part, owner relationship, non-visual identity, name/alternative text metadata,
and final pixel dimensions.

Picture removal is reference-aware. It removes the owning XML subtree and an
unused image relationship first, then removes the media part and content-type
override only when no relationship anywhere in the package still targets that
part. All XML, relationship, content-type, and media changes participate in the
existing atomic batch rollback. Process-level tests cover Word, Spreadsheet,
and Presentation with an unusable OfficeCLI provider path; separate format
tests cover PNG, JPEG, GIF, dimension inference, invalid data, rollback, and
batch `createdImages` receipts. SVG is deferred because interoperable OOXML SVG
requires a raster fallback representation. Replacement, crop, effects,
floating/advanced anchors, and rich image rendering are not implemented yet.

Basic Presentation table structure is deliberately bounded. Table dimensions
must be positive, no mutation may exceed 5,000 rows, 5,000 columns, or 100,000
cells, and an explicit row width must equal the parent grid. `add --type cell`
only fills an underfull row; a full row rejects the append because PowerPoint
would silently discard a cell beyond `a:tblGrid`. Direct cell removal is
similarly limited to repairing an overflow row. Removing the final row is
rejected, and row/cell removal fails closed when merged-cell spans would need
rewriting. Column insertion/removal, merged-cell editing, custom dimensions,
table styles, fills, borders, and rich cell formatting remain later
Presentation work. None of these operations invokes OfficeCLI or LibreOffice.

Typed move/copy/swap is implemented as a bounded arrangement layer. `Index` is
zero-based and is evaluated after source removal for a move; `Before` and
`After` resolve stable semantic paths before mutation. A copy with no position
is inserted immediately after its source, while a move with no position moves
to the end of its supported sibling set. Every operation participates in the
same atomic batch rollback and semantic post-validation as add/set/remove.

- Word moves and swaps paragraphs/tables inside a body or table cell, rows
  inside a table, cells inside a row, and runs inside a paragraph. Copies cover
  identity-free paragraphs, tables, rows, and runs. Cross-parent movement,
  relationship or document-identity copies, and table-cell copy fail closed;
  table-cell copy remains blocked until table-grid resizing is defined.
- Spreadsheet moves, copies, and swaps worksheets. Worksheet copy requires a
  distinct name and retains the existing loss-preserving owned-subgraph clone.
  Dense plain rows can also move, copy, and swap with row/cell reference
  renumbering. Row arrangement rejects sparse rows, formulas anywhere in the
  workbook, defined names, row-addressed metadata, worksheet relationships, and
  unsafe shared-string or identity copies before mutation.
- Presentation moves, copies, and swaps slides. Slide copy currently accepts
  only a layout-only relationship graph. Top-level shapes, pictures, tables,
  charts, connectors, and groups can move or swap within one slide. Copy is
  limited to a plain relationship-free shape without placeholders, extension
  identities, or relationship attributes; the copy receives a fresh `cNvPr`
  ID and name. Cross-slide object movement and relationship-owning copies fail
  closed.

Root-scoped replay dump is implemented for the canonical subset that current
typed mutations can reproduce exactly: plain Word paragraphs and rectangular
tables, Spreadsheet worksheets and typed cells without styles or cached formula
results, and Presentation slides with plain one-run text shapes and canonical
basic tables. The versioned
artifact records document kind, `/` scope, blank-template part-map SHA-256,
ordered mutations, and expected result part-map SHA-256. Native `batch` checks
both fingerprints and restores the original package on a failed result check.
Unsupported rich or non-canonical content fails with
`use.office.dump_unsupported`; no element or resource is skipped. Inputs and
file output are limited to 8 MiB and 10,000 mutations, inline output is limited
to 1 MiB, and dump refuses to overwrite an existing path.

Native template merge is implemented across Word, Spreadsheet, and
Presentation. JSON data must be an object; literal top-level keys override
flattened dot/bracket paths, and replacements are single pass. Word processes
the main document, headers, footers, footnotes, endnotes, and comments.
Presentation processes slides and notes. Spreadsheet processes inline strings,
direct string values, and referenced shared rich strings while retaining run
ownership and skipping phonetic text. Shared-string replacements are counted
per referencing cell. Resolved placeholders in unsupported non-string cells
fail closed instead of changing their value type.

The editor restores the original package on any XML, type, or semantic failure.
The CLI accepts inline JSON, `@file.json`, or an existing `.json` path. File
inputs are regular, non-symlink files bounded to 8 MiB; flattened data also has
entry, key, depth, and total-byte limits. Template/output identity, including
Unix hard links, is rejected. Output creation is atomic and no-clobber by
default, with explicit `--force` replacement. Process-level tests cover all
three formats with an unusable OfficeCLI path and verify template bytes remain
unchanged.

Cross-parent/reference-graph arrangement beyond the bounded move/copy/swap
coverage above, advanced image mutation and SVG fallback, complex/custom part
carriers, Presentation table columns/merges/rich styles, subtree and
rich-structure dump, advanced rich-format operations, and
the formula parser/dependency/recalculation engine remain before their
respective gates can be promoted. Creation and structural mutation remain
under the interoperability gate until Microsoft Office and optional CI
LibreOffice checks confirm that no repair dialog is required.

### Gate 3 — Rich Word

- Complete the Word scope above, including revisions, fields, forms, charts,
  equations, international text, and RTL.

### Gate 4 — Rich Spreadsheet

- Complete the Spreadsheet scope above.
- Formula conformance corpus, reference rewrite, dynamic arrays, charts, and
  pivot-table interoperability gates.

### Gate 5 — Rich Presentation

- Complete the Presentation scope above.
- Layout, theme, animation, transition, media, diagram, and chart fidelity gates.

### Gate 6 — Native product promotion

- HTML/SVG/screenshot rendering and live watch.
- Native standard MCP server and Office Skills.
- CLI compatibility corpus for every core command.
- Fuzzing for ZIP, XML, selector, formula, and mutation inputs.
- macOS and Linux release evidence; Windows remains preview until its separate
  platform gate is promoted.

At Gate 6, native becomes the default and `a3s install use/office` no longer
downloads an engine. The OfficeCLI backend moves to an explicitly named
compatibility component for one deprecation cycle, then is removed.

## Verification strategy

- Unit tests cover package, XML, selectors, models, relationships, formulas,
  and mutations without spawning another process.
- Golden fixtures are small, synthetic, and checked for untouched-part hashes.
- Differential tests run the same compatibility corpus against the native
  engine and the pinned OfficeCLI binary, then compare normalized semantic
  results rather than ZIP byte layout.
- Interoperability CI may open and save outputs with Microsoft Open XML
  validation and LibreOffice; release candidates also undergo Microsoft Office
  repair-dialog checks. These are external acceptance oracles, never runtime
  dependencies.
- Rendering uses DOM and image golden tests with explicit tolerances.
- Fuzz targets retain every reproducer as a regression fixture.

## Current migration boundary

The `0.1.x` CLI exposes native blank creation, reads, typed
add/set/remove/move/copy/swap, Spreadsheet range and row/column structure edits,
worksheet rename/reorder and loss-preserving worksheet copy, safe
`raw`/`raw-set`, known `add-part` carriers, exact root replay dump for the
canonical typed subset, native PNG/JPEG/GIF add/read/remove, cross-format
template merge with `merge`, plus atomic batches under the native Office route;
other Office commands and MCP startup still delegate to the pinned OfficeCLI
provider. This keeps existing users functional while native coverage grows.
The native APIs are deliberately not advertised as full Office readiness, and
`doctor` continues to report compatibility-provider readiness until the native
read and mutation gates are met.
