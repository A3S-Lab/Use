use std::path::PathBuf;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::ExtensionPaths;

use super::CommandOutput;
use crate::state_backup::{
    StateBackupManager, StateBackupRetentionPolicy, DEFAULT_STATE_BACKUP_RETENTION_MAX_BACKUPS,
    DEFAULT_STATE_BACKUP_RETENTION_MAX_BYTES,
};
use crate::state_restore::StateRestoreManager;

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    let command = args.first().map(String::as_str).ok_or_else(|| {
        usage_error(
            "state requires a backup, retention, plan-restore, restore, or restore-status command",
        )
    })?;
    let installation = super::managed_scope_argument(args)?;
    match command {
        "backup" => {
            validate_backup_options(args)?;
            let path = required_path(args, command)?;
            let manager = StateBackupManager::new(ExtensionPaths::from_env(installation.clone())?);
            let manifest = manager.backup(path).await?;
            let human = format!(
                "Created a coordinated Use state backup with {} files and {} bytes ({}).",
                manifest.file_count, manifest.byte_count, manifest.inventory_digest
            );
            Ok(CommandOutput::success(human, encode_manifest(manifest)?))
        }
        "verify-backup" => {
            validate_backup_options(args)?;
            let path = required_path(args, command)?;
            let manifest = StateBackupManager::verify_backup(path).await?;
            if manifest.installation != installation {
                return Err(UseError::new(
                    "use.state_backup_installation_mismatch",
                    "The state backup belongs to a different installation.",
                ));
            }
            let human = format!(
                "Verified a coordinated Use state backup with {} files and {} bytes ({}).",
                manifest.file_count, manifest.byte_count, manifest.inventory_digest
            );
            Ok(CommandOutput::success(human, encode_manifest(manifest)?))
        }
        "backup-retention" => {
            backup_retention(installation, args, required_path(args, command)?).await
        }
        "plan-restore" => plan_restore(installation, args, required_path(args, command)?).await,
        "restore" => restore(installation, args, required_path(args, command)?).await,
        "restore-status" => restore_status(installation, args).await,
        value => Err(usage_error(format!("unknown state command '{value}'"))),
    }
}

async fn restore_status(
    installation: a3s_use_core::InstallationId,
    args: &[String],
) -> UseResult<CommandOutput> {
    validate_status_options(args)?;
    let diagnostic = StateRestoreManager::new(ExtensionPaths::from_env(installation)?)
        .diagnose_restore()
        .await?;
    Ok(CommandOutput::success(
        match &diagnostic.active {
            Some(active) => format!(
                "Whole-installation restore {} is {:?}.",
                active.plan_digest, active.status
            ),
            None => format!(
                "No whole-installation restore is active; {} operation record(s) are retained.",
                diagnostic.operations.len()
            ),
        },
        serde_json::json!({ "state": { "restore": { "diagnostic": diagnostic } } }),
    ))
}

async fn plan_restore(
    installation: a3s_use_core::InstallationId,
    args: &[String],
    backup_path: PathBuf,
) -> UseResult<CommandOutput> {
    validate_backup_options(args)?;
    let manager = StateRestoreManager::new(ExtensionPaths::from_env(installation)?);
    let plan = manager.plan_restore(backup_path).await?;
    let plan_digest = plan.descriptor_digest()?;
    Ok(CommandOutput::success(
        format!(
            "Reviewed whole-installation restore: add {}, replace {}, remove {}, retain {} file(s) (plan {}).",
            plan.summary.add_files,
            plan.summary.replace_files,
            plan.summary.remove_files,
            plan.summary.retain_files,
            plan_digest,
        ),
        serde_json::json!({
            "state": {
                "restore": {
                    "plan": plan,
                    "planDigest": plan_digest,
                }
            }
        }),
    ))
}

