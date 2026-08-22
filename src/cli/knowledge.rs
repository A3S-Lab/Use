use a3s_use_core::{UseError, UseResult};

use super::CommandOutput;

#[cfg(feature = "extensions")]
use super::{integer_option, option_argument, usage_error, value_argument};
#[cfg(feature = "extensions")]
use crate::capability_registry::snapshot as capability_registry_snapshot;

#[cfg(feature = "extensions")]
pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        Some("search") => search(args).await,
        Some("usage") => usage(args).await,
        Some("audit") => audit(args).await,
        Some("backup") => backup(args).await,
        Some("verify-backup") => verify_backup(args).await,
        Some("backup-retention") => backup_retention(args).await,
        Some("plan-restore") => plan_restore(args).await,
        Some("restore") => restore(args).await,
        Some("restore-status") => restore_status(args).await,
        Some("repair-search-index") => repair_search_index(args).await,
        Some(value) => Err(usage_error(format!("unknown knowledge command '{value}'"))),
        None => Err(usage_error(
            "knowledge requires search, usage, audit, backup, verify-backup, backup-retention, plan-restore, restore, restore-status, or repair-search-index",
        )),
    }
}

#[cfg(feature = "extensions")]
async fn search(args: &[String]) -> UseResult<CommandOutput> {
    validate_search_options(args)?;
    let query = value_argument(args, 1, "knowledge search requires a query")?;
    let limit = usize::try_from(integer_option(args, "--limit", 10)?)
        .map_err(|_| usage_error("--limit exceeds the platform range"))?;
    let snapshot = capability_registry_snapshot().await?;
    let projections = snapshot.knowledge_projections();
    if projections.is_empty() {
        return Err(UseError::new(
            "use.okf.knowledge_unavailable",
            "No promoted OKF Knowledge projection is active in the current User scope.",
        )
        .with_suggestion(
            "Install and enable a signed cognitive package with an OKF surface, then retry the search.",
        ));
    }
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let client = crate::okf_knowledge::OkfKnowledgeClient::new(std::sync::Arc::new(
        crate::okf_knowledge::SqliteOkfKnowledgeAdapter::from_extension_paths(&paths),
    ));
    let request = crate::okf_knowledge::OkfKnowledgeSearchRequest::new(
        a3s_use_core::PlanScope {
            kind: a3s_use_core::PlanScopeKind::User,
            id: crate::cognitive_package::COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
        },
        query,
        limit,
        projections,
    )?;
    let response = client.search(&request).await?;
    Ok(CommandOutput::success(
        format!(
            "Found {} cited OKF concept(s) for '{query}'.",
            response.hits.len()
        ),
        serde_json::json!({ "knowledge": response }),
    ))
}

#[cfg(feature = "extensions")]
async fn usage(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 1, false)?;
    let scope = scope(args)?;
    let adapter = adapter()?;
    let usage = adapter.usage(&scope).await?;
    Ok(CommandOutput::success(
        format!(
            "Knowledge scope {}/{} retains {} projection(s), {} tombstone(s), and {} expanded byte(s).",
            usage.scope.kind.as_str(),
            usage.scope.id,
            usage.retained_projections,
            usage.removed_tombstones,
            usage.retained_expanded_bytes,
        ),
        serde_json::json!({ "knowledge": { "storage": usage } }),
    ))
}

#[cfg(feature = "extensions")]
async fn audit(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 1, false)?;
    let scope = scope(args)?;
    let report = adapter()?.audit(&scope).await?;
    Ok(CommandOutput::success(
        format!(
            "Knowledge scope {}/{} passed SQLite, receipt, scope, foreign-key, and FTS integrity checks for {} document(s).",
            scope.kind.as_str(),
            scope.id,
            report.document_count,
        ),
        serde_json::json!({ "knowledge": { "integrity": report } }),
    ))
}

#[cfg(feature = "extensions")]
async fn backup(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 2, false)?;
    let destination = value_argument(args, 1, "knowledge backup requires a path")?;
    let scope = scope(args)?;
    let manifest = adapter()?.backup(&scope, destination).await?;
    Ok(CommandOutput::success(
        format!(
            "Backed up Knowledge scope {}/{} to '{}' ({} bytes, {}).",
            scope.kind.as_str(),
            scope.id,
            destination,
            manifest.database_bytes,
            manifest.database_sha256,
        ),
        serde_json::json!({
            "knowledge": {
                "backup": manifest,
                "path": destination,
            }
        }),
    ))
}

