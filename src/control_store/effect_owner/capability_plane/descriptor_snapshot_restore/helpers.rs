//! Digests and error constructors for descriptor-snapshot clean-target restore.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseError, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::canonical_json;
use super::{
    ACTIVATION_SCHEMA, CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_RESULT_SCHEMA,
    ERROR_INVALID, ERROR_TARGET_NOT_EMPTY, INVENTORY_DOMAIN, MAX_ACTIVATION_BYTES,
    STAGING_PREFIX, ControlCapabilityDescriptorSnapshotRestoreEntry,
    ControlCapabilityDescriptorSnapshotRestorePlan,
    ControlCapabilityDescriptorSnapshotRestoreResult,
};

pub(crate) fn staging_directory(parent: &Path, plan_digest: &str) -> UseResult<PathBuf> {
    let hex = plan_digest
        .strip_prefix("sha256:")
        .filter(|value| valid_hex(value, 64))
        .ok_or_else(|| restore_invalid("The descriptor restore plan digest is invalid."))?;
    Ok(parent.join(format!("{STAGING_PREFIX}{hex}")))
}

pub(crate) fn restore_result(
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: String,
    changed: bool,
) -> UseResult<ControlCapabilityDescriptorSnapshotRestoreResult> {
    let result = ControlCapabilityDescriptorSnapshotRestoreResult {
        schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_RESULT_SCHEMA.to_owned(),
        installation: plan.installation.clone(),
        plan_digest,
        inventory_digest: plan.inventory_digest.clone(),
        changed,
        restored_record_count: plan.record_count,
        restored_byte_count: plan.byte_count,
    };
    result.validate()?;
    Ok(result)
}

pub(crate) fn inventory_digest(
    entries: &[ControlCapabilityDescriptorSnapshotRestoreEntry],
) -> UseResult<String> {
    let bytes = canonical_json(&entries, "descriptor snapshot restore inventory")?;
    let mut hasher = Sha256::new();
    hasher.update(INVENTORY_DOMAIN);
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| valid_hex(hex, 64))
}

pub(crate) fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

pub(crate) fn restore_target_not_empty() -> UseError {
    UseError::new(
        ERROR_TARGET_NOT_EMPTY,
        "The clean-target descriptor snapshot restore refuses to merge or replace an existing owner directory.",
    )
}

pub(crate) fn restore_io(action: &str, error: io::Error) -> UseError {
    UseError::new(ERROR_INVALID, format!("Failed to {action}: {error}"))
}
