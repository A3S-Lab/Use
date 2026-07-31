use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};

use super::super::model::StdioMcpSessionPlan;
use super::super::validation::paths_overlap;
use super::{native_error, native_io_error};

pub(super) async fn validate_native_paths(plan: &StdioMcpSessionPlan) -> UseResult<()> {
    let package_root = canonical_directory("packageRoot", plan.package_root()).await?;
    let roots = [
        canonical_directory("pluginDataRoot", plan.roots().plugin_data_root()).await?,
        canonical_directory("temporaryRoot", plan.roots().temporary_root()).await?,
        canonical_directory("workspaceRoot", plan.roots().workspace_root()).await?,
    ];
    if roots.iter().enumerate().any(|(index, left)| {
        roots[index + 1..]
            .iter()
            .any(|right| paths_overlap(left, right))
    }) || roots.iter().any(|root| paths_overlap(root, &package_root))
    {
        return Err(native_error(
            "use.plugin.stdio_mcp.native_path_invalid",
            "The native stdio MCP filesystem roots alias another session or package root.",
        ));
    }

    let (executable, executable_metadata) = canonical_file("executable", plan.executable()).await?;
    if !executable.starts_with(&package_root) || !platform_executable(&executable_metadata) {
        return Err(native_error(
            "use.plugin.stdio_mcp.native_path_invalid",
            "The native stdio MCP executable is not an executable regular file owned by the immutable package root.",
        ));
    }
    Ok(())
}

async fn canonical_directory(role: &'static str, path: &Path) -> UseResult<PathBuf> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| native_path_error(role, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(native_path_role_error(role));
    }
    tokio::fs::canonicalize(path)
        .await
        .map_err(|error| native_path_error(role, &error))
}

async fn canonical_file(
    role: &'static str,
    path: &Path,
) -> UseResult<(PathBuf, std::fs::Metadata)> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| native_path_error(role, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(native_path_role_error(role));
    }
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|error| native_path_error(role, &error))?;
    Ok((canonical, metadata))
}

#[cfg(unix)]
fn platform_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn platform_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

fn native_path_error(role: &'static str, error: &io::Error) -> UseError {
    native_io_error(
        "use.plugin.stdio_mcp.native_path_invalid",
        "A required native stdio MCP filesystem entry is unavailable.",
        error,
    )
    .with_detail("pathRole", role)
}

fn native_path_role_error(role: &'static str) -> UseError {
    native_error(
        "use.plugin.stdio_mcp.native_path_invalid",
        "A required native stdio MCP filesystem entry has the wrong type.",
    )
    .with_detail("pathRole", role)
}
