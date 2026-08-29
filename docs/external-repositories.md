# External Repository Cognitive Packages

Status: development preview
Last updated: 2026-08-07

## Boundary

An external repository may build and publish a versioned cognitive package
without becoming a source-code dependency of A3S Use. Use installs immutable
package artifacts; it does not clone a repository, execute its build scripts,
resolve a branch, or infer capabilities from source files.

The current package line is manifest schema 3. A package has one canonical
`<publisher>/<name>` identity, one SemVer version, one required ACL manifest,
one required bounded UTF-8 `README.md`, optional package dependencies, and one
or more named Tool, MCP, OKF, Flow, Skill, or UI surfaces.

Superseded preview manifests and receipts are not supported. If unsupported
state is present, Use fails closed and requires cleanup followed by reinstall.

## Package shape

```text
calendar-package/
├── a3s-use-extension.acl
├── README.md
├── tools/
├── releases/
├── okf/
├── flows/
├── skills/
└── ui/
```

Only the manifest and `README.md` names are fixed. Every contribution path is
manifest-owned, package-relative, normalized, and content-bound. Use rejects
path traversal, links, duplicate normalized paths, archive ambiguity, size
overflows, content drift, route collisions, and incompatible host or target
ranges before publication.

The source repository URL and immutable revision are provenance. They are not
instructions to fetch or execute source code.

## Dependencies and versions

A package dependency declares only a canonical package ID and SemVer
requirement. Package content cannot choose a Registry, URL, trust root,
channel, target, mirror, or mutable tag.

The host resolves the complete transitive closure from its enabled named
Registries. Resolution is bounded and rejects cycles, missing releases,
incompatible constraints, target/provider incompatibility, and ambiguous
equal-priority candidates. The resulting package lock freezes every selected
catalog-v3 record, dependency edge, artifact digest, Registry identity, and
TUF metadata version.

Dependencies prepare before dependents. Shared exact dependencies may be
retained. Unused packages retire in reverse order after one graph cutover.

## Six native surfaces

| Surface | Package contribution | Host owner |
| --- | --- | --- |
| Tool | Finite CLI Task or private HTTP Service | Runtime provider |
| MCP | Standard stdio or Streamable HTTP server | stdio supervisor or Runtime/Gateway |
| OKF | Content-bound OKF v0.2 concept graph | Knowledge host |
| Flow | `a3s-flow` Native TypeScript source/export | Flow host |
| Skill | Content-bound `SKILL.md` and supporting files | Skill projection host |
| UI | Integrity-bound static entry and declared bindings | Product UI host |

Tool is the native program or Service, not an MCP `tools/list` item. MCP keeps
the standard MCP protocol. Flow uses `a3s-flow`; `flow.json` does not create a
second package or lifecycle. OKF is Knowledge data, not an executable.

Each required surface publishes only when its exact dependency evidence is
ready for the same package generation. Missing host ownership is an error, not
permission to substitute a native runner, source-only binding, or metadata
fallback.

## Registry and trust

Remote package discovery uses host-selected, replaceable Registry
configuration and TUF verification. A signed target must carry one complete
catalog-v3 record in `custom.a3s`. That evidence binds package identity,
version, target, archive and expanded-package digests, surfaces, dependencies,
permission ceiling, and planning target.

Registry replacement changes future resolution input; it never rewrites an
installed receipt. Upgrade remains bound to the receipt's exact provenance.
If that source cannot be restored, the package must be removed and installed
again from the newly trusted source.

Local unsigned packages are development input only and require explicit human
trust. They do not establish a production distribution path.

## Reviewed lifecycle

The lifecycle is plan, confirm, then apply:

```text
verify catalog and provenance
→ resolve the exact SemVer graph
→ freeze the lock and immutable plan
→ confirm the operation ID and plan digest
→ prepare dependencies and all required surfaces
→ publish one Registry snapshot
→ drain prior-generation calls and grants
→ remove receipt-owned prior state in reverse order
```

Install, upgrade, uninstall, enable, and disable use the same plan-v4 and apply
boundary. There is no direct enable/disable mutation tool. Crash recovery
resumes exact durable evidence; deleted journals or the installation snapshot
are corruption and are never reconstructed heuristically.

The complete contract inventory is in [Plugin Contracts](plugin-contracts.md),
and lifecycle ownership is in [Plugin Platform Architecture](plugin-platform-architecture.md).
