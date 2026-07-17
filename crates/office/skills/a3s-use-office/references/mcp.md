# Native Office MCP

## Contents

- [Session Workflow](#session-workflow)
- [Typed Mutations](#typed-mutations)
- [Views and Compatibility](#views-and-compatibility)

## Session Workflow

Start the explicit native standard MCP server:

```bash
a3s use mcp serve office-native
```

Use its typed tools rather than passing shell command strings:

- `office_validate` checks one file without opening a session.
- `office_create` and `office_open` register bounded in-memory sessions.
- `office_get`, `office_query`, and `office_view` read the current session.
- `office_apply_batch` applies typed mutations atomically in memory.
- `office_raw_xml` inspects one bounded XML part.
- `office_merge_template` writes a distinct merged output.
- `office_save` persists a mutable session.
- `office_close` refuses unsaved changes unless `discard=true` is explicit.
- `office_list` reports sessions owned by this server process.

Mutations remain unsaved until `office_save`. Do not discard a dirty session
unless the user explicitly accepts losing its changes. Release the session as
soon as the workflow finishes.

## Typed Mutations

Rich-text changes use the typed `set-text-format` mutation inside
`office_apply_batch`; do not send generic property maps:

```json
{
  "session": "report",
  "mutations": [{
    "operation": "set-text-format",
    "path": "/body/p[1]/r[1]",
    "format": {
      "bold": true,
      "underline": "double",
      "script": "superscript",
      "strikethrough": true,
      "doubleStrikethrough": false,
      "textCase": "small-caps",
      "highlight": "yellow",
      "language": "en-US",
      "fontFamily": "Aptos",
      "fontSizeCentipoints": 1400,
      "textColor": { "red": 18, "green": 52, "blue": 86 }
    }
  }]
}
```

Use a run path for Word/Presentation character properties, a paragraph path
for their alignment, and a cell or bounded range path for Spreadsheet.
Underline accepts `none`, `single`, or `double`; script accepts `baseline`,
`superscript`, or `subscript`. Strikethrough is native for Word and Spreadsheet
and is rejected for Presentation. `textCase`, `highlight`, and `language` are
native for Word and Presentation runs. `doubleStrikethrough` is native only for
Word. Unsupported format/property combinations fail the whole batch through a
typed error.

Spreadsheet cell presentation uses the separate typed `set-cell-format`
mutation. Do not put these properties in `set-text-format`:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "set-cell-format",
    "path": "/Sheet1/A1:C3",
    "format": {
      "numberFormat": "currency",
      "fill": {
        "kind": "solid",
        "color": { "red": 255, "green": 242, "blue": 204 }
      },
      "border": {
        "left": {
          "kind": "line",
          "style": "thin",
          "color": { "red": 128, "green": 128, "blue": 128 }
        },
        "right": { "kind": "line", "style": "thin" },
        "top": { "kind": "line", "style": "thin" },
        "bottom": { "kind": "line", "style": "double" },
        "diagonalUp": false,
        "diagonalDown": false
      },
      "verticalAlignment": "center",
      "wrapText": true,
      "textRotation": 0,
      "indent": 1,
      "shrinkToFit": false,
      "readingOrder": "left-to-right"
    }
  }]
}
```

Use `{ "kind": "none" }` to remove a fill. `numberFormat` accepts a validated
Excel format code or the native aliases documented in
[spreadsheet.md](spreadsheet.md). Each border side is an explicit `none` or
`line` object. A line contains one typed SpreadsheetML style and an optional
RGB color; `diagonalUp` and `diagonalDown` control the shared diagonal line.
Rotation accepts 0–180 or 255, indentation is 0–255, and reading order is
`context`, `left-to-right`, or `right-to-left`.
The same `office_apply_batch` call may also include content and text-format
mutations. Unknown fields, invalid values, empty format objects, and
non-Spreadsheet targets fail the entire in-memory batch; no change persists
until `office_save`.

Spreadsheet merged cells use the separate `merge-cells` and `unmerge-cells`
mutations:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "merge-cells",
    "path": "/Sheet1/A1:C1"
  }]
}
```

