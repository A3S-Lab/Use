use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    ActivationResult, ExtensionRegistry, ExtensionRegistrySnapshot, InstallOptions, InstallResult,
    InstalledExtension, TrustedRegistry, UninstallResult,
};
use std::time::Duration;

pub async fn list() -> UseResult<Vec<InstalledExtension>> {
    ExtensionRegistry::from_env()?.list().await
}

pub async fn get(package_id: &str) -> UseResult<Option<InstalledExtension>> {
    ExtensionRegistry::from_env()?.get(package_id).await
}

pub async fn install(
    package_id: &str,
    source: &Path,
    force: bool,
    allow_unsigned: bool,
) -> UseResult<InstallResult> {
    ExtensionRegistry::from_env()?
        .install_local(
            package_id,
            source,
            InstallOptions {
                force,
                allow_unsigned,
            },
        )
        .await
}

pub async fn install_remote(
    package_id: &str,
    registry: &TrustedRegistry,
    version: Option<&str>,
    channel: &str,
    expected_plan_digest: Option<&str>,
    force: bool,
) -> UseResult<InstallResult> {
    ExtensionRegistry::from_env()?
        .install_remote(
            package_id,
            registry,
            version,
            channel,
            expected_plan_digest,
            force,
        )
        .await
}

pub async fn uninstall(package_id: &str) -> UseResult<UninstallResult> {
    ExtensionRegistry::from_env()?.uninstall(package_id).await
}

pub async fn enable(package_id: &str) -> UseResult<ActivationResult> {
    ExtensionRegistry::from_env()?.enable(package_id).await
}

pub async fn disable(package_id: &str, timeout: Duration) -> UseResult<ActivationResult> {
    ExtensionRegistry::from_env()?
        .disable_with_timeout(package_id, timeout)
        .await
}

pub async fn snapshot() -> UseResult<ExtensionRegistrySnapshot> {
    ExtensionRegistry::from_env()?.snapshot().await
}

pub async fn wait_for_change(
    after_generation: u64,
    timeout: Duration,
) -> UseResult<Option<ExtensionRegistrySnapshot>> {
    ExtensionRegistry::from_env()?
        .wait_for_change(after_generation, timeout)
        .await
}

pub async fn run_route(route: &str, args: &[String]) -> UseResult<Option<u8>> {
    let Some(lease) = ExtensionRegistry::from_env()?.acquire_route(route).await? else {
        return Ok(None);
    };
    let extension = lease.extension();
    let Some(executable) = extension.cli_executable() else {
        let surfaces = extension.surfaces();
        let suggestion = if extension.manifest.mcp.is_some() {
            format!(
                "Connect to the standard MCP surface declared by '{}'.",
                extension.receipt.package_id
            )
        } else {
            format!(
                "Load the Skill declared by '{}'.",
                extension.receipt.package_id
            )
        };
        return Err(UseError::new(
            "use.extension.surface_unavailable",
            format!("Extension route '{route}' does not provide a CLI surface."),
        )
        .with_detail("availableSurfaces", serde_json::json!(surfaces))
        .with_suggestion(suggestion));
    };
    let status = tokio::process::Command::new(&executable)
        .args(args)
        .env("A3S_USE_EXTENSION_ID", &extension.receipt.package_id)
        .env("A3S_USE_PACKAGE_ROOT", &extension.receipt.package_root)
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| {
            UseError::new(
                "use.extension.launch_failed",
                format!(
                    "Failed to launch extension '{}': {error}",
                    extension.receipt.package_id
                ),
            )
        })?;
    let code = status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1);
    Ok(Some(code))
}

pub async fn run_mcp(package_id: &str) -> UseResult<u8> {
    let lease = ExtensionRegistry::from_env()?
        .acquire_extension(package_id)
        .await?
        .ok_or_else(|| {
            UseError::new(
                "use.extension.not_active",
                format!("Extension '{package_id}' is not installed or is disabled."),
            )
            .with_suggestion("Install or enable the extension before starting its MCP surface.")
        })?;
    let extension = lease.extension();
    let executable = extension.mcp_executable().ok_or_else(|| {
        UseError::new(
            "use.extension.surface_unavailable",
            format!("Extension '{package_id}' does not provide an MCP surface."),
        )
        .with_detail("availableSurfaces", serde_json::json!(extension.surfaces()))
    })?;
    if extension.mcp_transport() != Some(a3s_use_extension::McpTransport::Stdio) {
        return Err(UseError::new(
            "use.extension.mcp_transport_unsupported",
            format!(
                "Extension '{package_id}' declares Streamable HTTP; it cannot be attached to this stdio process."
            ),
        ));
    }
    let status = tokio::process::Command::new(&executable)
        .args(extension.mcp_args().unwrap_or_default())
        .env("A3S_USE_EXTENSION_ID", &extension.receipt.package_id)
        .env("A3S_USE_PACKAGE_ROOT", &extension.receipt.package_root)
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| {
            UseError::new(
                "use.extension.launch_failed",
                format!("Failed to launch extension MCP server '{package_id}': {error}"),
            )
        })?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}
