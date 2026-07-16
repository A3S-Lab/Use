# Spreadsheet Workflows

Use stable worksheet and A1 paths such as `/Sheet1`, `/Sheet1/A1`, and
`/Sheet1/A1:C20`. Preserve value types instead of writing every value as text.

## Inspect

```bash
a3s use office native get workbook.xlsx /Sheet1 --depth 2 --json
a3s use office native query workbook.xlsx 'cell[formula]' --json
a3s use office native view workbook.xlsx stats --json
a3s use office native view workbook.xlsx annotated --limit 200 --json
a3s use office native view workbook.xlsx issues --type content --json
```

## Values and Formulas

```bash
a3s use office native set workbook.xlsx /Sheet1/A1 --text 'Revenue' --json
a3s use office native set workbook.xlsx /Sheet1/B1 --number 42.5 --json
a3s use office native set workbook.xlsx /Sheet1/C1 --boolean true --json
a3s use office native set workbook.xlsx /Sheet1/D1 --formula 'SUM(B1:B12)' --json
a3s use office native set workbook.xlsx /Sheet1/E1 --url https://example.com/data --display Data --tooltip 'Open data' --json
a3s use office native set workbook.xlsx /Sheet1/F1 --location 'Sheet1!B2' --display B2 --json
a3s use office native query workbook.xlsx hyperlink --json
a3s use office native remove workbook.xlsx /Sheet1/E1/hyperlink --json
a3s use office native add workbook.xlsx /Sheet1/B2 --type comment --author Alice --text 'Check this formula' --json
a3s use office native set workbook.xlsx /Sheet1/B2/comment --author Bob --text 'Formula checked' --json
a3s use office native query workbook.xlsx comment --json
a3s use office native remove workbook.xlsx /Sheet1/B2/comment --json
```

Formula writes store validated formula text, invalidate stale calculation
caches, and request application recalculation. The native engine does not yet
provide a complete formula evaluator. Check `formula_not_evaluated` and
`formula_eval_error` issue records before delivery.

Hyperlinks target one cell and auto-create it when absent. External targets
accept only absolute HTTP, HTTPS, or mailto URIs without credentials; internal
targets are workbook locations such as `Sheet1!B2`. Display text and tooltips
are optional. Update through the cell or returned `/hyperlink` path, and remove
through the hyperlink path. Multi-cell hyperlink ranges are not yet a native
write surface. Reads and previews never fetch external targets.

Classic cell comments use stable `/SheetName/A1/comment` paths and may be added
to an otherwise blank cell. Add requires an author and plain text. Update the
author or text through the comment path; Spreadsheet rejects separate initials
and slide coordinates instead of ignoring them. Native removal also cleans up
the matching VML note shape and removes unused comment/VML parts. Threaded
comments, replies, writable dates, and rich bodies are not yet native.

## Cell Text Formatting

```bash
a3s use office native set workbook.xlsx /Sheet1/A1:C1 --bold true --font-family Aptos --font-size 11.5 --text-color 0066CC --align center --json
a3s use office native set workbook.xlsx /Sheet1/A2 --text 'Total' --bold true --align right --json
```

Formatting accepts one cell or a bounded rectangular range and may be combined
with one content write. It auto-creates empty styled cells and deduplicates
OOXML font and cell-style records. The native typed subset covers bold, italic,
font family, point size, RGB text color, and horizontal alignment. Number
formats, fills, borders, vertical alignment, wrapping, and conditional styles
remain separate work.

## Structure

```bash
a3s use office native add workbook.xlsx / --type sheet --name Data --json
a3s use office native insert-rows workbook.xlsx /Sheet1 2 --count 3 --json
a3s use office native delete-columns workbook.xlsx /Sheet1 C --count 1 --json
a3s use office native rename-sheet workbook.xlsx /Sheet1 Summary --json
a3s use office native copy-sheet workbook.xlsx /Summary 'Summary Copy' --json
a3s use office native move-sheet workbook.xlsx /Data 1 --json
a3s use office native add workbook.xlsx /Sheet1/A1 --type picture --input chart.png --alt 'Sales chart' --json
```

Supported structural edits rewrite bounded A1 references and related metadata.
Pivot-table changes, unsafe 3D references, rich conditional formatting, full
chart authoring, and complete recalculation remain outside the native subset
and fail closed where safety cannot be proven.

## Verify

```bash
a3s use office native validate workbook.xlsx --json
a3s use office native view workbook.xlsx issues --limit 200 --json
a3s use office native view workbook.xlsx html --output workbook.html --json
a3s use office native view workbook.xlsx svg --output workbook.svg --json
a3s use office native watch workbook.xlsx --port 0
```

HTML, SVG, and screenshots are sparse semantic previews, not Excel layout or
print fidelity. Watch reloads saved revisions; it does not provide inline cell
editing or calculate formulas.
