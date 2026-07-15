# A3S Use

A3S Use is the typed application-capability layer for A3S. The default
`a3s-use` binary includes Browser and Office command domains. Independently
distributed packages can add domains through native CLI, standard MCP, and
Skill surfaces declared in an A3S ACL manifest.

## Commands

```text
a3s-use capabilities --json
a3s-use doctor [browser|office] --json
a3s-use component list --json
a3s-use component status browser --json
a3s-use component install office --json
a3s-use component install acme/slack --from ./slack-extension \
  --allow-unsigned --json

a3s-use browser doctor --json
a3s-use browser render https://example.com --output page.html
a3s-use browser open https://example.com --session research
a3s-use browser snapshot --session research --json
a3s-use browser click @e1 --session research
a3s-use browser close --session research
a3s-use office doctor --json
a3s-use office get report.docx /body --json
a3s-use office batch report.xlsx --input updates.json --json
a3s-use mcp serve browser
a3s-use mcp serve office
a3s-use mcp start browser --json
a3s-use mcp status browser --json
a3s-use mcp stop browser --json
a3s-use extension list --json
a3s-use extension inspect acme/slack --json
a3s-use slack channels list
```

The umbrella CLI exposes the same domain arguments through `a3s use`.

## Crates

| Crate | Responsibility |
| --- | --- |
| `a3s-use-core` | Shared diagnostics, errors, artifacts, and risk classes |
| `a3s-use-browser` | Browser providers, rendering, and sessions |
| `a3s-use-office` | Typed Office batches, pinned OfficeCLI installation, and native CLI delegation |
| `a3s-use-extension` | ACL manifest model and native CLI/MCP/Skill descriptors |
| `a3s-use` | Facade, CLI host, and standard MCP entry point |

`a3s-search` depends only on `a3s-use-browser` and injects an
`Arc<dyn PageRenderer>`.

## Protocol boundary

A3S Use does not define a custom extension JSON-RPC protocol.

- CLI uses argv, stdin, stdout, stderr, and process status.
- MCP uses the standard MCP client/server contract.
- Skill uses the existing `SKILL.md` package convention.
- `--json` is versioned CLI output, not JSON-RPC.

MCP servers remain independently owned. `mcp serve browser` runs A3S Use's
typed Browser server, `mcp serve office` launches OfficeCLI's standard MCP
server, and an external package target launches the MCP executable declared in
its ACL manifest. A3S Use does not translate one server's tools into another
protocol.

Stateless `browser render` runs directly against the typed Rust provider.
Stateful Browser commands use the same Browser MCP tools through an
authenticated, loopback-only standard MCP Streamable HTTP deployment. The
first session command starts it when needed; `mcp start`, `mcp status`, and
`mcp stop` expose explicit lifecycle control. The bearer token is stored only
in a private generated receipt and is never printed by normal CLI output.

## Status

The implementation includes typed contracts, built-in diagnostics,
ownership-safe local extension activation, native CLI and standard MCP
delegation, and component management. Browser owns typed Chrome and Lightpanda
provider selection, bounded and atomic managed installation, rendering, tab
limits, semantic snapshots, cross-invocation sessions over standard MCP, and
cancellation-safe process cleanup. Search injects that Browser contract
directly. Office delegates the native OfficeCLI vocabulary, installs the
publisher-checksummed `1.0.136` binary only after explicit authorization, and
never reimplements OfficeCLI's resident transport.

## License

MIT
