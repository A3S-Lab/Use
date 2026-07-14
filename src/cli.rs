use a3s_use_core::{DomainDiagnostic, Readiness, UseError, UseResult};

use crate::extension_cli::{
    extension_capabilities, extension_inspect, extension_list, external_component_value,
    external_package_id, install_extension, installed_extension, installed_extensions,
    uninstall_extension,
};

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

    #[cfg(feature = "extensions")]
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
        "doctor" => doctor(args.get(1).map(String::as_str)),
        "component" => component(&args[1..]).await,
        "browser" => browser(&args[1..]),
        "office" => office(&args[1..]),
        "extension" => extension(&args[1..]).await,
        "mcp" => mcp(&args[1..]),
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
            "  a3s-use doctor [browser|office] [--json]\n",
            "  a3s-use component list|status|install|uninstall [args] [--json]\n",
            "  a3s-use browser doctor [--json]\n",
            "  a3s-use office doctor [--json]\n",
            "  a3s-use extension list|inspect|doctor [args] [--json]\n",
            "  a3s-use mcp stop [--json]"
        ),
        serde_json::json!({
            "commands": [
                "capabilities",
                "doctor",
                "component",
                "browser",
                "office",
                "extension",
                "mcp"
            ]
        }),
    )
}

async fn capabilities() -> UseResult<CommandOutput> {
    let browser = browser_diagnostic();
    let office = office_diagnostic();
    let extensions = extension_capabilities().await?;
    Ok(CommandOutput::success(
        "Built-in domains: browser, office",
        serde_json::json!({
            "domains": [
                {
                    "id": "browser",
                    "builtIn": true,
                    "readiness": browser.readiness,
                    "surfaces": ["cli", "mcp"]
                },
                {
                    "id": "office",
                    "builtIn": true,
                    "readiness": office.readiness,
                    "surfaces": ["cli", "mcp"]
                }
            ],
            "externalSurfaces": ["cli", "mcp", "skill"],
            "extensions": extensions
        }),
    ))
}

fn doctor(domain: Option<&str>) -> UseResult<CommandOutput> {
    let diagnostics = match domain {
        None | Some("--json") => vec![browser_diagnostic(), office_diagnostic()],
        Some("browser") => vec![browser_diagnostic()],
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
    let office = component_value("office", &office_diagnostic());
    let extensions = installed_extensions().await?;
    let mut components = vec![browser, office];
    components.extend(
        extensions
            .iter()
            .map(|extension| external_component_value(extension, false)),
    );
    let mut human = vec!["browser".to_string(), "office".to_string()];
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
                    "Extension '{}' is ready on route '{}'.",
                    extension.package_id, extension.route
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

fn browser(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("doctor") => doctor(Some("browser")),
        Some(command) => Err(UseError::new(
            "use.browser.command_unavailable",
            format!("Browser command '{command}' is not implemented yet."),
        )
        .with_suggestion("Run 'a3s use browser doctor --json'.")),
    }
}

fn office(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("doctor") => doctor(Some("office")),
        Some(command) => Err(UseError::new(
            "use.office.command_unavailable",
            format!("Office command '{command}' is not implemented yet."),
        )
        .with_suggestion("Run 'a3s use office doctor --json'.")),
    }
}

async fn extension(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("list") => extension_list().await,
        Some("inspect" | "doctor") => {
            let package_id = value_argument(args, 1, "extension inspect requires an ID")?;
            extension_inspect(package_id).await
        }
        Some(command) => Err(UseError::new(
            "use.extension.command_unknown",
            format!("Unknown extension command '{command}'."),
        )),
    }
}

fn mcp(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        Some("stop") => Ok(CommandOutput::success(
            "No persistent MCP service is running.",
            serde_json::json!({
                "running": false,
                "stopped": false,
                "protocol": "mcp"
            }),
        )),
        Some("serve") => Err(UseError::new(
            "use.mcp.unavailable",
            "The standard MCP server is not enabled in this initial release.",
        )),
        _ => Err(usage_error("mcp requires serve or stop")),
    }
}

fn component_value(id: &str, diagnostic: &DomainDiagnostic) -> serde_json::Value {
    let (presence, health) = match diagnostic.readiness {
        Readiness::Ready => ("system", "ready"),
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

fn builtin_diagnostic(id: &str) -> Option<DomainDiagnostic> {
    match id {
        "browser" | "use/browser" => Some(browser_diagnostic()),
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
