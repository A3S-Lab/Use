# Registry Cache Operations

Status: development preview

Last updated: 2026-08-08

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

## Default policy

The standalone CLI and `TrustedRegistry::new` use these per-Registry defaults:

| Bound | Default |
| --- | ---: |
| Retained target bytes | 4 GiB |
| Retained target entries | 4,096 |
| Free-space reserve | 256 MiB |

Embedding hosts may supply a validated `VerifiedTargetCachePolicy`.
`install`, `upgrade`, and `registry cache` accept matching standalone overrides:

```text
--cache-max-bytes <unsigned-decimal-bytes>
--cache-max-entries <unsigned-decimal-count>
--cache-min-free-bytes <unsigned-decimal-bytes>
```

CLI overrides apply to that invocation. A host that requires a persistent
non-default policy must supply the same typed policy whenever it constructs the
Registry until durable Registry source configuration is completed.

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
target. The complete bytes are re-read, checked against the signed length and
SHA-256, synchronized, and atomically promoted to `<sha256>`.

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
  --registry-url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --json
```

The command performs no network request. Schema v2 JSON reports verified-target,
resumable-partial, and stale-write entry counts and bytes, available filesystem
bytes, and the effective policy. If a verified catalog-cache stamp exists, its
Registry name, URL, and trust-root digest must match the command.

## Prune

Preview the current usage first, then run a confirmed prune:

```bash
a3s-use registry cache prune \
  --registry-name packages \
  --registry-url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
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
| `use.extension.catalog_cache_invalid` | Restore the exact Registry source identity or intentionally replace/reset that Registry state. |
| `use.extension.registry_busy` | Retry after the active cache or metadata operation completes. |

Do not repair a target by renaming arbitrary bytes to their expected digest.
Refresh it from the exact trusted Registry or use an already verified backup of
the complete Registry datastore. Cache pruning is not coordinated backup,
incident response, or whole-product recovery; those remain release gates.
