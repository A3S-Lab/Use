use a3s_use_core::{DomainDiagnostic, Readiness, UseError, UseResult};

use crate::capability_registry::{
    snapshot as capability_registry_snapshot, wait_for_change as wait_for_capability_change,
};
use crate::extension_cli::{
    extension_capabilities, extension_disable, extension_enable, extension_inspect, extension_list,
    extension_snapshot, extension_watch, external_component_value, external_package_id,
    install_extension, installed_extension, installed_extensions, uninstall_extension,
};
use std::time::Duration;

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
        "capabilities" => capabilities().await,
        "capability" => capability(&args[1..]).await,
        "doctor" => doctor(args.get(1).map(String::as_str)),
        "component" => component(&args[1..]).await,
        "browser" => browser(&args[1..]).await,
        "box" => {
            let exit_code = crate::component_route::run_box(&args[1..]).await?;
            Ok(CommandOutput::delegated(exit_code))
        }
        "office" => office(&args[1..]).await,
        "extension" => extension(&args[1..]).await,
        "mcp" => mcp(&args[1..]).await,
        route => {
            #[cfg(feature = "extensions")]
            if let Some(exit_code) = crate::extension_host::run_route(route, &args[1..]).await? {
                return Ok(CommandOutput::delegated(exit_code));
            }
            Err(
                UseError::new("use.route_unknown", format!("Unknown Use route '{route}'."))
                    .with_suggestion("Run 'a3s use capabilities --json'."),
            )
        }
    }
}

fn version() -> CommandOutput {
    CommandOutput {
        human: format!("a3s-use {}", env!("CARGO_PKG_VERSION")),
        json: serde_json::json!({
            "schemaVersion": 1,
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
        }),
        exit_code: 0,
        should_print: true,
    }
}

fn help() -> CommandOutput {
    CommandOutput::success(
        concat!(
            "a3s-use — typed application capabilities\n\n",
            "usage:\n",
            "  a3s-use capabilities [--json]\n",
            "  a3s-use capability snapshot [--json]\n",
            "  a3s-use capability watch [--after-generation <n>] [--after-revision <sha256>] [--timeout-ms <ms>] [--json]\n",
            "  a3s-use doctor [browser|box|office] [--json]\n",
            "  a3s-use component list|status|install|uninstall [args] [--json]\n",
            "  a3s-use browser doctor [--json]\n",
            "  a3s-use browser render <url> [--output <path>] [--screenshot <path>] [--json]\n",
            "  a3s-use browser open|list|navigate|snapshot|click|type|press|select|scroll|screenshot|close [args] [--json]\n",
            "  a3s-use box <a3s-box-args...>\n",
            "  a3s-use office doctor [--json]\n",
            "  a3s-use office native get|query|view|validate|create|add|set|remove|batch [args] [--json]\n",
            "  a3s-use office <officecli-args...>\n",
            "  a3s-use extension list|inspect|doctor [args] [--json]\n",
            "  a3s-use extension enable <publisher/name> [--json]\n",
            "  a3s-use extension disable <publisher/name> [--timeout-ms <ms>] [--json]\n",
            "  a3s-use extension snapshot|watch [--after-generation <n>] [--timeout-ms <ms>] [--json]\n",
            "  a3s-use mcp serve browser [--tools <profiles>]\n",
            "  a3s-use mcp serve office|<publisher/name>\n",
            "  a3s-use mcp start|status|stop [browser] [--json]"
        ),
        serde_json::json!({
            "commands": [
                "capabilities",
                "capability",
                "doctor",
                "component",
                "browser",
                "box",
                "office",
                "extension",
                "mcp"
            ]
        }),
    )
}

