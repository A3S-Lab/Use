use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceGuard};

use super::{authority_mismatch, digest_entries};
use crate::state_backup::{StateBackupEntry, StateBackupFamily, StateBackupManifest};

pub(super) async fn validate_live_authority(
    paths: &ExtensionPaths,
    backup: &StateBackupManifest,
    live: &[StateBackupEntry],
    maintenance: &StateMaintenanceGuard,
) -> UseResult<String> {
    let state_root = paths.installation_state_root();
    if !crate::control_store::control_database_present(&state_root) {
        return Err(UseError::new(
            "use.state_restore_control_required",
            "Whole-installation restore planning requires an initialized Control Store.",
        )
        .with_suggestion(
            "Initialize Control Store for this installation state root before reviewing a backup.",
        ));
    }
    validate_live_control_authority(paths, backup, live, maintenance).await
}

async fn validate_live_control_authority(
    paths: &ExtensionPaths,
    backup: &StateBackupManifest,
    live: &[StateBackupEntry],
    maintenance: &StateMaintenanceGuard,
) -> UseResult<String> {
    let state_root = paths.installation_state_root();
    crate::control_store::reject_legacy_authority_paths(&state_root)?;

    let backup_authority = authority_entries(&backup.entries);
    let live_authority = authority_entries(live);
    if backup_authority != live_authority {
        return Err(authority_mismatch(
            "Live Registry or Grant authority differs from the coordinated backup.",
        ));
    }
    if !backup.authority.packages.is_empty() {
        return Err(authority_mismatch(
            "Control-authority backups must not retain legacy package receipt digests.",
        ));
    }
    let export_entry = backup
        .entries
        .iter()
        .find(|entry| entry.path == crate::control_store::CONTROL_STORE_EXPORT_BACKUP_PATH)
        .ok_or_else(|| {
            authority_mismatch(
                "A Control-authority backup is missing its portable Control Store export.",
            )
        })?;
    if export_entry.sha256 != backup.authority.registry_digest {
        return Err(authority_mismatch(
            "The Control Store export digest does not match the backup authority.",
        ));
    }

    let (_bytes, generation, digest) = crate::control_store::export_control_store_under_exclusive(
        &state_root,
        paths.installation(),
        maintenance,
    )
    .await?;
    if generation != backup.authority.registry_generation
        || digest != backup.authority.registry_digest
    {
        return Err(authority_mismatch(
            "The live Control Store export does not match the backup authority.",
        ));
    }
    digest_entries(&live_authority)
}

pub(super) fn authority_entries(entries: &[StateBackupEntry]) -> Vec<StateBackupEntry> {
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.family,
                StateBackupFamily::Registry | StateBackupFamily::Grants
            )
        })
        .cloned()
        .collect()
}
