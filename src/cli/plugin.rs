use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    PlanActor, PlanPolicyDecision, PluginHostEnablementPlanResult, PluginHostEnablementPlanStatus,
    PluginHostObservationStatus, PluginManagerApplyPlanInput, PluginManagerInspectInput,
    PluginManagerInstallPlanInput, PluginManagerListInstalledInput, PluginManagerPackageScopeInput,
    PluginManagerSearchInput, PluginManagerUpgradePlanInput, PluginOperationConfirmation,
    PluginPackageId, PluginReleaseChannel, PluginSurfaceKind, PluginSurfaceRef, UseError,
    UseResult, PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};
use serde::Serialize;

use crate::cognitive_package::CognitiveRegistryAccess;
use crate::extension_cli::standalone_plugin_manager_service;

use super::*;

const CLI_ERROR: &str = "use.plugin.manager_cli_invalid";
#[cfg(test)]
const CLI_COMMANDS: [&str; 10] = [
    "search",
    "inspect",
    "list-installed",
    "status",
    "plan-install",
    "plan-upgrade",
    "plan-uninstall",
    "apply-plan",
    "plan-enable",
    "plan-disable",
];

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    let command = args.first().map(String::as_str).ok_or_else(|| {
        usage_error(
            "plugin requires search, inspect, list-installed, status, a plan command, or apply-plan",
        )
    })?;
    let tool_name = manager_tool_name(command)
        .ok_or_else(|| usage_error(format!("unknown Plugin Manager command '{command}'")))?;
    match tool_name {
        "plugin_search" => search(args).await,
        "plugin_inspect" => inspect(args).await,
        "plugin_list_installed" => list_installed(args).await,
        "plugin_status" => status(args).await,
        "plugin_plan_install" => plan_install(args).await,
        "plugin_plan_upgrade" => plan_upgrade(args).await,
        "plugin_plan_uninstall" => plan_package(args, PluginPlanCommand::Uninstall).await,
        "plugin_plan_enable" => plan_package(args, PluginPlanCommand::Enable).await,
        "plugin_plan_disable" => plan_package(args, PluginPlanCommand::Disable).await,
        "plugin_apply_plan" => apply_plan(args).await,
        _ => Err(cli_error(
            "The Plugin Manager CLI mapping differs from the frozen tool inventory.",
        )),
    }
}

fn manager_tool_name(command: &str) -> Option<&'static str> {
    match command {
        "search" => Some("plugin_search"),
        "inspect" => Some("plugin_inspect"),
        "list-installed" => Some("plugin_list_installed"),
        "status" => Some("plugin_status"),
        "plan-install" => Some("plugin_plan_install"),
        "plan-upgrade" => Some("plugin_plan_upgrade"),
        "plan-uninstall" => Some("plugin_plan_uninstall"),
        "apply-plan" => Some("plugin_apply_plan"),
        "plan-enable" => Some("plugin_plan_enable"),
        "plan-disable" => Some("plugin_plan_disable"),
        _ => None,
    }
}

async fn search(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(
        args,
        2,
        &["--json", "--offline"],
        &[
            "--kind",
            "--channel",
            "--cursor",
            "--limit",
            "--scope-kind",
            "--scope-id",
        ],
        &[],
        "plugin search",
    )?;
    let query = value_argument(args, 1, "plugin search requires a query")?;
    let result = standalone_plugin_manager_service(managed_scope_argument(args)?)?
        .search(
            PluginManagerSearchInput {
                query: query.to_owned(),
                kind: option_argument(args, "--kind")?
                    .map(parse_surface_kind)
                    .transpose()?,
                channel: option_argument(args, "--channel")?
                    .map(parse_channel)
                    .transpose()?,
                cursor: option_argument(args, "--cursor")?.map(str::to_owned),
                limit: unsigned_option(args, "--limit")?,
            },
            registry_access(args)?,
        )
        .await?;
    encoded_output(
        format!("Found {} verified plugin release(s).", result.total_matches),
        &result,
    )
}

