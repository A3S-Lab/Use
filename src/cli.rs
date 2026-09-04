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

async fn mcp(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        Some("start") => mcp_start(args).await,
        Some("status") => mcp_status(args).await,
        Some("stop") => mcp_stop(args).await,
        Some("serve") => {
            let target = value_argument(args, 1, "mcp serve requires a domain or package ID")?;
            match target {
                "manager" | "package-manager" | "use/package-manager" => {
                    #[cfg(all(feature = "extensions", feature = "mcp"))]
                    {
                        mcp_serve_manager(args).await?;
                        Ok(CommandOutput::delegated(0))
                    }
                    #[cfg(not(all(feature = "extensions", feature = "mcp")))]
                    Err(UseError::new(
                        "use.mcp.disabled",
                        "The standard Plugin Manager MCP endpoint requires the 'extensions' and 'mcp' features.",
                    ))
                }
                "browser" | "use/browser" => {
                    #[cfg(feature = "browser")]
                    {
                        if args.len() == 5
                            && args[2] == "--streamable-http"
                            && args[3] == "--runtime-dir"
                            && !args[4].starts_with('-')
                        {
                            #[cfg(feature = "mcp")]
                            crate::mcp::serve_browser_http(args[4].clone().into()).await?;
                            #[cfg(not(feature = "mcp"))]
                            return Err(UseError::new(
                                "use.mcp.disabled",
                                "Managed Browser MCP HTTP support is disabled in this custom build.",
                            ));
                            Ok(CommandOutput::delegated(0))
                        } else if args[2..]
                            .iter()
                            .any(|argument| argument == "--streamable-http")
                        {
                            Err(usage_error(
                                "mcp serve browser --streamable-http requires '--runtime-dir <path>'",
                            ))
                        } else {
                            let mut driver_args = vec!["mcp".to_string()];
                            driver_args.extend_from_slice(&args[2..]);
                            let exit_code = crate::browser_driver::run(&driver_args).await?;
                            Ok(CommandOutput::delegated(exit_code))
                        }
                    }
                    #[cfg(not(feature = "browser"))]
                    Err(UseError::new(
                        "use.mcp.disabled",
                        "Standard Browser MCP support is disabled in this custom build.",
                    ))
                }
                "ocr" | "use/ocr" | "ocr-native" | "use/ocr-native" => {
                    if args.len() != 2 {
                        return Err(usage_error("mcp serve ocr accepts exactly one target"));
                    }
                    #[cfg(all(feature = "ocr", feature = "mcp"))]
                    {
                        a3s_use_ocr::OcrMcpServer::from_env()?.serve_stdio().await?;
                        Ok(CommandOutput::delegated(0))
                    }
                    #[cfg(not(all(feature = "ocr", feature = "mcp")))]
                    Err(UseError::new(
                        "use.mcp.disabled",
                        "OCR MCP support is disabled in this custom build.",
                    ))
                }
                value => Err(UseError::new(
                    "use.mcp.target_unknown",
                    format!("Unknown MCP target '{value}'."),
                )),
            }
        }
        _ => Err(usage_error("mcp requires start, status, stop, or serve")),
    }
}

#[cfg(all(feature = "extensions", feature = "mcp"))]
async fn mcp_serve_manager(args: &[String]) -> UseResult<()> {
    validate_manager_mcp_args(args)?;
    let installation = managed_scope_argument(args)?;
    let access = if flag_argument(args, "--offline")? {
        crate::cognitive_package::CognitiveRegistryAccess::Cached
    } else {
        crate::cognitive_package::CognitiveRegistryAccess::Refreshed
    };
    let service = crate::extension_cli::standalone_plugin_manager_service(installation)?;
    let server = crate::plugin_manager::PluginManagerMcpServer::with_registry_access(
        service,
        access,
        std::sync::Arc::new(crate::plugin_manager::FailClosedPluginManagerConfirmationProvider),
    )?;
    server.serve_stdio().await
}

#[cfg(all(feature = "extensions", feature = "mcp"))]
fn validate_manager_mcp_args(args: &[String]) -> UseResult<()> {
    // `args` still contains the `mcp` command's complete argument vector, so
    // the target occupies index 1 and all endpoint options begin at index 2.
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--offline" => index += 1,
            "--scope-kind" | "--scope-id" => {
                if args
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            "--json" => {
                return Err(usage_error(
                    "mcp serve manager speaks standard MCP on stdout; remove --json",
                ))
            }
            value => {
                return Err(usage_error(format!(
                    "unknown mcp serve manager option '{value}'"
                )))
            }
        }
    }
    // Validate duplicate options and require both parts of the installation
    // identity before constructing any Registry or lifecycle state.
    let _ = managed_scope_argument(args)?;
    flag_argument(args, "--offline")?;
    Ok(())
}