#[cfg(feature = "extensions")]
async fn verify_backup(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 2, false)?;
    let backup_path = value_argument(args, 1, "knowledge verify-backup requires a path")?;
    let scope = scope(args)?;
    let manifest =
        crate::okf_knowledge::SqliteOkfKnowledgeAdapter::verify_backup(backup_path, Some(&scope))
            .await?;
    Ok(CommandOutput::success(
        format!(
            "Verified Knowledge backup '{}' for scope {}/{} ({} bytes, {}).",
            backup_path,
            scope.kind.as_str(),
            scope.id,
            manifest.database_bytes,
            manifest.database_sha256,
        ),
        serde_json::json!({
            "knowledge": {
                "backup": manifest,
                "path": backup_path,
                "verified": true,
            }
        }),
    ))
}

#[cfg(feature = "extensions")]
async fn backup_retention(args: &[String]) -> UseResult<CommandOutput> {
    validate_backup_retention_options(args)?;
    let directory = value_argument(args, 1, "knowledge backup-retention requires a directory")?;
    let scope = scope(args)?;
    let policy = crate::okf_knowledge::OkfKnowledgeBackupRetentionPolicy::new(
        integer_option(
            args,
            "--max-backups",
            crate::okf_knowledge::DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BACKUPS,
        )?,
        integer_option(
            args,
            "--max-bytes",
            crate::okf_knowledge::DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BYTES,
        )?,
    )?;
    let expected_plan_digest = option_argument(args, "--plan-digest")?;
    if args.iter().any(|argument| argument == "--yes") {
        let expected_plan_digest = expected_plan_digest.ok_or_else(|| {
            usage_error(
                "knowledge backup-retention requires --plan-digest with --yes before removing reviewed backups",
            )
        })?;
        let result = crate::okf_knowledge::SqliteOkfKnowledgeAdapter::apply_backup_retention(
            directory,
            &scope,
            policy,
            expected_plan_digest,
        )
        .await?;
        return Ok(CommandOutput::success(
            format!(
                "Applied Knowledge backup retention for {}/{}: removed {} backup(s) and retained {} ({} bytes).",
                scope.kind.as_str(),
                scope.id,
                result.removed.len(),
                result.retained_backup_count,
                result.retained_archive_bytes,
            ),
            serde_json::json!({ "knowledge": { "backupRetention": { "result": result } } }),
        ));
    }
    if expected_plan_digest.is_some() {
        return Err(usage_error(
            "knowledge backup-retention accepts --plan-digest only together with --yes",
        ));
    }
    let plan = crate::okf_knowledge::SqliteOkfKnowledgeAdapter::plan_backup_retention(
        directory, &scope, policy,
    )
    .await?;
    let plan_digest = plan.descriptor_digest()?;
    Ok(CommandOutput::success(
        format!(
            "Reviewed {} Knowledge backup(s) for {}/{}: remove {} and retain {} (plan {}).",
            plan.before_backup_count,
            scope.kind.as_str(),
            scope.id,
            plan.remove.len(),
            plan.retained_backup_count,
            plan_digest,
        ),
        serde_json::json!({
            "knowledge": {
                "backupRetention": {
                    "plan": plan,
                    "planDigest": plan_digest,
                }
            }
        }),
    ))
}

#[cfg(feature = "extensions")]
async fn plan_restore(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 2, false)?;
    let backup_path = value_argument(args, 1, "knowledge plan-restore requires a backup path")?;
    let scope = scope(args)?;
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let plan = crate::okf_knowledge::OkfKnowledgeRecoveryManager::from_extension_paths(&paths)
        .plan_restore(&scope, backup_path)
        .await?;
    let plan_digest = plan.descriptor_digest()?;
    Ok(CommandOutput::success(
        format!(
            "Reviewed Knowledge restore '{}' for {}/{}: {:?} (plan {}).",
            backup_path,
            scope.kind.as_str(),
            scope.id,
            plan.status,
            plan_digest,
        ),
        serde_json::json!({
            "knowledge": {
                "restorePlan": plan,
                "planDigest": plan_digest,
                "path": backup_path,
            }
        }),
    ))
}

#[cfg(feature = "extensions")]
async fn restore(args: &[String]) -> UseResult<CommandOutput> {
    validate_restore_options(args)?;
    if !args.iter().any(|argument| argument == "--yes") {
        return Err(usage_error(
            "knowledge restore requires --yes because it atomically replaces the live scope database",
        ));
    }
    let backup_path = value_argument(args, 1, "knowledge restore requires a backup path")?;
    let plan_digest = option_argument(args, "--plan-digest")?.ok_or_else(|| {
        usage_error("knowledge restore requires the exact --plan-digest returned by plan-restore")
    })?;
    let scope = scope(args)?;
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let result = crate::okf_knowledge::OkfKnowledgeRecoveryManager::from_extension_paths(&paths)
        .apply_restore(&scope, backup_path, plan_digest)
        .await?;
    let human = if result.changed {
        format!(
            "Restored Knowledge scope {}/{} from '{}' and preserved {} prior database file(s) (plan {}).",
            scope.kind.as_str(),
            scope.id,
            backup_path,
            result.preserved_prior_files,
            result.plan_digest,
        )
    } else {
        format!(
            "Knowledge scope {}/{} already matches '{}' (plan {}).",
            scope.kind.as_str(),
            scope.id,
            backup_path,
            result.plan_digest,
        )
    };
    Ok(CommandOutput::success(
        human,
        serde_json::json!({
            "knowledge": {
                "restore": result,
                "path": backup_path,
            }
        }),
    ))
}