The path is normalized. An exact repeated merge is idempotent; a geometric
overlap or ListObject table intersection fails the complete batch. Use
`unmerge-cells` only with one exact existing range. Any non-exact intersecting
range fails with `use.office.spreadsheet_merge_not_exact` and reports `validRanges`; it
never sweeps multiple merges. Merge state can share one `office_apply_batch`
call with content, text-format, cell-format, and hyperlink mutations. Query
`mergeCell` for stable nodes, or read a covered cell to inspect `merge` and
`mergeAnchor`. Blank covered cells remain virtual. All changes stay unsaved
until `office_save`, and unknown merge collection data is preserved or causes a
fail-closed error when exact removal cannot be lossless.

Spreadsheet data validation uses the separate `add-data-validation` and
`set-data-validation` mutations:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "add-data-validation",
    "sheet": "/Sheet1",
    "validation": {
      "type": "list",
      "ranges": ["A2:A20", "C2:C20"],
      "formula1": "Draft,Review,Approved",
      "allowBlank": true,
      "showInput": true,
      "showError": true,
      "promptTitle": "Status",
      "prompt": "Choose a workflow state",
      "errorTitle": "Invalid status",
      "error": "Choose a listed state",
      "errorStyle": "stop",
      "inCellDropdown": true
    }
  }]
}
```

Use `set-data-validation` with a stable `path` such as
`/Sheet1/dataValidation[1]` and a complete `validation` value. Set is not a
partial property patch. Delete with the ordinary `remove` mutation. The seven
types are `list`, `whole`, `decimal`, `date`, `time`, `textLength`, and
`custom`; operators and error styles are closed camelCase enums. List and
custom reject operators and `formula2`. The five comparison types require an
operator, and only `between` or `notBetween` accept and require `formula2`.

Rules and ranges are bounded, normalized, and globally non-overlapping within
one worksheet. Invalid formulas, flags, messages, XML text, ranges, or overlap
fail the complete `office_apply_batch`. Inline lists, ISO dates, and clock
times are normalized but formulas are never evaluated. Query
`dataValidation[type=list]` or call `office_get` on the returned path for
unsaved semantic readback. Covered observed and virtual blank cells expose
`dataValidation` and `validationType`. Updates retain unknown attributes and
fail closed when unknown children or final collection data cannot be preserved.
All mutations remain in memory until `office_save`.

Spreadsheet conditional formatting uses the separate
`add-conditional-format` and `set-conditional-format` mutations:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "add-conditional-format",
    "sheet": "/Sheet1",
    "conditionalFormat": {
      "ranges": ["A2:A20", "C2:C20"],
      "stopIfTrue": true,
      "rule": {
        "type": "cellIs",
        "operator": "greaterThan",
        "formula1": "80",
        "format": {
          "fill": {"red": 198, "green": 239, "blue": 206},
          "fontColor": {"red": 0, "green": 97, "blue": 0},
          "bold": true
        }
      }
    }
  }]
}
```

Use `set-conditional-format` with a stable path such as `/Sheet1/cf[1]` and a
complete `conditionalFormat` value. MCP set is not a partial patch. Delete with
the ordinary `remove` mutation. Classic rule types cover cell comparison,
expression formula, contains/not-contains/begins-with/ends-with text,
top/bottom, above/below average, duplicate/unique values, blanks, errors, and
date windows. Their only differential-format properties are `fill`,
`fontColor`, and `bold`.

Visual rule types are `dataBar`, `colorScale`, and `iconSet`. They use closed
RGB colors and min/max/number/percent/percentile/formula thresholds. Data bars
own value visibility and optional lengths. Color scales require midpoint and
midpoint color together for a three-color scale. Icon sets accept only standard
legacy 3/4/5-icon names and exactly the corresponding threshold count, or an
empty threshold array that generates percent defaults. Unknown fields and
unsupported rule variants fail MCP schema decoding before mutation.