async fn inspect(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(
        args,
        2,
        &["--json", "--offline"],
        &["--version", "--channel", "--scope-kind", "--scope-id"],
        &[],
        "plugin inspect",
    )?;
    let package_id = package_id_argument(args, 1, "plugin inspect requires a package ID")?;
    let result = standalone_plugin_manager_service(managed_scope_argument(args)?)?
        .inspect(
            PluginManagerInspectInput {
                package_id,
                version: option_argument(args, "--version")?.map(str::to_owned),
                channel: option_argument(args, "--channel")?
                    .map(parse_channel)
                    .transpose()?,
            },
            registry_access(args)?,
        )
        .await?;
    encoded_output(
        format!(
            "Inspected verified plugin '{}' {}.",
            result.plugin.record.package_id, result.plugin.record.version
        ),
        &result,
    )
}

async fn list_installed(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(
        args,
        1,
        &["--json"],
        &["--scope-kind", "--scope-id", "--cursor", "--limit"],
        &[],
        "plugin list-installed",
    )?;
    let scope = managed_scope_argument(args)?;
    let result = standalone_plugin_manager_service(scope.clone())?
        .list_installed(PluginManagerListInstalledInput {
            scope_kind: scope.kind,
            scope_id: scope.id,
            cursor: option_argument(args, "--cursor")?.map(str::to_owned),
            limit: unsigned_option(args, "--limit")?,
        })
        .await?;
    encoded_output(
        format!("Listed {} installed plugin(s).", result.packages.len()),
        &result,
    )
}

async fn status(args: &[String]) -> UseResult<CommandOutput> {
    validate_package_scope_options(args, "plugin status")?;
    let installation = managed_scope_argument(args)?;
    let input = package_scope_input(args, "plugin status requires a package ID")?;
    let result = standalone_plugin_manager_service(installation)?
        .status(input)
        .await?;
    let state = match &result.status {
        PluginHostObservationStatus::Available { state } => format!(
            "{} / {}",
            serialized_label(&state.desired)?,
            serialized_label(&state.observed)?
        ),
        PluginHostObservationStatus::Unavailable { reason } => {
            format!("unavailable / {}", serialized_label(reason)?)
        }
    };
    encoded_output(
        format!("Plugin '{}' is {state}.", result.package_id),
        &result,
    )
}

async fn plan_install(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(
        args,
        2,
        &["--json", "--offline"],
        &[
            "--registry-name",
            "--version-requirement",
            "--channel",
            "--scope-kind",
            "--scope-id",
        ],
        &["--surface"],
        "plugin plan-install",
    )?;
    let scope = managed_scope_argument(args)?;
    let result = standalone_plugin_manager_service(scope.clone())?
        .plan_install(
            PluginManagerInstallPlanInput {
                package_id: package_id_argument(
                    args,
                    1,
                    "plugin plan-install requires a package ID",
                )?,
                registry_name: option_argument(args, "--registry-name")?.map(str::to_owned),
                version_requirement: option_argument(args, "--version-requirement")?
                    .map(str::to_owned),
                channel: option_argument(args, "--channel")?
                    .map(parse_channel)
                    .transpose()?,
                surfaces: surface_arguments(args)?,
                scope_kind: scope.kind,
                scope_id: scope.id,
            },
            registry_access(args)?,
        )
        .await?;
    plan_output(&result)
}

async fn plan_upgrade(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(
        args,
        2,
        &["--json", "--offline"],
        &[
            "--version-requirement",
            "--channel",
            "--scope-kind",
            "--scope-id",
        ],
        &["--surface"],
        "plugin plan-upgrade",
    )?;
    let scope = managed_scope_argument(args)?;
    let result = standalone_plugin_manager_service(scope.clone())?
        .plan_upgrade(
            PluginManagerUpgradePlanInput {
                package_id: package_id_argument(
                    args,
                    1,
                    "plugin plan-upgrade requires a package ID",
                )?,
                version_requirement: option_argument(args, "--version-requirement")?
                    .map(str::to_owned),
                channel: option_argument(args, "--channel")?
                    .map(parse_channel)
                    .transpose()?,
                surfaces: surface_arguments(args)?,
                scope_kind: scope.kind,
                scope_id: scope.id,
            },
            registry_access(args)?,
        )
        .await?;
    plan_output(&result)
}

#[derive(Clone, Copy)]
enum PluginPlanCommand {
    Uninstall,
    Enable,
    Disable,
}

