# Registry Cache Operations

Status: development preview

Last updated: 2026-08-30

## Boundary

A3S Use separates source evidence from immutable content. Each Registry source
stores a small canonical observation and resumable download state at:

```text
<registry-datastore>/verified-targets/sha256/<digest>.json
<registry-datastore>/verified-targets/sha256/.target-<digest>.part
```

Only bytes that match the TUF-signed length and SHA-256 are committed to the
process-wide Artifact Store:

```text
<data-root>/artifacts/blobs/sha256/<first-two-hex>/<digest>/content
```

The source cache and global blob tier are optimizations and explicit offline
inputs. Neither is installation state, a trust root, a receipt, or recovery
authority. TUF metadata remains the source authority. Removing a source
observation cannot change an installed package or delete a global blob; exact
cached TUF metadata may recreate the observation from the reverified blob.

Schema v3 is a pre-release clean cutover. A legacy digest-named raw file in a
source cache is unowned state and fails closed; it is never silently treated as
an observation or migrated into the global tier. Stop Use hosts, preserve the
old datastore if incident evidence matters, then remove only the proven legacy
cache before repopulating from a trusted Registry.

## Durable source configuration

The standalone CLI stores at most 64 named sources in canonical
`<state-root>/registries.acl`. Each entry binds:

- stable lowercase source name;
- canonical HTTPS base URL (loopback HTTP is test-only);
- pinned bootstrap TUF root SHA-256;
- enabled state;
- whether an exact trusted-root file was imported; and
- the durable source-observation and partial-cache policy.

The first enabled source becomes the default. `install` and `upgrade` select
that default unless `--registry-name` chooses another enabled source. All other
enabled sources are passed to bounded dependency resolution; the same package
identity in more than one enabled source is rejected as ambiguous.

```bash
a3s-use registry source add packages \
  --url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --cache-max-bytes 4294967296 \
  --cache-max-entries 4096 \
  --cache-min-free-bytes 268435456 \
  --json
```

An optional `--trusted-root /absolute/path/root.json` is accepted only when it
is a bounded regular JSON file whose complete bytes match the pinned digest.
Use copies it into a managed content-addressed path and revalidates it whenever
the source is resolved.

`registry source list --json` returns the canonical configuration revision.
`replace`, `default`, `enable`, `disable`, and `remove` require that exact
reviewed revision plus `--yes`. A stale revision fails without mutation.

Source identity is the digest of the canonical name, URL, and bootstrap-root
digest. Each identity receives a distinct TUF/cache datastore. Replacing a
source never lets prior metadata cross into the new identity. Disabling,
removing, or replacing a source preserves its old datastore; restoring the
exact identity reuses the old verified evidence. Installed receipts and locks
are immutable and are never rewritten by source administration.

## Default policy

New standalone sources and `TrustedRegistry::new` use these per-Registry defaults:

| Bound | Default |
| --- | ---: |
| Logical referenced target bytes | 4 GiB |
| Source target observations and partials | 4,096 |
| Source-partial/staging free-space reserve | 256 MiB |

Embedding hosts may supply a validated `VerifiedTargetCachePolicy`. Standalone
source `add` and `replace` persist the matching policy options. Confirmed cache
prune may supply a stricter transient policy without rewriting the source:

```text
--cache-max-bytes <unsigned-decimal-bytes>
--cache-max-entries <unsigned-decimal-count>
--cache-min-free-bytes <unsigned-decimal-bytes>
```

Prune overrides apply only to that invocation. Install and upgrade always use
the policy in the reviewed source revision.

## Admission and retention

Before a target request, A3S Use rejects a signed target that cannot fit the
source's logical byte bound. It checks the source partial filesystem for the
remaining download bytes plus the reserve and checks the operation staging
filesystem before copying a completed blob. Source observation deletion can
release logical policy capacity, but it does not claim to release physical
Artifact Store space. Global blob references and physical content are joined in
one guarded collection pass with checked usage and bounded quota assessment;
reference retirement can leave conservative extra owners.

An embedding host may enable the global hard ceiling through the Artifact Store
returned by `UsePaths::artifact_store()`. `storage_quota()` returns the canonical
policy revision; `set_storage_quota()` and `clear_storage_quota()` require that
exact reviewed revision. The policy is stored at
`<data-root>/artifacts/storage-quota.acl` and bounds logical regular-file bytes
plus digest containers across Blob and expanded-package tiers. It does not count
Registry partials or source observations, which remain governed by the
per-source policy above.

