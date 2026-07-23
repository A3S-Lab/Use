use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};

use crate::cli::CommandOutput;

#[derive(Debug, Clone)]
pub(crate) struct ExtensionView {
    pub package_id: String,
    pub component_id: String,
    pub route: String,
    pub version: String,
    pub requires_use: Option<String>,
    pub repository: Option<serde_json::Value>,
    pub enabled: bool,
    pub compatible: bool,
    pub package_root: PathBuf,
    pub package_sha256: Option<String>,
    pub surfaces: Vec<&'static str>,
    pub trust: &'static str,
    pub registry: Option<serde_json::Value>,
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
        (Some(publisher), Some(name), None) if valid_segment(publisher) && valid_segment(name) => {
            Some(id)
        }
        _ => None,
    }
}

pub(crate) fn external_route(id: &str) -> Option<&str> {
    let route = id.strip_prefix("use/").unwrap_or(id);
    (valid_segment(route) && !route.contains('/')).then_some(route)
}

pub(crate) async fn installed_extension_for_id(id: &str) -> UseResult<Option<ExtensionView>> {
    if let Some(package_id) = external_package_id(id) {
        return installed_extension(package_id).await;
    }
    let Some(route) = external_route(id) else {
        return Ok(None);
    };
    Ok(installed_extensions()
        .await?
        .into_iter()
        .find(|extension| extension.route == route))
}

pub(crate) async fn extension_capabilities() -> UseResult<(u64, Vec<serde_json::Value>)> {
    let generation = extension_registry_generation().await?;
    let extensions = installed_extensions()
        .await?
        .into_iter()
        .map(|extension| {
            serde_json::json!({
                "id": extension.package_id,
                "route": extension.route,
                "version": extension.version,
                "requiresUse": extension.requires_use,
                "repository": extension.repository,
                "enabled": extension.enabled && extension.compatible,
                "readiness": if !extension.compatible {
                    "incompatible"
                } else if extension.enabled {
                    "ready"
                } else {
                    "disabled"
                },
                "surfaces": extension.surfaces,
                "builtIn": false
            })
        })
        .collect();
    Ok((generation, extensions))
}

