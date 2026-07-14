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

## Persistent sessions

Stateless CLI operations may run embedded. When Browser or Office sessions must
outlive one process, a local service exposes standard MCP. This is not a fourth
extension protocol. Foreground MCP uses stdio; a future background form uses an
authenticated loopback Streamable HTTP endpoint.

## Component CLI contract

The umbrella CLI delegates runtime lifecycle through ordinary commands:

    a3s-use component list --json
    a3s-use component status browser --json
    a3s-use component install browser --json
    a3s-use component uninstall office --json

Each invocation accepts argv and returns one versioned JSON document plus an
exit status. This is CLI automation, not JSON-RPC.

## Roadmap

1. Stabilize core, Browser, Office, extension, and component contracts.
2. Extract Chrome and Lightpanda rendering from A3S Search.
3. Migrate Search to an injected Arc<dyn PageRenderer>.
4. Add stateful Browser sessions and the standard MCP adapter.
5. Integrate a pinned OfficeCLI provider with non-retryable ambiguous writes.
6. Extend the initial local extension activation with signed remote publishers.
