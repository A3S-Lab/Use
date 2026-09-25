use a3s_use_core::{DomainDiagnostic, Readiness, UseError, UseResult};

use crate::capability_registry::{
    snapshot as capability_registry_snapshot, wait_for_change as wait_for_capability_change,
};
use crate::extension_cli::{
    extension_capabilities, extension_inspect, extension_list, extension_operation_diagnostic,
    extension_planning_evidence, extension_snapshot, extension_watch, external_component_value,
    external_package_id, install_remote_extension, installed_extension_for_id,
    installed_extensions, uninstall_extension, upgrade_remote_extension,
};
use std::time::Duration;

mod component;
mod knowledge;
#[cfg(feature = "extensions")]
mod plugin;
#[cfg(not(feature = "extensions"))]
mod plugin {
    use a3s_use_core::{UseError, UseResult};

    use super::CommandOutput;

    pub(super) async fn run(_args: &[String]) -> UseResult<CommandOutput> {
        Err(UseError::new(
            "use.extension.disabled",
            "Plugin Manager commands require the 'extensions' feature.",
        ))
    }
}
#[cfg(feature = "extensions")]
mod registry;
#[cfg(feature = "extensions")]
mod state;
#[cfg(not(feature = "extensions"))]
mod registry {
    use a3s_use_core::{UseError, UseResult};

    use super::CommandOutput;

    pub(super) async fn run(_args: &[String]) -> UseResult<CommandOutput> {
        Err(UseError::new(
            "use.extension.disabled",
            "Registry source and cache operations require the 'extensions' feature.",
        ))
    }
}
#[cfg(not(feature = "extensions"))]
mod state {
    use a3s_use_core::{UseError, UseResult};

    use super::CommandOutput;

    pub(super) async fn run(_args: &[String]) -> UseResult<CommandOutput> {
        Err(UseError::new(
            "use.state_backup_disabled",
            "Coordinated state backup requires the 'extensions' feature.",
        ))
    }
}

pub struct CommandOutput {
    pub human: String,
    pub json: serde_json::Value,
    pub exit_code: u8,
    pub should_print: bool,
}

impl CommandOutput {
    pub(crate) fn success(human: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            human: human.into(),
            json: serde_json::json!({
                "schemaVersion": 1,
                "ok": true,
                "data": data,
            }),
            exit_code: 0,
            should_print: true,
        }
    }

    fn delegated(exit_code: u8) -> Self {
        Self {
            human: String::new(),
            json: serde_json::Value::Null,
            exit_code,
            should_print: false,
        }
    }
}

pub async fn run(args: Vec<String>) -> UseResult<CommandOutput> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(help());
    };
    match command {
        "-V" | "--version" | "version" => Ok(version()),
        "-h" | "--help" | "help" => Ok(help()),
        "capabilities" => capabilities(&args[1..]).await,
        "capability" => capability(&args[1..]).await,
        "doctor" => doctor(&args[1..]).await,
        "install" => Box::pin(package_command_alias("install", &args[1..])).await,
        "upgrade" => Box::pin(package_command_alias("upgrade", &args[1..])).await,
        "uninstall" => Box::pin(package_command_alias("uninstall", &args[1..])).await,
        "component" => Box::pin(component::run(&args[1..])).await,
        "plugin" => Box::pin(plugin::run(&args[1..])).await,
        "knowledge" => knowledge::run(&args[1..]).await,
        "registry" => registry::run(&args[1..]).await,
        "state" => state::run(&args[1..]).await,
        "browser" => browser(&args[1..]).await,
        "ocr" => ocr(&args[1..]).await,
        "box" => {
            let exit_code = crate::component_route::run_box(&args[1..]).await?;
            Ok(CommandOutput::delegated(exit_code))
        }
        "extension" => extension(&args[1..]).await,
        "mcp" => mcp(&args[1..]).await,
        route => Err(
            UseError::new("use.route_unknown", format!("Unknown Use route '{route}'."))
                .with_suggestion("Run 'a3s use capabilities --json'."),
        ),
    }
}

