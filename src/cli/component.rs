use super::*;

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    let command = args.first().map(String::as_str).ok_or_else(|| {
        usage_error("component requires list, status, install, upgrade, or uninstall")
    })?;
    match command {
        "list" => list(args).await,
        "status" => {
            let id = value_argument(args, 1, "component status requires an ID")?;
            status(id, args).await
        }
        "install" => install(args).await,
        "upgrade" => upgrade(args).await,
        "uninstall" => {
            let id = value_argument(args, 1, "component uninstall requires an ID")?;
            uninstall(id, args).await
        }
        value => Err(usage_error(format!("unknown component command '{value}'"))),
    }
}

async fn upgrade(args: &[String]) -> UseResult<CommandOutput> {
    let id = value_argument(args, 1, "component upgrade requires an ID")?;
    validate_component_upgrade_options(args)?;
    let offline = flag_argument(args, "--offline")?;
    if builtin_diagnostic(id).is_some() {
        return Err(UseError::new(
            "use.plugin.package_upgrade_unsupported",
            format!("Built-in component '{id}' is not a cognitive package graph."),
        ));
    }
    let installation = managed_scope_argument(args)?;
    let resolved = installed_extension_for_id(installation.clone(), id).await?;
    let package_id = external_package_id(id).or_else(|| {
        resolved
            .as_ref()
            .map(|extension| extension.package_id.as_str())
    });
    let package_id = package_id.ok_or_else(|| {
        UseError::new(
            "use.component_unknown",
            format!("Unknown cognitive package '{id}'."),
        )
    })?;
    let registry_name = option_argument(args, "--registry-name")?;
    let version = option_argument(args, "--version")?;
    let channel = option_argument(args, "--channel")?;
    let expected_lock = option_argument(args, "--package-lock-digest")?;
    let result = upgrade_remote_extension(
        installation,
        package_id,
        registry_name,
        version,
        channel,
        expected_lock,
        offline,
    )
    .await?;
    Ok(CommandOutput::success(
        if result.changed {
            format!(
                "Upgraded cognitive package '{}'.",
                result.extension.package_id
            )
        } else {
            format!(
                "Cognitive package '{}' already matches the resolved graph.",
                result.extension.package_id
            )
        },
        serde_json::json!({
            "component": external_component_value(&result.extension, id.starts_with("use/")),
            "changed": result.changed,
            "registryAccess": result.registry_access,
            "registrySourceRevision": result.registry_source_revision,
            "packageGraph": result.package_graph,
            "pluginManager": result.plugin_manager
        }),
    ))
}