async fn plan_package(args: &[String], command: PluginPlanCommand) -> UseResult<CommandOutput> {
    let label = match command {
        PluginPlanCommand::Uninstall => "plugin plan-uninstall",
        PluginPlanCommand::Enable => "plugin plan-enable",
        PluginPlanCommand::Disable => "plugin plan-disable",
    };
    validate_package_scope_options(args, label)?;
    let installation = managed_scope_argument(args)?;
    let input = package_scope_input(args, &format!("{label} requires a package ID"))?;
    let service = standalone_plugin_manager_service(installation)?;
    match command {
        PluginPlanCommand::Uninstall => {
            let result = service.plan_uninstall(input).await?;
            plan_output(&result)
        }
        PluginPlanCommand::Enable | PluginPlanCommand::Disable => {
            let result = if matches!(command, PluginPlanCommand::Enable) {
                service.plan_enable(input).await?
            } else {
                service.plan_disable(input).await?
            };
            enablement_plan_output(&result)
        }
    }
}

async fn apply_plan(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(
        args,
        1,
        &["--json", "--yes"],
        &[
            "--operation-id",
            "--plan-digest",
            "--scope-kind",
            "--scope-id",
        ],
        &[],
        "plugin apply-plan",
    )?;
    let operation_id = option_argument(args, "--operation-id")?
        .ok_or_else(|| usage_error("plugin apply-plan requires --operation-id <id>"))?;
    let plan_digest = option_argument(args, "--plan-digest")?
        .ok_or_else(|| usage_error("plugin apply-plan requires --plan-digest <sha256>"))?;
    if !flag_argument(args, "--yes")? {
        return Err(usage_error(
            "plugin apply-plan requires --yes after reviewing the exact operation ID and plan digest",
        ));
    }

    let service = standalone_plugin_manager_service(managed_scope_argument(args)?)?;
    let input = PluginManagerApplyPlanInput {
        operation_id: operation_id.to_owned(),
        plan_digest: plan_digest.to_owned(),
    };
    let reviewed = service.reviewed_plan(&input).await?;
    let confirmation = match reviewed.plan.plan.authority.decision {
        PlanPolicyDecision::Ask => Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: input.operation_id.clone(),
            plan_digest: input.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: now_ms()?,
        }),
        PlanPolicyDecision::Allow | PlanPolicyDecision::Deny => None,
    };
    let result = service.apply_plan(input, confirmation).await?;
    encoded_output(
        format!(
            "{} reviewed operation {} for plugin '{}'.",
            if result.replayed {
                "Replayed"
            } else {
                "Applied"
            },
            result.operation_id,
            result.package_id
        ),
        &result,
    )
}

fn plan_output(result: &a3s_use_core::PluginHostPlanResult) -> UseResult<CommandOutput> {
    encoded_output(
        format!(
            "Planned {} for '{}' as operation {} with digest {}.",
            serialized_label(&result.plan.plan.action)?,
            result.package_id,
            result.plan.plan.operation_id,
            result.plan.plan_digest
        ),
        result,
    )
}

