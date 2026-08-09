<p align="center">
  <img
    src="assets/readme/hero.svg"
    width="100%"
    alt="A3S Use resolves one exact cognitive-package graph and publishes Tool, MCP, OKF, A3S Flow, Skill, and UI through one atomic cutover"
  />
</p>

<p align="center">
  <strong>AI Native Package Manager for native capabilities and versioned cognitive packages.</strong>
</p>

<p align="center">
  <a href="https://a3s-lab.github.io/Use/">Website</a> ·
  <a href="#install-or-build">Install</a> ·
  <a href="#cognitive-package-format">Package format</a> ·
  <a href="#replaceable-registries-and-exact-locks">Registries</a> ·
  <a href="#current-contract-baseline">Contracts</a> ·
  <a href="#implementation-status">Status</a> ·
  <a href="ROADMAP.md">Roadmap</a>
</p>

> [!WARNING]
> **Development preview — not production-ready.** The cognitive-package
> platform has not shipped a supported product release. Pre-release manifests,
> receipts, operation records, catalog metadata, and host protocols are not
> compatibility targets: unsupported state is rejected with a cleanup and
> reinstall instruction. Version tags do not change this release status.

## What A3S Use is

A3S Use resolves, verifies, installs, upgrades, and removes an exact SemVer
package graph. A cognitive package can contribute six named surfaces:
**Tool, MCP, OKF, A3S Flow, Skill, and UI**. The package graph is the lifecycle
unit; its surfaces are prepared together and become visible through one
immutable capability-snapshot cutover.

It is designed for A3S hosts on Linux, macOS, and Windows. It does not try to
replace `apt`, Homebrew, or WinGet for arbitrary system software. A3S Use owns
package trust, immutable generations, receipts, dependency ordering, lifecycle
journals, and capability evidence. Runtime, Gateway, Flow, Knowledge, and UI
hosts keep ownership of execution and presentation.

The current architecture has three non-negotiable properties:

- **One package graph:** dependencies install forward; retirement runs in
  reverse; retained dependencies must match their exact lock evidence.
- **One reviewed mutation path:** planning is read-only; apply accepts the
  reviewed operation ID, plan digest, and confirmation. There is no direct
  enable/disable mutation API.
- **One current protocol baseline:** pre-release formats are rejected rather
  than decoded, migrated, or silently defaulted.

## Proof in this repository

The implementation and fixtures exercise the product model directly:

- [`plugin-v3-cognitive`](crates/extension/fixtures/packages/plugin-v3-cognitive/)
  is a content-addressed package containing all six surface kinds.
- [`PluginPackageResolver`](crates/core/src/plugin/package_resolution.rs)
  resolves bounded SemVer closures and rejects cycles, incompatible releases,
  and cross-Registry ambiguity.
- [`RegistrySourceStore`](crates/extension/src/registry_sources/mod.rs) persists
  canonical revision-addressed ACL source configuration, imports digest-bound
  trusted roots, and isolates TUF metadata and caches by source identity.
- [`CognitivePackageManager`](src/cognitive_package/) binds signed catalog
  evidence, exact locks, reviewed plans, authorization, and crash replay.
- [`bind_cognitive_package_provider_plan`](src/cognitive_package/provider_plan.rs)
  executes the authorization-safe two-pass provider protocol: unbound draft,
  assigned-provider preflight, host authority, canonical Grant semantics, and
  drift-checked final selection.
- [`PluginPackageGraphLifecycleCoordinator`](src/plugin_lifecycle/graph.rs)
  prepares dependency closures, performs one durable Registry cutover, drains
  accepted calls, and retires exact prior generations.
- [`SqliteOkfKnowledgeAdapter`](src/okf_knowledge/sqlite/mod.rs) stages,
  promotes, searches, and removes scope-isolated OKF projections with exact
  package-generation citations, bounded receipt-accounted storage, global
  tombstone pruning, physical SQLite compaction after removal, integrity
  auditing, non-overwriting verified backups, and authority-preserving FTS
  repair.
- [`A3sFlowLifecycleHost`](src/flow_runtime/lifecycle.rs) delegates Flow
  preflight to the real `a3s-flow` Native TypeScript runtime and records an
  exact-generation binding.
- [`StandaloneCognitivePackageLifecycleFactory`](src/cognitive_package/hosts.rs)
  composes that host only from an explicit absolute compiler path; failed
  preflight remains unpublished and can replay from exact durable evidence.
- Contract fixtures under [`crates/core/fixtures/plugins`](crates/core/fixtures/plugins/)
  freeze canonical JSON and SHA-256 digests for the current schemas.

