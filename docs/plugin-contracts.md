# A3S Use Plugin Contract Reference

- Status: M0 complete
- Baseline date: 2026-07-30
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Lifecycle: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Delivery: [Plugin Platform Development Plan](plugin-platform-development-plan.md)

This document records the machine-readable plugin contracts implemented in
`a3s-use-core`, plus the signed catalog reader and durable workspace-grant
store implemented in `a3s-use-extension`. It freezes the control-plane
vocabulary before shared lifecycle mutation is implemented. It does not claim
that the Plugin Manager, surface reconciler, or Runtime providers are complete.

## Contract Set

| Contract | Schema | Purpose |
| --- | --- | --- |
| Plugin manifest | `a3s.extension/v3` | Named Skill, MCP, Tool, and UI surfaces |
| Tool release | `a3s.use.tool-release.v1` | Immutable CLI Task or HTTP Service workload |
| Permission ceiling | `a3s.use.plugin-permissions.v1` | Maximum authority per executable/UI surface |
| Workspace grant proposal | `a3s.use.plugin-workspace-grant-proposal.v1` | Reviewable pre-confirmation resolved authority |
| Grant confirmation | `a3s.use.plugin-grant-confirmation.v1` | User evidence binding an exact plan and proposal |
| Operation confirmation | `a3s.use.plugin-operation-confirmation.v1` | User evidence binding every `ask` apply to one plan |
| Workspace grant snapshot | `a3s.use.plugin-workspace-grant-snapshot.v1` | Revisioned active-grant evidence before mutation |
| Workspace grant changes | `a3s.use.plugin-workspace-grant-changes.v1` | Sorted root/dependency grant and revoke transition set |
| Workspace grant operation | `a3s.use.plugin-workspace-grant-operation.v1` | Durable immutable intent and resumable grant lifecycle phase |
| Workspace grant cutover | `a3s.use.plugin-workspace-grant-cutover.v1` | Evidence that capability publication selected the prepared generation |
| Workspace grant | `a3s.use.plugin-workspace-grant.v1` | Scope-bound resolved authority within a signed ceiling |
| Catalog record | `a3s.use.plugin-catalog.v1` | Compatible search and review metadata without package download |
| Catalog record | `a3s.use.plugin-catalog.v2` | Plan-ready manifest evidence and surface dependency closure |
| Operation plan draft | `a3s.use.plugin-operation-plan-draft.v1` | Untrusted planner evidence before host identity and authority |
| Operation plan | `a3s.use.plugin-operation-plan.v1` | Exact install, upgrade, or uninstall delta |
| Manager toolset | `a3s.use.plugin-manager-tools.v1` | Bounded MCP management interface |

All JSON contracts:

- reject unknown fields;
- enforce bounded input and collection sizes; and
- avoid secret values, executable paths, public service endpoints, or generic
  action payloads.

Immutable review and receipt contracts use OLPC canonical JSON and expose a
`sha256:` descriptor digest. The planner draft is deliberately neither
authorized nor independently digest-authoritative; the host binds it into the
canonical operation plan before review.

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

## Workspace Grant

`PluginWorkspaceGrant` binds a canonical resolved permission set to one
workspace, package ID and digest, signed permission-ceiling digest, policy
digest, actor, confirmation decision, grant time, and optional expiry.
`PluginPermissionCeiling::is_within` independently verifies that the resolved
set only narrows the signed ceiling.

Filesystem scope/path/access, exact network hosts and ports, resources,
boolean execution/Service authorities, secret names, and UI methods/path
prefixes are compared structurally. Secret-bearing grants are valid only for a
user-confirmed `ask` decision; an agent grant cannot contain secret authority.
The contract stores secret names but never values.

### Proposal and confirmation

Grant planning is intentionally two phase. A
`PluginWorkspaceGrantProposal` contains the operation ID, scope, package ID and
digest, signed ceiling digest, canonical resolved permissions, policy
authority, proposal lifetime, and optional eventual grant expiry. It contains
no confirmation claim. It is independently checked against the signed ceiling
and has canonical JSON plus a cross-SDK SHA-256 golden.