async fn restore(
    installation: a3s_use_core::InstallationId,
    args: &[String],
    backup_path: PathBuf,
) -> UseResult<CommandOutput> {
    let options = restore_options(args)?;
    if !options.confirmed {
        return Err(usage_error(
            "state restore requires --yes after reviewing the exact plan digest and rollback destination",
        ));
    }
    let rollback_backup = options
        .rollback_backup
        .ok_or_else(|| usage_error("state restore requires --rollback-backup <external-path>"))?;
    let plan_digest = options
        .plan_digest
        .ok_or_else(|| usage_error("state restore requires --plan-digest <sha256>"))?;
    let manager = StateRestoreManager::new(ExtensionPaths::from_env(installation)?);
    let result = manager
        .apply_restore(backup_path, rollback_backup, &plan_digest)
        .await?;
    Ok(CommandOutput::success(
        if result.changed {
            format!(
                "Completed whole-installation restore: added {}, replaced {}, removed {}, retained {} file(s) (plan {}).",
                result.summary.add_files,
                result.summary.replace_files,
                result.summary.remove_files,
                result.summary.retain_files,
                result.plan_digest,
            )
        } else {
            format!(
                "Whole-installation state already matches the reviewed backup (plan {}).",
                result.plan_digest,
            )
        },
        serde_json::json!({ "state": { "restore": { "result": result } } }),
    ))
}

fn required_path(args: &[String], command: &str) -> UseResult<PathBuf> {
    args.get(1)
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| usage_error(format!("state {command} requires a path or directory")))
}

async fn backup_retention(
    installation: a3s_use_core::InstallationId,
    args: &[String],
    directory: PathBuf,
) -> UseResult<CommandOutput> {
    let options = retention_options(args)?;
    let policy = StateBackupRetentionPolicy::new(options.max_backups, options.max_bytes)?;
    let manager = StateBackupManager::new(ExtensionPaths::from_env(installation)?);
    if options.confirmed {
        let expected_plan_digest = options.plan_digest.ok_or_else(|| {
            usage_error(
                "state backup-retention requires --plan-digest with --yes before removing reviewed backups",
            )
        })?;
        let result = manager
            .apply_backup_retention(directory, policy, expected_plan_digest)
            .await?;
        return Ok(CommandOutput::success(
            format!(
                "Applied coordinated state backup retention: removed {} backup(s) and retained {} ({} bytes).",
                result.removed.len(),
                result.retained_backup_count,
                result.retained_archive_bytes,
            ),
            serde_json::json!({ "state": { "backupRetention": { "result": result } } }),
        ));
    }
    if options.plan_digest.is_some() {
        return Err(usage_error(
            "state backup-retention accepts --plan-digest only together with --yes",
        ));
    }
    let plan = manager.plan_backup_retention(directory, policy).await?;
    let plan_digest = plan.descriptor_digest()?;
    Ok(CommandOutput::success(
        format!(
            "Reviewed {} coordinated state backup(s): remove {} and retain {} (plan {}).",
            plan.before_backup_count,
            plan.remove.len(),
            plan.retained_backup_count,
            plan_digest,
        ),
        serde_json::json!({
            "state": {
                "backupRetention": {
                    "plan": plan,
                    "planDigest": plan_digest,
                }
            }
        }),
    ))
}

fn validate_backup_options(args: &[String]) -> UseResult<()> {
    if args.len() < 2 {
        return Err(usage_error("state backup commands require a path"));
    }
    let mut json_seen = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" if !json_seen => {
                json_seen = true;
                index += 1;
            }
            "--json" => return Err(usage_error("--json may be provided only once")),
            "--scope-kind" | "--scope-id" => {
                option_value(args, &mut index)?;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown state backup option '{value}'"
                )))
            }
        }
    }
    Ok(())
}

fn validate_status_options(args: &[String]) -> UseResult<()> {
    let mut json_seen = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--json" if !json_seen => {
                json_seen = true;
                index += 1;
            }
            "--json" => return Err(usage_error("--json may be provided only once")),
            "--scope-kind" | "--scope-id" => {
                option_value(args, &mut index)?;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown state restore-status option '{value}'"
                )))
            }
        }
    }
    Ok(())
}

struct RetentionOptions {
    max_backups: u64,
    max_bytes: u64,
    plan_digest: Option<String>,
    confirmed: bool,
}

struct RestoreOptions {
    rollback_backup: Option<PathBuf>,
    plan_digest: Option<String>,
    confirmed: bool,
}

