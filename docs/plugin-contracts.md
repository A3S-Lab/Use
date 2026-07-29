# A3S Use Plugin Contract Reference

- Status: M0 complete
- Baseline date: 2026-07-30
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Lifecycle: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Delivery: [Plugin Platform Development Plan](plugin-platform-development-plan.md)

This document records the machine-readable plugin contracts implemented in
`a3s-use-core` and the signed catalog reader implemented in
`a3s-use-extension`. It freezes the control-plane vocabulary before shared
lifecycle mutation is implemented. It does not claim that the Plugin Manager,
surface reconciler, or Runtime providers are complete.

## Contract Set

| Contract | Schema | Purpose |
| --- | --- | --- |
| Plugin manifest | `a3s.extension/v3` | Named Skill, MCP, Tool, and UI surfaces |
| Tool release | `a3s.use.tool-release.v1` | Immutable CLI Task or HTTP Service workload |
| Permission ceiling | `a3s.use.plugin-permissions.v1` | Maximum authority per executable/UI surface |
| Catalog record | `a3s.use.plugin-catalog.v1` | Search and review metadata without package download |
| Operation plan | `a3s.use.plugin-operation-plan.v1` | Exact install, upgrade, or uninstall delta |
| Manager toolset | `a3s.use.plugin-manager-tools.v1` | Bounded MCP management interface |

All JSON contracts:

- reject unknown fields;
- enforce bounded input and collection sizes;
- use OLPC canonical JSON;
- expose a `sha256:` descriptor digest; and
- avoid secret values, executable paths, public service endpoints, or generic
  action payloads.

## Catalog and Trust Provenance

`PluginCatalogRecord` contains the searchable signed target content:

- package identity, version, channel, target, compatibility, and availability;
- named surface metadata, including Tool workload and MCP transport;
- the complete permission ceiling and its digest;
- archive target, compressed length, archive digest, expanded size, file
  count, and optional expanded-package digest; and
- publisher, license, and canonical repository.

Registry identity and TUF role versions are intentionally outside the signed
target record. `VerifiedPluginCatalogRecord` pairs a record with
`VerifiedCatalogProvenance` and verifies that the outer provenance binds the
canonical record digest. Search and inspect responses must preserve that pair;
displaying a bare record as verified is invalid.

Search operates on bounded verified metadata. It does not download or activate
the package payload.

The extension library exposes this contract through:

- `PluginCatalogHost`, a manager-owned target and A3S Use compatibility
  context;
- `PluginCatalogSearch`, with a 256-byte query, exact filters, a maximum
  50-record page, and a snapshot/query-bound cursor;
- `PluginCatalogPage`, which carries the verified snapshot, total match count,
  full verified records, and the next cursor; and
- `PluginCatalogInspection`, which selects the newest compatible release unless
  an exact version or channel is requested.

`search_remote_plugins` and `inspect_remote_plugin` perform a bounded online
TUF refresh. `search_cached_plugins` and `inspect_cached_plugin` are separate
filesystem-only operations, so offline intent cannot silently fall back to the
network. An online refresh retains the exact verified root, timestamp,
snapshot, and targets bytes plus their digests and role versions. An offline
read verifies that checkpoint, re-runs TUF signature and expiration checks,
and reports the elapsed seconds since the online verification.

Search and inspection enforce compatibility before returning an installable
record. Target `any` is used only when no exact host target exists for the same
package, version, and channel. A catalog archive path, length, or SHA-256 that
differs from its enclosing TUF target is invalid even when both structures are
individually signed.

An empty text query is the bounded catalog-browse operation used by
Marketplace adapters. It keeps the same filters, deterministic ordering,
snapshot-bound cursor, 50-record page limit, and one-MiB serialized response
limit as a non-empty search.

`ResolvedRemotePackage::from_verified_catalog` adapts a returned complete
record into the exact metadata-only target proof consumed by the existing
umbrella planner and installer. The adapter revalidates current-host target
compatibility and does not download the archive.

Legacy `custom.a3s` schema v1 targets remain readable by installation and
`list_remote_packages`, but they are not promoted into verified plugin search
results because they lack the review metadata required by this contract.

## Permission Ceiling

Permissions are declared per qualified surface. Skill surfaces cannot carry
runtime permissions because Skill content is guidance, not authority.

Executable Tool and MCP surfaces declare:

- native execution and child-process authority;
- scope-relative filesystem roots;
- exact egress hosts and sorted nonzero ports;
- private Service authority;
- secret names, never secret values; and
- CPU, memory, process, ephemeral-storage, timeout, and captured-output
  ceilings.

Tool Task permissions require bounded timeout, stdout, and stderr values.
Tool Service and Streamable HTTP MCP permissions require private, non-native,
long-running resources. Stdio MCP requires explicit native execution.

UI surfaces have no ambient execution, filesystem, network, secret, or
resource authority. A UI can declare only method/path bindings to a Tool
Service in the same package.