For `allow`, apply finalizes the proposal without confirmation at the trusted
apply time. For `ask`, `PluginGrantConfirmation` must be created by the user
confirmation boundary after review. It binds the operation ID, canonical plan
digest, proposal digest, user actor, and confirmation time. Finalization
rejects a different plan, proposal, operation, actor, future time, or expired
review window, then places only the confirmation-record digest in the final
grant.

This ordering avoids a digest cycle: the plan can bind a proposal before a
user decision exists, while the later confirmation binds both immutable
objects. Untrusted package, Skill, Tool, MCP, or UI content cannot act as
confirmation evidence.

### Snapshot and multi-package changes

`PluginWorkspaceGrantSnapshot` is the canonical before-state for one scope and
durable state revision. Its sorted evidence entries bind package ID and digest,
grant receipt revision, and canonical grant digest. Evidence cannot claim a
revision newer than the enclosing durable state.

`PluginWorkspaceGrantChangeSet` binds an operation, scope, state revision,
optional before-snapshot digest, and sorted package changes. A change carries
exact prior evidence, a reviewed after proposal, or both. Against an immutable
operation plan, the resolver:

1. requires `grantBeforeDigest` to equal the snapshot digest;
2. requires `grantAfterDigest` to equal the change-set digest;
3. derives required entries from every permission-bearing root and dependency
   Add, Replace, or Remove transition and workspace enablement state;
4. rechecks proposal package generation, ceiling, authority, and lifetime;
5. rejects missing, extra, reordered, stale, or substituted evidence; and
6. resolves candidate grants separately from exact delayed revocations.

The plan-level `PluginOperationConfirmation` covers every `ask` mutation,
including revoke-only uninstall. Proposal confirmations additionally bind each
new authority proposal to that same plan and confirmation event. `allow`
accepts neither form of unrelated confirmation.

### Durable grant state

`WorkspaceGrantReceipt` stores a monotonic revision, the canonical grant, and
its verified digest under schema
`a3s.use.plugin-workspace-grant-receipt.v1`.
`WorkspaceGrantRevocation` is a durable tombstone under schema
`a3s.use.plugin-workspace-grant-revocation.v1`; it binds the exact prior
revision and grant digest, package generation, policy authority, and revocation
time.

The storage key is workspace scope, package ID, and immutable package digest.
This is deliberate: N and candidate N+1 authorization can coexist while an
upgrade prepares and health-checks N+1. The capability snapshot remains the
visibility boundary. Once the snapshot switches and old leases drain, N is
revoked without affecting N+1.

Planning obtains `PluginWorkspaceGrantSnapshot` from a locked traversal of this
store, not from package-declared metadata. The traversal validates the hashed
scope root and every publisher/package/generation path, bounds all directory
and record counts, and checks grant and tombstone revisions against the
requested global state revision. Granted receipts become sorted exact
evidence; tombstones remain revision evidence but do not become active grants.
Two granted generations for one package make the scope unstable and block new
planning until lifecycle recovery completes the interrupted cutover.
Abandoned `.grant-*.tmp` files are non-authoritative and ignored.

Before grant side effects, `WorkspaceGrantOperationJournal` stores an immutable
intent under
`<state-root>/grants/.operations/<operation-sha256>.json`. It binds:

- operation, plan, and grant-change-set digests;
- planned and locked-observed before-snapshot digests;
- prior/next global state revision and capability generation;
- exact candidate receipts, proposal digests, and signed ceilings; and
- exact prior receipts plus revocation authority.

The phase sequence is `intent-recorded`, `preparing`, `prepared`,
`cutover-committed`, `retiring`, and `completed`. Every journal replacement is
bounded, atomic, symlink-checked, and serialized with grant records under the
same cross-process lock. `prepared` is reached only after all candidate writes
converge. `WorkspaceGrantCutoverEvidence` must bind the expected generation
transition, an immutable capability-snapshot digest, and a non-future commit
time. Retirement cannot begin without it. A retry reuses the same immutable
receipts and cutover time, so a crash between record and checkpoint writes
converges instead of inventing new evidence.

