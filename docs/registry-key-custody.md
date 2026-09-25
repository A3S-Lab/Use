# Registry key custody (operator procedures)

This document is the offline custody runbook for `a3s-use-registry-tools`.
It does not complete the A5 exit gate by itself: production expiry
monitoring, every-intermediate-root rotation drills against live mirrors,
and official bootstrap publication must still be exercised against
`A3S-Lab/Use-Registry`.

## Roles and material

| Role | Produced by | Published? | Custody |
| --- | --- | --- | --- |
| Root | `keygen` (offline) | Public keys + signed root metadata only | Offline; single share or threshold shares; seeds never in CI or client |
| Targets | `keygen` | Signed `targets.json` | Online (single key outside the git tree). Multi-signer TUF target delegation is deferred until independently signed package authorities are a real admission requirement. |
| Snapshot | `keygen` | Signed `snapshot.json` | Online operator or CI with short-lived access |
| Timestamp | `keygen` | Signed `timestamp.json` | Online operator or CI with short-lived access |

### Single-operator root (default)

`a3s-use-registry-tools keygen --keys-dir <dir>` writes one Ed25519 seed file
per role (`root.key`, `targets.key`, `snapshot.key`, `timestamp.key`) with
`0600` permissions.

### Threshold root ceremony

Enterprise staging and production roots SHOULD use threshold custody so one
compromised share cannot rewrite trust:

```bash
a3s-use-registry-tools keygen \
  --keys-dir <dir> \
  --root-share-count 3 \
  --root-threshold 2
```

This writes `root-0.key` … `root-2.key` plus `root.policy.json`
(`threshold` / `shareCount`). Distribute each share to a distinct offline
custodian. Online roles stay single-key.

Assemble requires at least `threshold` root shares present in `--keys-dir`.
When more shares are present, every present share signs the root metadata
(deterministic signature order). Clients pin only the bootstrap root SHA-256
printed by `assemble`; share files never leave offline custody.

## Root rotation (every intermediate retained)

A3S clients pin an exact bootstrap root digest. Rotating the published root
always requires an explicit Registry source replacement with the new pin;
GitHub redirects never become trust authority.

```bash
a3s-use-registry-tools keygen --keys-dir <next-keys> \
  --root-share-count 3 --root-threshold 2
a3s-use-registry-tools rotate-root \
  --registry <registry> \
  --previous-keys-dir <current-keys> \
  --next-keys-dir <next-keys>
```

`rotate-root` fail-closes unless `--previous-keys-dir` matches the published
root keyids (current custody). It writes root version N+1 signed by the next
root threshold, resigns online roles with the next keys, retains
`metadata/root.history/root.N.json`, and prints the new `rootSha256`.

## Mirror replacement

```bash
a3s-use-registry-tools compare-mirrors --left <primary> --right <mirror>
```

Requires identical bootstrap root digests and identical target path/digest
sets. Fail-closed with `registry_tools.mirror_mismatch` on drift. Run this
before promoting a failover mirror and after copying a staged tree.

## Expiry monitoring

```bash
a3s-use-registry-tools check-expiry --registry <registry> [--warn-within-hours 72]
```

Fails closed with `registry_tools.expiry_failed` when any role is past
`signed.expires`, or `registry_tools.expiry_warning` when any role expires
inside the warn window. Use this against staging and production mirrors
before clients refresh.

## Assemble and verify (fail-closed)

1. Lint each package: `a3s-use-registry-tools lint --package-dir <pkg>`.
2. Pack when reviewing archives: `pack --package-dir <pkg> --out <archive.tar.gz>`.
3. Gather at least `threshold` root shares offline into one keys directory
   that also holds the online role seeds.
4. Assemble the staged tree offline:
   `assemble --keys-dir <keys> --admissions <file.acl> --out-root <registry>`
   with optional `--description-trust-store` for
   `capability/description-trust-store-v1.json`.