#[cfg(feature = "extensions")]
async fn restore_status(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 1, false)?;
    let scope = scope(args)?;
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let diagnostic =
        crate::okf_knowledge::OkfKnowledgeRecoveryManager::from_extension_paths(&paths)
            .diagnose_restores(&scope)
            .await?;
    let human = match &diagnostic.active {
        Some(active) => format!(
            "Knowledge restore {} is {} for {}/{}; requested scope {}/{} has {} retained restore directories.",
            active.plan_digest,
            active.status.as_str(),
            active.scope.kind.as_str(),
            active.scope.id,
            scope.kind.as_str(),
            scope.id,
            diagnostic.retained_operation_directories,
        ),
        None => format!(
            "No Knowledge restore is active; scope {}/{} has {} retained restore directories and capacity for {} more.",
            scope.kind.as_str(),
            scope.id,
            diagnostic.retained_operation_directories,
            diagnostic.retention_remaining,
        ),
    };
    Ok(CommandOutput::success(
        human,
        serde_json::json!({ "knowledge": { "restoreStatus": diagnostic } }),
    ))
}

#[cfg(feature = "extensions")]
async fn repair_search_index(args: &[String]) -> UseResult<CommandOutput> {
    validate_scope_options(args, 1, true)?;
    if !args.iter().any(|argument| argument == "--yes") {
        return Err(usage_error(
            "knowledge repair-search-index requires --yes because it rebuilds the derived FTS index",
        ));
    }
    let scope = scope(args)?;
    let repaired = adapter()?.repair_search_index(&scope).await?;
    Ok(CommandOutput::success(
        format!(
            "Rebuilt the derived Knowledge search index for {}/{} from {} validated document(s).",
            scope.kind.as_str(),
            scope.id,
            repaired.rebuilt_document_count,
        ),
        serde_json::json!({ "knowledge": { "repair": repaired } }),
    ))
}

#[cfg(feature = "extensions")]
fn adapter() -> UseResult<crate::okf_knowledge::SqliteOkfKnowledgeAdapter> {
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    Ok(crate::okf_knowledge::SqliteOkfKnowledgeAdapter::from_extension_paths(&paths))
}

#[cfg(feature = "extensions")]
fn validate_search_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--limit" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error("--limit requires a value"));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown knowledge option '{value}'"))),
        }
    }
    Ok(())
}

#[cfg(feature = "extensions")]
fn validate_scope_options(args: &[String], first_option: usize, allow_yes: bool) -> UseResult<()> {
    let mut index = first_option;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--yes" if allow_yes => index += 1,
            "--scope-kind" | "--scope-id" => {
                if args
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown knowledge option '{value}'"))),
        }
    }
    Ok(())
}

#[cfg(feature = "extensions")]
fn validate_restore_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--yes" => index += 1,
            "--scope-kind" | "--scope-id" | "--plan-digest" => {
                if args
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown knowledge option '{value}'"))),
        }
    }
    Ok(())
}

#[cfg(feature = "extensions")]
fn validate_backup_retention_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--yes" => index += 1,
            "--scope-kind" | "--scope-id" | "--max-backups" | "--max-bytes" | "--plan-digest" => {
                if args
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown knowledge option '{value}'"))),
        }
    }
    Ok(())
}

#[cfg(feature = "extensions")]
fn scope(args: &[String]) -> UseResult<a3s_use_core::PlanScope> {
    let kind = match option_argument(args, "--scope-kind")?.unwrap_or("user") {
        "user" => a3s_use_core::PlanScopeKind::User,
        "workspace" => a3s_use_core::PlanScopeKind::Workspace,
        value => {
            return Err(usage_error(format!(
                "--scope-kind must be 'user' or 'workspace', received '{value}'"
            )))
        }
    };
    let scope_id = option_argument(args, "--scope-id")?;
    if kind == a3s_use_core::PlanScopeKind::Workspace && scope_id.is_none() {
        return Err(usage_error(
            "--scope-id is required when --scope-kind is 'workspace'",
        ));
    }
    Ok(a3s_use_core::PlanScope {
        kind,
        id: scope_id
            .unwrap_or(crate::cognitive_package::COGNITIVE_PACKAGE_DEFAULT_SCOPE)
            .to_owned(),
    })
}

#[cfg(not(feature = "extensions"))]
pub(super) async fn run(_args: &[String]) -> UseResult<CommandOutput> {
    Err(UseError::new(
        "use.okf.knowledge_disabled",
        "OKF Knowledge support is disabled in this custom build.",
    ))
}