Policy-disabled global commits share a cross-process storage lock. A configured
policy serializes physical scan, exact projection, same-digest staging cleanup,
and final publication under the exclusive counterpart. Two processes therefore
cannot spend the same remaining capacity. Tightening below current use stops
growth but still permits exact replay or cleanup that does not worsen an
exceeded dimension. This admission does not delete content; confirmed
cross-source GC remains an explicit roadmap item.

An embedding host can quarantine a complete digest mismatch through the
Artifact Store returned by `UsePaths::artifact_store()`. Acquire the exact
store-bound collection guard, call `plan_quarantine(kind, digest)`, present its
path-free evidence to a trusted operator, and pass only the exact
`descriptor_digest()` to `apply_quarantine`. Apply repeats the full digest audit
before atomically publishing a canonical marker. The operation never moves or
overwrites canonical bytes. Exact replay reports `changed: false`; stale review
is rejected. New ordinary Blob and expanded-package access fails closed while
the marker exists. This is incident containment, not repair or cleanup; the
marker grants neither authority.

Verified recovery uses `ArtifactStoreMaintenance`, not a direct content-path
overwrite. Call `plan_rehydration(kind, digest, candidate)` with a candidate
outside the Artifact Store, present the returned path-free plan to a trusted
operator, then pass its exact `descriptor_digest()` to
`apply_rehydration`. Planning and every nonterminal apply acquire the exact
collection guard and derive a fresh global reference inventory. Registry
observations, installation snapshots, current and retained receipts, pending
package graphs, and nonterminal lifecycle operations must contribute zero
references to the target before replacement. Initial apply reverifies the
candidate and quarantine binding, persists a canonical prepared record, stages
under the digest lock and hard-quota peak, switches the canonical content
fail-closed, and publishes the matching completion record before ordinary
access resumes. Matching terminal replay is read-only: it validates the durable
completion and canonical replacement without reopening the external candidate
or requiring later owners to retire again. Interrupted bounded phases retry
exactly; moved, malformed, stale, or conflicting evidence fails closed. Apply
consumes the reviewed corrupt bytes, so preserve required forensic evidence
outside the store before confirmation. Existing open handles are not revoked.
Confirmed global GC remains unimplemented and separate.

Interrupted downloads are retained as
`.target-<sha256>.part`. The partial must be a bounded regular file and its
length becomes the next HTTP Range offset. A `206` response must describe the
exact offset, final byte, total signed length, and remaining content length. A
complete `200` response is accepted when a server ignores Range, but the old
partial is truncated first. No partial is staged or exposed as an offline
target. The complete bytes are re-read through the transaction-owned handle,
checked against the signed length and SHA-256, copied and rehashed into a
digest-locked global staging file, synchronized, and atomically published
without replacing an existing blob. A canonical path-free source observation
is published only after the blob is durable. The source partial is removed
last. The global file is then reopened without following its last component,
rehashed, and retained by handle as the only staging authority.

An existing partial is opened once without following its final path before
cache admission, and that exact handle remains the append/checkpoint authority.
New partial creation uses the same no-follow policy with create-new semantics.
On Windows the live partial and blob handles share read access for scanners and
diagnostics but deny external write, delete, and replacement access. The
verified handle remains live through staging: Unix path replacement cannot
redirect staged bytes, and every stage operation rehashes the held blob while
copying it.

Windows promotion retries access-denied, sharing-violation, and lock-violation
publication and cleanup failures for at most two seconds on a blocking worker. Native tests model
a read-only scanner handle that shares the transaction's existing read/write
access but withholds delete sharing. Releasing a transient handle lets the same
commit finish publication and cleanup. If a scanner prevents final partial
deletion through the bound, commit returns `use.extension.io` but preserves the
already durable blob, observation, and exact complete partial. After the scanner
releases it, retrying rehashes the global blob and removes the redundant partial
before any target request. This is the verified-target publication
qualification, not a claim that the full product antivirus and reboot matrix is
complete.

Under the exclusive target-cache lock, commit removes:

1. bounded stale `.target-<pid>-<time>.tmp` writes;
2. the oldest inactive `.target-<sha256>.part` downloads;
3. the oldest source observations, ordered by modification time and then digest;
4. only as many entries as required to satisfy byte, entry, and disk-space
   bounds.

On Windows, invalid-partial cleanup and every selected cache deletion use the
same blocking two-second retry for access-denied, sharing-violation, and
lock-violation failures. Native tests hold no-delete-share scanner handles over
stale entries, inactive partials, and source observations. Transient release
lets cleanup finish; a persistent selected-file lock returns
`use.extension.io` at the bound and leaves that entry intact. Deletions
completed earlier in the same prune remain durable. After releasing the
scanner, retry the confirmed prune; the new inventory excludes those earlier
deletions and finishes the residual selection without touching installed
package state or global blobs.