5. Record the printed bootstrap root SHA-256 as the independently obtained pin.
6. Verify with the released client loader:
   `verify --registry <registry> --expected-root-sha256 <pin>`.

Any digest mismatch, missing target, under-threshold root custody, or
unexpected root fails closed. Do not publish a tree that fails `verify`.

## Offline recovery and rollback

- Restore keys from the offline root backup (all threshold shares still
  required to meet policy) before resigning metadata.
- Rebuild the staged tree from the same admissions and package directories;
  `assemble` is deterministic for identical inputs and identical key material.
- Clients that already pin an older root digest must receive an explicit
  source replacement (`a3s-use registry source replace`) with the new pin;
  GitHub redirects never become trust authority.
- Emergency withdrawal: use `withdraw-targets` (below), stop serving the old
  `registry/` tree until the resigned tree is promoted, and require clients to
  refresh before install.

## Emergency withdrawal

```bash
a3s-use-registry-tools withdraw-targets \
  --registry <registry> \
  --keys-dir <online-keys> \
  --target <relative/target/path> \
  [--target <path> ...] \
  [--metadata-expires <rfc3339>]
```

Removes the named signed targets and their on-disk bytes, resigns
`targets` / `snapshot` / `timestamp` with the current online keys, and leaves
the bootstrap root pin unchanged. An empty package catalog after full
withdrawal still verifies against the same pin. A second withdraw of the same
path fails closed with `registry_tools.withdraw_failed`. Clients must refresh
before install; do not treat GitHub redirects as trust authority.

## What remains to exercise for A5 GA

Exercised in-tree:

- `keygen` → `assemble` → `verify` round-trip
  (`skill_package_assembles_and_verifies` and related registry-tools tests).
- Offline recovery: wipe the staged tree, rebuild with the same keys and
  admissions, confirm the bootstrap pin is reproduced
  (`offline_custody_recovery_rebuilds_the_same_bootstrap_pin`).
- Threshold root ceremony (2-of-3 keygen + assemble + verify) and
  under-threshold fail-closed
  (`threshold_root_ceremony_assembles_and_verifies_with_two_of_three_shares`,
  `threshold_assemble_fails_closed_when_too_few_root_shares_are_present`).
- Every-intermediate-root rotation with retained prior root and mandatory new
  bootstrap pin
  (`root_rotation_retains_the_previous_root_and_requires_a_new_bootstrap_pin`,
  `root_rotation_fails_closed_when_previous_keys_do_not_match_published_root`).
- Expiry monitoring (`check-expiry`) for fresh trees and near-expiry fail-closed
  (`check_expiry_passes_for_a_freshly_assembled_registry`,
  `check_expiry_fails_closed_when_metadata_expires_inside_the_warn_window`).
- Mirror replacement compare (`compare-mirrors`) for identical trees and
  drift fail-closed
  (`compare_mirrors_accepts_identical_trees_and_rejects_drift`).
- Emergency withdrawal (`withdraw-targets`) keeps the bootstrap pin, removes
  target bytes, and still verifies an empty catalog
  (`withdraw_targets_removes_a_package_while_keeping_the_bootstrap_pin`).
- Use-Registry staging CI gate with real assemble+verify+expiry when admissions
  and environment-injected keys are present
  (`registry-staging-gate.yml`, `docs/staging-ci.md`).

Still open against `A3S-Lab/Use-Registry` production operation (cannot close
inside the Use crate alone):

- Official production bootstrap root publication and client pin distribution.
- Live production mirror promotion and incident response drills.
- Witness + SBOM retention outside the mutable delivery boundary for
  production promotions.

In-repo scaffolding that fail-closes until those ops exist:
`use-registry/.github/workflows/registry-production-gate.yml` and
`docs/production-ci.md` (no keys in git; assemble+verify only with
environment-injected `REGISTRY_PRODUCTION_KEYS_DIR`).
