# A3S Use Plugin Contract Reference

- Status: M0 complete; M0K-B OKF control-plane and package lifecycle foundation complete
- Baseline date: 2026-07-30
- Product amendment: OKF bundle contract/conformance frozen 2026-07-31;
  plugin-surface control plane frozen 2026-08-01; managed-host protocol frozen
  2026-08-02; package lifecycle intent/journal and cognitive-package
  dependency/lock contracts frozen 2026-08-03; unified A3S Flow contracts and
  exact-generation preflight binding foundation frozen 2026-08-04; operation
  plan v3 dual-lock upgrades, host capabilities v3, and Grant-aware graph saga
  foundation frozen 2026-08-05
- Architecture: [Plugin Platform Architecture](plugin-platform-architecture.md)
- Lifecycle: [Plugin Lifecycle and Security](plugin-platform-lifecycle-and-security.md)
- Delivery: [Plugin Platform Development Plan](plugin-platform-development-plan.md)

This document records the machine-readable plugin contracts implemented in
`a3s-use-core`, plus the signed catalog reader and durable workspace-grant
store implemented in `a3s-use-extension`, plus the package lifecycle contracts
implemented in `a3s-use`. It does not claim that production Plugin Manager
wiring, Knowledge, Gateway, or Runtime generation retirement are complete. The
package/capability, dependency-graph, and Grant-aware graph hosts are
implemented foundations.
Signed remote schema-v3 records enter that graph from `a3s-use install` and
the compatible `component install` command. A3S Code TUI/Web now composes the
supported Tool Task, stdio MCP, A3S Flow, Skill, and UI host set; production
Knowledge, Service/HTTP, durable Flow execution, and managed-host composition
remain product integration gates.

## Contract Set

