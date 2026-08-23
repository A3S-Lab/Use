# Registry Cache Operations

Status: development preview

Last updated: 2026-08-23

## Boundary

A3S Use stores online-verified package archives and separately signed
`planning-v1.json` targets at:

```text
<registry-datastore>/verified-targets/sha256/<digest>
```

The cache is an optimization and an explicit offline input. It is not package
installation state, a trust root, a receipt, or recovery authority. Removing a
cache entry cannot change a currently installed package or published capability,
but it can make a future offline install or upgrade unavailable.

## Durable source configuration

The standalone CLI stores at most 64 named sources in canonical
`<state-root>/registries.acl`. Each entry binds:

- stable lowercase source name;
- canonical HTTPS base URL (loopback HTTP is test-only);
- pinned bootstrap TUF root SHA-256;
- enabled state;
- whether an exact trusted-root file was imported; and
- the durable verified-target cache policy.

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
| Retained target bytes | 4 GiB |
| Retained target entries | 4,096 |
| Free-space reserve | 256 MiB |

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
configured byte bound and checks the temporary download filesystem for the
target plus the free-space reserve. The Registry cache separately reserves the
remaining bytes after any admitted partial plus the same reserve because the
cache and temporary staging directory may be different volumes.

Interrupted downloads are retained as
`.target-<sha256>.part`. The partial must be a bounded regular file and its
length becomes the next HTTP Range offset. A `206` response must describe the
exact offset, final byte, total signed length, and remaining content length. A
complete `200` response is accepted when a server ignores Range, but the old
partial is truncated first. No partial is staged or exposed as an offline
target. The complete bytes are re-read through the transaction-owned handle,
checked against the signed length and SHA-256, synchronized, and atomically
promoted to `<sha256>`. The final path is then reopened without following its
last component, rehashed, and retained as the only staging authority.

An existing partial is opened once without following its final path before
cache admission, and that exact handle remains the append/checkpoint authority.
New partial creation uses the same no-follow policy with create-new semantics.
On Windows the live handle shares read access for scanners and diagnostics but
denies external write, delete, and replacement access until the transaction
releases it after initial verification. A deterministic replacement in the
release-to-promotion window fails commit during the final-handle rehash. The
verified handle remains live through staging: Unix path replacement cannot
redirect the staged bytes, and Windows continues to allow readers while
denying external write, delete, and replacement access.

Windows promotion retries access-denied, sharing-violation, and lock-violation
rename failures for at most two seconds on a blocking worker. Native tests model
a read-only scanner handle that shares the transaction's existing read/write
access but withholds delete sharing. Releasing a transient handle lets the same
commit finish promotion and staging. If the handle persists through the bound,
commit returns `use.extension.io`, publishes no final target, and retains the
exact complete partial. After the scanner releases it, retry the same operation;
`begin` rehashes, promotes, and stages those retained bytes before any target
request. This is the verified-target promotion qualification, not a claim that
the full product antivirus and reboot matrix is complete.

Under the exclusive target-cache lock, commit removes:

1. bounded stale `.target-<pid>-<time>.tmp` writes;
2. the oldest inactive `.target-<sha256>.part` downloads;
3. the oldest verified targets, ordered by modification time and then digest;
4. only as many entries as required to satisfy byte, entry, and disk-space
   bounds.

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

The command performs no network request. Schema v2 JSON reports verified-target,
resumable-partial, and stale-write entry counts and bytes, available filesystem
bytes, and the effective policy. If a verified catalog-cache stamp exists, its
Registry name, URL, and trust-root digest must match the command.

## Observe a retained operation

`a3s-use extension diagnose <publisher/name> --json` correlates a retained
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
a retry or transfer is active. Ambiguous complete-plus-partial evidence, links
or reparse points, non-regular entries, and oversized partials fail closed.

`complete` is an exact-length observation at the location that only verified
promotion normally writes. The diagnostic does not rehash an archive or
planning target and cannot be used as download, planning, apply, or recovery
authority. A partial is likewise never authority or an offline target. The
attempt survives failure or process exit, and an exact retry replaces it only
after the prior process lock is released. It is removed only after reviewed
graph retention. A real killed-process test proves planning-target active and
retained partial observation, exact Range resume, complete promotion, and
handoff to the reviewed graph without a diagnostic gap.

After an install or upgrade reaches a validated terminal outcome,
`a3s-use extension diagnose <publisher/name> --history --json` retains its
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

Prune is zero-network and reports before/after usage plus removed target,
partial, and stale-write bytes. Without `--yes`, it makes no change. Missing,
malformed, linked, non-regular, unowned, or source-mismatched cache state fails
closed.

## Failure response

| Error | Operator action |
| --- | --- |
| `use.extension.registry_download_failed` | Retry the same exact operation; a valid retained partial resumes automatically. Investigate proxy, redirect, Range, or Registry availability errors if it repeats. |
| `use.extension.registry_target_cache_policy_exceeded` | Increase the explicit byte/entry policy or select a smaller signed target. |
| `use.extension.registry_target_cache_storage_insufficient` | Free space on the reported staging/cache volume or reduce other retained cache data. |
| `use.extension.registry_target_cache_invalid` | Quarantine the Registry datastore and investigate unexpected or tampered entries. |
| `use.extension.catalog_cache_invalid` | Quarantine the affected identity datastore. Restore a verified backup of that exact source identity, or replace the source and repopulate its isolated new datastore. |
| `use.extension.registry_sources_busy` | Retry after the process changing Registry source configuration releases the source lock. |
| `use.extension.registry_sources_revision_mismatch` | List sources again, review the new revision, and retry the confirmed mutation. |
| `use.extension.registry_source_disabled` | Enable the reviewed source or choose another enabled source explicitly. |
| `use.extension.registry_sources_invalid` | Quarantine the source ACL and managed root files; restore only a canonical verified backup or recreate the source explicitly. |
| `use.extension.registry_busy` | Retry after the active cache or metadata operation completes. |

Do not repair a target by renaming arbitrary bytes to their expected digest.
Refresh it from the exact trusted Registry or use an already verified backup of
the complete Registry datastore. Cache pruning is not coordinated backup,
incident response, or whole-product recovery; those remain release gates.
