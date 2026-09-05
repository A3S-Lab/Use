//! Layout and payload validation for Capability Gateway state.
//!
//! The Gateway catalog and Control descriptor snapshots are immutable
//! projections, but they still need an explicit backup contract.  This
//! module keeps their on-disk namespace narrow and validates the canonical
//! bytes before they enter a coordinated archive.  Cryptographic trust of a
//! signed descriptor is intentionally deferred to the owner on replay: an
//! offline backup verifier has no current Registry trust policy.

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseResult};

use super::{
    state_backup_invalid, state_backup_layout_unsupported, state_backup_limit,
    MAX_STATE_BACKUP_FILE_BYTES,
};

const ROOT: &str = "capability-gateway";
const CATALOGS: &str = "catalogs";
const DESCRIPTOR_SNAPSHOTS: &str = "descriptor-snapshots";
const SHA256: &str = "sha256";
const STAGING: &str = ".staging";
const MUTATION_LOCK: &str = ".mutation.lock";
const RETENTION_JOURNAL: &str = ".retention.journal";
pub(super) const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Catalog,
    DescriptorSnapshot,
}

/// Validate one live filesystem entry in the Capability Gateway namespace.
/// Directories are accepted only when they are part of one of the two owner
/// layouts; record and operational files are classified separately by the
/// scanner after this check.
pub(super) fn validate_layout(path: &str, directory: bool) -> UseResult<()> {
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.first().copied() != Some(ROOT) {
        return Ok(());
    }

    let valid = match parts.as_slice() {
        [ROOT] if directory => true,
        [ROOT, CATALOGS] if directory => true,
        [ROOT, CATALOGS, SHA256] if directory => true,
        [ROOT, CATALOGS, STAGING] if directory => true,
        [ROOT, CATALOGS, MUTATION_LOCK | RETENTION_JOURNAL] if !directory => true,
        [ROOT, CATALOGS, SHA256, shard] if directory => valid_hex(shard, 2),
        [ROOT, CATALOGS, SHA256, shard, file] if !directory => {
            valid_record_name(file) && record_shard(file) == Some(*shard)
        }
        // A staging file is deliberately accepted by the layout pass so the
        // nonterminal pass can report it as in-flight evidence.
        [ROOT, CATALOGS, STAGING, _] if !directory => true,
        [ROOT, DESCRIPTOR_SNAPSHOTS] if directory => true,
        [ROOT, DESCRIPTOR_SNAPSHOTS, STAGING] if directory => true,
        [ROOT, DESCRIPTOR_SNAPSHOTS, MUTATION_LOCK] if !directory => true,
        [ROOT, DESCRIPTOR_SNAPSHOTS, file] if !directory => valid_record_name(file),
        [ROOT, DESCRIPTOR_SNAPSHOTS, STAGING, _] if !directory => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(state_backup_layout_unsupported(
            "The Capability Gateway state root contains an unknown or malformed payload path.",
        ))
    }
}

/// Return the owner kind for a durable content-addressed record path.
pub(super) fn record_kind(path: &str) -> UseResult<Kind> {
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [ROOT, CATALOGS, SHA256, shard, file]
            if valid_record_name(file) && record_shard(file) == Some(*shard) =>
        {
            Ok(Kind::Catalog)
        }
        [ROOT, DESCRIPTOR_SNAPSHOTS, file] if valid_record_name(file) => {
            Ok(Kind::DescriptorSnapshot)
        }
        _ => Err(state_backup_invalid(
            "A Capability Gateway backup entry is not an immutable payload record.",
        )),
    }
}

/// Return whether a Capability Gateway file is operational evidence that must
/// not enter a backup. Empty staging directories are harmless; a file inside
/// one means a publication was interrupted and must be recovered first.
pub(super) fn is_nonterminal(path: &str, directory: bool) -> bool {
    if directory {
        return false;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    parts.contains(&STAGING) || matches!(parts.as_slice(), [ROOT, CATALOGS, RETENTION_JOURNAL])
}

/// Validate canonical bytes for one immutable record.  The descriptor
/// snapshot owner performs structural and canonical validation here; current
/// trust-key/expiry verification remains an explicit replay-time operation.
pub(super) fn validate_bytes(
    path: &str,
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<()> {
    if bytes.is_empty()
        || bytes.len() > MAX_RECORD_BYTES
        || bytes.len() as u64 > MAX_STATE_BACKUP_FILE_BYTES
    {
        return Err(state_backup_limit(
            "A Capability Gateway payload exceeds its bounded record size.",
        ));
    }
    let kind = record_kind(path)?;
    let expected_digest = record_digest(path)?;
    match kind {
        Kind::Catalog => {
            let catalog = CapabilityGatewayCatalog::from_json(bytes).map_err(|_| {
                state_backup_invalid(
                    "A Capability Gateway catalog record is not valid canonical JSON.",
                )
            })?;
            if catalog.installation() != installation
                || catalog.canonical_bytes().map_err(|_| {
                    state_backup_invalid(
                        "A Capability Gateway catalog record failed canonical validation.",
                    )
                })? != bytes
                || catalog.descriptor_digest().map_err(|_| {
                    state_backup_invalid(
                        "A Capability Gateway catalog record digest could not be derived.",
                    )
                })? != expected_digest
            {
                return Err(state_backup_invalid(
                    "A Capability Gateway catalog record is foreign, noncanonical, or addressed by the wrong digest.",
                ));
            }
        }
        Kind::DescriptorSnapshot => {
            crate::control_store::validate_capability_descriptor_snapshot_backup_bytes(
                bytes,
                installation,
                &expected_digest,
            )
            .map_err(|error| {
                state_backup_invalid(format!(
                    "A Control descriptor snapshot record failed owner validation: {}",
                    error.message
                ))
            })?;
        }
    }
    Ok(())
}

fn valid_record_name(name: &str) -> bool {
    let Some(hex) = name.strip_suffix(".json") else {
        return false;
    };
    valid_hex(hex, 64)
}

fn record_shard(name: &str) -> Option<&str> {
    name.strip_suffix(".json").and_then(|hex| hex.get(..2))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn record_digest(path: &str) -> UseResult<String> {
    let name = path
        .rsplit('/')
        .next()
        .and_then(|value| value.strip_suffix(".json"))
        .ok_or_else(|| {
            state_backup_invalid("A Capability Gateway record has no canonical digest name.")
        })?;
    if !valid_hex(name, 64) {
        return Err(state_backup_invalid(
            "A Capability Gateway record has an invalid digest name.",
        ));
    }
    Ok(format!("sha256:{name}"))
}
