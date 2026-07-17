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
