# A3S Use Plugin Contract Reference

Status: development preview
Last updated: 2026-08-09

## Scope

This document describes the one cognitive-package contract line accepted by
the current A3S Use implementation. It is not a compatibility matrix.

The cognitive-package product has not shipped a supported release. Superseded
pre-release manifests, catalogs, receipts, plans, host inventories, manager
toolsets, and disk records are rejected. Negative test fixtures exist only to
prove rejection. Unsupported disk state must be removed and packages reinstalled.

Schema identifiers ending in `v1` below are not automatically “legacy.” They
are the current version of that specific contract when they appear in the
current set.

## Current contract set

| Contract | Current identifier |
| --- | --- |
| Package manifest | ACL `schema_version = 3` |
| Catalog record | `a3s.use.plugin-catalog.v3` |
| Package lock | `a3s.use.plugin-package-lock.v1` |
| Planning bundle | `a3s.use.plugin-planning-bundle.v1` |
| Operation plan draft | `a3s.use.plugin-operation-plan-draft.v3` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Operation confirmation | `a3s.use.plugin-operation-confirmation.v1` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v4` |
| Host protocol level | `4` |
| Host plan request/result | `a3s.use.plugin-host-plan-request/result.v1` |
| Host enablement plan request/result | `a3s.use.plugin-host-enablement-plan-request/result.v1` |
| Host apply request/result | `a3s.use.plugin-host-apply-request/result.v1` |
| Host observation request/result | `a3s.use.plugin-host-observation-request/result.v1` |
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v4` |
| Installed receipt | numeric schema version `3` |
| Installed package graph | `a3s.use.installed-package-graph.v1` |
| Pending package graph | `a3s.use.pending-package-graph-operation.v2` |
| Lifecycle intent/operation | `a3s.use.plugin-lifecycle-intent/operation.v2` |
| Lifecycle diagnostic | `a3s.use.plugin-lifecycle-diagnostic.v1` |
| Enablement request/result | `a3s.use.cognitive-package-enablement-request/result.v1` |
| Enablement plan result | `a3s.use.cognitive-package-enablement-plan-result.v1` |
| Enablement state/operation | `a3s.use.cognitive-package-enablement-state/operation.v2` |
| Workspace Grant | `a3s.use.plugin-workspace-grant.v1` |
| Registry cutover | `a3s.use.registry-cutover.v1` |
| OKF Knowledge binding | `a3s.use.okf-knowledge-binding.v2` |
| OKF Knowledge backup | `a3s.use.okf-knowledge-backup.v1` |

The host capability inventory is exact: catalog v3, plan v4, and manager
toolset v4 only. Toolset v4 exposes optional canonical `registryName` only on
install planning; upgrade stays bound to installed Registry provenance. A host
advertising a different inventory is rejected.

## Package manifest

The package manifest is `a3s-use-extension.acl`. It is parsed with `a3s-acl`;
ACL is not HCL.

Required package-level fields:

- canonical `<publisher>/<name>` package ID;
- `schema_version = 3`;
- SemVer `version`;
- canonical route;
- `requires_use` that includes the current 0.3 host line and excludes pre-0.3
  hosts;
- sorted risk actions;
- repository URL plus immutable revision; and
- at least one named Tool, MCP, OKF, Flow, Skill, or UI surface.

The package root also requires a bounded, regular UTF-8 `README.md`.

Package dependencies are named `dependency` blocks containing only a canonical
package ID and SemVer requirement. A dependency cannot select a URL, Registry,
trust root, target, or mutable tag. Dependencies are sorted, unique, non-self,
bounded, and resolved as one acyclic graph.

### Surface contracts

| Surface | Required identity and dependency behavior |
| --- | --- |
| Tool | Named Task or Service; explicit interface, activation, timeout, and immutable executable/release evidence |
| MCP | Named standard MCP server; stdio or HTTP/release evidence; no A3S-specific RPC envelope |
| OKF | Named OKF v0.2 bundle; exact content digest, counts, byte limits, and Knowledge-host ownership |
| Flow | Named `a3s-flow` source/export; explicit Tool/MCP/OKF dependencies |
| Skill | Named `SKILL.md`; explicit Tool/MCP/OKF/Flow dependencies |
| UI | Named integrity-bound entry; explicit Skill/Tool/MCP/Flow bindings |

All package paths are relative, normalized, and package-owned. Absolute paths,
parent traversal, symlinks, hard-link ambiguity, archive link entries, duplicate
normalized paths, and size/count overflows fail before activation.