| Contract | Schema | Purpose |
| --- | --- | --- |
| Plugin manifest | `a3s.extension/v3` | Named Tool, MCP, OKF, Flow, Skill, and UI surfaces |
| Flow surface | schema-v3 `flow` block | Fixed `a3s-flow` engine, typed runtime/source/export, capability dependencies, and optionality |
| OKF bundle | `a3s.use.okf-bundle.v1` | Exact non-executable OKF version, root, digest, counts, bytes, and conformance limits |
| MCP release | `a3s.use.mcp-release.v1` | Immutable Streamable HTTP Runtime Service and headless lifecycle contract |
| Skill release | `a3s.use.skill-release.v1` | Immutable content-bound Agent input with no Runtime workload |
| Tool release | `a3s.use.tool-release.v1` | Immutable CLI Task or HTTP Service workload |
| Permission ceiling | `a3s.use.plugin-permissions.v1` | Maximum authority per executable/UI surface |
| Workspace grant proposal | `a3s.use.plugin-workspace-grant-proposal.v1` | Reviewable pre-confirmation resolved authority |
| Grant confirmation | `a3s.use.plugin-grant-confirmation.v1` | User evidence binding an exact plan and proposal |
| Operation confirmation | `a3s.use.plugin-operation-confirmation.v1` | User evidence binding every `ask` apply to one plan |
| Workspace grant snapshot | `a3s.use.plugin-workspace-grant-snapshot.v1` | Revisioned active-grant evidence before mutation |
| Workspace grant changes | `a3s.use.plugin-workspace-grant-changes.v1` | Sorted root/dependency grant and revoke transition set |
| Workspace grant operation | `a3s.use.plugin-workspace-grant-operation.v1` | Durable immutable intent and resumable grant lifecycle phase |
| Workspace grant cutover | `a3s.use.plugin-workspace-grant-cutover.v1` | Evidence that capability publication selected the prepared generation |
| Workspace grant rollback | `a3s.use.plugin-workspace-grant-rollback.v1` | Evidence for restoring exact candidate paths before capability cutover |
| Workspace grant | `a3s.use.plugin-workspace-grant.v1` | Scope-bound resolved authority within a signed ceiling |
| Catalog record | `a3s.use.plugin-catalog.v1` | Compatible search and review metadata without package download |
| Catalog record | `a3s.use.plugin-catalog.v2` | Plan-ready manifest evidence and surface dependency closure |
| Catalog record | `a3s.use.plugin-catalog.v3` | Exact OKF evidence, complete Flow graph, and a planning target when Tool/MCP surfaces require it |
| Package lock | `a3s.use.plugin-package-lock.v1` | Exact transitive versions, dependency edges, content digests, host target/version, and Registry/TUF provenance |
| Installed package graph | `a3s.use.installed-package-graph.v1` | Root-owned exact dependency lock retained for shared-node reference and reverse uninstall |
| Pending package graph operation | `a3s.use.pending-package-graph-operation.v1` | Durable admitted plan, exact changed manifests/generations, and crash-replay ownership |
| Planning bundle | `a3s.use.plugin-planning-bundle.v1` | Pre-archive Tool/MCP workload, release, and artifact evidence |
| Installed plan evidence | `a3s.use.installed-plugin-plan-evidence.v1` | Package-specific receipt, catalog, surface, and capability join |
| Operation plan draft | `a3s.use.plugin-operation-plan-draft.v1` | Untrusted planner evidence before host identity and authority |
| Operation plan draft | `a3s.use.plugin-operation-plan-draft.v2` | Draft v1 plus an exact derived OKF bundle delta |
| Operation plan | `a3s.use.plugin-operation-plan.v1` | Exact install, upgrade, or uninstall delta |
| Operation plan | `a3s.use.plugin-operation-plan.v2` | Plan v1 plus an exact derived OKF bundle delta |
| Operation plan | `a3s.use.plugin-operation-plan.v3` | Upgrade plan binding the complete exact prior/candidate lock union, including removed nodes |
| Manager toolset | `a3s.use.plugin-manager-tools.v1` | Bounded MCP management interface |
| Manager toolset | `a3s.use.plugin-manager-tools.v2` | Manager v1 with canonical `okf` search and selection values |
| Manager toolset | `a3s.use.plugin-manager-tools.v3` | Manager v2 with canonical `flow` search and selection values |
| Managed scope | `a3s.use.plugin-managed-scope.v1` | Host-derived workspace identity and exclusive mutation fence |
| Host capabilities | `a3s.use.plugin-host-capabilities.v1` | Exact manager build, protocol level, and frozen supported schema inventory |
| Host capabilities | `a3s.use.plugin-host-capabilities.v2` | Protocol level 2 with explicit first-class Flow support; v1 remains byte-frozen |
| Host capabilities | `a3s.use.plugin-host-capabilities.v3` | Protocol level 3 advertising operation plan v3; v1/v2 remain byte-frozen |
| Host plan | `a3s.use.plugin-host-plan-request/result.v1` | Exact verified selection in and canonical reviewed A3S Use plan out |
| Host apply | `a3s.use.plugin-host-apply-request/result.v1` | Digest-only apply with exact confirmation and idempotent operation evidence |
| Host enablement | `a3s.use.plugin-host-enablement-request/result.v1` | Optimistic-generation enable or disable through the same manager |
| Host observation | `a3s.use.plugin-host-observation-request/result.v1` | Exact Use-owned package/capability state or explicit unavailable evidence |
| Package lifecycle intent | `a3s.use.plugin-lifecycle-intent.v1` | Exact package generation, six-surface graph, action, and deterministic checkpoint schedule |
| Package lifecycle operation | `a3s.use.plugin-lifecycle-operation.v1` | Durable checkpoint receipts, optional failures, required failure evidence, and terminal status |
| OKF projection receipt | `a3s.use.okf-projection-receipt.v1` | Exact scope/package/surface generation staged for Knowledge |
| OKF Knowledge observation | `a3s.use.okf-knowledge-observation.v1` | Staged, promoted, failed, or removed index evidence and last-good selection |
| OKF capability projection | `a3s.use.okf-capability-projection.v1` | Exact promoted evidence safe to join to a scoped capability generation |
| OKF Knowledge binding | `a3s.use.okf-knowledge-binding.v1` | Durable exact receipt plus observation for one retained generation |

