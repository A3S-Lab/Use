use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    ExtensionRegistry, InstallOptions, InstallResult, InstalledExtension, UninstallResult,
};

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

pub async fn uninstall(package_id: &str) -> UseResult<UninstallResult> {
    ExtensionRegistry::from_env()?.uninstall(package_id).await
}

pub async fn run_route(route: &str, args: &[String]) -> UseResult<Option<u8>> {
    let Some(extension) = ExtensionRegistry::from_env()?.find_route(route).await? else {
        return Ok(None);
    };
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
