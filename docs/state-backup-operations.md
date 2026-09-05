# Coordinated A3S Use State Backup Operations

## Purpose

The coordinated backup captures one integrity-verifiable inventory of all
currently supported A3S Use-owned control-state families for one explicit User
or Workspace installation. It supports corruption detection, retention by an
external backup system, and reviewed same-version/OS/architecture recovery of
that exact installation.

It does not migrate, sign, encrypt, or upload state. A backup never authorizes
its own restore and cannot recreate missing Registry projection, installed
receipt, package, lifecycle, or Grant authority. Restore requires that exact
installation authority to remain live and equal to the evidence captured by
the archive. Global Registry source configuration, trust roots, TUF state, the
global Artifact Store and its durable quota policy, and derivable caches are
outside this backup boundary. Consequently the archive is not a self-contained
package backup:
restore requires every receipt-referenced artifact to remain present and exact.

## Commands

Create a backup outside `A3S_USE_HOME`, `A3S_DATA_HOME`, and `A3S_STATE_HOME`:

```bash
a3s-use state backup ./backups/use-state.a3s-use-state-backup \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Verify it without a Registry, network access, or the original Use home:

```bash
a3s-use state verify-backup \
  ./backups/use-state.a3s-use-state-backup \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Review retention without deleting anything:

```bash
a3s-use state backup-retention ./backups \
  --max-backups 32 \
  --max-bytes 2199023255552 \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Apply only the unchanged reviewed plan:

```bash
a3s-use state backup-retention ./backups \
  --max-backups 32 \
  --max-bytes 2199023255552 \
  --plan-digest sha256:<reviewed-plan-digest> \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --yes \
  --json
```

Creation never overwrites an existing destination. Choose a new immutable name
for every retained backup. Verification is read-only and never extracts bytes.

Review a whole-installation restore without changing state:

```bash
a3s-use state plan-restore \
  ./backups/use-state.a3s-use-state-backup \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

After independently reviewing the path-free actions and retaining the returned
`planDigest`, apply the exact plan with a separate external rollback archive:

```bash
a3s-use state restore \
  ./backups/use-state.a3s-use-state-backup \
  --rollback-backup ./backups/pre-restore-rollback.a3s-use-state-backup \
  --plan-digest sha256:<reviewed-plan-digest> \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --yes \
  --json
```

Inspect active and retained restore evidence without a backup path or write:

