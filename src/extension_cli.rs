use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};

use crate::cli::CommandOutput;

#[derive(Debug, Clone)]
pub(crate) struct ExtensionView {
    pub package_id: String,
    pub component_id: String,
    pub route: String,
    pub version: String,
    pub package_root: PathBuf,
    pub surfaces: Vec<&'static str>,
    pub manifest: serde_json::Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtensionInstallView {
    pub changed: bool,
    pub extension: ExtensionView,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtensionUninstallView {
    pub package_id: String,
    pub changed: bool,
}

pub(crate) fn external_package_id(id: &str) -> Option<&str> {
    let id = id.strip_prefix("use/").unwrap_or(id);
    let mut segments = id.split('/');
    match (segments.next(), segments.next(), segments.next()) {
        (Some(publisher), Some(name), None) if !publisher.is_empty() && !name.is_empty() => {
            Some(id)
        }
        _ => None,
    }
}

pub(crate) async fn extension_capabilities() -> UseResult<Vec<serde_json::Value>> {
    Ok(installed_extensions()
        .await?
        .into_iter()
        .map(|extension| {
            serde_json::json!({
                "id": extension.package_id,
                "route": extension.route,
                "version": extension.version,
                "surfaces": extension.surfaces,
                "builtIn": false
            })
        })
        .collect())
}

pub(crate) async fn extension_list() -> UseResult<CommandOutput> {
    let extensions = installed_extensions().await?;
    let human = if extensions.is_empty() {
        "No external Use extensions are installed.".to_string()
    } else {
        extensions
            .iter()
            .map(|extension| {
                format!(
                    "{}\t{}\t{}",
                    extension.package_id, extension.route, extension.version
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let values = extensions.iter().map(extension_value).collect::<Vec<_>>();
    Ok(CommandOutput::success(
        human,
        serde_json::json!({ "extensions": values }),
    ))
}

pub(crate) async fn extension_inspect(package_id: &str) -> UseResult<CommandOutput> {
    let Some(extension) = installed_extension(package_id).await? else {
        return Err(UseError::new(
            "use.extension.not_installed",
            format!("Extension '{package_id}' is not installed."),
        ));
    };
    Ok(CommandOutput::success(
        format!(
            "Extension '{}' is ready on route '{}'.",
            extension.package_id, extension.route
        ),
        serde_json::json!({
            "extension": extension_value(&extension),
            "manifest": extension.manifest
        }),
    ))
}

pub(crate) fn external_component_value(
    extension: &ExtensionView,
    full_id: bool,
) -> serde_json::Value {
    serde_json::json!({
        "id": if full_id { &extension.component_id } else { &extension.package_id },
        "description": format!("External Use domain on route '{}'.", extension.route),
        "presence": "managed",
        "health": "ready",
        "version": extension.version,
        "path": extension.package_root,
        "route": extension.route,
        "surfaces": extension.surfaces,
        "trust": "local-explicit"
    })
}

fn extension_value(extension: &ExtensionView) -> serde_json::Value {
    serde_json::json!({
        "packageId": extension.package_id,
        "componentId": extension.component_id,
        "route": extension.route,
        "version": extension.version,
        "packageRoot": extension.package_root,
        "surfaces": extension.surfaces,
        "trust": "local-explicit"
    })
}

#[cfg(feature = "extensions")]
pub(crate) async fn installed_extensions() -> UseResult<Vec<ExtensionView>> {
    crate::extension_host::list()
        .await?
        .into_iter()
        .map(extension_view)
        .collect()
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn installed_extensions() -> UseResult<Vec<ExtensionView>> {
    Ok(Vec::new())
}

#[cfg(feature = "extensions")]
pub(crate) async fn installed_extension(package_id: &str) -> UseResult<Option<ExtensionView>> {
    crate::extension_host::get(package_id)
        .await?
        .map(extension_view)
        .transpose()
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn installed_extension(_package_id: &str) -> UseResult<Option<ExtensionView>> {
    Ok(None)
}

#[cfg(feature = "extensions")]
pub(crate) async fn install_extension(
    package_id: &str,
    source: &Path,
    force: bool,
    allow_unsigned: bool,
) -> UseResult<ExtensionInstallView> {
    let result = crate::extension_host::install(package_id, source, force, allow_unsigned).await?;
    Ok(ExtensionInstallView {
        changed: result.changed,
        extension: extension_view(result.extension)?,
    })
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn install_extension(
    _package_id: &str,
    _source: &Path,
    _force: bool,
    _allow_unsigned: bool,
) -> UseResult<ExtensionInstallView> {
    Err(extensions_disabled())
}

#[cfg(feature = "extensions")]
pub(crate) async fn uninstall_extension(package_id: &str) -> UseResult<ExtensionUninstallView> {
    let result = crate::extension_host::uninstall(package_id).await?;
    Ok(ExtensionUninstallView {
        package_id: result.package_id,
        changed: result.changed,
    })
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn uninstall_extension(_package_id: &str) -> UseResult<ExtensionUninstallView> {
    Err(extensions_disabled())
}

#[cfg(feature = "extensions")]
fn extension_view(extension: a3s_use_extension::InstalledExtension) -> UseResult<ExtensionView> {
    let surfaces = extension.surfaces();
    let manifest = serde_json::to_value(&extension.manifest).map_err(|error| {
        UseError::new(
            "use.extension.manifest_invalid",
            format!("Failed to encode the installed extension manifest: {error}"),
        )
    })?;
    Ok(ExtensionView {
        package_id: extension.receipt.package_id,
        component_id: extension.receipt.component_id,
        route: extension.receipt.route,
        version: extension.receipt.version,
        package_root: extension.receipt.package_root,
        surfaces,
        manifest,
    })
}

#[cfg(not(feature = "extensions"))]
fn extensions_disabled() -> UseError {
    UseError::new(
        "use.extension.disabled",
        "External extension support is disabled in this custom build.",
    )
}
