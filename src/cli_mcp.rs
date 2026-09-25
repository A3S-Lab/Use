// CLI MCP/browser presence command handlers (included into cli).

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
                "gateway" | "capability-gateway" | "use/capability-gateway" => {
                    #[cfg(all(feature = "extensions", feature = "mcp"))]
                    {
                        mcp_serve_gateway(args).await?;
                        Ok(CommandOutput::delegated(0))
                    }
                    #[cfg(not(all(feature = "extensions", feature = "mcp")))]
                    Err(UseError::new(
                        "use.mcp.disabled",
                        "The Capability Gateway MCP endpoint requires the 'extensions' and 'mcp' features.",
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
async fn mcp_serve_gateway(args: &[String]) -> UseResult<()> {
    validate_gateway_mcp_args(args)?;
    let installation = managed_scope_argument(args)?;
    let manager = crate::cognitive_package::CognitivePackageManager::from_env(installation)?;
    let registry_name = option_argument(args, "--registry-name")?;
    // Product path: when a TrustedRegistry is selected or defaulted, load the
    // signed description trust store from Registry/TUF before Control open.
    manager
        .ensure_control_for_registry(
            registry_name,
            crate::cognitive_package::CognitiveRegistryAccess::Refreshed,
        )
        .await?;
    let options = crate::capability_gateway::CapabilityGatewayCompositionOptions::default();
    if flag_argument(args, "--streamable-http")? {
        let bind = option_argument(args, "--bind")?.unwrap_or("127.0.0.1:0");
        let token = match option_argument(args, "--token")? {
            Some(token) => token.to_owned(),
            None => {
                let mut bytes = [0_u8; 32];
                getrandom::fill(&mut bytes).map_err(|error| {
                    UseError::new(
                        "use.mcp.gateway_token_unavailable",
                        format!("Failed to generate a Gateway bearer token: {error}"),
                    )
                })?;
                bytes.iter().map(|byte| format!("{byte:02x}")).collect()
            }
        };
        let principal = option_argument(args, "--principal")?.unwrap_or("agent/cli");
        let listener = tokio::net::TcpListener::bind(bind).await.map_err(|error| {
            UseError::new(
                "use.mcp.gateway_bind_failed",
                format!("Failed to bind the Capability Gateway HTTP listener: {error}"),
            )
        })?;
        let address = listener.local_addr().map_err(|error| {
            UseError::new(
                "use.mcp.gateway_bind_failed",
                format!("Failed to read the Capability Gateway listener address: {error}"),
            )
        })?;
        let endpoint = format!("http://{address}/mcp");
        // Endpoint metadata must not share stdout with the MCP stream.
        eprintln!(
            "{}",
            serde_json::json!({
                "protocol": "mcp-streamable-http",
                "endpoint": endpoint,
                "token": token,
                "principal": principal,
            })
        );
        let shutdown = tokio_util::sync::CancellationToken::new();
        let serving = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            serving.cancel();
        });
        // Public product face: reconcile on Control publication advance and
        // drain+retain on shutdown.
        manager
            .serve_published_capability_gateway_streamable_http(
                listener,
                crate::capability_gateway::CapabilityGatewayHttpConfig::for_principal(
                    token, principal,
                )?,
                shutdown,
                options,
            )
            .await
    } else {
        manager
            .serve_published_capability_gateway_stdio(options)
            .await
    }
}

#[cfg(all(feature = "extensions", feature = "mcp"))]
fn validate_gateway_mcp_args(args: &[String]) -> UseResult<()> {
    let mut index = 2;
    let mut streamable_http = false;
    while index < args.len() {
        match args[index].as_str() {
            "--streamable-http" => {
                streamable_http = true;
                index += 1;
            }
            "--scope-kind" | "--scope-id" | "--bind" | "--token" | "--principal" | "--registry-name" => {
                if args
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            "--offline" => {
                return Err(usage_error(
                    "mcp serve gateway does not accept '--offline'; Control reopen uses the durable published catalog",
                ))
            }
            "--json" => {
                return Err(usage_error(
                    "mcp serve gateway speaks standard MCP on stdout; remove --json",
                ))
            }
            value => {
                return Err(usage_error(format!(
                    "unknown mcp serve gateway option '{value}'"
                )))
            }
        }
    }
    let _ = managed_scope_argument(args)?;
    flag_argument(args, "--streamable-http")?;
    option_argument(args, "--bind")?;
    option_argument(args, "--token")?;
    option_argument(args, "--principal")?;
    option_argument(args, "--registry-name")?;
    if !streamable_http
        && (option_argument(args, "--bind")?.is_some()
            || option_argument(args, "--token")?.is_some()
            || option_argument(args, "--principal")?.is_some())
    {
        return Err(usage_error(
            "mcp serve gateway HTTP options require '--streamable-http'",
        ));
    }
    Ok(())
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