```bash
a3s-use state restore-status \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

## Consistency boundary

Creation takes the exact installation's exclusive state-maintenance lock.
Ordinary Registry, package, Grant, lifecycle, Runtime, Flow, and Knowledge
operations for that installation use the shared side of the same lock, so a
backup cannot overlap a compliant mutation. A different installation is an
independent consistency domain and remains available.
Backup publication also holds the external destination-directory lock shared
with retention planning/apply, so a reviewed directory inventory cannot race
with a compliant archive publication.

The creator then performs three checks under that fence:

1. Scan and hash the complete allowlisted inventory.
2. Copy every file in canonical order while rechecking its length and SHA-256.
3. Scan and hash the complete inventory again and require exact equality.

The destination is published only after the staging file is synchronized and
the two inventories match. A destination race fails instead of overwriting the
winner.

## Included state families

| Family | Owned paths |
| --- | --- |
| Registry | installation receipts and `registry.json` |
| Retained generations | `state/extension-generations/` |
| Grants | active/revoked receipts, snapshots, and terminal Grant journals |
| Bindings | Runtime, Flow, and Knowledge bindings |
| Lifecycle operations | terminal package lifecycle checkpoint records |
| Package operations | terminal resolution diagnostics and retained operation history; pending graph/download records are rejected |
| Knowledge | SQLite main/WAL/SHM files and terminal restore evidence |
| Installation snapshot | `state/installation-snapshot.json` |
| Package enablement | snapshot-bound recovery projections and completed enablement operations |
| Host Manager | reviewed requests, observations, cancellations, and outcomes |

Cross-process lock files and generation lease files are excluded. Empty directories
do not create manifest entries.

Global `registries.acl`, Registry trust roots, TUF metadata, verified target and
planning caches, the global Artifact Store under `data/artifacts` including its
`storage-quota.acl` resource policy, and the Flow compiled-artifact cache are
deliberately excluded. They are shared or derivable inputs, not
mutable authority owned by one installation. Global reachability inventory
joins every installation and durable operation with physical evidence and
checked usage, but unreferenced artifacts remain retained until audit and
confirmed deletion are implemented.

## Archive format

The binary archive is deterministic for unchanged state on the same OS and
architecture:

```text
"A3S-USE-STATE-BACKUP-V1\n"
8-byte unsigned big-endian canonical-manifest length
32 raw bytes of SHA-256(canonical manifest)
canonical JSON manifest
file payload bytes concatenated in manifest order
```

The fixed `V1` header is the binary framing version. The current
`a3s.use.state-backup.v2` manifest records:

- the exact installation kind and ID;
- the producing Use version, OS, and architecture;
- the state-root discriminator plus a portable relative path for every file;
- exact length, SHA-256, read-only bit, and Unix mode where available;
- total and per-family file/byte accounting and inventory digests;
- the published Registry generation and projection digest; and
- sorted installed package IDs with canonical receipt digests.

The `CapabilityPayloads` family is reserved for the two immutable Capability
Gateway owner layouts: sharded canonical catalog records under
`capability-gateway/catalogs/sha256/` and content-addressed Control descriptor
snapshots under `capability-gateway/descriptor-snapshots/`. Inventory and
archive verification re-parse these records, enforce the installation binding,
and recompute each content address. The derived Capability Index is not
portable authority. The current trust policy for signed descriptors is
revalidated by the owner during replay; offline backup verification does not
invent or persist a new trust decision.

Absolute source or destination paths and creation time are absent. The payloads
are the original files, however, and may contain configured paths, endpoints,
or other sensitive operational data. Protect the complete archive as a secret
backup artifact even though the manifest is path-free.

## Fail-closed admission

Creation rejects:

- a durable active restore marker or pending Registry cutover;
- applying/rolling-back lifecycle or Grant journals;
- pending package graph/download/resolution work, active enablement, or Runtime
  Service provisioning evidence;
- atomic `.tmp`, `.partial`, and other nonterminal state entries;
- unknown Capability Gateway paths, mutation locks, staging files, or catalog
  and descriptor-snapshot retention journals (the immutable payload family
  admits only canonical records);
- any installation data payload or unknown state family;
- absolute, parent-traversing, non-UTF-8, Windows-reserved, case-colliding, or
  otherwise non-portable paths;
- symbolic links, Windows reparse points, sockets, devices, and other special
  files; and
- more than 200,000 filesystem entries or 100,000 files, 64 GiB total payload,
  16 GiB per file, a 16 MiB manifest, a 1,024-byte relative path, or 32
  directory levels.

These failures mean the operator must finish or recover the exact existing
operation. Deleting pending evidence merely to make backup succeed is not a
recovery procedure.

## Coordinated retention

Only regular files ending in `.a3s-use-state-backup` are managed. Planning
holds the same external-directory lock used for backup publication and fully
verifies the canonical manifest, archive length, and every payload of every
managed candidate. A malformed archive, link/reparse point, special file,
non-portable file name, overlapping live Use root, or directory with more than
4,096 inspected entries fails closed.

The path-free `a3s.use.state-backup-retention-plan.v2` plan binds the selected
installation and records only each matching archive's portable file name,
exact modification time in nanoseconds, archive length, canonical manifest
digest, inventory digest, Registry generation and digest, file count, and
payload bytes. Verified archives for another installation are never selected.
Modification time establishes the oldest-first order because the deterministic
backup manifest deliberately has no clock value. The complete reviewed
inventory and policy are covered by the canonical plan digest.

Apply reacquires the directory lock, rebuilds and verifies the complete
inventory, and requires the exact reviewed digest. It preflights every removal
candidate before deleting the first file, synchronizes the directory after
each deletion, and verifies the retained inventory afterward. The bounded
policy accepts 2 through 4,096 backups and up to 256 TiB, defaults to 32
backups and 2 TiB, and never removes either of the newest two verified recovery
generations. A concurrent publication, changed modification time, new archive,
renamed archive, tampered payload, or policy change makes the plan stale.
Partial deletion is reported as outcome-unknown with the exact removed entries;
verify the directory and create a new plan instead of recreating a name by
assumption.

## Reviewed whole-installation restore

`state plan-restore` fully verifies the archive, requires its exact selected
installation, current Use version, OS, and architecture, scans that
installation's live state under the exclusive maintenance fence, and validates
independent authority. Its canonical
`a3s.use.state-restore-plan.v1` output contains no archive, rollback, data-root,
or state-root path. It classifies every allowlisted file as Add, Replace,
Remove, or Retain and binds the backup manifest, before/after inventories,
authority, byte/file accounting, and the complete ordered action list.

Registry and Grant files are always Retain actions. Planning and apply fail if
the live Registry projection, installed receipts, Registry-owned files, or
Grant files differ from the backup. This deliberately prevents a compromised
or stale archive from restoring the authority needed to trust itself.

Confirmed apply re-verifies all evidence under the same exclusive fence. It
refuses archive paths inside Use-owned roots and requires a distinct external
rollback destination. Before publishing the active marker or changing live
state, it either creates a non-overwriting coordinated rollback archive of the
reviewed live inventory or verifies an existing archive as an exact match.
A mismatched rollback archive is never replaced.

Restore apply enters global Artifact Store reference admission before taking
the installation maintenance lock and holds it through publication and resume.
An exclusive global collector therefore cannot miss references introduced by a
restored snapshot, receipt, or nonterminal operation.

Only Add and Replace payloads are staged beneath digest-named hidden candidate
roots. Candidate and publication traversal rejects symbolic links, Windows
reparse points, special files, unknown entries, incomplete inventory, changed
content, and changed mode/read-only evidence. Remove and Replace targets must
still match their reviewed prior evidence. Publication preserves the reviewed
Unix mode and read-only attribute.

The durable `a3s.use.state-restore-operation.v1` journal advances in this exact
order:

```text
planned → staged → publishing → published → candidates-removed
        → verified → completed