### OKF control-plane contract is frozen

Open Knowledge Format (OKF) is now an accepted first-class cognitive package
surface. The shared `a3s.use.okf-bundle.v1` descriptor and inspector are now
implemented in `a3s-use-core`. They bind the declared v0.2 or v0.1 format,
bundle root, byte-exact deterministic digest, concept/file counts, expanded
bytes, and hard-bounded declared limits. The inspector never rewrites bundle
bytes, tolerates unknown concept types and extension keys, reports safe
dangling links as diagnostics, rejects unsafe path resolution, and treats
Attested Computation executor/attester fields as inert data.

M0K-B adds `PluginSurfaceKind::Okf` and a bounded named `okf` block to schema
v3. The manifest binds the full bundle contract and optionality; a Skill may
declare `requires_okf`. Package admission recursively rejects links and
special files, reads every bundle file within declared limits, reruns the
shared inspector, and requires exact contract equality. Executor-like unknown
manifest fields fail closed. Existing schema v1/v2 and the original schema-v3
fixture remain byte-compatible.

Catalog v3 carries the same exact OKF bundle evidence. Skill-to-OKF edges join
the existing transitive surface closure. An OKF-only catalog record omits an
executable planning target; any record containing Tool or MCP still requires
one. OKF and Skill surfaces cannot carry runtime permission ceilings, and the
Runtime provider/binding contracts accept only Tool and MCP.

The versioned plan/draft v2 contracts derive sorted `okfChanges` from exact
package before/after states. Manager-toolset v2 adds `okf` to bounded search
and surface selection without changing the v1 bytes. The projection receipt,
Knowledge observation, and capability projection bind scope, package/surface
identity, package generation, package/manifest/bundle digests, index
schema/build identity, selected last-good generation, and observation digest.
A staged or failed candidate cannot select itself; only exact promoted evidence
can produce a capability projection.

The shared Surface Reconciler treats Knowledge as a distinct host owner. A
missing observation remains `pending`, `staged` remains prepared and
unpublished, and only `promoted` is healthy. Dependent Skills remain hidden
until that evidence is ready. M0K-C-A provides a public `Send + Sync`
stage/promote/observe/remove port, an evidence-checking client, and a bounded
durable binding store. The stage request revalidates the exact in-memory bundle
bytes; the store accepts only monotonic observation updates and creates a
capability projection only from an exact retained promoted record. The package
lifecycle adapter implements receipt-owned OKF
stage/promote/hide/remove semantics and durable replay. Production Knowledge
indexing, scope-aware session publication, cited retrieval, and end-to-end
parent-saga recovery remain M0K-C-B work; no second lifecycle or Runtime path
is implied by these contracts.

### A3S Flow surface contract is additive and unified

`PluginSurfaceKind::Flow` is the sixth schema-v3 contribution kind. Its
manifest block requires `engine = "a3s-flow"`, the currently supported
`runtime = "native-ts"`, one bounded package-relative `.ts` source, one
portable export identifier, optionality, and sorted named Tool/MCP/OKF
dependencies. Flow itself cannot carry a runtime permission ceiling; authority
remains attached to those explicitly declared capabilities.

The complete catalog graph must exactly match the admitted manifest inventory,
optionality, and dependency edges. Package validation checks bounded UTF-8
source and content-addresses it. Lifecycle prepares Flow after Tool/MCP/OKF,
then prepares dependent Skill/UI; stop and removal reverse that order. The
capability catalog binds engine, runtime, source path/digest/media type, export,
and capability edges. A corrupted required source withholds the generation;
valid source bytes without typed A3S Flow host preflight remain pending and are
not published.

