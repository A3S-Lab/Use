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
a3s-use component install acme/slack --from ./slack-extension \
  --allow-unsigned --json

a3s-use browser doctor --json
a3s-use office doctor --json
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
| `a3s-use-office` | Typed Office operations and OfficeCLI provider boundary |
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

## Status

The initial implementation includes typed contracts, built-in diagnostics,
ownership-safe local extension activation, native CLI delegation, and
component management. Browser provider extraction from A3S Search, OfficeCLI
integration, and the standard MCP server follow in the documented roadmap.

## License

MIT
