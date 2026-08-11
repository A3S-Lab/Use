# OKF Knowledge Operations

Status: development preview

This runbook covers the scope-local SQLite/FTS5 backend embedded in standalone
A3S Use. It defines the operations that are implemented today and the recovery
authority they deliberately do not create.

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

Use the default User scope:

```bash
a3s-use knowledge usage --json
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
a3s-use knowledge audit --json
```

For a Workspace, include its exact `--scope-kind` and `--scope-id`. Audit holds
the scope's shared lock and verifies:

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
route lease participates in lifecycle drain.

Search returns `a3s.use.okf-knowledge-citation.v1` citations. A subsequent
`a3s.use.okf-knowledge-read-request.v1` must present the unchanged projection,
citation, scope, path, concept, generation, and caller-selected byte ceiling.
The SQLite host rechecks the promoted projection, retained source digest,
actual Markdown SHA-256, UTF-8 validity, and byte count before returning
`a3s.use.okf-knowledge-read-response.v1`.

A cutover or hide rejects new leases for the prior generation. Work already
holding that generation may finish search/read while retirement waits for its
route lock. Installed package drift and retained database content drift fail
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

## Recovery status

Restore is intentionally not implemented yet. A safe restore must coordinate
the database with exact Registry receipts, installed immutable package roots,
Knowledge bindings, lifecycle journals, and Grants. Copying a verified SQLite
snapshot into the live state directory would bypass that authority check and is
not a supported procedure.

Before a supported product release, the project must still provide and test:

- coordinated backup and restore for every Use-owned state family;
- recovery from missing or corrupted binding and lifecycle evidence;
- retention and rotation policy for backup artifacts;
- encryption and operator access policy where backup content is sensitive;
- clean-machine restore and disaster-recovery drills on every supported target;
- incident escalation and support ownership; and
- signed release artifacts and independently reproducible installation.

Until those gates pass, A3S Use remains a development preview.