`A3sFlowLifecycleHost` is the concrete Use adapter for that preflight boundary.
It delegates to `a3s_flow::NativeTsRuntime::preflight` and persists
`a3s.use.flow-runtime-binding.v1` by scope, package, Flow surface, and lifecycle
generation. Capability observation revalidates the admitted source and exact
compiled artifact before reporting `Prepared`; missing evidence remains
pending and substitution reports failure. Stop preserves retained evidence for
drain, while removal deletes only the exact receipt-owned generation.

Host-capabilities v1 remains frozen without Flow. V2 uses protocol level 2 and
advertises it explicitly. V3 uses protocol level 3 and adds only operation
plan v3 to the advertised plan inventory, so a managed host cannot accept a
dual-lock removal plan without negotiating support. Manager-toolset v1/v2 also
remain frozen; v3 adds the canonical `flow` selection value. Compilation,
preflight, durable execution,
replay, and storage belong to the typed `a3s-flow` host adapter. `flow.json` is
a design/deployment document that must map to the same Flow identity; it does
not create a second engine or package lifecycle.

### Package lifecycle operation is package-owned

The schema-v3 manifest is the only surface inventory for both reconciliation
and lifecycle. Tool, MCP, OKF, Flow, Skill, and UI contributions are not independent
installation records. The intent binds the reviewed plan, scope, package and
manifest digests, generation, action, canonical dependency levels, required
closure, and per-checkpoint idempotency keys.

Install/enable prepares forward and publishes once. Disable hides, drains, and
stops in reverse. Uninstall hides, drains, removes receipt-owned contributions
in reverse, and only then removes the package. The operation journal is
bounded, strict, atomic, cross-process locked, and rejects reordered,
substituted, or tampered receipts. Exact package/Runtime N/N+1 storage,
snapshot-selected routing, and receipt-owned prior removal are implemented.
The graph coordinator classifies Add/Replace/Remove/Retain, downloads and
prepares only changed candidates dependency-first, publishes candidates and
removed routes once, automatically rolls back before cutover, retires replaced
or unreferenced generations in reverse order, and durably replays every
outcome. Its Grant-aware entry points persist plan-bound candidate Grants before
package preparation, require exact snapshot/generation cutover evidence,
restore package and Grant candidates together before cutover, and drain prior
calls before revocation. Product-level Grant planning/apply selection and
production provider composition remain outside the completed foundation.

Package ownership also applies above one manifest. A schema-v3 package may
declare canonical package ID plus SemVer dependency blocks. The package lock
binds the complete transitive closure; the plan classifies each locked node as
root or dependency and as Add, Remove, Replace, or Retain. Operation plan v3
binds both lock digests and embeds both complete locks so removed nodes remain
reviewable after they disappear from the candidate closure. The graph
coordinator requires lifecycle units only for changed nodes, installs them in
dependency order, verifies exact already-published retained nodes, publishes
changed and removed nodes in one snapshot cutover, and removes changed nodes in
reverse order. Direct removal is rejected while another installed graph or
manifest still depends on the package.

All JSON contracts:

- reject unknown fields;
- enforce bounded input and collection sizes; and
- avoid secret values, executable paths, public service endpoints, or generic
  action payloads.

Immutable review and receipt contracts use OLPC canonical JSON and expose a
`sha256:` descriptor digest. The planner draft is deliberately neither
authorized nor independently digest-authoritative; the host binds it into the
canonical operation plan before review.

## Managed Host Protocol

`PluginHostManager` is the sole typed application port for a remote managed
workspace. It is not another manager implementation. A host adapter delegates
its four distinct operations—plan, apply, set enablement, and observe—to the
same shared Plugin Manager used by local presentation adapters. Catalog/TUF
verification, immutable generations, operation replay, Workspace Grants,
Runtime Bindings, capability publication, drain, reference counting, and
cleanup remain behind that one manager.

Every request carries the descriptor digest of
`a3s.use.plugin-host-capabilities.v1`, a positive assignment generation, and an
exact `a3s.use.plugin-managed-scope.v1` value. The scope contains only opaque
host, workspace, and authority identities plus a positive fence generation and
digest. It contains no path or bearer token. The manager compares the complete
value with its durable current fence; stale, future, standalone, or
different-authority scopes fail closed.

