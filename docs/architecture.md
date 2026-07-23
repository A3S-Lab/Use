# Architecture

## Domain boundary

Browser and OCR are typed libraries with reserved built-in command routes. Box
is a separately managed component-backed route. Other application domains,
including A3S Office, are installed from independently released external
capability packages.

External source repositories do not become runtime dependencies of the host.
They produce immutable packages whose ACL manifests declare repository
identity, A3S Use compatibility, and native CLI, standard MCP, and/or Skill
surfaces. The complete boundary is documented in
[External Repository Capabilities](external-repositories.md).

Search depends directly on the object-safe PageRenderer contract in
a3s-use-browser. It never executes the CLI or requires a background service.

Provider selection is typed. `DiscoveredChrome` remains the non-installing
default for embedded callers such as Search. A validated direct A3S Browser
launch selects first-use preparation, while a Code worker requests the same
bounded installer through parent confirmation. Both reuse system Chrome and
the shared A3S-managed cache before downloading. Managed downloads are
restricted to approved HTTPS hosts and redirects, bounded by size, hashed into
an installation receipt, staged outside the active version, and atomically
activated. Lightpanda assets must match the publisher SHA-256 exposed by GitHub
Releases. Chrome for Testing's current version feed does not publish an
independent SHA-256 value, so its receipt records HTTPS provenance and locally
observed hashes without claiming publisher checksum verification.

## Native extension surfaces

An external package declares any useful combination of:

- CLI: argv, stdin, stdout, stderr, and process status;
- MCP: standard MCP tools, resources, prompts, and lifecycle;
- Skill: an existing SKILL.md package.

Packages may also declare non-callable workbench contributions. An
`activity_bar` contribution references a package-owned UTF-8 HTML entry,
optional declared CSS and JavaScript assets, and a same-package Skill. A3S Use
validates every regular file, size, media type, package boundary, and SHA-256
before publishing them through the immutable capability snapshot. The consuming
Web host owns sandboxing, CSP, messaging, and human review; the contribution
never becomes another execution protocol.

The package manifest is `a3s-use-extension.acl` and is parsed by `a3s-acl`.
Schema version 2 binds the package to a credential-free HTTPS repository
identity and a SemVer `requires_use` range. A3S Use owns identity, routes,
trust, compatibility, activation, and lifecycle around the surfaces. It does
not clone source repositories, run their build scripts, define JSON-RPC
methods, or convert surfaces implicitly.

