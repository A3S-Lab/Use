# Spreadsheet Workflows

Use stable worksheet and A1 paths such as `/Sheet1`, `/Sheet1/A1`, and
`/Sheet1/A1:C20`. Preserve value types instead of writing every value as text.

## Contents

- [Inspect](#inspect)
- [Values and Formulas](#values-and-formulas)
- [Cell Text Formatting](#cell-text-formatting)
- [Cell Presentation Formatting](#cell-presentation-formatting)
- [Structure](#structure)
- [Verify](#verify)

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
a3s use office native set workbook.xlsx /Sheet1/A1:C20 --find Draft --replace Final --json
a3s use office native set workbook.xlsx /Sheet1/B1 --number 42.5 --json
a3s use office native set workbook.xlsx /Sheet1/C1 --boolean true --json
a3s use office native set workbook.xlsx /Sheet1/D1 --formula 'SUM(B1:B12)' --json
a3s use office native set workbook.xlsx /Sheet1/E1 --url https://example.com/data --display Data --tooltip 'Open data' --json
a3s use office native set workbook.xlsx /Sheet1/F1 --location 'Sheet1!B2' --display B2 --json
a3s use office native set workbook.xlsx /Sheet1/G2:H4 --url https://example.com/range --display Range --json
a3s use office native query workbook.xlsx hyperlink --json
a3s use office native remove workbook.xlsx /Sheet1/E1/hyperlink --json
a3s use office native add workbook.xlsx /Sheet1/B2 --type comment --author Alice --text 'Check this formula' --json
a3s use office native set workbook.xlsx /Sheet1/B2/comment --author Bob --text 'Formula checked' --json
a3s use office native query workbook.xlsx comment --json
a3s use office native remove workbook.xlsx /Sheet1/B2/comment --json
```

General replacement accepts `/`, one worksheet, one cell, or a rectangular A1
range and edits string cells only. Literal matching is the default; add
`--regex` for Rust regex and capture expansion. A scoped edit of a shared rich
string clones and redirects only selected cells when references exist outside
the scope. Rich runs and unknown XML survive, and phonetic text is excluded.
Numeric, boolean, formula, and error values are not coerced. Zero matches are
reported as an unchanged success.

Formula writes store validated formula text, invalidate stale calculation
caches, and request application recalculation. The native engine does not yet
provide a complete formula evaluator. Check `formula_not_evaluated` and
`formula_eval_error` issue records before delivery.

Hyperlinks target one cell or a bounded rectangular range. A missing single
cell is auto-created; a range link neither creates cells nor rewrites their
contents. External targets accept only absolute HTTP, HTTPS, or mailto URIs
without credentials; internal targets are workbook locations such as
`Sheet1!B2`. Display text and tooltips are optional. Single-cell links use a
cell `/hyperlink` child path; range links use a stable worksheet
`/hyperlink[N]` path returned by the mutation or a query. Update through the
cell/range or returned hyperlink path, and remove through the hyperlink path.
Overlapping hyperlink ranges fail with `use.office.hyperlink_range_conflict`.
Reads and previews never fetch external targets.

Classic cell comments use stable `/SheetName/A1/comment` paths and may be added
to an otherwise blank cell. Add requires an author and plain text. Update the
author or text through the comment path; Spreadsheet rejects separate initials
and slide coordinates instead of ignoring them. Native removal also cleans up
the matching VML note shape and removes unused comment/VML parts. Threaded
comments, replies, writable dates, and rich bodies are not yet native.

## Cell Text Formatting

```bash
a3s use office native set workbook.xlsx /Sheet1/A1:C1 --bold true --underline double --script superscript --strikethrough true --font-family Aptos --font-size 11.5 --text-color 0066CC --align center --json
a3s use office native set workbook.xlsx /Sheet1/A2 --text 'Total' --bold true --align right --json
```

Formatting accepts one cell or a bounded rectangular range and may be combined
with one content write. It auto-creates empty styled cells and deduplicates
OOXML font and cell-style records. The native typed subset covers bold, italic,
`none`/single/double underline, baseline/superscript/subscript text, explicit
single strikethrough, font family, point size, RGB text color, and horizontal
alignment. Run-only text case, highlight, language, and double strikethrough
fail atomically with `use.office.spreadsheet_run_format_unsupported`; they are
not silently flattened into a cell style. Use the separate cell-presentation
options below for non-text properties. Conditional formatting, named styles,
and gradient/pattern/theme fills remain outside the native subset.

## Cell Presentation Formatting

```bash
a3s use office native set workbook.xlsx /Sheet1/A1:C3 --number-format currency --fill FFF2CC --border-all thin --border-color 808080 --border-bottom double --border-bottom-color 000000 --vertical-align center --wrap-text true --json
a3s use office native set workbook.xlsx /Sheet1/D1 --number 0.125 --bold true --number-format percent --fill 0066CC --text-rotation 45 --indent 1 --shrink-to-fit false --reading-order rtl --json
a3s use office native set workbook.xlsx /Sheet1/E1 --border-diagonal slant-dash-dot --border-diagonal-color FF0000 --border-diagonal-up true --border-diagonal-down false --json
a3s use office native set workbook.xlsx /Sheet1/A1:C3 --fill none --wrap-text false --reading-order context --json
```

Cell presentation accepts one cell or a bounded rectangular range and may be
combined atomically with one content write, text formatting, and a hyperlink.
Use `--number-format` for an explicit Excel format code or one of `general`,
`number`, `currency`, `accounting`, `percent`, `scientific`, `text`, `date`,
`time`, or `datetime`. Codes may contain at most four sections and must keep
quotes and square brackets balanced.

`--fill` accepts `none` or exactly six hexadecimal RGB digits.
`--border-all` (alias `--border`) sets all four cardinal line styles;
`--border-color` supplies their default RGB color. Override one side with
`--border-left`, `--border-right`, `--border-top`, or `--border-bottom` and the
matching `-color` option. Use `none` to clear a line. Styles are `thin`,
`medium`, `thick`, `double`, `dashed`, `dotted`, `dash-dot`, `dash-dot-dot`,
`hair`, `medium-dashed`, `medium-dash-dot`, `medium-dash-dot-dot`, and
`slant-dash-dot`. The shared diagonal line uses `--border-diagonal` and
`--border-diagonal-color`; explicitly select its direction with
`--border-diagonal-up` and `--border-diagonal-down`. A color option requires a
non-`none` style in the same command.
`--vertical-align` accepts `top`, `center`, `bottom`, `justify`, or
`distributed`. `--wrap-text` and `--shrink-to-fit` require an explicit boolean;
`--text-rotation` accepts 0–180 or 255 for stacked text; `--indent` accepts
0–255; and `--reading-order` accepts `context`, `ltr`, or `rtl`. Explicit
`none`, `false`, zero, and `context` values clear or reset the corresponding
property instead of being treated as omitted.

The writer preserves unknown style and border data and deduplicates
number-format, fill, border, and cell-style records. Invalid values, an empty
cell-format or border object, a bad
target kind, or any other mutation failure rolls back the complete in-memory
batch before save. Verify with a targeted `get`; HTML/SVG expose observed
values as inert `data-*` attributes but remain semantic previews rather than
Excel layout evidence.

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