The host capability schema freezes its full v1 contract, catalog, plan, and
surface inventory. A different inventory requires a new schema and protocol
level instead of silently mixing versions. Plan input can select only one
complete verified catalog record and bounded named surfaces. Policy authority,
provider choice, operation identity, and confirmation are host-owned. Apply
submits only the stored operation ID and plan digest plus exact canonical user
confirmation when required. Enablement uses an expected installed generation.
Observation returns either the shared Surface Reconciler state with exact
receipt/package/capability evidence or a typed unavailable reason; absence and
success are never inferred from missing evidence.

Canonical capability, managed-scope, and observation JSON/SHA-256 fixtures are
checked into `crates/core/fixtures/plugins/` for Cloud and other host adapters.
Unknown fields, unbounded input, mixed schema versions, zero generations,
noncanonical package IDs, stale fences, and substituted request/result
identities are rejected.

## Catalog and Trust Provenance

`PluginCatalogRecord` contains the searchable signed target content:

- package identity, version, channel, target, compatibility, and availability;
- for schema v3, the normalized package ID and SemVer dependency inventory;
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

## Cognitive Package Dependencies and Lock

`PluginPackageDependency` is a versioned edge to another cognitive package. It
contains only `packageId` and `versionRequirement`. Package IDs are canonical,
requirements use canonical SemVer syntax, self-dependencies are rejected, and
the normalized set is sorted uniquely with a bound of 128 entries. Dependency
blocks are accepted only by schema-v3 manifests and catalog-v3 records. A
verified catalog dependency inventory must exactly equal the admitted manifest
inventory.

`PluginPackageResolver` consumes one exact verified root, a concrete host target
and A3S Use version, and at most 4,096 complete verified catalog-v3 candidates.
It performs deterministic highest-compatible selection with at most 65,536
search attempts and backtracks when a later constraint conflicts. Missing,
incompatible, conflicting, cyclic, over-bound, or Registry-ambiguous closures
return distinct typed errors. Registry ambiguity is identity-based: the same
required package in more than one Registry trust identity is never resolved by
priority or iteration order.

`a3s.use.plugin-package-lock.v1` contains:

- `rootPackageId`;
- `host.target` and canonical `host.useVersion`;
- a package-ID-sorted, root-reachable set of complete
  `VerifiedPluginCatalogRecord` nodes; and
- for each node, sorted edges carrying the signed requirement and exact
  selected version.

Validation reconstructs reachability and topological order, rejects cycles or
unreachable extras, rechecks target/host compatibility, and proves every edge
equals the signed catalog dependency inventory. The descriptor uses canonical
JSON and a `sha256:` digest.

`PluginOperationPlanEnvelope::new_with_package_lock` stores the complete lock
beside the plan and adds `packageLockDigest` to the plan before calculating its
own digest. Validation requires both or neither, checks root/dependency roles,
and reconstructs exact planned states and Registry sources from every locked
catalog record. Host plan requests/results transport the same lock and reject
substitution.

`resolve_remote_package_lock` reads candidates only from the host-selected set
of named `TrustedRegistry` values. `download_locked_remote_packages` first
revalidates every Registry name, URL, trust root, TUF snapshot, catalog record,
target, and digest in dependency-forward order without downloading archives;
only then does it download the prepared closure. This gives apply a no-partial-
download boundary for Registry drift.

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
objects. Untrusted package, Tool, MCP, OKF, Flow, Skill, or UI content cannot
act as confirmation evidence.

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

The success phase sequence is `intent-recorded`, `preparing`, `prepared`,
`cutover-committed`, `retiring`, and `completed`. Before cutover, a failed
candidate can instead enter `rolling-back` and finish `rolled-back` with
`a3s.use.plugin-workspace-grant-rollback.v1` evidence. Every journal replacement
is bounded, atomic, symlink-checked, and serialized with grant records under the
same cross-process lock. `prepared` is reached only after all candidate writes
converge. Rollback restores the exact previously observed Grant or tombstone at
each candidate path; when no prior record existed it removes only the prepared
candidate. Rollback after cutover is rejected.

