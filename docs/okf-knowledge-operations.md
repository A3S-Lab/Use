# OKF Knowledge Operations

Status: development preview

This runbook covers the scope-local SQLite/FTS5 backend embedded in standalone
A3S Use. It defines the implemented audit, backup, repair, and authority-bound
database plus missing-binding restore operations, plus the recovery authority
they deliberately do not create.

## Safety boundary

One complete User or Workspace scope owns one Knowledge database. Every
operation binds both scope kind and scope ID. Equal textual IDs in different
scope kinds never share a database, lock, backup, audit, or repair operation.

The Knowledge database contains package-owned OKF projections, their cited
search fields, and the original bounded Markdown bytes needed for an exact
read. It does not own:

- Registry trust roots, TUF metadata, or installed package receipts;
- immutable package roots or lifecycle journals;
- Workspace Grants or confirmation evidence;
- Flow history, UI storage, or personal notes; or
- the durable Knowledge binding records used by capability publication.

A Knowledge backup is therefore necessary but not sufficient for whole-product
recovery. Its embedded SHA-256 detects corruption; it is not a publisher
signature and cannot establish package authority by itself.

## Inspect storage

Every Knowledge command requires an explicit installation. For example, inspect
one User installation:

```bash
a3s-use knowledge usage \
  --scope-kind user \
  --scope-id user/alice \
  --json
```

Use one exact Workspace scope:

```bash
a3s-use knowledge usage \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

The report contains retained projection and tombstone counts, receipt-accounted
expanded bytes, configured limits, allocated SQLite bytes, and reclaimable
bytes. It does not read or print concept content.

## Audit a scope

```bash
a3s-use knowledge audit \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Audit holds the selected installation's shared lock and verifies:

1. the current SQLite `user_version`;
2. bounded SQLite integrity output;
3. foreign-key integrity;
4. every retained receipt, row identity, digest, and complete scope;
5. every retained document's non-empty UTF-8 bytes, immutable byte bound, and
   source SHA-256;
6. the complete derived search descriptor against the retained projection;
7. storage accounting and hard row bounds; and
8. FTS5 integrity against the authoritative document table.

Audit fails closed. It does not migrate an unknown preview database or infer a
replacement receipt.

## Exact-generation search and read

Embedding hosts acquire `OkfKnowledgeLease` from
`OkfKnowledgeLeaseProvider` using one complete `OkfCapabilityProjection`. The
provider accepts the request only when the exact package, manifest, lifecycle
generation, and OKF bundle are still published by the Registry. The retained
generation lease participates in lifecycle drain.

Search returns `a3s.use.okf-knowledge-citation.v1` citations. A subsequent
`a3s.use.okf-knowledge-read-request.v1` must present the unchanged projection,
citation, scope, path, concept, generation, and caller-selected byte ceiling.
The SQLite host rechecks the promoted projection, retained source digest,
actual Markdown SHA-256, UTF-8 validity, and byte count before returning
`a3s.use.okf-knowledge-read-response.v1`.

A cutover or hide rejects new leases for the prior generation. Work already
holding that generation may finish search/read while retirement waits for its
generation lock. Installed package drift and retained database content drift fail
closed. These typed APIs do not expose an Ontology review graph, source tree,
or an unbounded package filesystem reader.

## Create a backup

Choose a new destination file:

```bash
a3s-use knowledge backup ./workspace.a3s-okf-backup \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Backup takes the scope's exclusive lock, audits the live database, checkpoints
the WAL, creates a consistent compact SQLite snapshot, audits the snapshot, and
writes a fixed header, bounded manifest length and SHA-256, versioned manifest,
and raw database bytes to one file. The manifest binds:

- schema `a3s.use.okf-knowledge-backup.v1`;
- complete scope kind and ID;
- creation timestamp;
- exact database length and SHA-256; and
- exact storage accounting and policy limits.

The command uses an adjacent temporary file, synchronizes it, and publishes it
without overwrite. If the destination already exists, the command returns
`use.okf.knowledge_backup_exists`. Choose another path or verify the existing
file; never delete an unverified backup merely to make a retry succeed.

## Verify a backup

Verification is offline and never changes live Knowledge state:

```bash
a3s-use knowledge verify-backup ./workspace.a3s-okf-backup \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Verification rejects symlinks and non-regular files, unknown schemas, oversized
manifests or databases, length/digest mismatch, scope substitution, unsupported
SQLite schema, invalid receipts, inconsistent storage evidence, and FTS
corruption. Store the returned manifest with the operator's broader backup
inventory.

