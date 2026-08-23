use std::path::PathBuf;
use std::time::Duration;

use a3s_use_core::{PluginHostApplyResult, PluginHostPlanResult, UseError, UseResult};

use crate::cli::CommandOutput;

#[cfg(feature = "extensions")]
mod manager;

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
    pub package_graph: Option<serde_json::Value>,
    pub registry_access: &'static str,
    pub registry_source_revision: String,
    pub plugin_manager: PluginManagerMutationView,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtensionUninstallView {
    pub package_id: String,
    pub changed: bool,
    pub package_graph: Option<serde_json::Value>,
    pub plugin_manager: PluginManagerMutationView,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginManagerMutationView {
    pub operation_id: String,
    pub plan_digest: String,
    pub plan: Box<PluginHostPlanResult>,
    pub result: Box<PluginHostApplyResult>,
}

impl PluginManagerMutationView {
    fn new(plan: PluginHostPlanResult, result: PluginHostApplyResult) -> UseResult<Self> {
        plan.validate()?;
        result.validate()?;
        if result.operation_id != plan.plan.plan.operation_id
            || result.plan_digest != plan.plan.plan_digest
            || result.package_id != plan.package_id
            || result.scope != plan.scope
        {
            return Err(UseError::new(
                "use.plugin.manager_cli_invalid",
                "The Plugin Manager CLI result does not bind its exact reviewed plan.",
            ));
        }
        Ok(Self {
            operation_id: plan.plan.plan.operation_id.clone(),
            plan_digest: plan.plan.plan_digest.clone(),
            plan: Box::new(plan),
            result: Box::new(result),
        })
    }
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

pub(crate) async fn extension_inspect(package_id: &str) -> UseResult<CommandOutput> {
    let Some(extension) = installed_extension(package_id).await? else {
        return Err(UseError::new(
            "use.extension.not_installed",
            format!("Extension '{package_id}' is not installed."),
        ));
    };
    let lifecycle = extension_lifecycle_diagnostic(package_id).await?;
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
            "manifest": extension.manifest,
            "lifecycle": lifecycle
        }),
    ))
}

#[cfg(feature = "extensions")]
pub(crate) async fn extension_operation_diagnostic(
    package_id: &str,
    scope: a3s_use_core::PlanScope,
    include_history: bool,
) -> UseResult<CommandOutput> {
    let registry = a3s_use_extension::ExtensionRegistry::from_env()?;
    let manager = crate::cognitive_package::CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        registry,
        scope,
        std::sync::Arc::new(
            crate::cognitive_package::StandaloneCognitivePackageLifecycleFactory::default(),
        ),
        std::sync::Arc::new(
            crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        ),
    )?;
    let value = if include_history {
        serde_json::to_value(manager.diagnose_operation_history(package_id).await?)
            .map_err(|_| diagnostic_encoding_error())?
    } else {
        let mut value = None;
        // Resolution can hand off to download, and download can hand off to a
        // reviewed graph while a read-only diagnostic is sampling. Three
        // bounded passes cover both one-way transitions without waiting,
        // taking the package lock, or manufacturing a mixed projection.
        for _ in 0..3 {
            if let Some(observed) = active_extension_diagnostic(&manager, package_id).await? {
                value = Some(observed);
                break;
            }
        }
        value.ok_or_else(|| {
            UseError::new(
                "use.plugin.operation_diagnostic_not_found",
                "No diagnosable cognitive-package operation exists for this package and scope.",
            )
            .with_suggestion(
                "Use 'a3s-use extension inspect <publisher/name> --json' for installed lifecycle history.",
            )
        })?
    };
    Ok(CommandOutput::success(
        if include_history {
            format!("Cognitive-package diagnostic history for '{package_id}'.")
        } else {
            format!("Cognitive-package diagnostic for '{package_id}'.")
        },
        serde_json::json!({ "diagnostic": value }),
    ))
}