async fn list(args: &[String]) -> UseResult<CommandOutput> {
    let browser = component_value("browser", &browser_diagnostic());
    let box_component = component_value("box", &crate::component_route::box_diagnostic());
    let ocr = component_value("ocr", &ocr_diagnostic());
    let extensions = installed_extensions(managed_scope_argument(args)?).await?;
    let mut components = vec![browser, box_component, ocr];
    components.extend(
        extensions
            .iter()
            .map(|extension| external_component_value(extension, false)),
    );
    let mut human = vec!["browser".to_string(), "box".to_string(), "ocr".to_string()];
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

async fn status(id: &str, args: &[String]) -> UseResult<CommandOutput> {
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
    if let Some(extension) = installed_extension_for_id(managed_scope_argument(args)?, id).await? {
        return Ok(CommandOutput {
            human: format!(
                "Extension '{}' is {}{}.",
                extension.package_id,
                if !extension.compatible {
                    "incompatible"
                } else if extension.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                extension
                    .alias
                    .as_deref()
                    .map(|alias| format!(" with CLI alias '{alias}'"))
                    .unwrap_or_default()
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
    Err(UseError::new(
        "use.component_unknown",
        format!("Unknown delegated component '{id}'."),
    ))
}

async fn install(args: &[String]) -> UseResult<CommandOutput> {
    let id = value_argument(args, 1, "component install requires an ID")?;
    validate_component_install_options(args)?;
    let offline = flag_argument(args, "--offline")?;
    if offline && builtin_diagnostic(id).is_some() {
        return Err(usage_error(
            "--offline is available only for Registry-backed cognitive packages",
        ));
    }
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
    if matches!(id, "ocr" | "use/ocr") {
        #[cfg(feature = "ocr")]
        {
            let force = args.iter().any(|argument| argument == "--force");
            let previous = a3s_use_ocr::ocr_status();
            let status = a3s_use_ocr::install_ppocr_v6(force).await?;
            let changed = force
                || !previous.available
                || previous.model_dir != status.model_dir
                || previous.source != status.source;
            let diagnostic = ocr_diagnostic();
            return Ok(CommandOutput::success(
                format!(
                    "Local PP-OCRv6 model bundle is ready at {}.",
                    status.model_dir.as_ref().map_or_else(
                        || "an unknown path".to_string(),
                        |path| path.display().to_string()
                    )
                ),
                serde_json::json!({
                    "component": component_value(id, &diagnostic),
                    "changed": changed,
                    "runtime": status
                }),
            ));
        }
    }
    if let Some(diagnostic) = builtin_diagnostic(id) {
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

    let installation = managed_scope_argument(args)?;
    let resolved = installed_extension_for_id(installation.clone(), id).await?;
    let package_id = external_package_id(id).or_else(|| {
        resolved
            .as_ref()
            .map(|extension| extension.package_id.as_str())
    });
    let Some(package_id) = package_id else {
        return Err(UseError::new(
            "use.component_unknown",
            format!("Unknown delegated component '{id}'."),
        )
        .with_suggestion(
            "Install external capabilities by their '<publisher>/<name>' package ID.",
        ));
    };
    if args.iter().any(|argument| argument == "--force") {
        return Err(usage_error(
            "--force is not valid for cognitive packages; apply a newly resolved package-lock plan",
        ));
    }
    let registry_name = option_argument(args, "--registry-name")?;
    let version = option_argument(args, "--version")?;
    let channel = option_argument(args, "--channel")?;
    let expected_package_lock = option_argument(args, "--package-lock-digest")?;
    let result = install_remote_extension(
        installation,
        package_id,
        registry_name,
        version,
        channel,
        expected_package_lock,
        offline,
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
            "changed": result.changed,
            "registryAccess": result.registry_access,
            "registrySourceRevision": result.registry_source_revision,
            "packageGraph": result.package_graph,
            "pluginManager": result.plugin_manager
        }),
    ))
}

async fn uninstall(id: &str, args: &[String]) -> UseResult<CommandOutput> {
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
    if matches!(id, "ocr" | "use/ocr") {
        #[cfg(feature = "ocr")]
        {
            let changed = a3s_use_ocr::uninstall_managed_ppocr_v6().await?;
            return Ok(CommandOutput::success(
                if changed {
                    "Removed A3S-managed PP-OCRv6 model files."
                } else {
                    "No A3S-managed PP-OCRv6 model files are installed."
                },
                serde_json::json!({
                    "component": id,
                    "changed": changed,
                    "builtInCommandPreserved": true
                }),
            ));
        }
    }
    if matches!(id, "browser" | "use/browser" | "ocr" | "use/ocr") {
        return Ok(CommandOutput::success(
            format!("No managed runtime files are owned for '{id}'."),
            serde_json::json!({
                "component": id,
                "changed": false,
                "builtInCommandPreserved": true
            }),
        ));
    }
    let installation = managed_scope_argument(args)?;
    if let Some(extension) = installed_extension_for_id(installation.clone(), id).await? {
        let result = uninstall_extension(installation.clone(), &extension.package_id).await?;
        return Ok(CommandOutput::success(
            if result.changed {
                format!("Uninstalled extension '{}'.", result.package_id)
            } else {
                format!("Extension '{}' is not installed.", result.package_id)
            },
            serde_json::json!({
                "component": format!("use/{}", result.package_id),
                "alias": extension.alias,
                "changed": result.changed,
                "packageGraph": result.package_graph,
                "pluginManager": result.plugin_manager
            }),
        ));
    }
    if let Some(package_id) = external_package_id(id) {
        let result = uninstall_extension(installation, package_id).await?;
        return Ok(CommandOutput::success(
            if result.changed {
                format!("Uninstalled extension '{}'.", result.package_id)
            } else {
                format!("Extension '{}' is not installed.", result.package_id)
            },
            serde_json::json!({
                "component": format!("use/{}", result.package_id),
                "changed": result.changed,
                "packageGraph": result.package_graph,
                "pluginManager": result.plugin_manager
            }),
        ));
    }
    Err(UseError::new(
        "use.component_unknown",
        format!("Unknown delegated component '{id}'."),
    ))
}
