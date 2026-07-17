---
name: a3s-use-office
description: Inspect, create, edit, validate, merge, render, and live-preview .docx, .xlsx, and .pptx files through the A3S Use native CLI or standard MCP server, with explicit fallback to its pinned OfficeCLI compatibility route for unpromoted operations. Use when an agent needs to work with Microsoft Office OOXML documents, automate Word, Spreadsheet, or Presentation changes, diagnose document issues, preview saved changes, or choose between native and compatibility Office capabilities.
---

# A3S Use Office

Use A3S Use as the application boundary for Office documents. Prefer the
in-process native engine and its typed operations. Use the compatibility route
only when the requested operation is not yet native.

## Workflow

1. Identify and inspect the document before changing it.

   ```bash
   a3s use office native validate "$FILE" --json
   a3s use office native view "$FILE" annotated --limit 200 --json
   a3s use office native view "$FILE" outline --json
   a3s use office native view "$FILE" issues --json
   ```

2. Load the format reference relevant to the task:

   - Read [references/word.md](references/word.md) for `.docx`.
   - Read [references/spreadsheet.md](references/spreadsheet.md) for `.xlsx`.
   - Read [references/presentation.md](references/presentation.md) for `.pptx`.

3. Prefer a typed `office native` operation. Use `--output` for a distinct
   result when the command supports it; otherwise work on an intentional copy.
   Use one atomic `batch` for dependent changes. For general find/replace, use
   `set <file> <scope> --find ... --replace ...`; use literal mode unless regex
   captures are actually required.

4. Verify the result with `validate`, a targeted `get` or `query`, and
   `view ... issues`. Use HTML, SVG, or screenshot only as a semantic preview.

5. Report the exact output path and any remaining issue records. Do not claim
   Microsoft Office layout fidelity from a semantic preview.

For an iterative visual loop, run the foreground watch in a separate process:

```bash
a3s use office native watch "$FILE" --port 0
```

Open the authenticated loopback URL printed at startup. The watch refreshes
after on-disk saves, including separate native CLI mutations. It is read-only,
does not expose OfficeCLI's resident protocol, and does not observe unsaved MCP
session changes until `office_save`. Stop it with Ctrl+C; use `--timeout-ms`
when an agent must bound its lifetime.

## Choose the Surface

- Use `a3s use office native ... --json` for local automation and scripts.
- Use `a3s use mcp serve office-native` for typed, stateful agent sessions.
  Read [references/mcp.md](references/mcp.md) before using its session tools.
- Use the typed Rust API when embedding Office behavior in Rust.
- Use `a3s use office ...` only for an operation absent from the native route.
  Check `a3s use office doctor --json` first. Never install or repair the
  compatibility provider without explicit user authority.

`a3s-use` accepts the same arguments when the umbrella `a3s` executable is not
available.

## Safety Rules

- Do not invoke LibreOffice, Microsoft Office, Python, Node.js, or .NET as an
  implicit runtime. LibreOffice is only an optional external CI oracle.
- Do not use `raw-set` when a typed operation exists. When raw XML replacement
  is unavoidable, inspect the exact part, preserve its root QName, write to a
  distinct output, and validate the result.
- Do not evaluate formulas through a shell or general-purpose script runtime.
  Native formula writes request spreadsheet recalculation; they do not promise
  a computed cached value.
- Treat external OOXML relationships as inert. Do not fetch linked resources
  while inspecting or rendering a document. Native hyperlink writes accept
  only absolute HTTP, HTTPS, or mailto URIs without embedded credentials.
- Preserve no-clobber behavior. Do not add `--force` or replace an existing
  destination unless the user explicitly authorizes replacement.
- Treat a zero-match replacement as an unchanged successful receipt, not as a
  claimed edit. Keep Spreadsheet scopes narrow when only selected cells should
  change; the engine protects shared-string aliases outside the scope.
- Keep the default OfficeCLI compatibility route separate from the native
  engine. Do not depend on OfficeCLI's private resident protocol.

## Native Boundaries

The native engine currently owns safe OPC/ZIP admission, semantic reads,
bounded annotated and issue analysis, common typed mutations, atomic batches,
scoped literal/regex replacement, typed bold, italic, underline, vertical
script, font, size, RGB, and alignment formatting, Word/Spreadsheet single
strikethrough, Word double strikethrough, and Word/Presentation display case,
portable highlight, and primary-language formatting. It also owns typed inert
hyperlinks, typed legacy comments, typed Spreadsheet number formats, solid or
cleared fills, vertical alignment, wrapping, rotation, indentation,
shrink-to-fit, and reading order, template merge, constrained XML access,
deterministic all-format HTML/SVG, Browser-injected semantic screenshots, and
authenticated loopback live watch for saved files.
Hyperlinks cover Word body/header/footer paragraphs and bookmarks, Spreadsheet
cells or bounded ranges and internal locations, and external Presentation shape
clicks or internal jumps to existing slides. Remaining boundaries include
modern threaded comments, replies/resolution, writable comment dates,
rich comment bodies, Word header/footer comment anchors, borders,
gradient/pattern/theme fills, conditional and named styles, complete formula
calculation, advanced charts/media,
interactive preview editing/annotations, and full Office layout fidelity. Fail
closed or use the explicit compatibility route rather than inventing
unsupported native behavior.