async fn mcp_start(args: &[String]) -> UseResult<CommandOutput> {
    validate_mcp_management_args(args, "start")?;
    #[cfg(all(feature = "browser", feature = "mcp"))]
    {
        let status = crate::mcp::ensure_browser_service().await?;
        let human = format!(
            "Browser MCP service is running at {}.",
            status
                .endpoint
                .as_deref()
                .unwrap_or("its loopback endpoint")
        );
        Ok(CommandOutput::success(
            human,
            serde_json::to_value(status).map_err(output_encoding_error)?,
        ))
    }
    #[cfg(not(all(feature = "browser", feature = "mcp")))]
    Err(UseError::new(
        "use.mcp.disabled",
        "Persistent Browser MCP support is disabled in this custom build.",
    ))
}

async fn mcp_status(args: &[String]) -> UseResult<CommandOutput> {
    validate_mcp_management_args(args, "status")?;
    #[cfg(all(feature = "browser", feature = "mcp"))]
    {
        let status = crate::mcp::browser_service_status().await?;
        let human = if status.running {
            format!(
                "Browser MCP service is running at {}.",
                status
                    .endpoint
                    .as_deref()
                    .unwrap_or("its loopback endpoint")
            )
        } else {
            "No persistent Browser MCP service is running.".to_string()
        };
        Ok(CommandOutput::success(
            human,
            serde_json::to_value(status).map_err(output_encoding_error)?,
        ))
    }
    #[cfg(not(all(feature = "browser", feature = "mcp")))]
    Ok(CommandOutput::success(
        "No persistent Browser MCP service is running.",
        serde_json::json!({
            "running": false,
            "stopped": false,
            "protocol": "mcp-streamable-http"
        }),
    ))
}

async fn mcp_stop(args: &[String]) -> UseResult<CommandOutput> {
    validate_mcp_management_args(args, "stop")?;
    #[cfg(all(feature = "browser", feature = "mcp"))]
    {
        let status = crate::mcp::stop_browser_service().await?;
        let human = if status.stopped {
            "Stopped the persistent Browser MCP service."
        } else {
            "No persistent Browser MCP service is running."
        };
        Ok(CommandOutput::success(
            human,
            serde_json::to_value(status).map_err(output_encoding_error)?,
        ))
    }
    #[cfg(not(all(feature = "browser", feature = "mcp")))]
    Ok(CommandOutput::success(
        "No persistent Browser MCP service is running.",
        serde_json::json!({
            "running": false,
            "stopped": false,
            "protocol": "mcp-streamable-http"
        }),
    ))
}

fn validate_mcp_management_args(args: &[String], command: &str) -> UseResult<()> {
    for argument in &args[1..] {
        if !matches!(argument.as_str(), "browser" | "use/browser" | "--json") {
            return Err(usage_error(format!(
                "mcp {command} accepts only the optional Browser target and --json"
            )));
        }
    }
    let target_count = args[1..]
        .iter()
        .filter(|argument| matches!(argument.as_str(), "browser" | "use/browser"))
        .count();
    if target_count > 1 {
        return Err(usage_error(format!(
            "mcp {command} accepts the Browser target only once"
        )));
    }
    Ok(())
}

#[cfg(all(feature = "browser", feature = "mcp"))]
fn output_encoding_error(error: serde_json::Error) -> UseError {
    UseError::new(
        "use.cli.output_invalid",
        format!("Failed to encode command output: {error}"),
    )
}

fn component_value(id: &str, diagnostic: &DomainDiagnostic) -> serde_json::Value {
    let (presence, health) = match diagnostic.readiness {
        Readiness::Ready => (builtin_presence(id), "ready"),
        Readiness::Missing => ("missing", "unknown"),
        Readiness::Broken => ("external", "broken"),
        Readiness::Unknown => ("missing", "unknown"),
    };
    serde_json::json!({
        "id": id,
        "description": diagnostic.message,
        "presence": presence,
        "health": health,
        "version": diagnostic.version,
        "path": diagnostic.path
    })
}