An observed record is evidence, not executable authority. Callers must use the
active resolver, which rechecks the path identity, exact package digest,
current signed permission ceiling, grant subset, and lifetime. A missing or
revoked record resolves to no authority; malformed, moved, expired, stale, or
ceiling-mismatched evidence fails closed.

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

Catalog v2 adds the signed manifest digest required by a complete immutable
plan and a sorted `requires` list on each surface. Tool and MCP surfaces cannot
delegate further authority. Skills may require Tool or MCP surfaces; UIs may
require Skill, Tool, or MCP surfaces. Missing, duplicate, kind-invalid, and
cyclic edges fail closed. Catalog v1 remains readable and retains its exact
canonical digest, but cannot carry these v2-only fields and is not sufficient
by itself for complete-plan emission.

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

## Host Authorization Policy

The umbrella CLI owns the strict `a3s.plugin-policy.v1` ACL contract. The
normalized policy has a stable digest and bounds agent install, upgrade, and
uninstall decisions by exact registry and publisher lists, source kind,
download and installed bytes, package and surface counts, scope/workspace
identities, filesystem access, network host/port pairs, Runtime resources,
native and child execution, private Services, secret names, and UI HTTP
bindings.

Evaluation consumes the complete immutable `PluginOperationPlan`; it never
uses catalog display text, Skill instructions, Tool output, MCP descriptions,
UI messages, or API documentation as authority. A configured `allow` is
downgraded to `ask` when any ceiling fails. Agent secret grants are denied,
local reviewed packages remain user-only, and a `native-unconfined` provider
cannot receive unattended authority.

The resulting decision and normalized policy digest become
`PluginOperationPlan.authority`. Apply re-evaluates the stored plan against the
current host policy and rejects digest or decision drift. The parser and
evaluator are implemented and independently tested in the umbrella CLI.
Authorization is loaded through a bounded read from an explicit
operator-selected ACL or the existing user-level ACL. Automatically discovered
workspace configuration cannot pre-authorize plugin mutation.

The shared Plugin Manager stores one immutable authorization policy and
provides common complete-plan evaluation and apply-time verification APIs to
CLI, Web, and management MCP adapters. Web retains the default `ask` policy
until it receives a trusted host policy source.

The delegated planner may return `pluginOperationPlan` only as a draft. The
Manager replaces host identity, lifetime, actor, and authority; binds action,
package, fixed scope, requested release, and verified capability generation;
then persists a validated `PluginOperationPlanEnvelope`. The envelope digest
is the reviewed Manager identity. The upstream component digest is stored
separately and passed only to the existing mutation child.

The planner boundary is
`a3s.use.plugin-operation-plan-draft.v1`. Its strict JSON shape contains only
action, package and component identity, exact package transitions, Runtime
provider evidence, workspace impacts, aggregate impact, and durable state
evidence. Operation identity, timestamps, scope, actor, policy decision,
policy digest, confirmation requirements, and derived secret changes are not
accepted from the planner. The host supplies its fields through
`PluginOperationPlanBinding`; binding derives the secret delta and validates
the final `a3s.use.plugin-operation-plan.v1`. The typed transition constructor
likewise derives surface changes from exact before/after package states.

Before first intent, apply reproduces current policy authority and an `ask`
decision requires a matching `a3s.use.plugin-operation-confirmation.v1` from a
trusted user-facing adapter. The confirmation is stored in the append-only
intent. Recovery validates that recorded evidence rather than abandoning
already-started side effects after a later policy change. Legacy
component-only records remain compatible. A3S Use planner emission of the
complete draft remains pending on registry, receipt, Runtime provider, and
capability-state wiring.

Each reviewed Manager record binds the actor supplied by its trusted adapter:
CLI and Web select `user`, while management MCP selects `agent`. Untrusted
package or request content cannot select the principal. The current lifecycle
scope remains the frozen `user/current` scope and is returned alongside that
actor.

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