pub(crate) async fn extension_list() -> UseResult<CommandOutput> {
    let extensions = installed_extensions().await?;
    let generation = extension_registry_generation().await?;
    let human = if extensions.is_empty() {
        "No external Use extensions are installed.".to_string()
    } else {
        extensions
            .iter()
            .map(|extension| {
                format!(
                    "{}\t{}\t{}\t{}",
                    extension.package_id,
                    extension.route,
                    extension.version,
                    if !extension.compatible {
                        "incompatible"
                    } else if extension.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let values = extensions.iter().map(extension_value).collect::<Vec<_>>();
    Ok(CommandOutput::success(
        human,
        serde_json::json!({ "generation": generation, "extensions": values }),
    ))
}

#[cfg(feature = "extensions")]
pub(crate) async fn release_bundle_catalog() -> UseResult<CommandOutput> {
    let packages = crate::release_bundles::list().await?;
    Ok(CommandOutput::success(
        format!("{} release-bundled extension(s) available.", packages.len()),
        serde_json::json!({ "packages": packages }),
    ))
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn release_bundle_catalog() -> UseResult<CommandOutput> {
    Ok(CommandOutput::success(
        "No release-bundled extensions are available.",
        serde_json::json!({ "packages": [] }),
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
            "Extension '{}' is {} on route '{}'.",
            extension.package_id,
            if extension.enabled {
                "enabled"
            } else {
                "disabled"
            },
            extension.route
        ),
        serde_json::json!({
            "extension": extension_value(&extension),
            "manifest": extension.manifest
        }),
    ))
}

pub(crate) async fn extension_enable(package_id: &str) -> UseResult<CommandOutput> {
    let result = activate_extension(package_id, true, Duration::ZERO).await?;
    Ok(CommandOutput::success(
        if result.changed {
            format!("Enabled extension '{}'.", result.package_id)
        } else {
            format!("Extension '{}' is already enabled.", result.package_id)
        },
        serde_json::json!({
            "packageId": result.package_id,
            "changed": result.changed,
            "enabled": result.enabled,
            "generation": result.generation
        }),
    ))
}

pub(crate) async fn extension_disable(
    package_id: &str,
    timeout: Duration,
) -> UseResult<CommandOutput> {
    let result = activate_extension(package_id, false, timeout).await?;
    Ok(CommandOutput::success(
        if result.changed {
            format!("Disabled extension '{}'.", result.package_id)
        } else {
            format!("Extension '{}' is already disabled.", result.package_id)
        },
        serde_json::json!({
            "packageId": result.package_id,
            "changed": result.changed,
            "enabled": result.enabled,
            "generation": result.generation
        }),
    ))
}

pub(crate) async fn extension_snapshot() -> UseResult<CommandOutput> {
    let snapshot = current_registry_snapshot().await?;
    Ok(CommandOutput::success(
        format!("Extension registry generation {}.", snapshot["generation"]),
        serde_json::json!({ "registry": snapshot }),
    ))
}

pub(crate) async fn extension_watch(
    after_generation: u64,
    timeout: Duration,
) -> UseResult<CommandOutput> {
    let snapshot = watch_registry(after_generation, timeout).await?;
    match snapshot {
        Some(snapshot) => Ok(CommandOutput::success(
            format!("Extension registry advanced beyond generation {after_generation}."),
            serde_json::json!({ "changed": true, "registry": snapshot }),
        )),
        None => Ok(CommandOutput::success(
            format!("Extension registry did not change after generation {after_generation}."),
            serde_json::json!({
                "changed": false,
                "afterGeneration": after_generation,
                "timeoutMs": timeout.as_millis().min(u64::MAX as u128) as u64
            }),
        )),
    }
}

pub(crate) fn external_component_value(
    extension: &ExtensionView,
    full_id: bool,
) -> serde_json::Value {
    serde_json::json!({
        "id": if full_id { &extension.component_id } else { &extension.package_id },
        "description": format!("External Use domain on route '{}'.", extension.route),
        "presence": "managed",
        "health": if !extension.compatible {
            "incompatible"
        } else if extension.enabled {
            "ready"
        } else {
            "disabled"
        },
        "version": extension.version,
        "requiresUse": extension.requires_use,
        "repository": extension.repository,
        "path": extension.package_root,
        "packageSha256": extension.package_sha256,
        "route": extension.route,
        "enabled": extension.enabled,
        "compatible": extension.compatible,
        "surfaces": extension.surfaces,
        "trust": extension.trust,
        "registry": extension.registry
    })
}

fn extension_value(extension: &ExtensionView) -> serde_json::Value {
    serde_json::json!({
        "packageId": extension.package_id,
        "componentId": extension.component_id,
        "route": extension.route,
        "version": extension.version,
        "requiresUse": extension.requires_use,
        "repository": extension.repository,
        "enabled": extension.enabled,
        "compatible": extension.compatible,
        "packageRoot": extension.package_root,
        "packageSha256": extension.package_sha256,
        "surfaces": extension.surfaces,
        "trust": extension.trust,
        "registry": extension.registry
    })
}

fn valid_segment(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
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

#[cfg(feature = "extensions")]
pub(crate) async fn install_release_bundle_extension(
    package_id: &str,
    expected_package_sha256: &str,
    force: bool,
) -> UseResult<ExtensionInstallView> {
    let (source, package) = crate::release_bundles::resolve(package_id).await?;
    if package.package_sha256 != expected_package_sha256 {
        return Err(UseError::new(
            "use.extension.release_bundle_changed",
            format!(
                "Release bundle '{}' changed after its installation plan was reviewed.",
                package_id
            ),
        ));
    }
    let result = crate::extension_host::install_release_bundle(
        package_id,
        &source,
        expected_package_sha256,
        force,
    )
    .await?;
    Ok(ExtensionInstallView {
        changed: result.changed,
        extension: extension_view(result.extension)?,
    })
}

#[cfg(feature = "extensions")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn install_remote_extension(
    package_id: &str,
    registry_name: &str,
    registry_url: &str,
    trust_root: &str,
    trusted_root_path: Option<&Path>,
    version: Option<&str>,
    channel: &str,
    expected_plan_digest: Option<&str>,
    force: bool,
) -> UseResult<ExtensionInstallView> {
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let registry = a3s_use_extension::TrustedRegistry::new(
        registry_name,
        registry_url,
        trust_root,
        trusted_root_path.map(Path::to_path_buf),
        paths.tuf_datastore(registry_name),
    )?;
    let result = crate::extension_host::install_remote(
        package_id,
        &registry,
        version,
        channel,
        expected_plan_digest,
        force,
    )
    .await?;
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

#[cfg(not(feature = "extensions"))]
pub(crate) async fn install_release_bundle_extension(
    _package_id: &str,
    _expected_package_sha256: &str,
    _force: bool,
) -> UseResult<ExtensionInstallView> {
    Err(extensions_disabled())
}

#[cfg(not(feature = "extensions"))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn install_remote_extension(
    _package_id: &str,
    _registry_name: &str,
    _registry_url: &str,
    _trust_root: &str,
    _trusted_root_path: Option<&Path>,
    _version: Option<&str>,
    _channel: &str,
    _expected_plan_digest: Option<&str>,
    _force: bool,
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

#[derive(Debug, Clone)]
struct ExtensionActivationView {
    package_id: String,
    changed: bool,
    enabled: bool,
    generation: u64,
}

#[cfg(feature = "extensions")]
async fn activate_extension(
    package_id: &str,
    enabled: bool,
    timeout: Duration,
) -> UseResult<ExtensionActivationView> {
    let result = if enabled {
        crate::extension_host::enable(package_id).await?
    } else {
        crate::extension_host::disable(package_id, timeout).await?
    };
    Ok(ExtensionActivationView {
        package_id: result.package_id,
        changed: result.changed,
        enabled: result.enabled,
        generation: result.generation,
    })
}

#[cfg(not(feature = "extensions"))]
async fn activate_extension(
    _package_id: &str,
    _enabled: bool,
    _timeout: Duration,
) -> UseResult<ExtensionActivationView> {
    Err(extensions_disabled())
}

#[cfg(feature = "extensions")]
async fn current_registry_snapshot() -> UseResult<serde_json::Value> {
    serde_json::to_value(crate::extension_host::snapshot().await?).map_err(|error| {
        UseError::new(
            "use.extension.registry_invalid",
            format!("Failed to encode the extension registry snapshot: {error}"),
        )
    })
}

#[cfg(not(feature = "extensions"))]
async fn current_registry_snapshot() -> UseResult<serde_json::Value> {
    Ok(serde_json::json!({
        "schemaVersion": 1,
        "generation": 0,
        "routes": []
    }))
}

async fn extension_registry_generation() -> UseResult<u64> {
    Ok(current_registry_snapshot().await?["generation"]
        .as_u64()
        .unwrap_or(0))
}

#[cfg(feature = "extensions")]
async fn watch_registry(
    after_generation: u64,
    timeout: Duration,
) -> UseResult<Option<serde_json::Value>> {
    crate::extension_host::wait_for_change(after_generation, timeout)
        .await?
        .map(|snapshot| {
            serde_json::to_value(snapshot).map_err(|error| {
                UseError::new(
                    "use.extension.registry_invalid",
                    format!("Failed to encode the extension registry snapshot: {error}"),
                )
            })
        })
        .transpose()
}

#[cfg(not(feature = "extensions"))]
async fn watch_registry(
    _after_generation: u64,
    _timeout: Duration,
) -> UseResult<Option<serde_json::Value>> {
    Ok(None)
}

#[cfg(feature = "extensions")]
fn extension_view(extension: a3s_use_extension::InstalledExtension) -> UseResult<ExtensionView> {
    let surfaces = extension.surfaces();
    let compatible = extension.supports_use_version(env!("CARGO_PKG_VERSION"));
    let requires_use = extension.manifest.requires_use.clone();
    let repository = extension
        .manifest
        .repository
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| {
            UseError::new(
                "use.extension.manifest_invalid",
                format!("Failed to encode extension repository identity: {error}"),
            )
        })?;
    let trust = match extension.receipt.trust {
        a3s_use_extension::ExtensionTrust::LocalExplicit => "local-explicit",
        a3s_use_extension::ExtensionTrust::ReleaseBundle => "release-bundle",
        a3s_use_extension::ExtensionTrust::RegistryTuf => "registry-tuf",
    };
    let registry = extension
        .receipt
        .registry
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!("Failed to encode the extension registry provenance: {error}"),
            )
        })?;
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
        requires_use,
        repository,
        enabled: extension.receipt.enabled,
        compatible,
        package_root: extension.receipt.package_root,
        package_sha256: extension.receipt.package_sha256,
        surfaces,
        trust,
        registry,
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
