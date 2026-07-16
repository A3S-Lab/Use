# Architecture

## Domain boundary

Browser and Office are typed libraries and reserved built-in command routes.
The default binary cannot omit their command and diagnostic surfaces, although
provider runtimes may be missing.

Search depends directly on the object-safe PageRenderer contract in
a3s-use-browser. It never executes the CLI or requires a background service.

Provider selection is explicit. `DiscoveredChrome` is the default and never
downloads software. Only a `Managed*` provider or an explicit component install
authorizes a download. Managed downloads are restricted to approved HTTPS
hosts and redirects, bounded by size, hashed into an installation receipt,
staged outside the active version, and atomically activated. Lightpanda assets
must match the publisher SHA-256 exposed by GitHub Releases. Chrome for Testing's
current version feed does not publish an independent SHA-256 value, so its
receipt records HTTPS provenance and locally observed hashes without claiming
publisher checksum verification.

## Native extension surfaces

An external package declares any useful combination of:

- CLI: argv, stdin, stdout, stderr, and process status;
- MCP: standard MCP tools, resources, prompts, and lifecycle;
- Skill: an existing SKILL.md package.

The package manifest is a3s-use-extension.acl and is parsed by a3s-acl. A3S Use
owns identity, routes, trust, activation, and lifecycle around the surfaces. It
does not define JSON-RPC methods or convert surfaces implicitly.

## Hot-plug registry

Extension code remains behind native process boundaries. The registry is a
derived, immutable route projection with a schema version and monotonic
generation. Validated receipts are the source of truth. Each lifecycle commit
writes its receipt first and atomically publishes the resulting registry
snapshot; reconciliation repairs a missing publication after a crash.

Install and upgrade stage into a unique immutable package directory. The
active receipt switches atomically, while existing calls keep a shared package
lease and continue against the generation they accepted. Disable and uninstall
publish an invisible route before waiting for the exclusive drain lease. Each
binding includes the immutable package root, so every changed activation is
observable even when its version and manifest digest are unchanged. This
ordering gives the lifecycle three explicit guarantees:

1. no new call is admitted after the disable generation is visible;
2. an accepted CLI or MCP process retains its package until it exits;
3. a drain timeout returns an error while the route remains disabled.

Consumers read `extension snapshot` for the current projection or long-poll
`extension watch --after-generation <n>` for a later generation. No daemon,
custom RPC protocol, `dlopen`, or restart is required.

### Unified capability projection

Resident Code hosts do not need separate discovery paths for built-in and
external domains. `capability snapshot` projects Browser, Office, Box, and
enabled extensions through one schema while preserving each binding's
`built-in` or `extension` origin. `capability watch` accepts both the extension
generation and a content revision. The generation advances for extension
lifecycle commits; the SHA-256 revision also detects built-in provider
readiness and packaged Skill changes when the extension generation remains
unchanged. Each Skill projection includes an absolute package path and its own
lowercase SHA-256, allowing a resident host to reject raced or modified bytes
before replacing its live Skill.

The projection contains content-bound Skill references and an MCP launch target,
never executable extension code or a generic action payload. Consumers still
start `a3s-use mcp serve <target>` as a standard MCP server and load `SKILL.md`
through their native Skill registry. The capability commands are versioned JSON
CLI output, not a new RPC transport.

## Component-backed routes

`box` is a reserved Use route backed by the independently managed A3S Box
component. The umbrella CLI remains the only Box installer and receipt owner.
For `a3s use box ...`, it resolves or installs Box, canonicalizes its executable
path, and passes that path to Use for one child invocation. Use validates the
absolute regular executable and delegates argv, streams, working directory,
environment, and exit status. It does not discover Box on `PATH`, copy it, or
create a wrapper package. Other Use commands never auto-install Box.

## Persistent sessions

Browser exposes one typed session manager through the standard MCP tool set.
`mcp serve browser` provides stdio for MCP hosts. Separate Browser CLI
invocations connect to a managed standard MCP Streamable HTTP deployment so
that tabs, snapshots, and element references survive the invoking CLI process.
The deployment:

- binds only to an ephemeral `127.0.0.1` port;
- requires a random bearer token and validates `Origin` when present;
- records endpoint, token, PID, and ownership in a private generated receipt;
- shares one typed `BrowserSessions` instance across MCP client sessions;
- has bounded idle and maximum lifetimes;
- stops through a standard MCP tool and cleans up tabs, Chrome, and its receipt.

This is a deployment of the existing MCP server, not an A3S JSON-RPC service.
CLI session commands call MCP tools with their published schemas. The token is
never included in normal CLI output. `browser render` and Search remain direct
in-process Rust calls and never require the service.

Office commands are delegated to OfficeCLI's native CLI, and `mcp serve office`
launches OfficeCLI's own standard MCP server. OfficeCLI may internally reuse a
resident process, but its pipe and framing remain an OfficeCLI implementation
detail; A3S Use neither speaks nor reimplements them.

External MCP packages are launched from their declared executable, arguments,
and transport. A3S Use owns package identity and activation, not the package's
MCP tool vocabulary. The managed Browser deployment does not aggregate,
translate, or proxy Office and extension tool vocabularies.

## Component CLI contract

The umbrella CLI delegates runtime lifecycle through ordinary commands:

    a3s-use component list --json
    a3s-use component status browser --json
    a3s-use component install browser --json
    a3s-use component install office --json
    a3s-use component uninstall office --json

Each invocation accepts argv and returns one versioned JSON document plus an
exit status. This is CLI automation, not JSON-RPC.

Managed Office installation is fixed to a reviewed OfficeCLI release. The
binary is fetched only by an explicit install or repair command, restricted to
approved HTTPS hosts, bounded by size, and checked against the publisher's
SHA-256 before atomic activation. Native OfficeCLI execution sets
`OFFICECLI_SKIP_UPDATE=1`; A3S upgrades are explicit component operations.

## Roadmap

Implemented:

1. Core, Browser, Office, extension, and component contracts.
2. Chrome and Lightpanda extraction from Search.
3. Search injection through `Arc<dyn PageRenderer>`.
4. Typed Browser rendering and session tools over standard MCP stdio.
5. Native OfficeCLI delegation, pinned installation, and non-retryable
   ambiguous-write handling.
6. Direct standard-MCP launch for Office and external packages.
7. Authenticated loopback standard MCP Streamable HTTP for persistent Browser
   CLI sessions.
8. The complete locked agent-browser `0.31.2` command, MCP, Skill, Dashboard,
   lifecycle, and interactive Browser surface behind `a3s use browser`.
9. Generation-based extension hot plug with enable, disable, watch, graceful
   route draining, and crash reconciliation.
10. The component-backed `a3s use box` route with one Box binary and receipt.
11. A unified generation/revision capability projection for built-in and
    external MCP and Skill surfaces.

Next:

1. Windows real-Chrome persistent sessions. Windows remains a preview build
   until separate `a3s use browser` invocations can open and reuse a session
   with the same runtime guarantees as macOS and Linux. Windows compilation,
   CLI/MCP schemas, packaged assets, and non-runtime tests remain continuously
   checked in CI meanwhile.
2. Signed remote extension publishers. External publisher infrastructure is
   independent of the built-in Browser compatibility contract.