fn extension_diagnostic(extension: &crate::extension_cli::ExtensionView) -> DomainDiagnostic {
    let (readiness, message, suggestions) = if !extension.compatible {
        (
            Readiness::Broken,
            format!(
                "Extension '{}' {} is incompatible with A3S Use {}.",
                extension.package_id,
                extension.version,
                env!("CARGO_PKG_VERSION")
            ),
            vec!["Install a compatible extension version or update A3S Use.".to_string()],
        )
    } else if extension.enabled {
        (
            Readiness::Ready,
            match extension.alias.as_deref() {
                Some(alias) => format!(
                    "Extension '{}' is ready with CLI alias '{}'.",
                    extension.package_id, alias
                ),
                None => format!("Extension '{}' is ready.", extension.package_id),
            },
            Vec::new(),
        )
    } else {
        (
            Readiness::Unknown,
            format!(
                "Extension '{}' is installed but disabled.",
                extension.package_id
            ),
            vec![
                "Create and apply a reviewed enablement plan through the package manager."
                    .to_string(),
            ],
        )
    };
    DomainDiagnostic {
        domain: extension.component_id.clone(),
        readiness,
        provider: Some(extension.package_id.clone()),
        version: Some(extension.version.clone()),
        path: Some(extension.package_root.clone()),
        message,
        suggestions,
    }
}

fn builtin_presence(id: &str) -> &'static str {
    match id {
        #[cfg(feature = "browser")]
        "browser" | "use/browser" => browser_presence(
            a3s_use_browser::browser_status(a3s_use_browser::ManagedBrowser::Chrome).source,
        ),
        #[cfg(feature = "ocr")]
        "ocr" | "use/ocr" => ocr_presence(a3s_use_ocr::ocr_status().source),
        _ => "external",
    }
}

#[cfg(feature = "browser")]
fn browser_presence(source: a3s_use_browser::BrowserInstallSource) -> &'static str {
    match source {
        a3s_use_browser::BrowserInstallSource::Environment => "external",
        a3s_use_browser::BrowserInstallSource::System => "system",
        a3s_use_browser::BrowserInstallSource::ManagedCache => "managed",
        a3s_use_browser::BrowserInstallSource::Missing
        | a3s_use_browser::BrowserInstallSource::Unsupported => "missing",
    }
}

#[cfg(feature = "ocr")]
fn ocr_presence(source: a3s_use_ocr::OcrInstallSource) -> &'static str {
    match source {
        a3s_use_ocr::OcrInstallSource::Environment => "external",
        a3s_use_ocr::OcrInstallSource::Packaged => "packaged",
        a3s_use_ocr::OcrInstallSource::Managed => "managed",
        a3s_use_ocr::OcrInstallSource::Missing => "missing",
    }
}

fn builtin_diagnostic(id: &str) -> Option<DomainDiagnostic> {
    match id {
        "browser" | "use/browser" => Some(browser_diagnostic()),
        "box" | "use/box" => Some(crate::component_route::box_diagnostic()),
        "ocr" | "use/ocr" => Some(ocr_diagnostic()),
        _ => None,
    }
}

fn option_argument<'a>(args: &'a [String], name: &str) -> UseResult<Option<&'a str>> {
    let mut value = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            if value.is_some() {
                return Err(usage_error(format!("{name} may be provided only once")));
            }
            value = Some(
                args.get(index + 1)
                    .map(String::as_str)
                    .filter(|candidate| !candidate.starts_with('-'))
                    .ok_or_else(|| usage_error(format!("{name} requires a value")))?,
            );
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(value)
}

fn flag_argument(args: &[String], name: &str) -> UseResult<bool> {
    let count = args
        .iter()
        .filter(|argument| argument.as_str() == name)
        .count();
    if count > 1 {
        Err(usage_error(format!("{name} may be provided only once")))
    } else {
        Ok(count == 1)
    }
}

fn validate_component_install_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--force" | "--offline" => index += 1,
            "--registry-name"
            | "--version"
            | "--channel"
            | "--package-lock-digest"
            | "--scope-kind"
            | "--scope-id" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown component install option '{value}'"
                )))
            }
        }
    }
    Ok(())
}

fn validate_component_upgrade_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--offline" => index += 1,
            "--registry-name"
            | "--version"
            | "--channel"
            | "--package-lock-digest"
            | "--scope-kind"
            | "--scope-id" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown component upgrade option '{value}'"
                )))
            }
        }
    }
    Ok(())
}