The surface dependency graph must be acyclic. Required surfaces publish only
when all required dependencies have exact ready evidence for the same package
generation. An optional surface failure may produce a degraded package but
cannot satisfy another required surface.

## OKF contract

Only OKF format `0.2` is accepted by the cognitive-package manifest and bundle
inspector. There is no 0.1 fallback.

Use verifies package ownership, byte/count bounds, content digest, concept
paths, frontmatter, and links. A Knowledge host owns stage, promote, observe,
search, drain, and receipt-owned removal. Staged content is not live evidence.
Only an exact promoted binding may enter the capability snapshot.

The standalone SQLite/FTS5 backend creates the one current database schema for
new state and rejects every unknown `user_version`. It does not migrate or
rewrite pre-release databases.

One scope-local backup file starts with the fixed A3S OKF backup header, a
bounded manifest length and SHA-256, the versioned JSON manifest, and the exact
compact SQLite snapshot bytes.
The manifest binds complete scope, creation time, database length and SHA-256,
storage accounting, and policy limits. Verification rejects unknown fields,
scope substitution, symlinks, length/digest mismatch, unsupported SQLite
schema, invalid receipts, inconsistent accounting, and FTS corruption.

The backup digest detects corruption but is not a Registry signature. A
verified database cannot recreate package receipts, immutable package roots,
Knowledge bindings, lifecycle journals, or Grants. Restore remains unsupported
until those authorities can be validated together.

Search-index repair is deliberately narrower than lifecycle repair. It first
validates core SQLite integrity, foreign keys, projection receipts, row
identity, scope, and accounting, then derives FTS rows from the retained
document table and repeats the complete audit. It never edits lifecycle
authority.

## Flow contract

The only engine identity is `a3s-flow`. A Flow surface binds:

- package ID, surface ID, and immutable package generation;
- `native-ts` source and export;
- exact source digest;
- declared Tool, MCP, and OKF edges; and
- host preflight/compiled-binding evidence.

`flow.json` is a host-facing visual design or deployment document. It does not
define another package format, lock, receipt, or lifecycle. Local Code and
remote OS placement must consume the same package-owned Flow identity.

Missing Flow ownership fails before publication. Source presence is not a
readiness fallback.

## Catalog and TUF provenance

Every signed Registry target carries one complete catalog-v3 record in
`custom.a3s`. The record contains:

- package identity, version, channel, target, display/search metadata, license,
  and repository identity;
- complete six-surface inventory and dependency edges;
- package dependencies and `requires_use`;
- archive target, length, and digest;
- expanded package bounds and exact package/manifest digests;
- planning target identity and digest;
- provider requirements; and
- the complete permission ceiling plus digest.

TUF verification binds Registry name and URL, root digest/version, timestamp,
snapshot, targets version, target path, length, digest, and catalog-record
digest. A partial `custom.a3s` record is invalid; there is no metadata fallback.

Prepared and downloaded remote packages always carry the verified catalog
record. A Registry/TUF receipt is invalid unless both resolved source
provenance and the complete verified catalog evidence are present and agree
with package ID and version.

Registry endpoints are replaceable host configuration. Replacement never
rewrites receipt provenance. The exact source must be restored or the package
must be reinstalled before an upgrade can proceed.

## Package resolution and lock

Resolution is bounded and deterministic. Candidate selection applies:

1. enabled source set;
2. exact package identity;
3. requested release channel/version;
4. host target and `requires_use`;
5. provider capabilities; and
6. all accumulated SemVer constraints.

The resolver rejects cycles, missing releases, incompatible constraints,
source ambiguity, duplicate equal-priority candidates, and configured search
limits.

The package lock freezes each selected catalog record and its provenance.
Install order is dependency-forward; removal order is its exact reverse.
Retained packages are accepted only when their installed receipt and verified
catalog evidence match the locked node exactly.

Upgrade always carries two locks:

- the **prior lock**, which proves the installed dependency graph and reverse
  retirement order; and
- the **candidate lock**, which proves the graph to publish.

The candidate lock alone cannot authorize retirement.

## Immutable operation plan

Plan v4 is used for install, upgrade, uninstall, enable, and disable. A plan
contains:

- operation ID, action, actor, authority, scope, expiry, and policy decision;
- exact host capability evidence;
- exact prior/candidate package-lock bindings where applicable;
- sorted Add/Replace/Remove/Retain package transitions;
- selected surface closure and before/after state evidence;
- provider, permission, OKF, Flow, byte, download, and process impact;
- immutable current-state revision; and
- canonical plan digest.

Apply accepts the operation ID, plan digest, and exact confirmation. It
re-derives plan semantics and rejects any catalog, lock, scope, policy,
provider, receipt, generation, or state drift before mutation.