The incoming target or partial is protected throughout download, verification,
promotion, and staging. Every deletion is followed by directory synchronization
where the platform supports it. A race that consumes the admitted free space
still fails the write rather than weakening a bound.

## Inspect usage

```bash
a3s-use registry cache usage \
  --registry-name packages \
  --json
```

The command performs no network request. Schema v3 JSON reports source target
observations and their logical referenced blob bytes, resumable-partial and
stale-write physical bytes, available source-filesystem bytes, and the effective
policy. `targetBytes` and `removedTargetBytes` do not mean physical global blob
storage was consumed or reclaimed. If a verified catalog-cache stamp exists,
its Registry name, URL, and trust-root digest must match the command.

## Observe a retained operation

`a3s-use extension diagnose <publisher/name> --scope-kind <user|workspace>
--scope-id <id> --json` correlates a retained
install or upgrade graph with its exact digest-bound target cache entries. It
reports total signed `downloadBytes`, current `downloadRetainedBytes`, target
count, aggregate `missing`/`in-progress`/`complete` status, and per-package
expected/retained bytes with `missing`/`partial`/`complete` status. The exact
retained package lock also selects separately signed executable-planning
targets. Their independent `planningBytes`, `planningRetainedBytes`,
`planningTargetCount`, aggregate `planning`, and canonical `planningTargets`
inventory report the same byte states using `targetDigest`; packages without a
planning target contribute no entry, and an entirely static operation reports
`not-required`.

The lookup derives the old source-identity datastore from each target's exact
retained `VerifiedCatalogProvenance`; replacing a named Registry source cannot
redirect historical diagnostics to the replacement datastore. Before an exact
lock exists, `a3s.use.plugin-resolution-attempt.v1` retains refreshed/cached
Registry/TUF progress under the same process-held package lock and
`a3s.use.plugin-resolution-attempt-diagnostic.v1` exposes its path-free
`pre-lock` state. Failed or interrupted resolution survives for diagnosis;
success writes the download attempt before deleting it. The diagnostic makes
no network request or write and never waits for the package lock. Before a
reviewed graph exists, `a3s.use.plugin-download-attempt.v1` retains the exact
lock and selected archive set under a process-held package lock. The CLI returns
`a3s.use.plugin-download-attempt-diagnostic.v1` for that `pre-plan` window, and
switches to the operation diagnostic once the pending graph is durable.
Observation makes no network request or write, exposes no path, and deliberately
does not take the target-cache lock. It can therefore see a valid partial while
a retry or transfer is active. A complete observation plus a valid redundant
partial is reported as complete because blob publication precedes best-effort
partial cleanup. Dangling or malformed observations, links or reparse points,
non-regular entries, and oversized partials fail closed.

`complete` is a canonical source observation plus an owned exact-length global
blob. The diagnostic deliberately checks metadata rather than rehashing the
archive or planning target and cannot be used as download, planning, apply, or
recovery authority. Every actual blob open, commit, and staging copy rehashes
the bytes. A partial is likewise never authority or an offline target. The
attempt survives failure or process exit, and an exact retry replaces it only
after the prior process lock is released. It is removed only after reviewed
graph retention. A real killed-process test proves planning-target active and
retained partial observation, exact Range resume, complete promotion, and
handoff to the reviewed graph without a diagnostic gap.

After an install or upgrade reaches a validated terminal outcome,
`a3s-use extension diagnose <publisher/name> --scope-kind <user|workspace>
--scope-id <id> --history --json` retains its
complete path-free download projection together with the rest of the operation
snapshot. The per-scope/package history keeps the newest 16 occurrences within
8 MiB and survives package removal. It is written before pending recovery
authority is deleted, while exact replay deduplicates the pair
`(operationId, planDigest)`. History observation remains zero-network and
read-only; cached target state inside an old snapshot is historical evidence,
not current cache or recovery authority.

## Prune

Preview the current usage first, then run a confirmed prune:

```bash
a3s-use registry cache prune \
  --registry-name packages \
  --cache-max-bytes 2147483648 \
  --cache-max-entries 2048 \
  --cache-min-free-bytes 536870912 \
  --yes \
  --json
```

Prune is zero-network and reports before/after usage plus removed source
observations, partials, and stale writes. Removed target bytes are released
logical references; prune never removes a global blob. Without `--yes`, it
makes no change. Missing, malformed, linked, non-regular, unowned, or
source-mismatched cache state fails closed.