```

The active marker is durable before its journal or live mutation. A marker-only
handoff reconstructs only the exact plan and external rollback evidence. Every
file publication/removal, candidate cleanup, and journal boundary is
idempotent; subprocess tests terminate at 15 distinct checkpoints and resume
to the exact terminal inventory. Once candidates are durably staged, recovery
can continue if the external source archive is later lost. The rollback archive
was already fully verified and digest-bound before mutation and remains the
operator's explicit rollback artifact.

The marker is cleared last. Completed apply is terminally replayable and a
NoChange plan creates neither rollback archive nor journal. Marker/journal
substitution, a foreign standalone Knowledge restore marker, candidate-root
link/reparse replacement, stale live state, archive tampering, and mismatched
rollback evidence all fail closed.

`state restore-status` returns path-free
`a3s.use.state-restore-diagnostic.v1` evidence without acquiring the maintenance
lock or repairing temporary evidence. It reports the active phase, retained
operation summaries, unrecorded marker/pruning handoffs, and remaining capacity.
History retains at most 64 directories. Before the 65th operation, the oldest
validated completed record is moved through a recognizable durable pruning
tombstone; an interrupted prune is shown read-only and completed by exact
restore recovery, never by status inspection.

## Verification and operating procedure

For every retained backup:

1. Run `state verify-backup` immediately after creation.
2. Record the returned `inventoryDigest`, `registryGeneration`,
   `registryDigest`, file count, and byte count in an external inventory.
3. Store the archive in access-controlled, encrypted, immutable storage.
4. Re-run offline verification after transfer and during periodic restore
   drills.
5. Review `state backup-retention` output, independently retain its plan digest,
   and apply it only after confirming the selected archive names and limits.
6. Review `state plan-restore` output and independently retain its exact plan
   digest before any recovery window.
7. Choose a new external rollback destination, run confirmed `state restore`,
   and retain both the source and rollback archives until the restored product
   has passed application-level validation.
8. If interrupted, inspect `state restore-status` and resume only with the exact
   source path, rollback path, and reviewed plan digest. Never delete the marker,
   journal, or candidate roots to force progress.
9. Keep an independently controlled off-site copy outside this local retention
   directory and exercise verification on that copy.

Do not copy archive payloads directly into live Use directories. Direct copying
bypasses authority validation, rollback capture, candidate validation, durable
publication, and terminal verification.

## Remaining release gates

Before this becomes a complete production disaster-recovery facility, A3S Use
still needs:

- independent recovery or explicit loss handling for missing trust and Grant
  authority;
- Windows CI execution of the candidate-junction and read-only restore tests,
  cross-platform retention, and clean-machine recovery drills; and
- incident response, encryption/key custody, and support runbooks.

The Roadmap remains open until those gates are implemented and exercised.