Each rule owns 1–1,024 internally disjoint A1 areas; different rules may
overlap and remain ordered by priority. Query `conditionalFormatting` or
`conditionalFormatting[type=dataBar]`, then inspect `nativeMutable` before
updating imported content. Unsupported extension rules remain readable with
`nativeMutable=false`. Unknown child or collection content, or a range change
to one child in an imported shared-range carrier, fails the complete batch
rather than dropping sibling or extension data. Rule formulas are stored but
not evaluated, and semantic views do not prove Excel rendering. Mutations stay
unsaved until `office_save`.

Spreadsheet defined names use the separate `add-named-range` and
`set-named-range` mutations:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "add-named-range",
    "namedRange": {
      "name": "Revenue",
      "ref": "'Sheet1'!$A$2:$A$20",
      "scope": "workbook",
      "comment": "Workbook revenue",
      "volatile": false
    }
  }]
}
```

Use `set-named-range` with the canonical scoped `path` returned by the batch or
an `office_query` call and one complete `namedRange` value. The scope is
`workbook` or an existing worksheet name. Delete with the ordinary typed
`remove` mutation. Name-only compatibility paths are ambiguous when the same
name exists at workbook and worksheet scope, so prefer
`/namedrange[@name=NAME][@scope=SCOPE]`.
Use the explicit scope `worksheet:workbook` for a worksheet literally named
`workbook`; the same escaped label appears in semantic readback and its
canonical path.

Identifiers, refs, comments, collection size, case-insensitive `(name, scope)`
identity, ListObject table-name collisions, reserved `_xlnm.*`/`Slicer_*`
names, and unsupported cross-workbook refs are validated before mutation.
Workbook-scoped bare A1 refs are rejected; worksheet-local bare A1 refs are
qualified automatically by the domain layer. The mutation requests workbook
recalculation but does not evaluate the expression. Unknown OOXML attributes
are retained, while unknown content that cannot be preserved fails the whole
batch. Call `office_get` or `office_query` before `office_save` to verify the
unsaved scoped value, then save explicitly. Closing a dirty session still
requires save or explicit discard.

Spreadsheet worksheet AutoFilters use the separate
`add-spreadsheet-auto-filter` and `set-spreadsheet-auto-filter` mutations.
ListObject table `filters` use the same filter-column values:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "add-spreadsheet-auto-filter",
    "sheet": "/Sheet1",
    "filter": {
      "range": "A1:C20",
      "columns": [
        {
          "column": 0,
          "criteria": {
            "type": "values",
            "values": ["Open", "Closed"],
            "includeBlanks": true
          }
        },
        {
          "column": 2,
          "criteria": {"type": "greater-than", "value": "100"}
        }
      ]
    }
  }]
}
```

Use `set-spreadsheet-auto-filter` with the stable `/Sheet/autofilter` path and
one complete `filter` value; MCP set is not a partial patch. Delete with
ordinary typed `remove`. Columns are unique zero-based offsets inside the
normalized range. Closed criteria cover exact values/blanks, text and ordered
comparisons, between/not-between, blanks/non-blanks, top/bottom count or
percentage, and dynamic average/date/month/quarter families. A worksheet
accepts one filter and rejects table or merge overlap. Use `office_get` with
depth 2 or `office_query` for `autofilter`/`filtercolumn` before saving. Do not
replace a node whose semantic `nativeMutable` flag is false. Date-group,
color/icon, extension, unknown-content, and embedded sort-state imports fail
closed; persisted physical row sorting is a separate contract.

Spreadsheet ListObject tables use the separate `add-spreadsheet-table` and
`set-spreadsheet-table` mutations:

```json
{
  "session": "workbook",
  "mutations": [{
    "operation": "add-spreadsheet-table",
    "sheet": "/Sheet1",
    "table": {
      "name": "Sales",
      "range": "F1:H4",
      "columns": [
        {"name": "Name"},
        {"name": "Qty"},
        {"name": "Price"}
      ],
      "filters": [
        {"column": 1, "criteria": {"type": "top", "count": 10}}
      ],
      "headerRow": true,
      "totalsRow": false,
      "style": {"family": "medium", "number": 4},
      "showFirstColumn": false,
      "showLastColumn": false,
      "showRowStripes": true,
      "showColumnStripes": false
    }
  }]
}
```