async fn capabilities() -> UseResult<CommandOutput> {
    let browser = browser_diagnostic();
    let box_domain = crate::component_route::box_diagnostic();
    let office = office_diagnostic();
    let (extension_generation, extensions) = extension_capabilities().await?;
    Ok(CommandOutput::success(
        "Built-in routes: browser, box, office",
        serde_json::json!({
            "domains": [
                {
                    "id": "browser",
                    "builtIn": true,
                    "readiness": browser.readiness,
                    "surfaces": ["cli", "mcp", "skill"]
                },
                {
                    "id": "office",
                    "builtIn": true,
                    "readiness": office.readiness,
                    "surfaces": ["cli", "mcp"]
                },
                {
                    "id": "box",
                    "builtIn": true,
                    "readiness": box_domain.readiness,
                    "surfaces": ["cli"]
                }
            ],
            "externalSurfaces": ["cli", "mcp", "skill"],
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
    match args.first().map(String::as_str) {
        Some("snapshot") => {
            validate_capability_options(args, false)?;
            let snapshot = capability_registry_snapshot().await?;
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
            match wait_for_capability_change(after_generation, after_revision, timeout).await? {
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

fn doctor(domain: Option<&str>) -> UseResult<CommandOutput> {
    let diagnostics = match domain {
        None | Some("--json") => vec![
            browser_diagnostic(),
            office_diagnostic(),
            crate::component_route::box_diagnostic(),
        ],
        Some("browser") => vec![browser_diagnostic()],
        Some("box") => vec![crate::component_route::box_diagnostic()],
        Some("office") => vec![office_diagnostic()],
        Some(value) => {
            return Err(UseError::new(
                "use.domain_unknown",
                format!("Unknown domain '{value}'."),
            ))
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

async fn component(args: &[String]) -> UseResult<CommandOutput> {
    let command = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| usage_error("component requires list, status, install, or uninstall"))?;
    match command {
        "list" => component_list().await,
        "status" => {
            let id = value_argument(args, 1, "component status requires an ID")?;
            component_status(id).await
        }
        "install" => component_install(args).await,
        "uninstall" => {
            let id = value_argument(args, 1, "component uninstall requires an ID")?;
            component_uninstall(id).await
        }
        value => Err(usage_error(format!("unknown component command '{value}'"))),
    }
}

async fn component_list() -> UseResult<CommandOutput> {
    let browser = component_value("browser", &browser_diagnostic());
    let box_component = component_value("box", &crate::component_route::box_diagnostic());
    let office = component_value("office", &office_diagnostic());
    let extensions = installed_extensions().await?;
    let mut components = vec![browser, box_component, office];
    components.extend(
        extensions
            .iter()
            .map(|extension| external_component_value(extension, false)),
    );
    let mut human = vec![
        "browser".to_string(),
        "box".to_string(),
        "office".to_string(),
    ];
    human.extend(
        extensions
            .iter()
            .map(|extension| format!("use/{}", extension.package_id)),
    );
    Ok(CommandOutput::success(
        human.join("\n"),
        serde_json::json!({ "components": components }),
    ))
}

async fn component_status(id: &str) -> UseResult<CommandOutput> {
    if let Some(diagnostic) = builtin_diagnostic(id) {
        return Ok(CommandOutput {
            human: diagnostic.message.clone(),
            json: serde_json::json!({
                "schemaVersion": 1,
                "ok": true,
                "component": component_value(id, &diagnostic),
            }),
            exit_code: 0,
            should_print: true,
        });
    }
    if let Some(package_id) = external_package_id(id) {
        if let Some(extension) = installed_extension(package_id).await? {
            return Ok(CommandOutput {
                human: format!(
                    "Extension '{}' is {} on route '{}'.",
                    extension.package_id,
                    if extension.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    extension.route
                ),
                json: serde_json::json!({
                    "schemaVersion": 1,
                    "ok": true,
                    "component": external_component_value(&extension, id.starts_with("use/")),
                }),
                exit_code: 0,
                should_print: true,
            });
        }
    }
    Err(UseError::new(
        "use.component_unknown",
        format!("Unknown delegated component '{id}'."),
    ))
}

async fn component_install(args: &[String]) -> UseResult<CommandOutput> {
    let id = value_argument(args, 1, "component install requires an ID")?;
    validate_component_install_options(args)?;
    if matches!(id, "browser" | "use/browser") {
        #[cfg(feature = "browser")]
        {
            let force = args.iter().any(|argument| argument == "--force");
            let previous = a3s_use_browser::browser_status(a3s_use_browser::ManagedBrowser::Chrome);
            let status = if force {
                a3s_use_browser::update_browser(a3s_use_browser::ManagedBrowser::Chrome).await?
            } else {
                a3s_use_browser::install_browser(a3s_use_browser::ManagedBrowser::Chrome).await?
            };
            let changed = force
                || !previous.available
                || previous.path != status.path
                || previous.source != status.source
                || previous.version != status.version;
            let diagnostic = browser_diagnostic();
            return Ok(CommandOutput::success(
                format!(
                    "Browser provider is ready at {}.",
                    status.path.as_ref().map_or_else(
                        || "an unknown path".to_string(),
                        |path| path.display().to_string()
                    )
                ),
                serde_json::json!({
                    "component": component_value(id, &diagnostic),
                    "changed": changed,
                    "provider": status
                }),
            ));
        }
    }
    if matches!(id, "office" | "use/office") {
        #[cfg(feature = "office")]
        {
            if option_argument(args, "--from")?.is_some() {
                return Err(usage_error("--from is valid only for external extensions"));
            }
            let force = args.iter().any(|argument| argument == "--force");
            let previous = a3s_use_office::office_status();
            let status = a3s_use_office::install_office_cli(force).await?;
            let changed = force
                || !previous.available
                || previous.path != status.path
                || previous.source != status.source
                || previous.version != status.version;
            let diagnostic = office_diagnostic();
            return Ok(CommandOutput::success(
                format!(
                    "OfficeCLI provider is ready at {}.",
                    status.path.as_ref().map_or_else(
                        || "an unknown path".to_string(),
                        |path| path.display().to_string()
                    )
                ),
                serde_json::json!({
                    "component": component_value(id, &diagnostic),
                    "changed": changed,
                    "provider": status
                }),
            ));
        }
    }
    if let Some(diagnostic) = builtin_diagnostic(id) {
        if option_argument(args, "--from")?.is_some() {
            return Err(usage_error("--from is valid only for external extensions"));
        }
        if diagnostic.readiness != Readiness::Ready {
            return Err(UseError::new(
                "use.runtime.install_unavailable",
                format!(
                    "Managed installation for '{}' is not available in this initial release.",
                    id
                ),
            )
            .with_suggestion(
                diagnostic
                    .suggestions
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Install a compatible system provider.".to_string()),
            ));
        }
        return Ok(CommandOutput::success(
            format!("Component '{id}' is already ready."),
            serde_json::json!({
                "component": component_value(id, &diagnostic),
                "changed": false
            }),
        ));
    }

    let Some(package_id) = external_package_id(id) else {
        return Err(UseError::new(
            "use.component_unknown",
            format!("Unknown delegated component '{id}'."),
        ));
    };
    let source = option_argument(args, "--from")?
        .ok_or_else(|| usage_error("external extension install requires --from <directory>"))?;
    let result = install_extension(
        package_id,
        std::path::Path::new(source),
        args.iter().any(|argument| argument == "--force"),
        args.iter().any(|argument| argument == "--allow-unsigned"),
    )
    .await?;
    Ok(CommandOutput::success(
        if result.changed {
            format!("Installed extension '{}'.", result.extension.package_id)
        } else {
            format!(
                "Extension '{}' is already installed.",
                result.extension.package_id
            )
        },
        serde_json::json!({
            "component": external_component_value(&result.extension, id.starts_with("use/")),
            "changed": result.changed
        }),
    ))
}

async fn component_uninstall(id: &str) -> UseResult<CommandOutput> {
    if matches!(id, "browser" | "use/browser") {
        #[cfg(feature = "browser")]
        {
            let changed = a3s_use_browser::uninstall_managed_browsers().await?;
            return Ok(CommandOutput::success(
                if changed {
                    "Removed A3S-managed Browser provider files."
                } else {
                    "No A3S-managed Browser provider files are installed."
                },
                serde_json::json!({
                    "component": id,
                    "changed": changed,
                    "builtInCommandPreserved": true
                }),
            ));
        }
    }
    if matches!(id, "office" | "use/office") {
        #[cfg(feature = "office")]
        {
            let changed = a3s_use_office::uninstall_managed_office_cli().await?;
            return Ok(CommandOutput::success(
                if changed {
                    "Removed A3S-managed OfficeCLI provider files."
                } else {
                    "No A3S-managed OfficeCLI provider files are installed."
                },
                serde_json::json!({
                    "component": id,
                    "changed": changed,
                    "builtInCommandPreserved": true
                }),
            ));
        }
    }
    if matches!(id, "browser" | "use/browser" | "office" | "use/office") {
        return Ok(CommandOutput::success(
            format!("No managed runtime files are owned for '{id}'."),
            serde_json::json!({
                "component": id,
                "changed": false,
                "builtInCommandPreserved": true
            }),
        ));
    }
    if let Some(package_id) = external_package_id(id) {
        let result = uninstall_extension(package_id).await?;
        return Ok(CommandOutput::success(
            if result.changed {
                format!("Uninstalled extension '{}'.", result.package_id)
            } else {
                format!("Extension '{}' is not installed.", result.package_id)
            },
            serde_json::json!({
                "component": format!("use/{}", result.package_id),
                "changed": result.changed
            }),
        ));
    }
    Err(UseError::new(
        "use.component_unknown",
        format!("Unknown delegated component '{id}'."),
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

async fn office(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("doctor") => doctor(Some("office")),
        Some("native") => {
            #[cfg(feature = "office")]
            return crate::office_native_cli::run(&args[1..]).await;
            #[cfg(not(feature = "office"))]
            return Err(UseError::new(
                "use.office.disabled",
                "Office support is disabled in this custom build.",
            ));
        }
        Some(_) => {
            #[cfg(feature = "office")]
            {
                let exit_code = a3s_use_office::delegate_native(args).await?;
                Ok(CommandOutput::delegated(exit_code))
            }
            #[cfg(not(feature = "office"))]
            Err(UseError::new(
                "use.office.disabled",
                "Office support is disabled in this custom build.",
            ))
        }
    }
}

async fn extension(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("list") => extension_list().await,
        Some("inspect" | "doctor") => {
            let package_id = value_argument(args, 1, "extension inspect requires an ID")?;
            extension_inspect(package_id).await
        }
        Some("enable") => {
            validate_extension_options(args, 2, false)?;
            let package_id = value_argument(args, 1, "extension enable requires an ID")?;
            extension_enable(package_id).await
        }
        Some("disable") => {
            validate_extension_options(args, 2, true)?;
            let package_id = value_argument(args, 1, "extension disable requires an ID")?;
            let timeout = duration_option(args, "--timeout-ms", 30_000)?;
            extension_disable(package_id, timeout).await
        }
        Some("snapshot") => {
            validate_extension_options(args, 1, false)?;
            extension_snapshot().await
        }
        Some("watch") => {
            validate_extension_watch_options(args)?;
            let after_generation = integer_option(args, "--after-generation", 0)?;
            let timeout = duration_option(args, "--timeout-ms", 30_000)?;
            extension_watch(after_generation, timeout).await
        }
        Some(command) => Err(UseError::new(
            "use.extension.command_unknown",
            format!("Unknown extension command '{command}'."),
        )),
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
                "office" | "use/office" => {
                    if args.len() != 2 {
                        return Err(usage_error("mcp serve office accepts exactly one target"));
                    }
                    #[cfg(feature = "office")]
                    {
                        let exit_code =
                            a3s_use_office::delegate_native(&["mcp".to_string()]).await?;
                        Ok(CommandOutput::delegated(exit_code))
                    }
                    #[cfg(not(feature = "office"))]
                    Err(UseError::new(
                        "use.office.disabled",
                        "Office support is disabled in this custom build.",
                    ))
                }
                package_id if external_package_id(package_id).is_some() => {
                    if args.len() != 2 {
                        return Err(usage_error(
                            "mcp serve for an extension accepts exactly one package ID",
                        ));
                    }
                    #[cfg(feature = "extensions")]
                    {
                        let exit_code = crate::extension_host::run_mcp(package_id).await?;
                        Ok(CommandOutput::delegated(exit_code))
                    }
                    #[cfg(not(feature = "extensions"))]
                    Err(UseError::new(
                        "use.extension.disabled",
                        "External extension support is disabled in this custom build.",
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

fn builtin_presence(id: &str) -> &'static str {
    match id {
        #[cfg(feature = "browser")]
        "browser" | "use/browser" => browser_presence(
            a3s_use_browser::browser_status(a3s_use_browser::ManagedBrowser::Chrome).source,
        ),
        #[cfg(feature = "office")]
        "office" | "use/office" => office_presence(a3s_use_office::office_status().source),
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

#[cfg(feature = "office")]
fn office_presence(source: a3s_use_office::OfficeInstallSource) -> &'static str {
    match source {
        a3s_use_office::OfficeInstallSource::Environment => "external",
        a3s_use_office::OfficeInstallSource::System => "system",
        a3s_use_office::OfficeInstallSource::Managed => "managed",
        a3s_use_office::OfficeInstallSource::Missing => "missing",
    }
}

fn builtin_diagnostic(id: &str) -> Option<DomainDiagnostic> {
    match id {
        "browser" | "use/browser" => Some(browser_diagnostic()),
        "box" | "use/box" => Some(crate::component_route::box_diagnostic()),
        "office" | "use/office" => Some(office_diagnostic()),
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

fn validate_component_install_options(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "--force" | "--allow-unsigned" => index += 1,
            "--from" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error("--from requires a value"));
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
            "--after-generation" | "--timeout-ms" => {
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
            value => return Err(usage_error(format!("unknown capability option '{value}'"))),
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

#[cfg(feature = "office")]
fn office_diagnostic() -> DomainDiagnostic {
    a3s_use_office::doctor()
}

#[cfg(not(feature = "office"))]
fn office_diagnostic() -> DomainDiagnostic {
    disabled_diagnostic("office")
}

#[cfg(any(not(feature = "browser"), not(feature = "office")))]
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
