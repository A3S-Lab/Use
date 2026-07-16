# Agent Browser Compatibility Baseline

`a3s use browser` is required to be a functional replacement for the locked
agent-browser release below. A build is not considered compatible merely
because it vendors the same source files.

## Locked upstream

- Repository: `https://github.com/vercel-labs/agent-browser`
- Version: `0.32.1`
- Commit: `2b202640ee89dc7aadb5e8c9d600e089e9056985`
- License: Apache-2.0
- Imported engine provenance: `crates/browser-driver/UPSTREAM.md`

## Required compatibility surface

The baseline contains:

- 82 accepted top-level CLI command names, including compatibility aliases;
- 151 typed tools from `mcp --tools all`;
- the `core`, `electron`, `slack`, `dogfood`, `vercel-sandbox`, and
  `agentcore` packaged skills;
- navigation, interaction, accessibility snapshots, waits, downloads/uploads,
  cookies and storage, network routing and HAR, tabs/windows/frames/dialogs,
  tracing/profiling/video, auth and state, confirmation policy, mobile and
  WebDriver backends, React inspection, Web Vitals, diff, batch, streaming,
  plugins, chat, dashboard, install, upgrade, and doctor behavior.

Compatibility keeps the existing `agent_browser_*` MCP tool names so current
MCP clients can switch executables without rewriting tool bindings. The MCP
server identity, documentation, filesystem layout, configuration, and primary
environment variables are A3S-owned. Legacy environment variables may remain
as lower-precedence input aliases only.

The 0.32.1 containment baseline requires `allowedDomains` parity across CLI,
MCP, and Skills. A filtered Chromium launch installs controls before resuming
pages, popups, workers, out-of-process iframes, restored targets, and new
targets; blocks `RTCPeerConnection`; and forces Chrome's
`disable_non_proxied_udp` policy. Existing CDP sessions, auto-connect,
profiles, restore/state replay, direct-page providers, unsafe startup
arguments, iOS, and Safari must fail closed because they cannot guarantee
equivalent early containment. Lifecycle waits for `load` and
`domcontentloaded` must first probe `document.readyState` so completed pages do
not wait for an event that has already fired.

## Automated gates

`crates/browser-driver/tests/upstream_parity.rs` launches the packaged driver
and checks the complete MCP inventory. It removes human-readable descriptions,
then pins names, schemas, required fields, defaults, annotations, and pagination
to this structural digest:

```text
29c1947ac94366538d7e73a12254613a0180dbc28293b8efdc3d3077b907b620
```

The command parser independently pins the sorted command vocabulary to:

```text
b2f7a70d563cd6f436e9841616d34d06781ecafc08dc28837ca08f53226b23c4
```

The parity integration test also loads every packaged skill through the actual
CLI. These gates detect removal or schema drift; they do not replace real
browser tests.

On pushes to `main`, the Browser 0.32 regression job installs managed Chrome
through the A3S component lifecycle and runs the domain-containment and
completed-page lifecycle tests serially on both macOS and Linux.

## Completion evidence

The current replacement claim covers macOS and Linux. Full replacement on
those supported runtime platforms requires all of the following evidence:

1. The command, MCP, and Skill parity gates pass.
2. All non-ignored driver tests pass.
3. The real-Chrome tests pass serially on macOS and Linux with an isolated
   temporary home and runtime directory.
4. `a3s use browser` uses A3S ACL configuration and writes only under the A3S
   Use data/cache/runtime roots unless the caller supplies an explicit path.
5. `install` and `upgrade` delegate to the A3S component lifecycle and never
   update an unrelated npm, Homebrew, or Cargo package.
6. Release archives contain `a3s-use`, `a3s-use-browser-driver`, packaged
   skills, the dashboard, and required license/provenance notices on every
   supported macOS and Linux target.
7. An installed release passes smoke tests through the umbrella `a3s use
   browser` command and through standard MCP.

Until every item has direct evidence, the replacement remains incomplete.

## Windows roadmap

Windows is currently a preview packaging target, not a supported Browser
runtime target. CI still compiles the workspace and runs command parsing, MCP
schema, Skills, packaging, and non-browser-runtime tests on Windows. The
real-Chrome cross-process persistent-session test is compiled but explicitly
ignored there until the first `browser open` call has the same bounded,
cleanup-safe behavior proven on macOS and Linux. Passing that test and adding
Windows to every completion-evidence item above are the promotion criteria.
