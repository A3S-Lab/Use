//! Read-only Control Store inspection for installation package-graph authority.
//!
//! Consumers that must not open a full production lifecycle (capability index,
//! reachability, state backup) use these helpers. Callers must treat Control as
//! sole authority: reject legacy leaves before reading.

use std::path::Path;

use a3s_use_core::{InstallationId, InstallationSnapshot, UseResult};
use a3s_use_extension::StateMaintenanceGuard;

use super::export;
use super::production::reject_legacy_authority_paths;
use super::ControlStore;

/// Portable backup leaf for one canonical Control Store export.
pub(crate) const CONTROL_STORE_EXPORT_BACKUP_PATH: &str = "control-store-export.json";

/// Read the committed installation snapshot from Control, if any generation exists.
pub(crate) async fn read_current_installation_snapshot(
    state_root: &Path,
    installation: &InstallationId,
) -> UseResult<Option<InstallationSnapshot>> {
    reject_legacy_authority_paths(state_root)?;
    let store = ControlStore::new(state_root, installation.clone())?;
    Ok(store
        .current_generation()
        .await?
        .map(|generation| generation.snapshot))
}

/// Freeze one canonical Control export while the caller already holds exclusive
/// installation maintenance (for example coordinated state backup).
pub(crate) async fn export_control_store_under_exclusive(
    state_root: &Path,
    installation: &InstallationId,
    maintenance: &StateMaintenanceGuard,
) -> UseResult<(Vec<u8>, u64, String)> {
    reject_legacy_authority_paths(state_root)?;
    let store = ControlStore::new(state_root, installation.clone())?;
    let bytes = store.export_under_maintenance(maintenance).await?;
    let verified = export::verify(&bytes, installation)?;
    Ok((
        bytes,
        verified.export.current_generation,
        verified.descriptor_digest,
    ))
}

/// Offline-verify portable Control export bytes for one installation.
pub(crate) fn verify_control_store_export_bytes(
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<String> {
    let verified = export::verify(bytes, installation)?;
    Ok(verified.descriptor_digest)
}
