# Control Store and backup authority boundary

Status: development preview  
Applies to Use tip with Control Store as production mutable authority (ADR-003;
clean-state-only activation).

## Decision

One installation must have **one** mutable control authority. SQLite is the
engine; the architecture is the indivisible cutover, not “add a database beside
JSON files.”

```text
Control Store (mutable authority)     External payloads (bytes / observations)
─────────────────────────────────     ────────────────────────────────────────
graph / generations / Grants          Artifact Store (immutable package bytes)
reviewed operations / checkpoints     registries.acl (host Registry config)
provider bindings / capability gen    registry-trust-roots
effect outbox identities              remote-registries verified-targets (cache)
                                      OKF SQLite content
                                      Host / observation / restore archives
```

Verified-target cache and transport URLs are **never** install or recovery
authority.

## Ownership classes (cutover inventory)

Frozen in [`control-store-cutover.acl`](control-store-cutover.acl) and explained
in [`control-store-cutover.md`](control-store-cutover.md):

| Class | Role | Backup |
| --- | --- | --- |
| Legacy authority | Today’s JSON/file selectors of desired state | Must move into Control; old paths deleted at cutover |
| External owner | Typed bytes/observations that must not choose desired state | Snapshot via owner registry; path-free receipts |
| Operational state | Locks, leases, derived indexes, active restore markers | **Excluded** from portable backup |
| Cutover consumer | Readers/writers of the above | Must switch with no fallback in one change |

## Relation to today’s `state_backup` allowlist

`state_backup/inventory.rs` still retains a legacy filesystem allowlist for
pre-Control installations. When `control.sqlite3` is present:

1. Mutable control evidence is the verified `control-store-export.json` leaf
   (schema-derived Control export), not a live SQLite copy.
2. Legacy authority paths beside Control are rejected (`legacy_state_unsupported`).
3. Portable inventory admits only registered `ControlPayloadOwnerId` live
   locations; unregistered layout families fail closed even when
   `installation_state_layout` still lists them.
4. Dual-write and legacy fallback reads remain forbidden (ADR-003).

Owner-native complete-set snapshot/restore remains the stronger portable
archive path and continues to converge with restore wiring.

## Current implementation posture

- Inactive kernel: `src/control_store/` (schema v11+, WAL, outbox, owner
  snapshot/restore adapters). Production lifecycle **does not** construct it.
- A0/A1 exit gates are checked on tip; A2 checkboxes stay open until the
  coordinated activation deletes legacy mutable authority.
- Local Registry HTTP transport remains orthogonal: Use-Registry serves bytes;
  Use pins `--trust-root` and verifies TUF.

## Host Registry config (outside installation cutover)

`registries.acl`, `registry-trust-roots/`, and `remote-registries/` remain on the
Use state root for backup, but they are **not** leaves of the installation
cutover inventory (`installation_state_layout`). They are host-selected Registry
configuration and cache, orthogonal to one installation’s mutable control
aggregate. Transport (GitHub raw / local HTTP) never becomes authority.