`WorkspaceGrantCutoverEvidence` must bind the expected generation transition,
an immutable capability-snapshot digest, and a non-future commit time.
Retirement cannot begin without it. A retry reuses the durable cutover or
rollback time and immutable records, so a crash between record and checkpoint
writes converges instead of inventing new evidence. Capability generation zero
is valid only as the observed before-state of the initial exact 0 -> 1 cutover.

The Grant-aware package-graph entry points bind this journal to the exact
reviewed envelope and signed ceilings. They persist candidates before package
or Runtime preparation, commit only Registry-returned exact cutover evidence,
drain calls admitted by the prior snapshot, and then retire prior Grants before
surface/package cleanup. The standalone manager now derives and persists these
inputs after trusted authority binding and exact confirmation, and it selects
the Grant-aware entry point whenever required. Umbrella and managed-host
adapters still need to forward the same authority; the grant-free compatibility
methods do not provide authorization.

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

Catalog v3 adds OKF bundle evidence and allows a Skill to require a named OKF
surface. Canonical surface-kind order is MCP, OKF, Skill, Tool, UI. The same
dependency closure rules apply; OKF cannot depend on an executable surface or
delegate authority.

Catalog v3 also carries one exact `planning-v1.json` target name, byte length,
and SHA-256 exactly when the record has executable Tool or MCP surfaces. The
strict `a3s.use.plugin-planning-bundle.v1` target binds the package, archive,
manifest, permission ceiling, and every executable surface to a complete
release descriptor and digest-pinned artifact. It is fetched through TUF
before archive download. OKF-only records omit it. Mutable OCI tags, missing
executable surfaces, stdio MCP, or workload/catalog drift fail closed.

`VerifiedPluginCatalogRecord::install_transition` converts one plan-ready
catalog-v2 or catalog-v3 record into the exact registry `add` transition
consumed by the operation-plan draft. It preserves verified catalog
provenance and archive evidence, requires
both expanded-package and raw manifest digests, derives the selected permission
ceiling and its digest, and derives all surface additions. Surface selection
does not change archive download length or expanded package size. Avoiding
unrelated downloads requires separate package archives, not merely optional
surface flags.

For a legacy schema-v1/v2 package selected through complete catalog evidence,
`a3s-use-extension` persists the `VerifiedPluginCatalogRecord` in extension
receipt schema 2. A schema-v3 cognitive package instead enters through the
package lifecycle coordinator and uses receipt schema 3; it retains the same
verified catalog evidence when installed from TUF and additionally binds the
exact positive `lifecycleGeneration` and deterministic immutable root. Loading
either plan-ready receipt:

1. validates the canonical catalog record and its TUF role provenance;
2. reconstructs `ResolvedRemotePackage` and requires exact equality with the
   recorded target;
3. hashes the installed manifest and compares the signed raw-manifest digest;
4. hashes the complete expanded package and compares both the signed catalog
   digest and receipt digest.

This receipt is durable plan-ready before-state for later upgrade and
uninstall resolution. Receipt schema 1 remains readable for catalog-v1,
explicit-local, and release-bundle installs, but its absence of signed
plan-ready catalog evidence must not be silently upgraded into a complete plan.
Receipt schema 3 is committed disabled, projected with the same lifecycle
generation in the route snapshot, and can be published, hidden, drained, or
removed only through exact lifecycle operations. Legacy extension toggles
reject it rather than creating a second mutation path.

`PreparedRemotePackage::load_planning_bundle` performs a target-only read for
catalog v3. It requires the exact target in signed TUF metadata, compares TUF
and catalog length/SHA-256, rejects package-target custom metadata on the
planning target, and rebinds the parsed bundle to the verified catalog.
Catalog v1/v2 returns no planning bundle and preserves existing digests.