fn version() -> CommandOutput {
    CommandOutput {
        human: format!("a3s-use {}", env!("CARGO_PKG_VERSION")),
        json: serde_json::json!({
            "schemaVersion": 1,
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "data": {
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
        exit_code: 0,
        should_print: true,
    }
}

fn help() -> CommandOutput {
    CommandOutput::success(
        concat!(
            "a3s-use - AI Native Package Manager\n\n",
            "usage:\n",
            "  a3s-use capabilities --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use capability snapshot --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use capability watch --scope-kind <user|workspace> --scope-id <id> [--after-generation <n>] [--after-revision <sha256>] [--timeout-ms <ms>] [--json]\n",
            "  a3s-use doctor [<external-domain>] --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use doctor browser|box|ocr [--json]\n",
            "  a3s-use install <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>] [--offline] [--json]\n",
            "  a3s-use upgrade <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>] [--offline] [--json]\n",
            "  a3s-use uninstall <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use component list|status|install|upgrade|uninstall [args] --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use plugin search|inspect|list-installed|status|plan-install|plan-upgrade|plan-uninstall|plan-enable|plan-disable|apply-plan|observe-operation|watch-operation|cancel-operation [args] --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use knowledge <command> [args] --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use registry source list [--json]\n",
            "  a3s-use registry source add <name> (--url <https-url> | --github <owner/repository>) --trust-root <sha256> [source options] [--json]\n",
            "  a3s-use registry source replace <name> (--url <https-url> | --github <owner/repository>) --trust-root <sha256> --expected-revision <sha256> --yes [source options] [--json]\n",
            "  a3s-use registry source default|enable|disable|remove <name> --expected-revision <sha256> --yes [--json]\n",
            "  a3s-use registry cache usage [--registry-name <name>] [--json]\n",
            "  a3s-use registry cache prune [--registry-name <name>] [cache options] --yes [--json]\n",
            "  a3s-use state <command> [args] --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use browser doctor [--json]\n",
            "  a3s-use browser render <url> [--output <path>] [--screenshot <path>] [--json]\n",
            "  a3s-use browser open|list|navigate|snapshot|click|type|press|select|scroll|screenshot|close [args] [--json]\n",
            "  a3s-use box <a3s-box-args...>\n",
            "  a3s-use ocr doctor [--json]\n",
            "  a3s-use ocr extract <image> [--json]\n",
            "  a3s-use extension list|inspect|doctor|diagnose|planning-evidence|snapshot|watch [args] --scope-kind <user|workspace> --scope-id <id> [--json]\n",
            "  a3s-use mcp serve manager --scope-kind <user|workspace> --scope-id <id> [--offline]\n",
            "  a3s-use mcp serve gateway --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>]\n",
            "  a3s-use mcp serve gateway --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>] --streamable-http [--bind <addr>] [--token <secret>] [--principal <id>]\n",
            "  a3s-use mcp serve browser [--tools <profiles>]\n",
            "  a3s-use mcp serve ocr\n",
            "  a3s-use mcp start|status|stop [browser] [--json]"
        ),
        serde_json::json!({
            "commands": [
                "capabilities",
                "capability",
                "doctor",
                "install",
                "upgrade",
                "uninstall",
                "component",
                "plugin",
                "knowledge",
                "registry",
                "state",
                "browser",
                "box",
                "ocr",
                "extension",
                "mcp"
            ]
        }),
    )
}

async fn package_command_alias(command: &str, args: &[String]) -> UseResult<CommandOutput> {
    let mut delegated = Vec::with_capacity(args.len() + 1);
    delegated.push(command.to_string());
    delegated.extend_from_slice(args);
    component::run(&delegated).await
}

async fn capabilities(args: &[String]) -> UseResult<CommandOutput> {
    validate_scoped_read_options(args, "capabilities")?;
    let installation = managed_scope_argument(args)?;
    let browser = browser_diagnostic();
    let box_domain = crate::component_route::box_diagnostic();
    let ocr = ocr_diagnostic();
    let (extension_generation, extensions) = extension_capabilities(installation).await?;
    Ok(CommandOutput::success(
        "Built-in CLI aliases: browser, box, ocr",
        serde_json::json!({
            "domains": [
                {
                    "id": "browser",
                    "builtIn": true,
                    "readiness": browser.readiness,
                    "surfaces": ["cli", "mcp", "skill"]
                },
                {
                    "id": "ocr",
                    "builtIn": true,
                    "readiness": ocr.readiness,
                    "surfaces": ["cli", "mcp", "skill"]
                },
                {
                    "id": "box",
                    "builtIn": true,
                    "readiness": box_domain.readiness,
                    "surfaces": ["cli"]
                }
            ],
            "externalSurfaces": ["tool", "mcp", "okf", "flow", "skill", "ui"],
            "extensionRegistry": {
                "schemaVersion": 1,
                "generation": extension_generation,
                "hotPlug": true
            },
            "extensions": extensions
        }),
    ))
}

async fn capability(args: &[String]) -> UseResult<CommandOutput> {
    let installation = managed_scope_argument(args)?;
    match args.first().map(String::as_str) {
        Some("snapshot") => {
            validate_capability_options(args, false)?;
            let snapshot = capability_registry_snapshot(installation).await?;
            Ok(CommandOutput::success(
                format!(
                    "Capability registry generation {} ({}).",
                    snapshot.generation, snapshot.revision
                ),
                serde_json::json!({ "registry": snapshot }),
            ))
        }
        Some("watch") => {
            validate_capability_options(args, true)?;
            let after_generation = integer_option(args, "--after-generation", 0)?;
            let after_revision = option_argument(args, "--after-revision")?;
            let timeout = duration_option(args, "--timeout-ms", 30_000)?;
            match wait_for_capability_change(
                installation,
                after_generation,
                after_revision,
                timeout,
            )
            .await?
            {
                Some(snapshot) => Ok(CommandOutput::success(
                    "The capability registry changed.",
                    serde_json::json!({ "changed": true, "registry": snapshot }),
                )),
                None => Ok(CommandOutput::success(
                    "The capability registry did not change.",
                    serde_json::json!({
                        "changed": false,
                        "afterGeneration": after_generation,
                        "afterRevision": after_revision,
                        "timeoutMs": timeout.as_millis().min(u64::MAX as u128) as u64
                    }),
                )),
            }
        }
        Some(value) => Err(usage_error(format!("unknown capability command '{value}'"))),
        None => Err(usage_error("capability requires snapshot or watch")),
    }
}

async fn doctor(args: &[String]) -> UseResult<CommandOutput> {
    let domain = args.first().map(String::as_str);
    let diagnostics = match domain {
        None | Some("--json" | "--scope-kind" | "--scope-id") => {
            validate_scoped_read_options(args, "doctor")?;
            let installation = managed_scope_argument(args)?;
            let mut diagnostics = vec![
                browser_diagnostic(),
                ocr_diagnostic(),
                crate::component_route::box_diagnostic(),
            ];
            diagnostics.extend(
                installed_extensions(installation)
                    .await?
                    .iter()
                    .map(extension_diagnostic),
            );
            diagnostics
        }
        Some("browser") => vec![browser_diagnostic()],
        Some("box") => vec![crate::component_route::box_diagnostic()],
        Some("ocr") => vec![ocr_diagnostic()],
        Some(value) => {
            match installed_extension_for_id(managed_scope_argument(args)?, value).await? {
                Some(extension) => vec![extension_diagnostic(&extension)],
                None => {
                    return Err(UseError::new(
                        "use.domain_unknown",
                        format!("Unknown domain '{value}'."),
                    )
                    .with_suggestion(
                        "Install the external capability or run 'a3s use capabilities --json'.",
                    ))
                }
            }
        }
    };
    let ready = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.readiness == Readiness::Ready)
        .count();
    Ok(CommandOutput::success(
        format!("{ready}/{} domains ready", diagnostics.len()),
        serde_json::json!({ "diagnostics": diagnostics }),
    ))
}

async fn browser(args: &[String]) -> UseResult<CommandOutput> {
    #[cfg(feature = "browser")]
    {
        // `render` is the small, in-process typed surface used by Search and
        // embedding applications. Every interactive/automation command is
        // handled by the full Browser driver so `a3s use browser` has one
        // agent-browser-compatible command vocabulary.
        if args.first().map(String::as_str) == Some("render") {
            return crate::browser_cli::run(args).await;
        }
        let exit_code = crate::browser_driver::run(args).await?;
        Ok(CommandOutput::delegated(exit_code))
    }
    #[cfg(not(feature = "browser"))]
    {
        let _ = args;
        Err(UseError::new(
            "use.browser.disabled",
            "Browser support is disabled in this custom build.",
        ))
    }
}

async fn extension(args: &[String]) -> UseResult<CommandOutput> {
    let installation = managed_scope_argument(args)?;
    match args.first().map(String::as_str) {
        Some("list") => extension_list(installation).await,
        Some("inspect" | "doctor") => {
            let package_id = value_argument(args, 1, "extension inspect requires an ID")?;
            extension_inspect(installation, package_id).await
        }
        Some("diagnose") => {
            validate_extension_diagnostic_options(args)?;
            let package_id = value_argument(args, 1, "extension diagnose requires an ID")?;
            extension_operation_diagnostic(
                package_id,
                installation,
                flag_argument(args, "--history")?,
            )
            .await
        }
        Some("planning-evidence") => {
            validate_extension_options(args, 2, false)?;
            let package_id = value_argument(args, 1, "extension planning-evidence requires an ID")?;
            extension_planning_evidence(installation, package_id).await
        }
        Some("snapshot") => {
            validate_extension_options(args, 1, false)?;
            extension_snapshot(installation).await
        }
        Some("watch") => {
            validate_extension_watch_options(args)?;
            let after_generation = integer_option(args, "--after-generation", 0)?;
            let timeout = duration_option(args, "--timeout-ms", 30_000)?;
            extension_watch(installation, after_generation, timeout).await
        }
        Some(command) => Err(UseError::new(
            "use.extension.command_unknown",
            format!("Unknown extension command '{command}'."),
        )),
        None => Err(usage_error("extension requires an explicit command")),
    }
}

include!("cli_mcp.rs");
#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
