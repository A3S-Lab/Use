# A3S Use

A3S Use is the typed application-capability layer for A3S. The default
`a3s-use` binary includes Browser and Office command domains. Independently
distributed packages can add domains through native CLI, standard MCP, and
Skill surfaces declared in an A3S ACL manifest.

## Commands

```text
a3s-use capabilities --json
a3s-use doctor [browser|box|office] --json
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
a3s-use extension disable acme/slack --json
a3s-use extension enable acme/slack --json
a3s-use extension snapshot --json
a3s-use extension watch --after-generation 3 --timeout-ms 30000 --json
a3s-use capability snapshot --json
a3s-use capability watch --after-generation 3 \
  --after-revision <sha256> --timeout-ms 30000 --json
a3s-use slack channels list
```

The umbrella CLI exposes the same domain arguments through `a3s use`. It also
provides the component-backed Box route without transferring Box ownership to
Use:

```text
a3s use box compose up --detach
```

The umbrella CLI resolves or installs its authoritative Box component, passes
the canonical executable path to Use for that invocation, and preserves Box's
argv, streams, and exit status. Use never copies Box and never writes a second
Box receipt. A non-Box Use command only receives an already-ready Box path and
does not install Box as a side effect.

## Crates

| Crate | Responsibility |
| --- | --- |
| `a3s-use-core` | Shared diagnostics, errors, artifacts, and risk classes |
| `a3s-use-browser` | Browser providers, rendering, and sessions |
| `a3s-use-browser-driver` | Full interactive Browser CLI, MCP, Skills, Dashboard, and compatibility runtime |
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

## Hot-plug lifecycle

External Use packages are process-isolated and hot-plugged through a versioned
registry projection; they are not loaded as dynamic Rust libraries. Install,
upgrade, enable, disable, and uninstall publish an atomic immutable snapshot
with a monotonically increasing generation. `extension watch` lets a resident
consumer wait for a later generation without restarting.

Long-running hosts consume `capability snapshot` and `capability watch`, which
project Browser, Office, Box, and enabled external packages through one
read-only schema. Each binding identifies whether it is `built-in` or
`extension` and declares only its available CLI, standard MCP, and `SKILL.md`
surfaces. The extension generation tracks receipt mutations; the SHA-256
revision also changes when built-in readiness or packaged Skills change. Every
projected Skill carries its own lowercase SHA-256 so a resident consumer can
verify the exact bytes before loading them and can reload content even when its
absolute path is unchanged. This is a discovery contract only—invocation still
uses native CLI, standard MCP, or the Skill loader.

Every accepted CLI or MCP invocation holds a shared route lease until its
child process exits. Disable and uninstall remove route visibility first, then
take an exclusive drain lease before deleting owned files. New calls therefore
fail closed while accepted calls finish. A timed-out drain leaves the route
disabled and returns `use.extension.drain_timeout`; retrying the lifecycle
operation converges from that safe state. Upgrades activate a new immutable
package directory while old in-flight calls retain their prior package. The
receipt is authoritative, so a snapshot missed by a crash is rebuilt from
validated receipts on the next reconciliation. Each binding carries its
validated immutable package root, so a forced reactivation still publishes a
new generation when its version and manifest are unchanged.

Stateless `browser render` runs directly against the typed Rust provider.
Stateful Browser commands use the same Browser MCP tools through an
authenticated, loopback-only standard MCP Streamable HTTP deployment. The
first session command starts it when needed; `mcp start`, `mcp status`, and
`mcp stop` expose explicit lifecycle control. The bearer token is stored only
in a private generated receipt and is never printed by normal CLI output.

## Status

The implementation includes typed contracts, built-in diagnostics, hot-plug
extension generations with graceful draining, native CLI and standard MCP
delegation, the component-backed Box route, and component management. Browser
owns typed Chrome and Lightpanda provider selection, bounded and atomic managed
installation, rendering, and cancellation-safe process cleanup. Its full
driver tracks agent-browser
`0.31.2` at commit `3591f0f4b719c94bcb9aec83ebe811c5dd7f587a` and exposes
the locked 82-command compatibility vocabulary, 151 MCP tools, six packaged
Skills, and the Dashboard through `a3s use browser`. Search injects the small
typed Browser contract directly and never starts the CLI or MCP service. Office
delegates the native OfficeCLI vocabulary, installs the publisher-checksummed
`1.0.136` binary only after explicit authorization, and never reimplements
OfficeCLI's resident transport.

macOS and Linux are the current supported runtime platforms. Their Browser
compatibility gate includes real-Chrome persistent sessions through separate
`a3s use browser` invocations. Windows is a preview build: it must compile and
pass the protocol, command, package, and non-browser-runtime tests, but its
real-Chrome persistent-session path is roadmap work and is not part of the
current compatibility claim.

## License

MIT