## Review and apply backup retention

Plan bounded retention for one owned directory and exact scope. The defaults
retain at most 32 backups and 256 GiB:

```bash
a3s-use knowledge backup-retention ./backups \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

The command holds the same directory lock used by final backup publication,
scans at most 4,096 entries, and fully verifies every regular
`*.a3s-okf-backup` candidate before it can enter the plan. Verified backups for
another scope and unrelated files remain outside the inventory. A malformed,
linked, or oversized managed candidate fails closed. The canonical
`a3s.use.okf-knowledge-backup-retention-plan.v1` result orders candidates by
manifest creation time and file name, then selects only the oldest prefix
needed to satisfy both `--max-backups` and `--max-bytes`. It never selects the
last verified backup for the scope.

Review the relative file names, digests, timestamps, byte counts, limits, and
`planDigest`. Apply only that unchanged digest:

```bash
a3s-use knowledge backup-retention ./backups \
  --max-backups 32 \
  --max-bytes 274877906944 \
  --plan-digest sha256:<reviewed-plan-digest> \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --yes \
  --json
```

Apply re-verifies the complete scope inventory under the directory lock before
removing anything. A new, missing, or changed backup rejects the stale plan.
Deletion is oldest-first and directory-synced. A partial filesystem failure
returns `use.okf.knowledge_backup_retention_outcome_unknown` with the exact
already-removed entries; inspect and verify the directory, then create a new
plan. Never recreate a removed path or assume that an error rolled deletion
back. This rotates only verified scope-local database artifacts; it does not
rotate restore rollback directories or any other Use-owned state family.

## Repair the derived search index

If audit reports `use.okf.knowledge_search_index_invalid`, rebuild only the
derived FTS5 index:

```bash
a3s-use knowledge repair-search-index --yes \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

Repair first validates SQLite integrity, foreign keys, retained receipts,
scope identity, projection rows, original document bytes and source digests,
the immutable search descriptor, and storage accounting. It then rebuilds FTS
from the already-validated `knowledge_documents` table and reruns the complete
audit. It does not change projection receipts, lifecycle state, selected
generations, package roots, bindings, or Grants.

Do not use this command for `use.okf.knowledge_database_invalid`, missing
receipts, unknown `user_version`, or package/lifecycle corruption. Those cases
require recovery from exact external authority or cleanup and reinstall; repair
must never invent evidence.

## Plan a database restore

Never copy a SQLite file into the live state directory. First create a
path-free review from one verified backup and the complete retained authority:

```bash
a3s-use knowledge plan-restore ./workspace.a3s-okf-backup \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

The command changes no live Knowledge state. It verifies the archive offline,
then holds the selected installation's maintenance fence while it proves all of
the following:

- the backup policy exactly equals the configured storage policy;
- the current durable binding set is an exact subset of the backup projection
  inventory; any changed, conflicting, or newer binding fails closed;
- no projection is staged or failed, and every backup selection matches its
  exact retained promoted projection;
- selected generations are still published and leased in the Registry, while
  retained unselected generations still have immutable package receipts and
  bundle identity;
- every package lifecycle journal is terminal and the Registry has no pending
  cutover;
- permission-bearing selected packages have the exact active Grant and retired
  generations have the exact revoked Grant; and
- Registry authority remains stable across validation.

The `a3s.use.okf-knowledge-restore-plan.v2` plan also binds the exact current
binding-inventory digest and missing-binding count plus the current main
database, WAL, and SHM lengths and SHA-256 values. Integrity is audited against
a private copy so planning never checkpoints or rewrites the live database.
`status: "no-change"` is returned only when the healthy live main file is
byte-identical to the verified backup, no sidecar exists, and no binding is
missing. Store the returned `planDigest` with the review.

## Apply or resume the reviewed restore

Apply only the exact digest returned above:

```bash
a3s-use knowledge restore ./workspace.a3s-okf-backup \
  --plan-digest sha256:<64-lowercase-hex-digits> \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --yes \
  --json
