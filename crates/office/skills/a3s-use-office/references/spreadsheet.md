# Spreadsheet Workflows

Use stable worksheet and A1 paths such as `/Sheet1`, `/Sheet1/A1`, and
`/Sheet1/A1:C20`. Preserve value types instead of writing every value as text.

## Inspect

```bash
a3s use office native get workbook.xlsx /Sheet1 --depth 2 --json
a3s use office native query workbook.xlsx 'cell[formula]' --json
a3s use office native view workbook.xlsx stats --json
a3s use office native view workbook.xlsx issues --type content --json
```

## Values and Formulas

```bash
a3s use office native set workbook.xlsx /Sheet1/A1 --text 'Revenue' --json
a3s use office native set workbook.xlsx /Sheet1/B1 --number 42.5 --json
a3s use office native set workbook.xlsx /Sheet1/C1 --boolean true --json
a3s use office native set workbook.xlsx /Sheet1/D1 --formula 'SUM(B1:B12)' --json
```

Formula writes store validated formula text, invalidate stale calculation
caches, and request application recalculation. The native engine does not yet
provide a complete formula evaluator. Check `formula_not_evaluated` and
`formula_eval_error` issue records before delivery.

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
```

HTML and screenshots are sparse semantic previews, not Excel layout or print
fidelity.