Use `set-spreadsheet-table` with the stable `/Sheet/table[N]` path and one
complete `table` value; MCP set is not a partial patch. Delete with ordinary
typed `remove`. `range` is final and includes enabled header/totals rows, its
width must match the ordered column list, and at least one data row must remain.
Names, columns, built-in style families/numbers, flags, table/defined-name
identity, table/merge/worksheet-AutoFilter overlap, and relationship ownership
are validated before mutation.

Use `office_get` with depth 1 or `office_query` with `table[name=Sales]` to
inspect the unsaved table and its column children. Do not replace a node whose
semantic `nativeMutable` flag is false. Header stamping, OPC table parts, and
the typed table-owned AutoFilter stay inside the same editor transaction.
Calculated columns, totals functions, date-group/color/icon filters, persisted
sort state, custom styles, and external/query-table data remain outside the
typed value. See
[spreadsheet.md](spreadsheet.md#tables) for the complete boundary.

General find/replace uses the typed `replace-text` mutation. Keep `mode`
explicit and prefer `literal` for ordinary text:

```json
{
  "session": "report",
  "mutations": [{
    "operation": "replace-text",
    "path": "/body",
    "replacement": {
      "find": "Q([1-4]) 2025",
      "replace": "Q$1 2026",
      "mode": "regex"
    }
  }]
}
```

The batch receipt includes `textReplacements` with `matchCount`, `changed`, and
`changedParts`. Zero matches are successful and unchanged. Regex matches must
consume text. Spreadsheet cell/range scopes protect unselected shared-string
references; Word and Presentation replacements preserve split-run formatting.

Hyperlinks use the typed `set-hyperlink` mutation:

```json
{
  "session": "report",
  "mutations": [{
    "operation": "set-hyperlink",
    "path": "/body/p[1]",
    "hyperlink": {
      "target": {
        "kind": "external",
        "uri": "https://example.com/report"
      },
      "display": "Open report",
      "tooltip": "A3S report"
    }
  }]
}
```

Use an internal target with `"location"` for a Word bookmark, Spreadsheet
workbook location, or Presentation `slide[N]`/`/slide[N]` jump. Word accepts
body, header, and footer paragraphs. Spreadsheet accepts a cell or bounded
rectangular range and rejects overlapping link ranges. Presentation accepts a
shape-wide link and no separate display text. External targets accept only
absolute HTTP, HTTPS, or mailto URIs without credentials and remain inert.
Query `hyperlink` to discover stable paths and remove one with the ordinary
typed `remove` mutation. Hyperlink changes remain unsaved until `office_save`.

Legacy comments use typed `add-comment` and `set-comment` mutations:

```json
{
  "session": "deck",
  "mutations": [{
    "operation": "add-comment",
    "parent": "/slide[1]",
    "comment": {
      "author": "Alice",
      "text": "Review this slide",
      "initials": "AL",
      "position": { "xEmu": 914400, "yEmu": 457200 }
    }
  }]
}
```

Use `set-comment` with a partial `update` and a stable path returned by the add
mutation or a `comment` query. Word accepts main-document paragraph/run
anchors, Spreadsheet accepts classic cell notes, and Presentation accepts
legacy slide comments with optional coordinates. Remove a comment with the
ordinary `remove` mutation. Modern threaded comments and replies are outside
this contract. Comment changes remain unsaved until `office_save`.

## Views and Compatibility

`office_view` supports text, bounded annotated entries, outline, stats, issues,
all-format HTML/SVG, and all-format semantic screenshots. Annotated and issue
output accept a `limit` from 1 through 1,000; issue output is also filterable.
Annotated reads include unsaved mutations in the current typed session.
Screenshot output requires a no-clobber `.png` path and a ready A3S Browser
provider; other native Office tools do not require Browser or OfficeCLI.

Use `a3s use mcp serve office` only for the pinned OfficeCLI compatibility
server. It is a separate standard MCP target and is not the native session
engine.