```

Apply re-verifies the backup, the exact-subset binding state, and all package,
lifecycle, Registry, and Grant authority under one exclusive cross-process
maintenance fence. Any drift returns
`use.okf.knowledge_restore_plan_mismatch` or an authority-specific error before
a restore operation is created. Missing binding records are recreated only
from the audited backup inventory; existing files are never overwritten or
removed.

A required `a3s.use.okf-knowledge-restore-operation.v2` advances through
`planned`, `staged`, `bindings-restored`, `prior-moved`, `published`, and
`completed`. The verified candidate is synced before mutation. Exact missing
bindings are created and synced before the database cutover checkpoint. The
exact prior main database, WAL, and SHM are moved into a scope- and
plan-digest-owned restore directory before the candidate is renamed into the
live path. Every binding write, rename, and journal update is directory-synced.
The selected Registry generation leases remain held through publication. The
terminal `a3s.use.okf-knowledge-restore-result.v2` reports the reviewed number
of restored bindings.

If the process exits, rerun the same command with the same scope and plan
digest. A durable active marker blocks direct Registry, Workspace Grant,
lifecycle-journal, Knowledge binding, and Knowledge database mutation with
`use.state.maintenance_restore_active` until replay converges. These public
stores enforce the same maintenance fence at their lowest mutation layer, in
addition to the complete-operation guards held by lifecycle coordinators. The
last immutable Registry snapshot remains readable but cannot perform crash
reconciliation or write a new generation while exclusive maintenance is
active. Once `staged` is durable, replay can use the exact retained candidate
even if the external backup path is temporarily unavailable. It never accepts
a different valid backup under the same operation. Terminal replay revalidates
the published and retained file digests without rewriting them.

Each scope retains at most 32 restore-operation directories. This preview does
not silently rotate prior databases; it fails with
`use.okf.knowledge_restore_retention_required` so an operator cannot lose the
only rollback evidence through automatic cleanup. Do not edit the active
marker, journal, candidate, or retained prior files manually.

## Inspect restore status

Inspect recovery without supplying the backup path or reviewed plan digest:

```bash
a3s-use knowledge restore-status \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --json
```

The `a3s.use.okf-knowledge-restore-diagnostic.v2` result reports any active
restore for the selected installation, including its exact installation, plan
digest, and current `planned`,
`staged`, `bindings-restored`, `prior-moved`, `published`, or `completed`
phase. It also returns the reviewed binding-state digest and missing-binding
count, requested scope's canonically ordered history, retained
operation-directory count, directories created during the marker-to-journal
handoff, the fixed limit of 32, and remaining capacity. Each summary is
path-free and contains only bounded backup, authority, Registry generation,
projection, prior-file, and timing evidence.

The command takes the exclusive maintenance fence to make the marker and
journal view coherent. It does not rewrite the live database, marker, journal,
candidate, or retained prior files. Use it to recover the exact plan digest,
then rerun `knowledge restore` with that digest and the original scope. It is
not a cleanup command: restore rollback review/removal and whole-product
retention policy remain operator and release gates.

## Recovery boundary

The implemented command restores the scope-local Knowledge database and only
those Knowledge binding files absent from an otherwise exact subset of the
backup inventory. It requires Registry receipts, immutable package roots,
lifecycle journals, and Grants to be present and exact; the unsigned database
backup cannot recreate that authority. Conflicting or newer bindings are also
not repaired. Copying only the database backup to a clean machine is therefore
insufficient.

Before a supported product release, the project must still provide and test:

- recovery from conflicting/corrupted binding evidence and missing or
  corrupted lifecycle, Registry, package, and Grant evidence;
- cross-platform clean-machine drills for coordinated whole-installation
  restore and retention of its explicit rollback archives;
- encryption and operator access policy where backup content is sensitive;
- clean-machine restore and disaster-recovery drills on every supported target;
- incident escalation and support ownership; and
- signed release artifacts and independently reproducible installation.

Until those gates pass, A3S Use remains a development preview.
