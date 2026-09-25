# A3S Use First-Principles Agent Package Manager Audit

Status: development preview (2026-09-23)

This document audits A3S Use against the job it must perform as an Agent
Package Manager. It separates a mechanism that exists in the repository from a
production authority that is actually composed, and from a release gate that
has been exercised on supported hosts. Passing unit tests alone is not enough
to move a row into the last category.

## The product invariant

An Agent Package Manager is a trusted state machine, not a package downloader
with an MCP adapter. For every agent-visible capability it must be possible to
answer, after a restart and without host paths:

1. Which reviewed package and immutable generation owns this capability?
2. Which signed description and exact input/output contract were admitted?
3. Which principal, scope, Grant, and policy authorize this request?
4. Which isolated provider process or service will receive it?
5. Which lease keeps that generation alive for the whole call or stream?
6. Which durable record makes an interrupted transition converge without
   guessing or repeating an external side effect?

The identities must be bound together, while their authorities remain separate:
Registry trust chooses immutable bytes; the installation chooses a desired
generation; Control chooses a committed transition; Runtime owns provider
execution; the Gateway exposes only an opaque, authorized projection.

## Audit matrix

| Invariant | Repository evidence | Status | What still prevents release |
| --- | --- | --- | --- |
| Immutable package and dependency identity | TUF-backed Registry source, digest-pinned targets, global Artifact Store, graph lock and exact plan digests; registry-tools threshold root ceremony (`--root-share-count` / `--root-threshold`) with pin-stable offline recovery, under-threshold fail-closed, every-intermediate-root `rotate-root`, `compare-mirrors`, and `withdraw-targets` drills | Qualified for the current preview + ops tooling | Official production Registry bootstrap publication and live channel drills |
| One installation authority | Installation snapshot, graph/Grant journals, stale-generation rejection, User/Workspace scope fences; production open fail-closes legacy authority beside Control (`reject_legacy_authority_paths` / `legacy_state_unsupported`); legacy file-store constructors for Knowledge/Runtime/Flow bindings, lifecycle journal, Knowledge recovery Grants, `InstallationSnapshotStore`, and `PendingPackageGraphStore` are `#[cfg(test)]` only — production API exposes `for_control_authority`; public file-store Grant saga (`PluginGrantLifecycleUnit`, `apply_*_with_grants`) is `#[cfg(test)]` only | Qualified in A0/A1/A2 tests and production open path | Remaining product-host composition and live Grant/Runtime join beyond inactive Control proofs |
| Atomic lifecycle | Reviewed plan/apply service, six-surface lifecycle, cutover and retirement journals, subprocess recovery matrices | Qualified in inactive and managed-host test paths | Production Code/managed-host composition and the remaining platform/reboot fault matrix |
| Provider isolation and resource ceilings | Runtime plans bind unit class, isolation, mounts, secrets, resources, provider build and semantics digest | Qualified for Runtime contracts | Production Runtime Service composition, actual host secret delivery, and provider admission under the live Control authority |
| Agent-facing contract | Standard MCP Tools/Resources/Prompts, bounded closed JSON schemas, opaque references, consumer negotiation and cancellation | Contract-complete | A3S Flow/UI/Knowledge extension payloads and independent client interoperability |
| Signed Tool description | `CapabilityDescriptionProof`, package signer allowlist, durable signed v2 snapshots, exact descriptor and envelope digests, canonical Ed25519 envelopes, bounded public-key trust store with expiry/revocation, signed Gateway composition constructors, replay-time re-verification, Registry/TUF target `capability/description-trust-store-v1.json`, and product `ensure_control_for_registry` / `mcp serve gateway --registry-name` Control open | Registry key-source binding and product Registry→Control open closed on evidence | Live-host Grant Tool cutover and remaining CLI/service authorization beyond Gateway HTTP safeguards remain open; `from_verified` and proof-only v1 snapshots are still explicit compatibility host assertions |
| Runtime contract continuity | Tool release input/output schemas and domain-separated `RuntimeToolSchemaAttestation` now flow through plans, task/service receipts, provisioning and Control evidence; verified payload admission and strict projection compare digests | Implemented in the inactive kernel (PR #238) | Production Control/Runtime/receipt/Grant composition and real schema-bearing release fixtures |
| Live invocation authorization | Gateway resolver/factory seam, principal context, discovery policy, generation leases and provider `authorize` hook; product face exposes cutover/drain hooks and `open_control_lifecycle_with_host_ports`; public `ControlRuntimeServiceReadinessPort` + `CognitivePackageManager::{gateway_cutover_activation,watch_and_reconcile_published_capability_gateway,drain_and_retain_published_capability_gateway,serve_published_capability_gateway_*}`; `ManagedCognitivePackageLifecycleFactory::with_control_runtime_readiness` injects the Control readiness port into `ensure_control`; install/upgrade/uninstall admit managed `runtime_plan_publications()`; product HTTP Gateway serve reconciles retained sessions and drains on shutdown | Public embedding + managed factory + product HTTP serve seam qualified | Code/managed entry points must still supply a live readiness implementation and attach cutover beside same-process graph apply; Plugin lifecycle readiness remains a separate trait for surface sagas |
| Generation-safe upgrade and drain | Immutable session factory, snapshot leases, list-change hub, explicit retention plans, paired catalog/descriptor retention coordinator, durable Control cursor reopening, an internal lease guard that follows cloned Gateway servers, a bounded session-factory drain state machine that closes admission and releases the source lease, a one-shot proof for idempotent Control-bound drain replay, conditional source compare-and-swap for stale local replacements, a replay-safe graph cutover activation hook wired to a Control lease-backed Gateway adapter, a composition retention boundary that derives the durable current payload set and applies under an exclusive fence, and product HTTP serve that watches/reconciles then drain+retains on shutdown | Mechanism + product HTTP serve qualified | Embedding Code hosts that retain Gateway beside graph apply must attach `gateway_cutover_activation`; non-Control rollback/session identities remain host-owned |
| Crash/restart convergence | Durable journals, exact-key replay, no-generation-inflation tests across package, Grant, Runtime, Gateway and restore paths | Broad preview coverage | Code/Runtime product-host kill tests, reboot and remaining Windows contention/reparse races |
| Backup/restore authority | Whole-installation inventory, offline verification, reviewed restore plan, rollback archive and bounded recovery journal; canonical Capability Gateway catalog and descriptor-snapshot records are now admitted as the `CapabilityPayloads` family with owner-byte/content-address validation; artifact reachability now traverses the same payload-owner tree and fails closed on nested drift or in-flight publication evidence; both immutable owners now have plan-bound clean-target candidate/activation/replay adapters, with signed descriptor replay requiring current trust verification; dedicated restore and retention coordinators bind both owner plans under one exclusive fence with preflight, fixed-order replay, and a durable cross-owner phase journal that blocks backup/reachability until recovery; the inactive composition can derive the durable published Control cursor and reopen its exact Index, catalog, and package-generation lease set after restart | Qualified for listed legacy/Use-owned families, the Capability payload coordinators, and the cursor-reopen mechanism | Production Control owner registration, live Gateway session reconstruction from the reopened lease, lifecycle retention/lease activation, clean-machine recovery and operational drills |
| Cross-language/remote use | Standard Streamable HTTP, bearer/Origin/admission controls and an independent Rust contract test | Partial | TypeScript and Python clients, remote/container client with no shared filesystem, and install/upgrade/drain/restart/denied-scope matrix |
| Extensible package surfaces | Typed Flow, OKF/Knowledge, Skill and UI owners plus consumer profile negotiation | Partial | Negotiated Flow/UI/Knowledge metadata projection, distributed Flow identity and reviewed UI backend/rendering |
| Supply chain and operations | Reproducible five-target preview archives, Cosign/Sigstore checks, SBOMs, installers and bounded diagnostics; Use-Registry `registry-staging-gate` and `registry-production-gate` fail-close keys-in-tree/ownership bleed (production also rejects in-git `registry/`) and run real `assemble`+`verify`+`check-expiry` when admissions and environment-injected keys are present; registry-tools threshold ceremony, pin-stable offline recovery, `rotate-root` with retained intermediates, `check-expiry`, `compare-mirrors`, and `withdraw-targets` (empty-catalog verify); offline production bootstrap checklist in Use-Registry `docs/production-bootstrap.md` / `docs/production-ci.md` | Preview-qualified + staging/production assemble/verify gates + rotation/mirror/withdrawal tooling | External witness, official production Use-Registry bootstrap publication, live mirror promotion, and exercised incident drills |
| Reference package and release usability | MHS fixture and documentation/README contracts; fixture asserts standard surfaces and single-attempt observation (`mhs_bridge_fixture_is_a_bounded_standard_surface_package`) | Partial | A6 virtual-lab qualification and release-candidate examples against published artifacts |

## Critical path to a production Agent Gateway

The rows above are not independent checkboxes. The shortest safe order is:

### P0 — Make the authority real

1. ~~Activate the A2 Control Store in one host composition.~~ Closed on ROADMAP
   A2 evidence (typed SQLite authority, production open fail-closes legacy
   mutable leaves beside Control).
2. Register the Runtime, Capability Index, Gateway catalog, Flow, Knowledge,
   Skill and UI effect owners behind one dispatcher and maintenance fence.
3. Make the live session factory consume the Control-bound cursor, publish a
   new immutable catalog before notification, retain old leases through drain,
   explicitly close admission and await the bounded drain, and retire payloads
   only after the exact receipt is terminal.
4. Keep production reads/writes on Control; refuse any new legacy mutable
   authority beside Control (`legacy_state_unsupported`).

Inactive-kernel and production-open proofs now constitute the Control
authority path; remaining work is product-host Grant/Runtime join and live
channel Registry/MHS ops, not re-opening A2.

### P0 — Make “signed Tool” cryptographically meaningful

The current proof envelope is intentionally a host-owned hand-off. Its
`signerId` and descriptor digest are useful evidence, but `from_verified` does
not verify a signature and a signer allowlist is not a key store. The release
path now has a qualified canonical Ed25519 envelope and public-key verifier;
the remaining production trust boundary must:

- source the trust store from Registry-controlled/TUF-authenticated keys;
- bind key id, algorithm, signer, descriptor digest and expiry/revocation;
- persist the exact verified envelope for restart replay;
- recheck the same policy during projection and restore; and
- expose only schema-bearing Tools to generic agents.

The verifier must remain outside the universal Gateway protocol, but its
result must be a typed, non-forgeable input to the Control owner. The inactive
Control snapshot owner now admits canonical signed v2 envelopes, retains the
envelopes as replay authority, and rejects the proof-only projector for those
records. The coordinated backup inventory also validates and archives the
canonical snapshot record, but intentionally leaves current trust-policy
reverification to restore/replay. A caller-supplied boolean or signer string
is not sufficient evidence. The snapshot owner now has plan-bound retention,
per-unlink journal checkpoints, and exact restart recovery. The Capability
payload restore coordinator binds the catalog and descriptor plans under one
exclusive fence, preflights both clean targets, and retries fixed-order
activation without clobbering an already-published owner. Its retention sibling
preflights both inventories and exact pending journals before fixed-order
deletion. Production Registry/TUF key-source binding, owner registration,
live Gateway session reconstruction from the restart-reopened Control lease,
and lifecycle wiring are still required. Destructive owner retention now also
has an exclusive-fence path: it waits for live Control snapshot leases, derives
the currently published catalog and matching descriptor snapshot, and rejects
a reviewed plan that would prune the durable cursor. Hosts still have to add
any independently managed rollback or legacy endpoint identities explicitly.

Implementation note (2026-09-06): the package-graph coordinator now exposes a
replay-safe `PluginGraphCapabilityCutoverActivation` hook. When attached, it
runs after a durable publish (including a replay) and before any prior-
generation drain or retirement. The inactive Control composition provides a
lease-bound adapter that reopens the durable cursor, rejects an unleased or
newer in-memory endpoint, and avoids a redundant replacement when the current
catalog already matches. This closes the ordering seam; production hosts still
must attach it to their live lifecycle. The composition's retention helper
derives the durable current protection set and its apply path rechecks that
set under an exclusive maintenance fence; independently managed rollback or
legacy endpoint payloads remain explicit caller inputs.

The adapter now also verifies the opaque lifecycle key against the reviewed
operation that owns the published cursor. It follows the cursor's immutable
installation generation rather than the merely current generation, so a stale
graph replay cannot activate a newer enablement or unrelated publication. A
non-graph enablement cursor fails closed instead of accepting an arbitrary key.

Implementation note (2026-09-06): the inactive Control composition now also
provides a Control-backed opaque invocation resolver. It reopens the durable
published cursor for each operation, compares the complete descriptor against
the immutable catalog before opening host state, and wraps the host handle in
the same external generation lease used by the Gateway session. A forged or
cross-generation descriptor therefore fails before provider I/O. The injected
factory remains responsible for the private principal/Grant/Runtime join, and
the production lifecycle still has to compose that factory and remove legacy
authority paths.

Implementation note (2026-09-06): `CapabilityGatewaySessionFactory::drain`
now provides the missing endpoint-retirement boundary. It serializes with
catalog replacement, transitions the shared live adapter to a non-admitting
draining state, waits for every already-admitted operation under a caller
deadline, and detaches the factory's source generation lease only after the
operation count reaches zero. A timed-out drain remains closed for new work and
can be resumed; independent immutable server clones retain their own leases
until dropped. This makes the subsequent exclusive payload-retention fence
observable rather than dependent on dropping an implementation detail. The
inactive Control composition exposes one drain-and-retain helper that performs
this transition before deriving and applying the cursor-bound payload plan.

The same boundary now records a one-shot typed identity proof when an external
Control-bound session actually reaches `DRAINED`. An exact retry can repeat the
combined drain-and-retain operation after the source lease has been detached;
directly drained or copied unleased catalogs do not acquire that proof.
Reconciliation also uses a source compare-and-swap so a stale same-generation
build cannot overwrite a newer local cutover; a losing attempt returns a retry
signal for a fresh durable read. During an ordinary upgrade, the existing
endpoint's prior Control lease is validated against its own source identity
before the newly published lease is installed.

### P0 — Compose the real invocation path

An opaque `InvocationRef` deliberately omits package paths and secrets, so a
generic Gateway cannot reconstruct a User or Workspace scope. The production
host must supply a resolver that joins:

`principal → scope → committed Grant → package generation → Runtime receipt →
provider lease`

and rechecks the join at open time. The same handle must be used for authorize
and invoke; upgrade, disable and uninstall must drain it. This is the main
remaining implementation item behind the A3 exit gate.

### P1 — Prove interoperability and recovery

The endpoint is not complete until an independent Rust, TypeScript and Python
client can discover and invoke it remotely, with no package filesystem access,
while install, upgrade, prior-generation drain, uninstall, restart and denied
cross-scope access all converge. The matrix must run on Linux, macOS and
Windows, including reboot and persistent antivirus/rename contention where the
platform permits it.

### P1/P2 — Operate it as a product

The official Registry, external release witness, key rotation/revocation,
backup/restore of every selected payload owner, incident response, retention,
repair, support runbooks and MHS qualification are release gates. They cannot
be inferred from green Rust tests or a development-preview archive.

## Definition of done

The A3S Use release gate can close only when all of these statements are true:

- one production Control Store is the sole lifecycle authority;
- every published Tool has a cryptographically verified signed description,
  both bounded JSON schemas, and a Runtime attestation matching the exact
  release descriptor;
- a principal-scoped resolver authorizes and invokes through the same
  generation-fenced handle;
- the catalog/session/lease transition is durable and drain-safe;
- independent remote clients pass the complete lifecycle and recovery matrix;
- all selected payload owners participate in backup/restore and retention; and
- the official Registry, release witness and operational response procedures
  are independently exercised.

## Enterprise GA blockers (evidence date 2026-09-24)

In-repo work that **does** strengthen GA without overfitting:

- Control Store is production mutable authority; `reject_legacy_authority_paths`
  fail-closes listed legacy leaves at open.
- Legacy file-store writers/constructors for Knowledge/Runtime/Flow bindings,
  lifecycle journal, Knowledge recovery Grants, `InstallationSnapshotStore`,
  and `PendingPackageGraphStore` compile only under `#[cfg(test)]`; production
  surfaces use `for_control_authority`.
- Product face exposes `gateway_cutover_activation` /
  `drain_and_retain_published_capability_gateway` and
  `open_control_lifecycle_with_host_ports` for injected Runtime Service
  readiness. Public `ControlRuntimeServiceReadinessPort` and
  `CognitivePackageManager` Gateway cutover/serve/drain methods are the
  embedding-host join. A3S CLI now injects `ControlGatewayReadinessPort` when a
  private Gateway is present (`PluginRuntimeHost::lifecycle_factory` →
  `CodeCognitivePackageLifecycleFactory` forwards `control_runtime_readiness`,
  `runtime_client_registry`, and `runtime_plan_publications`;
  `code_factory_forwards_injected_control_runtime_readiness`,
  `managed_factory_forwards_runtime_client_registry_to_control_open`).
  `ensure_control_for_registry_lifecycle` opens Control with the factory's
  RuntimeClientRegistry so effect drain reconnects managed providers. Product
  `mcp serve gateway --streamable-http` attaches
  Control reconcile while the session is retained and drain+retain on
  shutdown (`serve_published_capability_gateway_streamable_http`,
  `production_retained_gateway_watch_reconciles_then_drains_on_shutdown`).
  Legacy `RuntimeBindingStore` / Knowledge / Flow `::new` constructors that
  write `bindings/*` are `#[cfg(test)]` only; production uses
  `for_control_authority` (CLI task dispatch and Code host tests follow).
- CLI Plugin Manager planning snapshots Grants via
  `CognitivePackageManager::planned_grant_snapshot` (Control-owned only). It
  must not open `WorkspaceGrantStore::from_extension_paths`, whose lock creates
  the legacy `grants/` leaf and fail-closes Control open
  (`planned_grant_snapshot_does_not_create_legacy_grants_leaf`).
- Production Control Grant commit leaves no `grants/` leaf
  (`production_apply_commits_grants_without_legacy_grants_leaf`). File-store
  `PackageGraphAuthorization::lifecycle_unit` is `#[cfg(test)]` only.
- File-store `WorkspaceGrantStore` lock fail-closes beside `control.sqlite3`
  (`use.plugin.grant_store.control_authority_required`;
  `grant_store_fails_closed_beside_control_database`,
  `production_control_blocks_file_grant_store_from_creating_grants_leaf`) so
  embedding hosts cannot poison Control open by opening the legacy Grant API.
- Public file-store Grant saga composition is test-only:
  `PluginGrantLifecycleUnit`, `apply_*_with_grants`, and package-coordinator
  `apply_enable_with_grants` / `apply_disable_with_grants` compile only under
  `#[cfg(test)]`. Production graph apply uses Control-owned Grant commit.
- Runtime Task invoke pins generations from the Control installation snapshot
  via `acquire_control_lifecycle_generation` and never reads legacy
  `registry.json` / `extensions/` publication
  (`RuntimeTaskDispatcher::invoke`,
  `dispatcher_fails_closed_without_control_installation_authority`).
- Managed OKF Knowledge lease acquisition is Control-only:
  `acquire_control_knowledge_generation_leases` (CLI managed Knowledge search)
  and `OkfKnowledgeRecoveryManager::for_control_authority` inventory validation
  pin generations from the Control installation snapshot
  (`acquire_control_lifecycle_generation` /
  `load_control_package_selection`). Published `registry.json` leases remain
  `#[cfg(test)]` only (`OkfKnowledgeLeaseProvider::acquire`,
  `validate_authority_inventory_published`). Evidence:
  `control_knowledge_leases_fail_closed_without_control_snapshot`,
  `control_restore_fails_closed_without_control_installation_snapshot`.
- Capability snapshot leasing pins Control-selected generations via
  `ExtensionRegistry::acquire_control_snapshot` when a Control installation
  snapshot is present; empty Control (no installation snapshot) uses
  `acquire_empty_control_snapshot` against the deterministic empty face
  (`ExtensionRegistrySnapshot::empty`) and never reads `registry.json`.
  Projection (`stable_extensions_from_control`) likewise fail-closes without
  Control (`use.capability.control_required`) and builds upstream evidence from
  Control / empty face only. Evidence:
  `injected_registry_acquires_one_exact_use_snapshot_lease`,
  `snapshot_lease_fails_closed_without_control_store`.
- Coordinated state backup and whole-installation restore planning fail closed
  without Control (`use.state_backup_control_required`,
  `use.state_restore_control_required`). Backup authority is the Control export
  leaf only; the published `registry.json` / receipt authority reader is
  removed. Restore projects the Control export as Retain inventory evidence
  after live export validation. Evidence: `coordinated_backup_requires_control_store`,
  `coordinated_backup_uses_control_export_as_package_graph_authority`,
  `control_state_restore_plans_against_control_export_authority`.
- Operation diagnostics bind Registry generation/digest from the Control
  installation snapshot via `CognitivePackageManager::control_registry_diagnostic_face`
  (empty pending `registry.json` cutovers under Control). Graph/enablement
  diagnose paths no longer call `published_snapshot()`. Evidence:
  `control_registry_diagnostic_face_binds_empty_control_without_published_snapshot`.
- Control open is one path: `ensure_control` delegates to
  `ensure_control_for_registry_lifecycle(None, Cached)` so configured
  TrustedRegistry sources load signed description trust for install/plan as
  well as Gateway serve. OnceCell records signed-trust injection;
  unsigned-then-signed upgrade fail-closes
  (`use.control.signed_description_trust_unavailable`,
  `control_open_tests`).
- Use-Registry staging and production CI gates fail-close keys-in-tree /
  ownership bleed; production assemble+verify arms only with environment-
  injected keys (no trust root in git).
- Engine invariant: admitting non-empty `runtime_plan_publications` without an
  injected `ControlRuntimeServiceReadinessPort` fail-closes
  (`use.control_store.runtime_readiness_required`); opaque `gateway:` minting
  remains only for skill/native-only roots. Plugin lifecycle readiness is
  documented as saga-only, not the Control effect bind face
  (`managed_publications_without_control_readiness_fail_closed`).
- MHS research-preview fixture reuses standard surfaces; extension MHS profile
  tests are green (`mhs` filter → 2 passed).

Gates that **cannot** be closed inside this monorepo alone (do not invent
substitutes):

| Gate | Missing authoritative evidence |
| --- | --- |
| A5 production Registry | Live offline ceremony → published bootstrap pin, production mirror promotion, witness/SBOM retention outside mutable delivery (`docs/registry-key-custody.md`, Use-Registry `docs/production-bootstrap.md`) |
| A6 MHS qualification | Owning adapter repo + separate virtual-lab repo exercising Grants, single-attempt mutation / unknown-outcome, revoke/disable (`docs/mhs-integration.md` Enterprise GA / A6 exit) |
| Product-host join | CLI `PluginRuntimeHost::lifecycle_factory` injects `ControlGatewayReadinessPort` (Control-shaped face over `GatewayRuntimeServiceHost`) via `CodeCognitivePackageLifecycleFactory::managed` + trait forwarding of `control_runtime_readiness` / `runtime_client_registry` / `runtime_plan_publications`; binding stores use `for_control_authority`; Control open reuses the managed `RuntimeClientRegistry`; Runtime Task invoke uses Control installation selection (`acquire_control_lifecycle_generation`); Knowledge leases/recovery and Capability snapshot leases pin Control generations (`acquire_control_knowledge_generation_leases`, `acquire_control_snapshot`); whole-installation restore validates live Control export authority (`validate_live_control_authority`, `control_state_restore_plans_against_control_export_authority`). Same-process hosts that retain a published Capability Gateway across graph apply attach via `ProductionControlLifecycle::attach_retained_gateway_cutover` / `CognitivePackageManager::attach_retained_gateway_cutover` so production drain activates after CapabilityCutover before prior-generation Remove/Prepare (`production_retained_gateway_cutover_activates_during_upgrade_drain`). Out-of-band serve still uses watch+reconcile+drain. Remaining: publish Use cut so production CLI can drop monorepo `[patch.crates-io]` |

Until those external evidence rows exist, product status remains development
preview. Do not mark enterprise GA complete on unit-test green alone.

## Schedule estimate (engineering, not a promise)

At the current single-line development pace, the remaining work is several
independent iterations rather than one final feature:

| Workstream | Expected focused engineering time | External/CI dependency |
| --- | ---: | --- |
| Control activation, live Runtime/Gateway/Grant composition and legacy-path deletion | 5–10 working days | A3S Code/host integration and migration review |
| Cryptographic description verification and key policy | 3–6 working days | Registry key format, rotation and security review |
| Extension projection plus real client/recovery matrix | 5–10 working days | TypeScript/Python clients and all supported platforms |
| Production owner-native payload restore/retention, official Registry and runbooks | 5–10 working days | Operations ownership and external witness |
| Final MHS/release-candidate qualification | 3–7 working days | CI capacity, virtual lab and release sign-off |

The first three rows describe the A3 Agent Gateway/capability-plane critical
path. They overlap only when the corresponding owners work in parallel; on a
single implementation stream, that slice is roughly **3–6 focused weeks**.
That is not an estimate for the complete A3S-USE product. Backup/restore and
the official Registry, A2 production activation, A4 host/provider adoption,
MHS qualification, cross-platform recovery, and security/operations sign-off
remain separate product gates. For one focused implementation stream, a
production-ready release is therefore roughly **9–18 focused weeks** in total
(the A3 slice plus those productization gates), before external waiting time.
With multiple staffed owners, the calendar can compress to about **6–12
weeks**, but only if Registry, host, platform, and security dependencies are
available and accepted in parallel. No fixed date is responsible until those
external gates are staffed. The current PR #238 reduces one P0 mechanism but
does not close those gates.
