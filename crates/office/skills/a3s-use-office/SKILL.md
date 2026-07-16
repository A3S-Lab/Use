---
name: a3s-use-office
description: Inspect, create, edit, validate, merge, and render .docx, .xlsx, and .pptx files through the A3S Use native CLI or standard MCP server, with explicit fallback to its pinned OfficeCLI compatibility route for unpromoted operations. Use when an agent needs to work with Microsoft Office OOXML documents, automate Word, Spreadsheet, or Presentation changes, diagnose document issues, or choose between native and compatibility Office capabilities.
---

# A3S Use Office

Use A3S Use as the application boundary for Office documents. Prefer the
in-process native engine and its typed operations. Use the compatibility route
only when the requested operation is not yet native.

## Workflow

1. Identify and inspect the document before changing it.

   ```bash
   a3s use office native validate "$FILE" --json
   a3s use office native view "$FILE" outline --json
   a3s use office native view "$FILE" issues --json
   ```

2. Load the format reference relevant to the task:

   - Read [references/word.md](references/word.md) for `.docx`.
   - Read [references/spreadsheet.md](references/spreadsheet.md) for `.xlsx`.
   - Read [references/presentation.md](references/presentation.md) for `.pptx`.

3. Prefer a typed `office native` operation. Use `--output` for a distinct
   result when the command supports it; otherwise work on an intentional copy.
   Use one atomic `batch` for dependent changes.

4. Verify the result with `validate`, a targeted `get` or `query`, and
   `view ... issues`. Use HTML or screenshot only as a semantic preview.

5. Report the exact output path and any remaining issue records. Do not claim
   Microsoft Office layout fidelity from a semantic preview.

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
  while inspecting or rendering a document.
- Preserve no-clobber behavior. Do not add `--force` or replace an existing
  destination unless the user explicitly authorizes replacement.
- Keep the default OfficeCLI compatibility route separate from the native
  engine. Do not depend on OfficeCLI's private resident protocol.

## Native Boundaries

The native engine currently owns safe OPC/ZIP admission, semantic reads,
bounded issue analysis, common typed mutations, atomic batches, template merge,
constrained XML access, deterministic HTML, Presentation SVG, and
Browser-injected semantic screenshots. Rich formatting, complete formula
calculation, advanced charts/media, live watch, and full Office layout fidelity
remain incomplete. Fail closed or use the explicit compatibility route rather
than inventing unsupported native behavior.