CI runs formatting, all non-Science workspace tests, Clippy,
release-container conformance, and platform jobs. The Windows preview gate now
executes the complete current non-Science workspace suite, including a real
directory-junction regression for the shared reparse-point guard. Signed
Registry, dependency-graph, Grant, Flow-preflight/lifecycle, and standalone OKF
scenarios also run through the real CLI. Its killed-process coverage now
includes removed-dependency cleanup after upgrade cutover and an uninstall
killed after the durable Registry hide but before the package hide receipt.
The latter restarts from the same plan, blocks on the accepted-call generation
lease, then drains and removes the generation without another Registry
generation; missing package state without the exact cutover still fails closed.
A test-binary subprocess matrix also exits after each durable host effect but
before its receipt for every canonical install, upgrade, enable, disable, and
uninstall checkpoint; recovery reuses the exact idempotency key without
duplicating an effect, and terminal replay makes no host call. A second
test-binary subprocess matrix covers grant-bearing install, upgrade, and
uninstall graph cutovers: it exits after the atomic publish or hide effect
but before package publication receipts and Grant cutover evidence,
then proves exact-key recovery, one graph effect, completed package and Grant
journals, and terminal replay without another publish or hide. These
real-process paths and harnesses do not replace the still-open provider and
complete cross-platform failure-injection gates. The Grant Store itself also
operates a test-binary subprocess matrix across all 14 durable checkpoints in
its canonical two-candidate/two-retirement lifecycle: forward prepare,
cutover/retirement, and pre-cutover rollback each include every candidate
receipt, prior revocation, and candidate restoration.
See [Platform support](#platform-support).

## Install or build

Tagged archives remain development previews. The installers select the current
OS and architecture, require Cosign, authenticate `checksums.txt` against the
exact A3S Use tag workflow identity and GitHub OIDC issuer, verify the selected
archive SHA-256 before extraction, reject unsafe archive entries, and
atomically publish a user-scoped command. Download the installer first so it
can be reviewed before execution.

Linux or macOS:

```bash
curl --proto '=https' --tlsv1.2 -fsSLo /tmp/a3s-use-install.sh \
  https://raw.githubusercontent.com/A3S-Lab/Use/main/install.sh
sh /tmp/a3s-use-install.sh
```

Windows x86_64 with PowerShell 7:

```powershell
$installer = Join-Path $env:TEMP 'a3s-use-install.ps1'
Invoke-WebRequest https://raw.githubusercontent.com/A3S-Lab/Use/main/install.ps1 -OutFile $installer
& $installer
```

`cosign` must be installed on `PATH`; an explicit trusted executable can be
selected with `--cosign <path>` on Unix or `-CosignPath <path>` on Windows.

Pass `--version <version>` on Unix or `-Version <version>` on Windows to pin a
tag. Unix installs under `$XDG_DATA_HOME/a3s-use` (or
`$HOME/.local/share/a3s-use`) and links from `$HOME/.local/bin`. Windows uses
`%LOCALAPPDATA%\A3S\Use`, creates an owned command shim under
`%LOCALAPPDATA%\A3S\bin`, and adds that bin directory to the user `PATH` unless
`-NoPathUpdate` is set. The managed launcher binds the packaged OCR models,
OCR Skills, and Browser Skills while preserving explicit environment
overrides. Reinstalling the same version revalidates the complete installed
tree. Missing Cosign, invalid Sigstore evidence, a checksum mismatch, tampered
existing release, unsafe path, link/reparse point, concurrent installer, or
unmanaged command conflict fails without changing the active command. The
verified checksum manifest and Sigstore bundle are retained in the immutable
version directory. See
[Verified release installation](docs/release-installation.md) for the trust
boundary and custom-path options.

Tagged releases now publish deterministically serialized archives, one SPDX
JSON SBOM per platform, GitHub OIDC build-provenance and SBOM attestations, and
a keyless Sigstore bundle for `checksums.txt`. The Release workflow pins every
Action plus the Rust, Python, Syft, and Cosign versions, derives archive
timestamps from the tag commit, and verifies its checksum signature before
publication. The installers now fail closed unless Cosign authenticates that
same bundle against the exact tag identity before the archive is downloaded.
For every target, a second clean runner without a compiled-artifact cache
rebuilds all shipped native executables and must byte-match the primary
archive. Its deterministic `.reproducibility.json` evidence is attested,
checksummed, signed, and published beside the archive. Operators can
additionally verify the GitHub attestations by following
[Verified release installation](docs/release-installation.md#additional-independent-verification).
An externally operated full-archive witness, evidence retention outside GitHub
Release, and the remaining product gates are still open, so this does not
change the preview status above.

### Build and verify

Rust 1.85 or newer is required. Until the product release gate is complete,
build from source:

```bash
git clone https://github.com/A3S-Lab/Use.git
cd Use
cargo build --workspace --bins --locked
./target/debug/a3s-use doctor --json
./target/debug/a3s-use capability snapshot --json
```

The standalone CLI currently exposes package-graph lifecycle, diagnostics,
capability observation, built-in Browser/OCR routes, cited OKF search, and
exact-scope Knowledge storage operations:

```text
a3s-use install <publisher/name> [--registry-name <name>] [--offline] [--json]
a3s-use upgrade <publisher/name> [--registry-name <name>] [--offline] [--json]
a3s-use uninstall <publisher/name> [--json]
a3s-use extension inspect <publisher/name> [--json]
a3s-use knowledge search <query> [--limit <n>] [--json]
a3s-use knowledge usage [--scope-kind <user|workspace>] [--scope-id <id>] [--json]
a3s-use knowledge audit [--scope-kind <user|workspace>] [--scope-id <id>] [--json]
a3s-use knowledge backup <path> [--scope-kind <user|workspace>] [--scope-id <id>] [--json]
a3s-use knowledge verify-backup <path> [--scope-kind <user|workspace>] [--scope-id <id>] [--json]
a3s-use knowledge repair-search-index --yes [--scope-kind <user|workspace>] [--scope-id <id>] [--json]
a3s-use registry source list [--json]
a3s-use registry source add <name> --url <https-url> --trust-root <sha256> [source options] [--json]
a3s-use registry source replace <name> --url <https-url> --trust-root <sha256> --expected-revision <sha256> --yes [source options] [--json]
a3s-use registry source default|enable|disable|remove <name> --expected-revision <sha256> --yes [--json]
a3s-use registry cache usage [--registry-name <name>] [--json]
a3s-use registry cache prune [--registry-name <name>] [cache options] --yes [--json]
a3s-use capability snapshot|watch [options] [--json]
```

The default Knowledge policy bounds each complete User or Workspace scope to
512 MiB of receipt-accounted expanded content, 256 retained projections, 32
generations per surface, and 256 removal tombstones. Staging checks the whole
scope atomically; receipt-owned removal frees quota, prunes old tombstones, and
compacts SQLite plus its WAL. `knowledge usage --json` reports the exact scope,
current counts, quota, allocated database bytes, and reclaimable bytes. These
standalone controls also audit SQLite, receipt, scope, foreign-key, and FTS
consistency. Backup writes one versioned, SHA-256-bound SQLite snapshot without
overwriting an existing file; verification reopens and audits the embedded
database offline. Search-index repair requires `--yes` and rebuilds only FTS
rows derived from already-validated documents. It never rewrites package
receipts, projection state, or authorization evidence.

The backup is an integrity-checked scope database snapshot, not a signed trust
artifact or a whole-product restore. Registry receipts, immutable package
roots, lifecycle journals, Grants, Flow history, bindings, and UI state still
require their own coordinated backup/restore procedure. Workspace operations
require an explicit `--scope-id`; the CLI never guesses a current Workspace
identity. See [OKF Knowledge operations](docs/okf-knowledge-operations.md).

`extension inspect --json` includes the latest and previous durable lifecycle
operations for the default User scope. The versioned diagnostic projection
reports action, status, generation, artifact digests, checkpoint progress,
bounded error codes, timings, and rollback evidence. It deliberately omits
checkpoint idempotency keys, credentials, tokens, secret values, and
package-authored error text. This is checkpoint evidence for diagnosis, not a
telemetry service or backup/restore mechanism.
One reviewed graph operation can create consecutive candidate and retirement
phase intents for the same package. Those records intentionally share an
`operationId`; consumers distinguish the exact phase by `intentDigest`, action,
generation, and artifact digests.

### Replaceable Registry sources

The standalone CLI persists a bounded set of named Registry sources in
canonical A3S ACL. The first enabled source becomes the default. Every enabled
source is supplied to dependency resolution, while `--registry-name` selects
the root source for one operation. Duplicate package identities across enabled
sources fail as ambiguous.

Configure trust before package resolution:

```bash
a3s-use registry source add packages \
  --url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --json
```

`--trusted-root /absolute/path/root.json` additionally imports an exact
digest-matching root into the managed, content-addressed trust-root store.
Source list output includes the complete configuration revision. Replacing
authority requires that reviewed revision and explicit confirmation:

```bash
a3s-use registry source list --json

a3s-use registry source replace packages \
  --url https://mirror.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --expected-revision sha256:<reviewed-configuration-revision> \
  --yes \
  --json
```

Replacing, disabling, or removing a source never rewrites installed receipts
and never deletes its identity-bound TUF metadata or target cache. Re-enabling
or restoring the exact name, URL, and bootstrap-root digest reuses that exact
state. A changed source identity receives a separate datastore, preventing old
metadata or cached targets from crossing the trust boundary.

Example development install from the configured Registry:

```bash
a3s-use install acme/research \
  --registry-name packages \
  --version 2.0.0 \
  --json
```

When a lock was reviewed separately, bind apply to it:

```bash
a3s-use install acme/research \
  --registry-name packages \
  --package-lock-digest sha256:<64-hex-digits> \
  --json
```

A mismatched lock digest fails before an archive download. Example package and
Registry names above are illustrative; this repository does not advertise a
public production Registry.

An online install verifies current TUF metadata and stores each selected
archive and signed `planning-v1.json` target in the Registry datastore's
content-addressed cache. After that exact graph has been removed, it can be
installed again without network access:

```bash
a3s-use install acme/research \
  --registry-name packages \
  --version 2.0.0 \
  --offline \
  --json
```

The same flag supports an upgrade only when the host has already refreshed the
candidate's TUF metadata and verified every selected target into the same
cache:

```bash
a3s-use upgrade acme/research \
  --registry-name packages \
  --version 2.1.0 \
  --offline \
  --json
```

Offline mode is explicit and fail-closed. It loads the same persisted Registry
source revision; revalidates cached TUF signatures, expiry, source identity,
target length, and SHA-256; and returns `registryAccess: "cached"` plus
`registrySourceRevision` in JSON. Normal online operations return
`registryAccess: "refreshed"`. Missing, disabled, expired, or tampered source
or cache evidence is an error. An online command never falls back to cached
targets after a network or refresh failure.

### Verified target cache operations

Each Registry has an independent default cache limit of 4 GiB and 4,096
combined verified targets and resumable partials, with a 256 MiB free-space
reserve. The same typed policy is enforced before downloading and while
committing a verified target. Interrupted HTTP downloads retain a digest-bound
`.target-<sha256>.part` file and retry from its exact length. A resumed response
must return the exact signed byte range; the complete file is rehashed before
atomic promotion. Stale atomic-write files are removed first, followed by the
oldest resumable partials and then the oldest verified targets until the byte,
entry, and disk-space bounds are satisfied.

Inspect cache usage without making a Registry request:

```bash
a3s-use registry cache usage \
  --registry-name packages \
  --json
```

Pruning can remove targets required for a later offline reinstall or upgrade,
so the standalone CLI requires explicit confirmation:

```bash
a3s-use registry cache prune \
  --registry-name packages \
  --cache-max-bytes 2147483648 \
  --cache-max-entries 2048 \
  --cache-min-free-bytes 536870912 \
  --yes \
  --json
```

The durable policy is configured on `registry source add` or `replace`.
Confirmed prune may use a stricter one-command override; it does not rewrite
the source configuration. Embedding hosts use the same typed
`VerifiedTargetCachePolicy`. Cache usage and pruning are zero-network
operations and validate any retained catalog-cache source identity before
inspecting or deleting targets. GC never changes installed package roots,
receipts, capability generations, or lifecycle journals. See [Registry cache
operations](docs/registry-cache-operations.md).

## Cognitive-package format

A cognitive package is an npm-like immutable distribution unit with one
`<publisher>/<name>` identity, one SemVer version, a required ACL manifest,
required package documentation, optional package dependencies, and zero or
more named surface contributions.

```text
acme-research/
├── a3s-use-extension.acl   package identity, dependencies, surfaces
├── README.md               required package documentation
├── tools/                  native Task or Service artifacts
├── releases/               immutable Tool or MCP descriptors
├── flows/                  A3S Flow TypeScript sources
├── skills/                 SKILL.md files and supporting content
├── ui/                     integrity-bound static assets
└── okf/                    Open Knowledge Format bundles
```

Only the manifest and `README.md` names are fixed. Contribution paths are
manifest-owned. The manifest is A3S ACL (`.acl`) and must be parsed with
[`a3s-acl`](https://github.com/A3S-Lab/ACL); ACL is not HCL.

```acl
extension "acme/research" {
  schema_version = 3
  version        = "2.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  dependency "acme/base" {
    version = "^1.4.0"
  }

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    executable  = "tools/convert/bin/convert"
    command     = "acme-research-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }

  mcp "library" {
    transport  = "stdio"
    executable = "tools/library/bin/library-mcp"
    args       = []
    activation = "lazy"
    optional   = false
  }

  okf "domain" {
    format_version         = "0.2"
    root                   = "okf/domain"
    content_digest         = "sha256:355b6f00153630b082e60a0f7e0b67fbbb74b2a29067bca481f7eefecbb86c7a"
    concept_count          = 4
    file_count             = 7
    expanded_bytes         = 2053
    max_files              = 256
    max_concepts           = 64
    max_expanded_bytes     = 67108864
    max_document_bytes     = 1048576
    max_links_per_document = 2048
    optional               = false
  }

  flow "review" {
    engine         = "a3s-flow"
    runtime        = "native-ts"
    source         = "flows/review.ts"
    export         = "run"
    requires_tool  = ["convert"]
    requires_mcp   = ["library"]
    requires_okf   = ["domain"]
    optional       = false
  }

  skill "review" {
    path          = "skills/review/SKILL.md"
    requires_tool = ["convert"]
    requires_mcp  = ["library"]
    requires_okf  = ["domain"]
    requires_flow = ["review"]
    optional      = false
  }

  ui "review" {
    entry     = "ui/review/index.html"
    skill     = "review"
    bind_mcp  = ["library"]
    bind_flow = ["review"]
    optional  = false
  }
}
```

| Surface | Package contribution | Readiness owner |
| --- | --- | --- |
| Tool | Package-local native Task or digest-pinned Task/Service release | Signed planning launcher plus the native provider, or an explicitly selected Runtime |
| MCP | Package-local stdio server or digest-pinned HTTP release | Signed stdio launcher plus the native provider, or Runtime/Gateway readiness |
| OKF | Open Knowledge Format concept graph | Knowledge host stage, promotion, observation, and cited retrieval |
| A3S Flow | TypeScript workflow source with explicit surface edges | `a3s-flow` preflight and exact compiled binding |
| Skill | Content-bound `SKILL.md` plus supporting files | Static projection after required dependencies are ready |
| UI | Integrity-bound static entry point | The Code Web host now publishes exact generation/revision-bound sandbox documents; browser-side iframe/message/navigation composition, active-generation drain, reviewed backend bindings, and native-host rendering remain host work |

Surfaces are selectable for projection, but they are not independently
installed, upgraded, or removed outside their owning package generation.

## One A3S Flow lifecycle

A3S Use does not define a second workflow engine.

- The package manifest declares the `flow` surface, source digest, export, and
  Tool/MCP/OKF dependencies.
- `a3s-flow` owns compilation and execution semantics.
- A host may use `flow.json` as a visual design or deployment document, but it
  is not another package receipt, dependency resolver, or lifecycle journal.
- A3S Code is a local host and A3S OS may be a remote execution target; both
  must resolve the same package-owned Flow identity.

Required Flow publication fails closed when the embedding host does not inject
the declared Flow runtime. There is no source-presence or `PATH` fallback. The
standalone CLI opts in with the same reviewed absolute compiler path for
install, upgrade, and uninstall across process restarts:

```bash
A3S_FLOW_NATIVE_TS_COMPILER=/opt/a3s/bin/a3s-flow-native-compiler \
  a3s-use install acme/workflows --registry-name packages --json
```

`CognitivePackageManager::new` remains provider-free and deterministic;
`CognitivePackageManager::from_env` is the explicit standalone composition.
A missing or failing compiler leaves an installed-disabled candidate receipt,
but the immutable capability snapshot remains at its exact prior generation
and does not project staged package state. Lifecycle diagnostics retain the
bounded failure evidence. A repaired retry resumes the same admitted plan and
exact package generation, then publishes one reviewed capability cutover
instead of guessing or exposing partial state.

## Replaceable Registries and exact locks

Registry URLs and trust roots are host input, never compiled into the resolver.
A host can select a mirror, private Registry, or another explicitly trusted TUF
source without changing package logic. Each dependency can resolve from a
different enabled source, but the same package appearing in multiple enabled
sources is rejected as ambiguous.

Current Registry rules:

- TUF target `custom.a3s` metadata contains one complete catalog-v3 record.
- Every executable catalog carries one separately signed `planning-v1.json`
  target. It distinguishes package-local Tool/stdio MCP launchers from
  release-backed Runtime workloads before the archive is downloaded.
- Mixed packages are planned as one exact provider set: native Tool Tasks and
  stdio MCP remain on the built-in launcher, while release-backed Tool Tasks,
  Tool Services, and HTTP MCP require explicit host assignments from a typed
  `RuntimeClientRegistry`. Missing Grants, generations, assignments, or
  providers fail without fallback.
- Provider selection is two-pass. Capability preflight exposes the real
  provider enforcement to host policy; the final pass binds canonical Grant
  semantics and must retain the same provider ID, build, normalized
  capabilities, and enforcement. The final policy decision must also remain
  unchanged.
- Installed schema-v3 receipts retain the exact signed planning bundle for
  every executable package. Enablement can therefore be reviewed again after
  restart without consulting a mutable Registry, while catalog, manifest, and
  installed package bytes are still revalidated.
- Apply-time host adapters re-derive Grant proposals from the immutable
  reviewed plan and durable snapshot, reconstruct the exact Runtime selection,
  and require provider evidence to match byte-for-byte. The shared A3S CLI,
  TUI, Web, and managed-host enablement paths persist the reconstruction inputs
  instead of process-local clients.
- Retirement never chooses a new activation provider. Disable, uninstall, and
  prior-generation upgrade cleanup reopen the provider recorded by the exact
  Runtime binding receipt; provider ID, build, and normalized capabilities are
  rechecked before a Service is drained and removed.
- The catalog record, archive, expanded package, and manifest all have exact
  digest/size evidence.
- Archive admission rebinds every planning launcher to the exact digest-bound
  `.acl` manifest and release descriptor; surface kind, activation, executable,
  argv, command, timeout, and transport drift fail closed.
- Prepared downloads and installed Registry/TUF receipts must retain the full
  verified catalog record and its provenance.
- Online preparation persists verified archives and signed planning targets at
  `<registry-datastore>/verified-targets/sha256/<digest>`. Cache reads reject
  links and non-regular files, stream-copy and rehash the target, and verify its
  signed length before package admission.
- Explicit cached resolution revalidates the last trusted, unexpired TUF
  metadata and exact Registry name, URL, and trust root. It never refreshes the
  network and never weakens source or package-lock provenance.
- A typed per-Registry policy bounds retained bytes and entries and reserves
  staging/cache disk space before target requests and commits. Digest-bound
  partials survive process interruption, resume only through an exact HTTP
  range response, and are never staged before full signed-length and SHA-256
  verification. Automatic and confirmed GC remove stale writes, then the
  oldest partials and verified targets, under the same cache lock and
  synchronize the directory after deletion.
- Watchers read immutable publications without waiting behind writers. If a
  one-time crash reconciliation briefly owns the Registry lock, lifecycle
  writers wait asynchronously for at most two seconds; genuinely concurrent
  mutations still fail with `use.extension.busy`.
- An installed receipt remains bound to its source name, URL, root digest,
  release channel, target, and TUF role versions.
- Replacing source configuration never rewrites installed receipt provenance;
  restoring the exact source or reinstalling is required before upgrade.

The canonical package lock freezes selected versions, dependency edges, host
target, `requires_use`, archive and package digests, Registry identity, and TUF
provenance for every node. Resolution fails closed on cycles, incompatible
constraints, missing providers, source ambiguity, and configured search bounds.

## Reviewed lifecycle

```text
verified catalog set
        ↓
resolve SemVer closure → freeze exact lock → build immutable plan
        ↓                                      ↓
policy + confirmation                  reviewed plan digest
        └──────────────────────┬───────────────┘
                               ↓
download changed nodes → commit disabled → prepare dependencies forward
                               ↓
                  one durable capability cutover
                               ↓
             drain prior calls → retire generations reverse
```

Install, upgrade, uninstall, enable, and disable are durable operations. Apply
revalidates the exact package locks, catalog evidence, host capabilities,
policy authority, scope, confirmation, and current state before mutation.
Upgrade plans bind both the prior and candidate locks and classify every node
as `Add`, `Replace`, `Remove`, or `Retain`.

Managed activation and retirement intentionally use different evidence. An
enable or candidate install/upgrade uses host-owned two-pass provider selection.
A disable, uninstall, or prior-generation upgrade cleanup carries no candidate
selection and retires the exact receipt-owned binding. If a stopped binding is
re-enabled with new authorization semantics, the old binding is retired before
the same package generation is rebound; conflicting immutable receipts are
never overwritten in place.

The manager MCP toolset exposes read-only planning separately from mutation:

```text
plugin_plan_install     plugin_plan_upgrade     plugin_plan_uninstall
plugin_plan_enable      plugin_plan_disable     plugin_apply_plan
```

`plugin_apply_plan` is the only manager mutation entry point. A `NoChange`
enablement result is terminal and has no synthetic mutation identity. Crash
recovery resumes the exact stored plan and authorization; re-reading a finished
operation returns its durable result without repeating side effects.
Applying and rolling-back records both retain exclusive operation ownership;
a different intent cannot replace either one before it reaches a terminal
record. Inspection reads the latest and previous records under the same
package-scoped journal lock.

Workspace Grants are composed into the same graph saga. Candidate grants are
persisted before package preparation, the exact Registry cutover is recorded,
accepted calls drain before prior grants are revoked, and a pre-cutover failure
rolls package and Grant candidates back together.

## Architecture

<p align="center">
  <img
    src="assets/readme/architecture.svg"
    width="100%"
    alt="Trusted sources enter one reviewed Plugin Manager and A3S Use graph lifecycle before an atomic capability snapshot reaches A3S hosts"
  />
</p>

| Boundary | Owns | Does not own |
| --- | --- | --- |
| Host Plugin Manager | Registry configuration, trust roots, policy, user confirmation, reviewed plan/apply | Package bytes or provider scheduling internals |
| A3S Use | Verification, exact locks, immutable generations, receipts, grants, lifecycle journals, cutover evidence | Generic scheduling or UI rendering |
| Runtime/Gateway | Tool and MCP provider execution, health, and drain | Package resolution or trust policy |
| A3S Flow | Workflow compilation, execution, replay, and observation | A parallel package lifecycle |
| Knowledge host | OKF validation, indexing, promotion, cited search | Process execution |
| A3S Code/Web/OS | Product UX, workspace/session scope, rendering, injected providers | A second package manager |

See [Plugin Platform Architecture](docs/plugin-platform-architecture.md),
[Lifecycle and Security](docs/plugin-platform-lifecycle-and-security.md), and
[ADR-002](docs/adr-002-cognitive-package-lifecycle-saga.md).

## Current contract baseline

Only the following cognitive-package protocol line is accepted:

| Contract | Current schema |
| --- | --- |
| Package manifest | schema version `3` |
| Registry source configuration | ACL schema version `1` |
| Signed catalog record | `a3s.use.plugin-catalog.v3` |
| Installed receipt | schema version `3` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v4` (protocol `4`) |
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v4` |
| Pending package graph | `a3s.use.pending-package-graph-operation.v2` |
| Lifecycle diagnostic | `a3s.use.plugin-lifecycle-diagnostic.v1` |
| Enablement state / operation | `v2` / `v2` |
| OKF Knowledge backup | `a3s.use.okf-knowledge-backup.v1` |

SemVer dependency constraints, `requires_use`, OS/target checks, and
host/provider capability checks are product behavior, not backward-compatibility
branches. Older pre-release schemas and persisted state are deliberately not
migrated. Delete the unsupported state and reinstall with the current build.

## Implementation status

| Area | Status |
| --- | --- |
| Six-surface ACL package contract | Implemented and fixture-backed |
| Signed catalog-v3, TUF verification, and durable replaceable Registry sources | Implemented in the engine and standalone CLI |
| Manager MCP install planning with canonical `registryName` source selection | Implemented in toolset v4; upgrade remains pinned to installed provenance |
| Verified target cache, explicit offline install/upgrade, bounded retention, resumable downloads, usage, and confirmed GC | Implemented with interruption, range, tamper, and zero-network tests |
| Signed native Tool/stdio MCP planning and post-download manifest binding | Implemented and contract-tested |
| Bounded SemVer dependency resolution and exact locks | Implemented |
| Install, upgrade, uninstall graph ordering | Implemented |
| Durable atomic Registry cutover and exact replay | Implemented |
| Package-host side-effect/receipt ambiguity recovery | Every canonical install, upgrade, enable, disable, and uninstall checkpoint passes subprocess-exit, exact-key recovery, single-effect, and terminal-replay tests. A real CLI uninstall also passes durable-hide-before-receipt kill, exact-plan restart, accepted-call drain, removal, and no-generation-inflation checks; the remaining real-process and Grant checkpoints stay open |
| Grant-bearing graph cutover effect/receipt ambiguity recovery | Install, upgrade, and uninstall atomic publish/hide boundaries pass subprocess-exit, exact-key recovery, single-effect, completed-journal, and no-republication tests; real-process failure injection remains open |
| Grant Store journal/receipt crash recovery | All 14 durable checkpoints in the canonical two-candidate/two-retirement lifecycle pass subprocess-exit convergence and exact terminal replay across prepare, cutover/retirement, and pre-cutover rollback; real CLI and cross-platform product qualification remain open |
| Secret-free lifecycle checkpoint diagnostics | Implemented for latest/previous package operations through `extension inspect --json`; broader operational telemetry remains open |
| Watcher-safe bounded Registry mutation locking | Implemented and real-process tested |
| Plan-v4 reviewed enable/disable and terminal `NoChange` | Implemented in the manager contract and package engine |
| Workspace Grant composition and drain-before-revoke | Implemented in core/standalone lifecycle paths |
| Mixed native/managed provider planning | Implemented in Use and the shared A3S host path: unbound drafts, assigned-provider preflight, host policy, canonical Grant-bound final selection, durable planning bundles/Grant snapshots/provider generations, exact apply-time reconstruction, restart replay, and provider-drift rejection are tested |
| Exact published-generation dispatch lease | Implemented in the Use Registry and the Code TUI/Web Knowledge query carrier; accepted queries hold exact package-generation leases through backend search and final Registry revision verification |
| Standalone Task, stdio MCP, explicit A3S Flow preflight, Skill/UI, and SQLite/FTS5 OKF hosts | Implemented |
| Code Web Activity document boundary | Enabled catalog entries publish an exact generation/revision URL; the host serves only verified HTML/CSS/JS with opaque-origin CSP and restrictive security headers, preserves the URL across restart, rejects stale generations with `410 Gone`, and exposes no managed paths. Browser-side iframe adoption, message/navigation brokering, backend bindings, and active-document drain remain open |
| Managed Runtime receipt lifecycle | Typed generation-bound endpoints, receipt-owned provider reconnection, drain-before-stop, route-remove-before-Runtime-remove, exact prior-generation retirement, and stopped-binding reauthorization are contract-tested; production provider/Gateway injection remains open |
| Scope-bounded OKF quota, retention, tombstone GC, SQLite compaction, and usage diagnostics | Implemented in the standalone Knowledge backend |
| Scope-local OKF integrity audit, verified database backup, and derived FTS repair | Implemented and real-process tested; restore and whole-product recovery remain open |
| Runtime Service, HTTP MCP, managed Knowledge recovery/rollback, and sandboxed UI composition in every declared host | In progress |
| A3S Code CLI/TUI/Web integration | Reviewed Runtime Task install, offline restart disable/re-enable, apply-time build drift rejection, watcher hot-plug, Web marketplace lifecycle, generation-bound Activity documents, and TUI `/packages` review are tested; release qualification remains |
| Verified preview installers and release evidence | Linux/macOS and Windows installers enforce HTTPS, exact tag-identity Sigstore verification, release checksums, safe extraction, packaged OCR/Skill binding, versioned atomic activation, complete-tree reinstall validation, retained local evidence, and managed command ownership. Deterministic archive serialization, per-platform SPDX SBOMs, GitHub OIDC provenance/SBOM attestations, pinned Actions/tools, and cache-free clean-runner byte comparison for every shipped native executable are implemented; an externally operated full-archive witness and off-Release evidence retention remain open |
| Complete Linux/macOS/Windows real-process E2E and recovery matrix | Release blocker |
| Public Registry operations, external full-archive reproducibility witness, off-Release evidence retention, support runbooks | Release blocker |

**Production-ready: no.** The code has a substantial tested foundation, but
the unfinished rows above remain required release gates. [ROADMAP.md](ROADMAP.md)
tracks the remaining product work without converting completed internals into a
release claim.

## Platform support

| Target | Current gate | Product status |
| --- | --- | --- |
| Linux x86_64 / arm64 | Full non-Science workspace CI plus release-container conformance | Development preview |
| macOS arm64 / x86_64 | Current non-Science workspace build and tests | Development preview |
| Windows x86_64 | Current non-Science workspace tests, junction rejection, signed Registry/graph/Grant/Flow/OKF CLI lifecycles, and killed-process cutover replay | Preview; full runtime/recovery matrix pending |

Trusted package and state paths use platform-aware metadata checks that reject
Unix symbolic links and Windows reparse points before traversal. Platform test
coverage is not the same as production qualification.

## Repository layout

```text
Use/
├── crates/core/             canonical contracts, resolver, plans, grants
├── crates/extension/        ACL packages, TUF Registry, receipts, package store
├── crates/science/          real cognitive-package fixture and tooling
├── src/cognitive_package/   reviewed package-graph application service
├── src/plugin_lifecycle/    durable six-surface lifecycle and host boundaries
├── src/plugin_runtime/      Runtime provider selection and exact bindings
├── src/okf_knowledge/       standalone OKF Knowledge backend
├── website/                 GitHub Pages documentation site
└── docs/                    architecture, contracts, ADRs, and release design
```

## Development

Run checks from this repository, not from the A3S monorepo root:

```bash
cargo fmt --all -- --check
cargo test --workspace --exclude a3s-use-science --all-targets
cargo clippy --workspace --exclude a3s-use-science --all-targets --all-features -- -D warnings
cargo check -p a3s-use --no-default-features
cargo check -p a3s-use --no-default-features --features extensions
```

Build and validate the documentation site:

```bash
cd website
npm ci
npm run format:check
npm run lint
npm run build
npm run check:site
```

Contribution rules are documented in [AGENTS.md](AGENTS.md). Public Rust types
should remain typed and `Send + Sync` where applicable; I/O uses Tokio; ACL is
the default human-authored configuration format.

## Documentation

- [Product roadmap](ROADMAP.md)
- [Plugin contract reference](docs/plugin-contracts.md)
- [Plugin platform architecture](docs/plugin-platform-architecture.md)
- [Lifecycle and security](docs/plugin-platform-lifecycle-and-security.md)
- [Development plan](docs/plugin-platform-development-plan.md)
- [Verified release installation](docs/release-installation.md)
- [Release descriptors](docs/release-descriptors.md)
- [OKF Knowledge operations](docs/okf-knowledge-operations.md)
- [Registry cache operations](docs/registry-cache-operations.md)
- [Documentation website](https://a3s-lab.github.io/Use/)

## License

Apache-2.0. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