#[cfg(feature = "extensions")]
async fn active_extension_diagnostic(
    manager: &crate::cognitive_package::CognitivePackageManager,
    package_id: &str,
) -> UseResult<Option<serde_json::Value>> {
    match manager.diagnose_operation(package_id).await {
        Ok(diagnostic) => {
            return serde_json::to_value(diagnostic)
                .map(Some)
                .map_err(|_| diagnostic_encoding_error())
        }
        Err(error) if error.code == "use.plugin.operation_diagnostic_not_found" => {}
        Err(error) => return Err(error),
    }
    match manager.diagnose_download_attempt(package_id).await {
        Ok(diagnostic) => {
            return serde_json::to_value(diagnostic)
                .map(Some)
                .map_err(|_| diagnostic_encoding_error())
        }
        Err(error) if error.code == "use.plugin.download_attempt_diagnostic_not_found" => {}
        Err(error) => return Err(error),
    }
    match manager.diagnose_resolution_attempt(package_id).await {
        Ok(diagnostic) => serde_json::to_value(diagnostic)
            .map(Some)
            .map_err(|_| diagnostic_encoding_error()),
        Err(error) if error.code == "use.plugin.resolution_attempt_diagnostic_not_found" => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(feature = "extensions")]
fn diagnostic_encoding_error() -> UseError {
    UseError::new(
        "use.plugin.operation_diagnostic_invalid",
        "Failed to encode the cognitive-package diagnostic.",
    )
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn extension_operation_diagnostic(
    _package_id: &str,
    _scope: a3s_use_core::PlanScope,
    _include_history: bool,
) -> UseResult<CommandOutput> {
    Err(UseError::new(
        "use.extension.disabled",
        "Cognitive-package operation diagnostics require the 'extensions' feature.",
    ))
}

#[cfg(feature = "extensions")]
async fn extension_lifecycle_diagnostic(package_id: &str) -> UseResult<serde_json::Value> {
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let scope = a3s_use_core::PlanScope {
        kind: a3s_use_core::PlanScopeKind::User,
        id: crate::cognitive_package::COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
    };
    let diagnostic =
        crate::plugin_lifecycle::PluginLifecycleJournalStore::from_extension_paths(&paths)
            .diagnose(&scope, package_id)
            .await?;
    serde_json::to_value(diagnostic).map_err(|error| {
        UseError::new(
            "use.plugin.lifecycle_diagnostic_invalid",
            format!("Failed to encode cognitive-package lifecycle diagnostics: {error}"),
        )
    })
}

#[cfg(not(feature = "extensions"))]
async fn extension_lifecycle_diagnostic(_package_id: &str) -> UseResult<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub(crate) async fn extension_planning_evidence(package_id: &str) -> UseResult<CommandOutput> {
    let evidence = crate::capability_registry::installed_plugin_plan_evidence(package_id).await?;
    Ok(CommandOutput::success(
        format!(
            "Resolved plan-ready installed evidence for '{}'.",
            evidence.package_id
        ),
        serde_json::json!({ "planningEvidence": evidence }),
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
pub(crate) async fn install_remote_extension(
    package_id: &str,
    registry_name: Option<&str>,
    version: Option<&str>,
    channel: Option<&str>,
    expected_package_lock_digest: Option<&str>,
    offline: bool,
) -> UseResult<ExtensionInstallView> {
    let result = manager::install(
        package_id,
        registry_name,
        version,
        channel,
        expected_package_lock_digest,
        offline,
    )
    .await?;
    let extension = installed_extension(package_id).await?.ok_or_else(|| {
        UseError::new(
            "use.plugin.package_graph_invalid",
            "The Plugin Manager completed install without an installed root extension.",
        )
    })?;
    Ok(ExtensionInstallView {
        changed: !result.manager.result.replayed,
        extension,
        package_graph: Some(result.package_graph),
        registry_access: result.registry_access,
        registry_source_revision: result.registry_source_revision,
        plugin_manager: result.manager,
    })
}

#[cfg(feature = "extensions")]
pub(crate) async fn upgrade_remote_extension(
    package_id: &str,
    registry_name: Option<&str>,
    version: Option<&str>,
    channel: Option<&str>,
    expected_package_lock_digest: Option<&str>,
    offline: bool,
) -> UseResult<ExtensionInstallView> {
    let result = manager::upgrade(
        package_id,
        registry_name,
        version,
        channel,
        expected_package_lock_digest,
        offline,
    )
    .await?;
    let extension = installed_extension(package_id).await?.ok_or_else(|| {
        UseError::new(
            "use.plugin.package_graph_invalid",
            "The Plugin Manager completed upgrade without an installed root extension.",
        )
    })?;
    Ok(ExtensionInstallView {
        changed: !result.manager.result.replayed,
        extension,
        package_graph: Some(result.package_graph),
        registry_access: result.registry_access,
        registry_source_revision: result.registry_source_revision,
        plugin_manager: result.manager,
    })
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn install_remote_extension(
    _package_id: &str,
    _registry_name: Option<&str>,
    _version: Option<&str>,
    _channel: Option<&str>,
    _expected_package_lock_digest: Option<&str>,
    _offline: bool,
) -> UseResult<ExtensionInstallView> {
    Err(extensions_disabled())
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn upgrade_remote_extension(
    _package_id: &str,
    _registry_name: Option<&str>,
    _version: Option<&str>,
    _channel: Option<&str>,
    _expected_package_lock_digest: Option<&str>,
    _offline: bool,
) -> UseResult<ExtensionInstallView> {
    Err(extensions_disabled())
}

#[cfg(feature = "extensions")]
pub(crate) async fn uninstall_extension(package_id: &str) -> UseResult<ExtensionUninstallView> {
    let result = manager::uninstall(package_id).await?;
    Ok(ExtensionUninstallView {
        package_id: package_id.to_owned(),
        changed: !result.manager.result.replayed,
        package_graph: Some(result.package_graph),
        plugin_manager: result.manager,
    })
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn uninstall_extension(_package_id: &str) -> UseResult<ExtensionUninstallView> {
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