## Failure response

| Error | Operator action |
| --- | --- |
| `use.extension.registry_download_failed` | Retry the same exact operation; a valid retained partial resumes automatically. Investigate proxy, redirect, Range, or Registry availability errors if it repeats. |
| `use.extension.registry_target_cache_policy_exceeded` | Increase the explicit byte/entry policy or select a smaller signed target. |
| `use.extension.registry_target_cache_storage_insufficient` | Free space on the reported staging/cache volume or reduce other retained cache data. |
| `use.extension.registry_target_cache_invalid` | Quarantine the Registry datastore and investigate unexpected or tampered entries. |
| `use.artifact_store.blob_invalid` | Preserve the digest container, run the full digest audit, and use exact-plan quarantine only if it yields a complete mismatch. Do not overwrite content in place. |
| `use.artifact_store.ownership_invalid` | Inspect the Artifact Store directory chain for links, reparse points, or unexpected ownership changes. |
| `use.artifact_store.quarantined` | Keep the preserved content and marker intact for incident review. Do not bypass the Artifact Store path or overwrite the digest in place; wait for a verified rehydration workflow. |
| `use.artifact_store.quarantine_not_required` | The fresh audit verified the content. Do not publish a corruption marker. |
| `use.artifact_store.quarantine_not_auditable` | The digest container is absent or incomplete. Preserve staging evidence and investigate; no complete mismatch exists to confirm. |
| `use.artifact_store.quarantine_plan_mismatch` | Content or reviewed evidence changed. Re-run planning and obtain fresh explicit confirmation; never replay the stale digest blindly. |
| `use.artifact_store.quarantine_state_invalid` | Preserve the container for incident review. A marker is malformed, noncanonical, unsafe, conflicting, or interrupted; do not bypass it or treat it as deletion authority. |
| `use.artifact_rehydration.referenced` | Retire every Registry observation, installation snapshot or receipt, pending graph, and nonterminal lifecycle operation for the exact artifact, then create a fresh plan. |
| `use.artifact_store.rehydration_not_quarantined` | Run digest audit and exact-plan quarantine first; a candidate or digest path alone grants no replacement authority. |
| `use.artifact_store.rehydration_candidate_mismatch` | Preserve the candidate, investigate provenance or drift, and plan again from independently verified bytes outside the Artifact Store. |
| `use.artifact_store.rehydration_plan_mismatch` | The candidate, quarantine evidence, or reviewed plan changed. Obtain a fresh zero-reference plan and explicit confirmation. |
| `use.artifact_store.rehydration_state_invalid` | Keep access closed and preserve the container. Prepared/completed records are malformed, moved, conflicting, or inconsistent with the quarantine record. Retry only the exact reviewed operation after investigating state. |
| `use.artifact_store.quota_exceeded` | Review current/projected logical bytes and containers. Increase the global policy through revision CAS or complete a future confirmed global cleanup before retrying. |
| `use.artifact_store.quota_config_invalid` | Stop publishers, preserve the malformed `storage-quota.acl` or staging file outside the store for incident review, remove only that proven invalid state, then recreate policy through the typed API. |
| `use.artifact_store.quota_revision_conflict` | Read the current policy and review the mutation again; never retry a stale change blindly. |
| `use.artifact_store.quota_busy` | Retry after the active global artifact publication or policy change releases the storage boundary. |
| `use.extension.catalog_cache_invalid` | Quarantine the affected identity datastore. Restore a verified backup of that exact source identity, or replace the source and repopulate its isolated new datastore. |
| `use.extension.registry_sources_busy` | Retry after the process changing Registry source configuration releases the source lock. |
| `use.extension.registry_sources_revision_mismatch` | List sources again, review the new revision, and retry the confirmed mutation. |
| `use.extension.registry_source_disabled` | Enable the reviewed source or choose another enabled source explicitly. |
| `use.extension.registry_sources_invalid` | Quarantine the source ACL and managed root files; restore only a canonical verified backup or recreate the source explicitly. |
| `use.extension.registry_busy` | Retry after the active cache or metadata operation completes. |

Do not repair a blob by renaming arbitrary bytes to its expected digest. The
current write path deliberately refuses to replace corrupt global content.
`ArtifactStore::audit_digests` can now produce read-only mismatch evidence;
exact-plan logical quarantine can contain a reviewed complete mismatch without
moving its bytes. Verified rehydration is a separate zero-reference,
exact-plan operation and never follows from audit or quarantine evidence alone.
Source-cache pruning is not global Artifact Store GC, coordinated backup,
incident response, or whole-product recovery; those remain release gates.
