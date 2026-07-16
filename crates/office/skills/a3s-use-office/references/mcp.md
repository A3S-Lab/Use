# Native Office MCP

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
      "fontFamily": "Aptos",
      "fontSizeCentipoints": 1400,
      "textColor": { "red": 18, "green": 52, "blue": 86 }
    }
  }]
}
```

Use a run path for Word/Presentation character properties, a paragraph path
for their alignment, and a cell or bounded range path for Spreadsheet.

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

Use an internal target with `"location"` for a Word bookmark or Spreadsheet
location. Presentation accepts only external shape-wide links and no separate
display text; internal slide jumps return
`use.office.hyperlink_target_unsupported`. External targets accept only
absolute HTTP, HTTPS, or mailto URIs without credentials and remain inert.
Query `hyperlink` to discover stable paths and remove one with the ordinary
typed `remove` mutation. Hyperlink changes remain unsaved until `office_save`.

`office_view` supports text, bounded annotated entries, outline, stats, issues,
all-format HTML/SVG, and all-format semantic screenshots. Annotated and issue
output accept a `limit` from 1 through 1,000; issue output is also filterable.
Annotated reads include unsaved mutations in the current typed session.
Screenshot output requires a no-clobber `.png` path and a ready A3S Browser
provider; other native Office tools do not require Browser or OfficeCLI.

Use `a3s use mcp serve office` only for the pinned OfficeCLI compatibility
server. It is a separate standard MCP target and is not the native session
engine.