fn validate_extension_options(
    args: &[String],
    first_option: usize,
    allow_timeout: bool,
) -> UseResult<()> {
    let mut index = first_option;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--timeout-ms" if allow_timeout => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error("--timeout-ms requires a value"));
                }
                index += 2;
            }
            "--scope-kind" | "--scope-id" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown extension option '{value}'"))),
        }
    }
    Ok(())
}

fn validate_extension_watch_options(args: &[String]) -> UseResult<()> {
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--after-generation" | "--timeout-ms" | "--scope-kind" | "--scope-id" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown extension watch option '{value}'"
                )))
            }
        }
    }
    Ok(())
}

fn validate_extension_diagnostic_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--history" => index += 1,
            "--scope-kind" | "--scope-id" => {
                if args
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown extension diagnose option '{value}'"
                )))
            }
        }
    }
    Ok(())
}

fn managed_scope_argument(args: &[String]) -> UseResult<a3s_use_core::PlanScope> {
    let kind = match option_argument(args, "--scope-kind")?.ok_or_else(|| {
        usage_error("--scope-kind <user|workspace> is required for installation-scoped commands")
    })? {
        "user" => a3s_use_core::PlanScopeKind::User,
        "workspace" => a3s_use_core::PlanScopeKind::Workspace,
        value => {
            return Err(usage_error(format!(
                "--scope-kind must be 'user' or 'workspace', received '{value}'"
            )))
        }
    };
    let scope_id = option_argument(args, "--scope-id")?.ok_or_else(|| {
        usage_error("--scope-id <id> is required for installation-scoped commands")
    })?;
    a3s_use_core::InstallationId::new(kind, scope_id)
}

fn validate_capability_options(args: &[String], watch: bool) -> UseResult<()> {
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--after-generation" | "--after-revision" | "--timeout-ms" if watch => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            "--scope-kind" | "--scope-id" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown capability option '{value}'"))),
        }
    }
    Ok(())
}

fn validate_scoped_read_options(args: &[String], command: &str) -> UseResult<()> {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--scope-kind" | "--scope-id" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => return Err(usage_error(format!("unknown {command} option '{value}'"))),
        }
    }
    Ok(())
}

fn integer_option(args: &[String], name: &str, default: u64) -> UseResult<u64> {
    let Some(value) = option_argument(args, name)? else {
        return Ok(default);
    };
    value.parse::<u64>().map_err(|_| {
        usage_error(format!(
            "{name} must be a non-negative integer, received '{value}'"
        ))
    })
}

fn duration_option(args: &[String], name: &str, default_ms: u64) -> UseResult<Duration> {
    Ok(Duration::from_millis(integer_option(
        args, name, default_ms,
    )?))
}

#[cfg(feature = "browser")]
fn browser_diagnostic() -> DomainDiagnostic {
    a3s_use_browser::doctor()
}

#[cfg(not(feature = "browser"))]
fn browser_diagnostic() -> DomainDiagnostic {
    disabled_diagnostic("browser")
}

#[cfg(feature = "ocr")]
fn ocr_diagnostic() -> DomainDiagnostic {
    crate::ocr_builtin::diagnostic()
}

#[cfg(not(feature = "ocr"))]
fn ocr_diagnostic() -> DomainDiagnostic {
    disabled_diagnostic("ocr")
}

#[cfg(any(not(feature = "browser"), not(feature = "ocr")))]
fn disabled_diagnostic(domain: &str) -> DomainDiagnostic {
    DomainDiagnostic {
        domain: domain.to_string(),
        readiness: Readiness::Missing,
        provider: None,
        version: None,
        path: None,
        message: format!("The '{domain}' feature is disabled in this custom build."),
        suggestions: Vec::new(),
    }
}

#[cfg(feature = "ocr")]
async fn ocr(args: &[String]) -> UseResult<CommandOutput> {
    let output = a3s_use_ocr::cli::run(args.to_vec()).await?;
    Ok(CommandOutput {
        human: output.human,
        json: output.json,
        exit_code: output.exit_code,
        should_print: output.should_print,
    })
}

#[cfg(not(feature = "ocr"))]
async fn ocr(_args: &[String]) -> UseResult<CommandOutput> {
    Err(UseError::new(
        "use.ocr.disabled",
        "OCR support is disabled in this custom build.",
    ))
}

fn value_argument<'a>(args: &'a [String], index: usize, message: &str) -> UseResult<&'a str> {
    args.get(index)
        .map(String::as_str)
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| usage_error(message))
}

fn usage_error(message: impl Into<String>) -> UseError {
    UseError::new("use.cli.invalid_usage", message)
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