Enablement planning returns either:

- `Planned`, with the exact plan-v4 envelope; or
- `NoChange`, a terminal read-only result without a synthetic mutation plan.

Completed operation reads return the durable terminal result. Recovery resumes
the exact stored plan and authorization. It never generates a replacement plan
or asks a child operation to invent authority.

## Host protocol

Host capabilities v4 advertise protocol level 4 and exact current schema
inventories. Managed hosts expose separate methods for:

1. capability discovery;
2. catalog/search inspection;
3. install/upgrade/uninstall planning;
4. enable/disable planning;
5. exact reviewed apply; and
6. observation/watch.

The managed scope includes both kind (`user` or `workspace`) and ID. Equal
textual IDs in different kinds cannot alias storage, authorization, plans, or
observations.

The manager toolset contains exactly:

```text
plugin_search
plugin_inspect
plugin_list_installed
plugin_status
plugin_plan_install
plugin_plan_upgrade
plugin_plan_uninstall
plugin_apply_plan
plugin_plan_enable
plugin_plan_disable
```

There is no `plugin_enable` or `plugin_disable` mutation tool. Enablement uses
plan then `plugin_apply_plan` like every other mutation.

## Workspace Grants

The permission ceiling is signed catalog input. A reviewed Workspace Grant is
a policy-approved subset bound to scope, package, surfaces, immutable package
generation, operation identity, and expiry.

Planning snapshots existing grants and emits canonical proposed and resolved
changes. Apply re-derives the set and rejects scope, revision, authority,
confirmation, ceiling, or resolution drift.

The graph saga orders Grant changes as follows:

```text
persist candidate grants
→ prepare package candidates
→ publish exact Registry snapshot
→ checkpoint Grant cutover
→ drain calls admitted by the prior generation
→ revoke exact prior grants
→ remove prior package generations
```

A failure before publication rolls package and Grant candidates back together.
After publication, recovery completes retirement; it does not restore the old
visible graph.

## Lifecycle and storage

The lifecycle intent and journal contain the complete package generation,
scope, action, surface schedule, and idempotency keys. Host traits must return
canonical SHA-256 evidence for each checkpoint.

Capability visibility methods always return exact immutable snapshot cutover
evidence. Host traits have no fallback publication API. A separate retirement
method may mark a prior receipt hidden only after the exact Registry route is
already absent; otherwise it fails before mutation.

Current disk records are bounded, canonical, path-owned, and atomically
replaced. Loading rejects:

- unknown schema/version;
- unknown fields;
- missing catalog or provenance evidence;
- path traversal, symlink, or ownership drift;
- changed package/manifest digest;
- generation or scope mismatch; and
- a missing exact journal required for recovery.

Deletion of recovery evidence is corruption, not permission to infer state.

The lifecycle diagnostic is a read-only JSON projection, not a mutable disk
record or recovery input. It reports latest/previous operation identity,
action, status, generation, digests, checkpoint progress, bounded failure
codes, timings, and rollback evidence. It omits checkpoint idempotency keys,
credentials, tokens, secret values, package-authored error text, and package
content. Consumers must not treat the projection as authority to recreate a
missing journal.

## Canonicalization and limits

Machine-owned JSON uses deterministic canonical serialization before digesting.
Collections that represent sets are sorted and unique. Unknown fields fail
closed. Contract, plan, package, dependency, Registry, target, and journal
sizes are bounded by checked constants.

Human-authored configuration remains ACL. JSON is used for signed/canonical
machine records, receipts, locks, plans, backup manifests, and command output.

## Golden fixtures

Current canonical fixtures live under:

- `crates/core/fixtures/plugins/`;
- `crates/extension/fixtures/manifests/`;
- `crates/extension/fixtures/packages/`; and
- `crates/extension/fixtures/registry/plugin-v3/`.

Each canonical JSON fixture has an adjacent SHA-256 golden where applicable.
Tests verify round trips, unknown-field rejection, digest stability, signed
catalog/manifest binding, current host inventory, operation confirmation, and
fail-closed rejection of superseded inputs.

## Evolution rule before first release

Until the first supported product release:

1. change the current schema when the design requires it;
2. update all producers, consumers, fixtures, tests, and documentation in the
   same change;
3. remove the superseded decode/API/storage path;
4. reject stale state with a cleanup and reinstall instruction; and
5. retain only negative fixtures needed to prove rejection.

After the first supported release, compatibility and migration policy must be
an explicit product decision. That future policy must not be implemented
preemptively in the current preview.
