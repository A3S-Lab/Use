# Word Workflows

Use stable one-based semantic paths such as `/body/p[1]`,
`/body/tbl[1]/tr[1]/tc[1]`, and header/footer paths returned by `get` or
`outline`. Discover paths instead of guessing them.

## Inspect

```bash
a3s use office native get report.docx /body --depth 2 --json
a3s use office native query report.docx 'p[style=Heading1]' --json
a3s use office native view report.docx text --json
a3s use office native view report.docx annotated --limit 200 --json
a3s use office native view report.docx issues --type content --json
```

## Create and Edit

```bash
a3s use office native create report.docx --json
a3s use office native add report.docx /body --type paragraph --text 'Summary' --json
a3s use office native add report.docx /body --type table --rows 2 --columns 3 --json
a3s use office native set report.docx '/body/p[1]' --text 'Updated summary' --json
a3s use office native add report.docx /body --type picture --input chart.png --alt 'Quarterly revenue chart' --json
a3s use office native move report.docx '/body/p[2]' --before '/body/p[1]' --json
```

Use `copy`, `swap`, and `remove` only with paths returned by a fresh read.
Identity-bearing copies, cross-parent ownership migration, and rich structures
outside the documented native subset fail closed.

`add-part --type header|footer|chart` creates a typed part carrier and
relationship; it does not make a chart or header visible by itself.

## Merge and Verify

```bash
a3s use office native merge template.docx report.docx --data @report.json --json
a3s use office native validate report.docx --json
a3s use office native view report.docx issues --json
a3s use office native view report.docx html --output report.html --json
a3s use office native view report.docx svg --output report.svg --json
a3s use office native watch report.docx --port 0
```

Tracked changes, comments, complete fields/forms, TOC updates, equations,
advanced styles, and layout-accurate pagination are not yet complete native
capabilities. HTML, SVG, and live watch are semantic previews, not Word
pagination. Watch follows saved disk revisions and is not an interactive Word
editor. Use the compatibility route only after checking provider readiness and
explaining the boundary.