`a3s-use-science` is the reference multi-surface extension. It uses the
first-party [A3S Science](https://github.com/A3S-Lab/Science) repository as the
canonical home for the broader scientific catalog while its native Rust source
is developed here. It remains a separate process and package. Its Rust API,
native CLI, 13 standard MCP tools, packaged Skill, and content-bound Science
Activity share typed source-specific operations; callable work remains limited
to the declared `a3s/science` CLI, MCP, and Skill surfaces. Official Use
archives may carry the package as a digest-bound installation source, but it
stays inactive until the user applies a reviewed installation plan. This
demonstrates how a first-party toolkit can ship without expanding the reserved
built-in route set or adding a generic action envelope.

`a3s-use-ocr` implements the reserved first-party `ocr` route in the default
Use build. The release packages its content-bound Skill and exposes the native
CLI plus standard stdio MCP without a separate extension install. The process
accepts bounded local image files and binds every result to the canonical
source digest. It runs the pinned `PP-OCRv6_small` detection and recognition
models locally through ONNX Runtime, without Python, PaddlePaddle, a remote OCR
endpoint, or an alternate backend. The first CLI extraction installs or repairs
the pinned model bundle when first-use policy permits it. Standard MCP keeps
`ocr_doctor` and `ocr_extract` closed-world and read-only, while the separate
idempotent `ocr_install` network mutation requires parent confirmation.
Explicit `use/ocr` component operations remain available for preparation.

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

Explicit local sources may be directories or bounded `.tar.gz`, `.tgz`, and
`.zip` archives. Archive extraction runs off the async executor, accepts one
manifest-rooted package, preserves executable permissions, and rejects links,
path traversal, duplicate entries, unsupported file types, and expansion beyond
the package limits before lifecycle activation begins. Standard bounded macOS
AppleDouble sidecars are ignored rather than installed; they still count toward
the archive entry and expanded-byte limits.

### Unified capability projection

Resident Code hosts do not need separate discovery paths for built-in and
external domains. `capability snapshot` projects Browser, OCR, Box, and
installed extensions through one schema while preserving each binding's
`built-in` or `extension` origin. `capability watch` accepts both the extension
generation and a content revision. The generation advances for extension
lifecycle commits; the SHA-256 revision also detects built-in provider
readiness and projected Skill changes when the extension generation remains
unchanged.

Each external binding includes its repository identity and required A3S Use
range. Incompatible packages remain diagnosable but project as broken and do
not activate their MCP, Skill, or workbench assets. Every projected Skill or
workbench asset includes an absolute package path and lowercase SHA-256 so a
resident host can reject raced or modified bytes before replacing live state.
The built-in `use/ocr` route targets `ocr-native`; an installed `a3s/office`
package projects route `office` and its own declared MCP and Skill surfaces.

The projection contains content-bound Skill references and an MCP launch target,
never executable extension code or a generic action payload. Consumers still
start `a3s-use mcp serve <target>` as a standard MCP server and load `SKILL.md`
through their native Skill registry. The capability commands are versioned JSON
CLI output, not a new RPC transport.

### Immutable release descriptors

The live capability projection describes locally callable surfaces; it is not
the Cloud release record. `a3s-use-core` separately owns the versioned
`a3s.use.mcp-release.v1` and `a3s.use.skill-release.v1` machine contracts.
Their OLPC canonical JSON digest binds source commit, admitted manifest,
artifact, compatibility, and exact release dependencies.

MCP v1 maps only a digest-pinned OCI artifact and standard Streamable HTTP
health/lifecycle contract to a Runtime Service. Skill v1 maps only a
content-bound `SKILL.md` bundle to immutable Agent input. It has no executable,
port, health, or Runtime fields and cannot be deployed alone. Cloud resolves
artifact storage separately by digest; mutable tags and source branches never
enter release identity. The complete contract and cross-SDK fixtures are in
[release descriptors](release-descriptors.md).

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

External repositories own their domain-specific session models. A3S Office, for example, exposes typed document sessions from its own `a3s-office mcp` standard MCP server. A3S Use only acquires the installed package lease and launches the manifest-declared surface; it does not duplicate the Office engine or its MCP tool vocabulary.

External MCP packages are launched from their declared executable, arguments,
and transport. A3S Use owns package identity and activation, not the package's
MCP tool vocabulary. The managed Browser deployment does not aggregate,
translate, or proxy Office and extension tool vocabularies.

## Component CLI contract

The umbrella CLI delegates lifecycle through ordinary versioned commands:

```bash
a3s-use component list --json
a3s-use component status office --json
a3s-use component install a3s/office \
  --from ./a3s-use-office \
  --allow-unsigned \
  --json
a3s-use component uninstall office --json
```

Initial installation requires the stable package ID because an uninstalled
route has no trustworthy owner. After installation, status, diagnosis, MCP
launch, command delegation, and uninstall can resolve the unique route.
Enable and disable continue to use the package ID so lifecycle automation is
not coupled to a presentation alias.

Each invocation returns one versioned JSON document plus an exit status. This
is CLI automation, not JSON-RPC. Provider-specific installation remains inside
the external repository package; A3S Use owns only package trust, compatibility,
activation, route leases, and removal.

## Roadmap

Implemented:

1. Typed Browser, OCR, Box, extension, and component contracts.
2. Native CLI, standard MCP, Skill, and content-bound workbench extension surfaces.
3. Atomic extension install, upgrade, enable, disable, uninstall, watch, and route draining.
4. Local reviewed packages, release-bundled packages, and TUF-verified registries.
5. A unified generation and revision capability projection for resident hosts.
6. Schema v2 repository identity and SemVer host compatibility.
7. Package-ID and route-based discovery, diagnosis, delegation, MCP launch, and removal.
8. A3S Office as an independently released reference repository package.

Next:

1. Production publication for the official A3S extension registry, including an offline-held root-key policy and release automation.
2. Additional external repositories adopting schema v2 and publishing reproducible package provenance.
3. Windows real-Chrome persistent sessions with the same runtime guarantees as macOS and Linux.