`VerifiedPluginCatalogRecord::selected_state` resolves a package state from
signed release evidence and a selected surface set.
`remove_transition` uses that state as the exact `before` side of an uninstall;
`replace_transition` combines installed receipt evidence with a newly verified
candidate record. `InstalledExtension` exposes equivalent helpers after
rechecking its receipt identity and digests. The selected surface input must
come from the same capability snapshot bound into plan state evidence. The
receipt does not claim which optional surfaces are currently enabled.

The capability registry is the join boundary for that activation fact. A
plan-ready schema-v3 extension binding carries `plannerEvidence` schema 1
containing the canonical full-receipt digest, verified catalog-record digest,
signed manifest and expanded-package digests, desired enabled state, and
sorted selected surface references. Emission requires the manifest inventory
to equal the verified catalog inventory and the selection to be dependency
closed. Packages without complete signed evidence remain observable but omit
this block.

`a3s use extension planning-evidence <publisher/name> --json` resolves the
package-specific `a3s.use.installed-plugin-plan-evidence.v1` record. The strict
contract joins the complete verified plan-ready catalog record and canonical
receipt digest to the same capability generation, revision, desired enabled
state, and sorted selected-surface closure. It is rederived from a stable
capability snapshot and a freshly validated installed receipt; any package,
digest, catalog, version, desired-state, or surface mismatch fails closed.
This is the authoritative `before` evidence for upgrade and uninstall draft
assembly.

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
- download, installed, reclaimed, drain, and retained-data impact;
- for plan v2, the exact derived OKF before/after bundle changes; and
- durable state revision, capability generation, and prior receipt digest.

The plan validator derives surface and secret deltas from the embedded package
states, and derives OKF impact for plan v2. OKF-bearing plans require v2 while
plans without OKF remain byte-compatible v1. Runtime provider evidence remains
required only for Tool and MCP. The validator also rejects:

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

Planning uses `RuntimeProviderSelector` to turn host-supplied, explicit
per-surface assignments into `PlannedProviderEvidence`. It resolves only the
named entries in `RuntimeClientRegistry`, validates each complete Runtime spec
and required lifecycle features against freshly read capabilities, binds the
provider ID/build, normalized capability digest, enforcement profile, and
semantics-profile digest, and returns evidence sorted by qualified surface.
The selection also retains the exact connected client for apply-time
revalidation. No missing or failed assignment falls back to another provider.

For a catalog-v3 candidate, `plan_runtime_bundle` first converts the verified
planning bundle, exact selected package state, canonical pre-confirmation
grant proposal, and generation into provider-neutral Runtime templates. The
initial safe subset maps a release-backed CLI Tool to a Runtime Task and an
HTTP Tool or Streamable HTTP MCP server to a private Runtime Service. Authority
that Runtime 0.2 cannot represent exactly fails with
`use.plugin.runtime.authorization_unsupported`.

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

Core surface selection and manager-toolset v2 now have a canonical `okf`
value. The umbrella host must explicitly version or compatibly extend its ACL
policy before it can authorize an OKF-bearing operation; omission never grants
implicit authority.

The delegated planner may return `pluginOperationPlan` only as a draft. The
Manager replaces host identity, lifetime, actor, and authority; binds action,
package, fixed scope, requested release, and verified capability generation;
then persists a validated `PluginOperationPlanEnvelope`. The envelope digest
is the reviewed Manager identity. The upstream component digest is stored
separately and passed only to the existing mutation child.

The planner boundary is
`a3s.use.plugin-operation-plan-draft.v1` or, for an OKF delta,
`a3s.use.plugin-operation-plan-draft.v2`. Its strict JSON shape contains only
action, package and component identity, exact package transitions, Runtime
provider evidence, workspace impacts, aggregate impact, and durable state
evidence. Operation identity, timestamps, scope, actor, policy decision,
policy digest, confirmation requirements, and derived secret changes are not
accepted from the planner. The host supplies its fields through
`PluginOperationPlanBinding`; binding derives the secret delta and validates
the matching `a3s.use.plugin-operation-plan.v1` or v2. The typed transition
constructor likewise derives surface changes from exact before/after package
states; v2 additionally derives exact OKF bundle impact.