fn restore_options(args: &[String]) -> UseResult<RestoreOptions> {
    let mut rollback_backup = None;
    let mut plan_digest = None;
    let mut confirmed = false;
    let mut json_seen = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" if !json_seen => {
                json_seen = true;
                index += 1;
            }
            "--json" => return Err(usage_error("--json may be provided only once")),
            "--yes" if !confirmed => {
                confirmed = true;
                index += 1;
            }
            "--yes" => return Err(usage_error("--yes may be provided only once")),
            "--rollback-backup" => {
                if rollback_backup.is_some() {
                    return Err(usage_error("--rollback-backup may be provided only once"));
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| usage_error("--rollback-backup requires a path"))?;
                rollback_backup = Some(PathBuf::from(value));
                index += 2;
            }
            "--plan-digest" => {
                if plan_digest.is_some() {
                    return Err(usage_error("--plan-digest may be provided only once"));
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| usage_error("--plan-digest requires a value"))?;
                plan_digest = Some(value.clone());
                index += 2;
            }
            "--scope-kind" | "--scope-id" => {
                option_value(args, &mut index)?;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown state restore option '{value}'"
                )))
            }
        }
    }
    Ok(RestoreOptions {
        rollback_backup,
        plan_digest,
        confirmed,
    })
}

fn retention_options(args: &[String]) -> UseResult<RetentionOptions> {
    let mut max_backups = None;
    let mut max_bytes = None;
    let mut plan_digest = None;
    let mut confirmed = false;
    let mut json_seen = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" if !json_seen => {
                json_seen = true;
                index += 1;
            }
            "--json" => return Err(usage_error("--json may be provided only once")),
            "--yes" if !confirmed => {
                confirmed = true;
                index += 1;
            }
            "--yes" => return Err(usage_error("--yes may be provided only once")),
            "--max-backups" => {
                max_backups = Some(parse_integer_option(
                    args,
                    &mut index,
                    max_backups.is_some(),
                )?);
            }
            "--max-bytes" => {
                max_bytes = Some(parse_integer_option(args, &mut index, max_bytes.is_some())?);
            }
            "--plan-digest" => {
                if plan_digest.is_some() {
                    return Err(usage_error("--plan-digest may be provided only once"));
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| usage_error("--plan-digest requires a value"))?;
                plan_digest = Some(value.clone());
                index += 2;
            }
            "--scope-kind" | "--scope-id" => {
                option_value(args, &mut index)?;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown state backup-retention option '{value}'"
                )))
            }
        }
    }
    Ok(RetentionOptions {
        max_backups: max_backups.unwrap_or(DEFAULT_STATE_BACKUP_RETENTION_MAX_BACKUPS),
        max_bytes: max_bytes.unwrap_or(DEFAULT_STATE_BACKUP_RETENTION_MAX_BYTES),
        plan_digest,
        confirmed,
    })
}

fn parse_integer_option(args: &[String], index: &mut usize, duplicate: bool) -> UseResult<u64> {
    let option = args[*index].as_str();
    if duplicate {
        return Err(usage_error(format!("{option} may be provided only once")));
    }
    let value = args
        .get(*index + 1)
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| usage_error(format!("{option} requires a value")))?;
    let parsed = value
        .parse::<u64>()
        .map_err(|_| usage_error(format!("{option} requires an unsigned integer")))?;
    *index += 2;
    Ok(parsed)
}

fn option_value(args: &[String], index: &mut usize) -> UseResult<()> {
    let option = args[*index].as_str();
    args.get(*index + 1)
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| usage_error(format!("{option} requires a value")))?;
    *index += 2;
    Ok(())
}

fn encode_manifest(
    manifest: crate::state_backup::StateBackupManifest,
) -> UseResult<serde_json::Value> {
    serde_json::to_value(manifest).map_err(|error| {
        UseError::new(
            "use.cli.output_invalid",
            format!("Failed to encode state backup output: {error}"),
        )
    })
}

fn usage_error(message: impl Into<String>) -> UseError {
    UseError::new("use.cli.invalid_usage", message)
}