## Selective Installation

A package can contain several named surfaces, while
`plugin_plan_install.surfaces` and `plugin_plan_upgrade.surfaces` select the
requested subset. Resolution adds:

1. every non-optional surface required by the package contract;
2. the transitive dependency closure of the selected surfaces; and
3. no unrelated optional surface or package.

This is the mechanism used to avoid installing the entire Science catalog.
Science should publish independently useful packages and mark genuinely
optional surfaces explicitly. Surface selection is not permission selection:
the resolved permission ceiling for every selected executable surface remains
mandatory and cannot be narrowed by untrusted package content.

## Immutable Operation Plan

`PluginOperationPlan` binds one complete resolution result:

- operation identity, action, actor, policy decision, scope, and expiry;
- root plus dependency package transitions, sorted by package ID;
- exact before/after releases and full permission ceilings;
- exact archive or local/bundle source evidence;
- the complete per-surface add/remove/replace set and descriptor digests;
- the derived secret grant/revoke delta;
- one compatible Runtime provider proof per resulting Tool or MCP surface;
- workspace enablement and grant impact;
- download, installed, reclaimed, drain, and retained-data impact; and
- durable state revision, capability generation, and prior receipt digest.

The plan validator derives surface and secret deltas from the embedded package
states and rejects omissions or additions. It also rejects:

- a root transition that differs from the requested operation;
- a permission ceiling that differs from the release digest;
- a Provider whose enforcement profile cannot satisfy the permission ceiling;
- unattended Agent use of unconfined native execution;
- unattended or policy-allowed installation from an unsigned local source;
- stale receipt or capability evidence; and
- noncanonical, expired, or digest-mismatched apply requests.

Apply accepts only `operationId` and `planDigest`. The manager must load the
stored immutable plan, re-resolve external state, compare every bound field,
persist durable intent, and then begin side effects. A changed result requires
a new plan and review.

## Manager MCP Toolset

The frozen management inventory is:

| Tool | Read only | Destructive | Idempotent | Open world |
| --- | --- | --- | --- | --- |
| `plugin_search` | yes | no | yes | yes |
| `plugin_inspect` | yes | no | yes | yes |
| `plugin_list_installed` | yes | no | yes | no |
| `plugin_status` | yes | no | yes | no |
| `plugin_plan_install` | yes | no | no | yes |
| `plugin_plan_upgrade` | yes | no | no | yes |
| `plugin_plan_uninstall` | yes | no | no | no |
| `plugin_apply_plan` | no | yes | yes | yes |
| `plugin_enable` | no | no | yes | no |
| `plugin_disable` | no | no | yes | no |

Plan tools are read-only with respect to installed capabilities but are not
idempotent because each plan has a new operation ID and validity interval.
`plugin_apply_plan` is idempotent by operation ID and plan digest; replay
returns the durable result instead of repeating effects.

Inputs contain only bounded query text, IDs, version/channel constraints,
surface selectors, cursors, limits, scopes, and the apply digest. They cannot
provide a registry URL, package path, command, provider, executable, endpoint,
or secret. There is no `plugin_execute`: activated Skills use separately
authorized native Tool or MCP bindings in the data plane.

## Golden Fixtures

Canonical interoperability fixtures live under
`crates/core/fixtures/plugins/`:

- `permission-ceiling-v1.json`;
- `catalog-record-v1.json`;
- `complete-package-catalog-v1.json`;
- `operation-plan-install-v1.json`; and
- `manager-toolset-v1.json`.

Each fixture has a sibling `.sha256` file. Tests require byte-for-byte
canonical form, stable descriptor digests, fail-closed unknown fields, and
cross-contract binding.

The complete installable package lives under
`crates/extension/fixtures/packages/plugin-v3/`. It contains all four surface
kinds and both Tool/MCP workload variants. Its expanded directory and
deterministic `tar.gz` reconstruction have fixed file-count, byte-count, and
SHA-256 evidence. Tests extract the archive through the real package source
validator and revalidate every referenced surface file.

The matching deterministic TUF repository lives under
`crates/extension/fixtures/registry/plugin-v3/`. Its signed targets metadata
embeds `complete-package-catalog-v1.json`; root, targets, snapshot, timestamp,
archive, catalog, and expanded package digests are checked as one chain. The
fixture key is intentionally public test material and must never be trusted by
a deployed registry.

## Evolution Rules

- A schema version never changes meaning after release.
- New optional descriptive fields require a new schema if current parsers use
  `deny_unknown_fields`.
- New privilege, source, provider, or lifecycle fields always require a new
  schema and explicit migration.
- Existing manifest v1/v2 packages remain readable through compatibility
  parsing; only v3 packages can declare the named multi-surface model.
- A manager may support several schema versions internally, but one plan uses
  exactly one version of every embedded contract.
