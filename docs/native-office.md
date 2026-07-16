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
columns, rename worksheets, and reorder worksheets. They update cell and row
references, dimensions, column definitions, defined names, workbook view state,
merges, filters, selections, validation, conditional formatting, hyperlinks,
sort state, ignored errors, tables, comments, VML note anchors, drawing anchors,
and chart formulas. Supported local and cross-sheet A1 formula references,
including absolute, rectangular, whole-row, and whole-column references, are
rewritten and their cached values are invalidated; external references and
string literals are preserved. Unsafe 3D-reference and pivot-table structural
edits fail closed and roll back.

Native add supports Word paragraphs and bounded table/row/cell structures,
while remove supports Word paragraphs, runs, tables, rows, and cells with
structural last-child invariants and table-grid maintenance. Spreadsheet cells
and worksheets and Presentation slides and text shapes also support native
add/remove. Slide and worksheet removal updates their OPC relationships,
content types, and owned parts. The typed editor and `office native batch`
provide all-or-nothing in-memory rollback, bounded versioned inputs, atomic
save/save-as, revision-conflict detection, and byte preservation for untouched
package parts and XML subtrees. Worksheet copy, generic move/copy/swap, image
mutation, raw access, template merge, dump, advanced rich-format operations,
and the formula parser/dependency/recalculation engine remain before their
respective gates can be promoted. Creation and structural mutation remain under
the interoperability gate until Microsoft Office and optional CI LibreOffice
checks confirm that no repair dialog is required.

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

The `0.1.x` CLI exposes native blank creation, reads, typed add/set/remove,
Spreadsheet range and row/column structure edits, worksheet rename/reorder, and
atomic batches under `office native`; other Office commands and MCP startup
still delegate to the pinned OfficeCLI provider. This keeps existing users
functional while native coverage grows. The native APIs are deliberately not
advertised as full Office readiness, and `doctor` continues to report
compatibility-provider readiness until the native read and mutation gates are
met.