Before first intent, apply reproduces current policy authority and an `ask`
decision requires a matching `a3s.use.plugin-operation-confirmation.v1` from a
trusted user-facing adapter. The confirmation is stored in the append-only
intent. Recovery validates that recorded evidence rather than abandoning
already-started side effects after a later policy change. Legacy
component-only records remain compatible. Registry install, replace, and
remove transitions plus plan-ready installed receipt evidence are now
available.

The umbrella CLI emits and host-binds complete live install, registry-upgrade,
and uninstall drafts for the safe first slice: a catalog-v2 package whose
package surfaces are all permission-free Skill or UI surfaces. Install and
upgrade component plans carry the verified candidate catalog in the upstream
digest and require exact equality with the resolved registry target. Upgrade
and uninstall also match `a3s.use.installed-plugin-plan-evidence.v1` to the
compact capability snapshot and umbrella current version before deriving exact
replace or remove transitions. Catalog-v2 upgrade fails closed instead of
falling back when installed evidence is absent or drifted.

Planning binds verified capability generation and a durable monotonic
planner-state revision. Apply rechecks capability generation/revision and the
planner revision before intent, then advances the planner revision atomically
and idempotently after successful child mutation.

Catalog-v1 component plans remain compatible without claiming a complete
plugin plan. For catalog v3, the CLI registry resolver fetches only the signed
planning target and includes its typed bundle in the upstream component plan
and digest. The shared Manager requires that bundle and rechecks its exact
catalog binding. A plan containing a Tool, MCP server, or any permission
ceiling still fails closed until the umbrella host provides explicit Runtime
provider assignments and durable grant-saga evidence. Registry no-op upgrades
remain compatible component-only plans.

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

Manager-toolset v1 remains frozen. V2 retains the same ten operations and
annotations, adding only `okf` to the canonical surface-kind enums used by
search, install planning, and upgrade planning. V3 retains the same inventory
and adds only `flow`, so older hosts never silently claim support.

## Golden Fixtures

Canonical interoperability fixtures live under
`crates/core/fixtures/plugins/`:

- `permission-ceiling-v1.json`;
- `catalog-record-v1.json`;
- `complete-package-catalog-v1.json`;
- `operation-plan-install-v1.json`;
- `manager-toolset-v1.json`;
- `host-capabilities-v1.json`;
- `catalog-record-okf-v3.json`;
- `operation-plan-install-okf-v2.json`;
- `manager-toolset-v2.json`;
- `host-capabilities-v2.json`;
- `host-capabilities-v3.json`;
- `manager-toolset-v3.json`.

Canonical OKF fixtures under `crates/core/fixtures/okf/` include the bundle
contract plus `projection-receipt-v1.json`,
`knowledge-observation-v1.json`, and `capability-projection-v1.json`.

Each fixture has a sibling `.sha256` file. Tests require byte-for-byte
canonical form, stable descriptor digests, fail-closed unknown fields, and
cross-contract binding.

The complete installable package lives under
`crates/extension/fixtures/packages/plugin-v3/`. It contains all four surface
kinds and both Tool/MCP workload variants. Its expanded directory and
deterministic `tar.gz` reconstruction have fixed file-count, byte-count, and
SHA-256 evidence. Tests extract the archive through the real package source
validator and revalidate every referenced surface file.

The additive `plugin-v3-okf` fixture freezes a second ACL manifest and a
complete OKF + dependent Skill package. Its nine-file, 3,258-byte expanded
package and deterministic archive are content-addressed, and validation reads
every OKF byte before comparing the declared bundle evidence.

The additive `plugin-v3-cognitive` fixture contains all six contribution kinds
and freezes Flow → Skill → UI dependency ordering on top of Tool/MCP/OKF. Its
manifest, Flow source, package byte count, file count, and digest are checked
through the real package validator and lifecycle tests.

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