fn enablement_plan_output(result: &PluginHostEnablementPlanResult) -> UseResult<CommandOutput> {
    let human = match result.status {
        PluginHostEnablementPlanStatus::Planned => {
            let plan = result.plan.as_ref().ok_or_else(|| {
                cli_error("A planned enablement result omitted its reviewed plan.")
            })?;
            format!(
                "Planned {} for '{}' as operation {} with digest {}.",
                if result.enabled { "enable" } else { "disable" },
                result.package_id,
                plan.plan.operation_id,
                plan.plan_digest
            )
        }
        PluginHostEnablementPlanStatus::NoChange => format!(
            "Plugin '{}' already has the requested {} state; no operation was created.",
            result.package_id,
            if result.enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
    };
    encoded_output(human, result)
}

fn package_scope_input(
    args: &[String],
    message: &str,
) -> UseResult<PluginManagerPackageScopeInput> {
    let scope = managed_scope_argument(args)?;
    Ok(PluginManagerPackageScopeInput {
        package_id: package_id_argument(args, 1, message)?,
        scope_kind: scope.kind,
        scope_id: scope.id,
    })
}

fn package_id_argument(args: &[String], index: usize, message: &str) -> UseResult<PluginPackageId> {
    PluginPackageId::parse(value_argument(args, index, message)?.to_owned())
}

fn validate_package_scope_options(args: &[String], command: &str) -> UseResult<()> {
    validate_options(
        args,
        2,
        &["--json"],
        &["--scope-kind", "--scope-id"],
        &[],
        command,
    )
}

fn validate_options(
    args: &[String],
    first_option: usize,
    flags: &[&str],
    values: &[&str],
    repeated_values: &[&str],
    command: &str,
) -> UseResult<()> {
    let mut index = first_option;
    while index < args.len() {
        let option = args[index].as_str();
        if flags.contains(&option) {
            index += 1;
        } else if values.contains(&option) || repeated_values.contains(&option) {
            if args
                .get(index + 1)
                .is_none_or(|value| value.starts_with('-'))
            {
                return Err(usage_error(format!("{option} requires a value")));
            }
            index += 2;
        } else {
            return Err(usage_error(format!("unknown {command} option '{option}'")));
        }
    }

    for flag in flags {
        flag_argument(args, flag)?;
    }
    for option in values {
        option_argument(args, option)?;
    }
    Ok(())
}

fn registry_access(args: &[String]) -> UseResult<CognitiveRegistryAccess> {
    Ok(if flag_argument(args, "--offline")? {
        CognitiveRegistryAccess::Cached
    } else {
        CognitiveRegistryAccess::Refreshed
    })
}

fn unsigned_option(args: &[String], name: &str) -> UseResult<Option<u16>> {
    option_argument(args, name)?
        .map(|value| {
            value.parse::<u16>().map_err(|_| {
                usage_error(format!(
                    "{name} must be a positive bounded integer, received '{value}'"
                ))
            })
        })
        .transpose()
}

fn surface_arguments(args: &[String]) -> UseResult<Option<Vec<PluginSurfaceRef>>> {
    let mut surfaces = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--surface" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| usage_error("--surface requires <kind>/<id>"))?;
            surfaces.push(parse_surface(value)?);
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok((!surfaces.is_empty()).then_some(surfaces))
}

fn parse_surface(value: &str) -> UseResult<PluginSurfaceRef> {
    let (kind, id) = value
        .split_once('/')
        .filter(|(_, id)| !id.is_empty() && !id.contains('/'))
        .ok_or_else(|| usage_error("--surface must use the exact '<kind>/<id>' form"))?;
    Ok(PluginSurfaceRef {
        kind: parse_surface_kind(kind)?,
        id: id.to_owned(),
    })
}

fn parse_surface_kind(value: &str) -> UseResult<PluginSurfaceKind> {
    match value {
        "flow" => Ok(PluginSurfaceKind::Flow),
        "mcp" => Ok(PluginSurfaceKind::Mcp),
        "okf" => Ok(PluginSurfaceKind::Okf),
        "skill" => Ok(PluginSurfaceKind::Skill),
        "tool" => Ok(PluginSurfaceKind::Tool),
        "ui" => Ok(PluginSurfaceKind::Ui),
        _ => Err(usage_error(format!(
            "plugin surface kind must be flow, mcp, okf, skill, tool, or ui; received '{value}'"
        ))),
    }
}

fn parse_channel(value: &str) -> UseResult<PluginReleaseChannel> {
    match value {
        "stable" => Ok(PluginReleaseChannel::Stable),
        "beta" => Ok(PluginReleaseChannel::Beta),
        "nightly" => Ok(PluginReleaseChannel::Nightly),
        _ => Err(usage_error(format!(
            "plugin channel must be stable, beta, or nightly; received '{value}'"
        ))),
    }
}

fn encoded_output<T: Serialize>(human: String, value: &T) -> UseResult<CommandOutput> {
    let value = serde_json::to_value(value)
        .map_err(|error| cli_error(format!("Failed to encode Plugin Manager output: {error}")))?;
    Ok(CommandOutput::success(human, value))
}

fn serialized_label<T: Serialize>(value: &T) -> UseResult<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| cli_error("Failed to encode a Plugin Manager status label."))
}

fn now_ms() -> UseResult<u64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| cli_error("The system clock is earlier than the Unix epoch."))?
        .as_millis();
    u64::try_from(value).map_err(|_| cli_error("The system clock exceeds the supported range."))
}

fn cli_error(message: impl Into<String>) -> UseError {
    UseError::new(CLI_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::{manager_tool_name, CLI_COMMANDS};

    #[test]
    fn command_vocabulary_matches_the_frozen_manager_toolset() {
        let cli_tools = CLI_COMMANDS
            .iter()
            .map(|command| manager_tool_name(command).unwrap())
            .collect::<Vec<_>>();
        let frozen_tools = a3s_use_core::PluginManagerToolset::v4()
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(cli_tools, frozen_tools);
    }
}
