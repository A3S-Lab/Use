use std::path::{Path, PathBuf};

use a3s_use_core::{
    inspect_okf_bundle, McpReleaseDescriptor, OkfBundleFile, ToolReleaseDescriptor,
    ToolWorkloadContract as ToolReleaseWorkload, UseError, UseResult, MAX_RELEASE_DESCRIPTOR_BYTES,
};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::package::{
    io_error, validate_surface_file, validate_text_asset, MAX_ACTIVITY_HTML_BYTES,
    MAX_ACTIVITY_RESOURCE_BYTES,
};
use super::{ExtensionManifest, PluginMcpLaunch, ToolTaskSource, ToolWorkload};

const MAX_TOOL_API_CONTRACT_BYTES: u64 = 4 * 1024 * 1024;

pub(super) async fn validate_named_surface_files(
    manifest: &ExtensionManifest,
    canonical_root: &Path,
    package_root: &Path,
) -> UseResult<()> {
    for tool in &manifest.tools {
        match &tool.workload {
            ToolWorkload::Task(task) => {
                validate_tool_task(task, canonical_root, package_root).await?
            }
            ToolWorkload::Service(service) => {
                validate_tool_service(service, canonical_root, package_root).await?
            }
        }
    }
    for mcp in &manifest.mcp_servers {
        match &mcp.launch {
            PluginMcpLaunch::Stdio { executable, .. } => {
                validate_surface_file(
                    "MCP stdio executable",
                    canonical_root,
                    &package_root.join(executable),
                    true,
                )
                .await?;
            }
            PluginMcpLaunch::StreamableHttp { release } => {
                let path = package_root.join(release);
                validate_surface_file("MCP release descriptor", canonical_root, &path, false)
                    .await?;
                let bytes = read_bounded_file(
                    "MCP release descriptor",
                    &path,
                    MAX_RELEASE_DESCRIPTOR_BYTES as u64,
                    "use.extension.release_descriptor_invalid",
                )
                .await?;
                McpReleaseDescriptor::from_json(&bytes)
                    .map_err(|error| release_descriptor_error("MCP", &path, error))?;
            }
        }
    }
    for skill in &manifest.skills {
        validate_surface_file(
            "Skill file",
            canonical_root,
            &package_root.join(&skill.path),
            false,
        )
        .await?;
    }
    for okf in &manifest.okf {
        validate_okf_bundle(okf, canonical_root, package_root).await?;
    }
    for ui in &manifest.ui {
        validate_ui_text_asset(
            "UI entry",
            "HTML",
            canonical_root,
            &package_root.join(&ui.entry),
            MAX_ACTIVITY_HTML_BYTES,
        )
        .await?;
        for style in &ui.styles {
            validate_ui_text_asset(
                "UI style",
                "CSS",
                canonical_root,
                &package_root.join(style),
                MAX_ACTIVITY_RESOURCE_BYTES,
            )
            .await?;
        }
        for script in &ui.scripts {
            validate_ui_text_asset(
                "UI script",
                "JavaScript",
                canonical_root,
                &package_root.join(script),
                MAX_ACTIVITY_RESOURCE_BYTES,
            )
            .await?;
        }
    }
    Ok(())
}

async fn validate_okf_bundle(
    surface: &super::PluginOkfSurface,
    canonical_root: &Path,
    package_root: &Path,
) -> UseResult<()> {
    let root = package_root.join(&surface.bundle.root);
    let metadata = fs::symlink_metadata(&root)
        .await
        .map_err(|error| io_error("inspect OKF bundle root", &root, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(okf_package_error(format!(
            "OKF bundle root '{}' must be a real package directory.",
            root.display()
        )));
    }
    let resolved_root = fs::canonicalize(&root)
        .await
        .map_err(|error| io_error("resolve OKF bundle root", &root, error))?;
    if !resolved_root.starts_with(canonical_root) {
        return Err(UseError::new(
            "use.extension.path_escape",
            format!("OKF bundle root '{}' escapes the package.", root.display()),
        ));
    }

    let mut pending = vec![(root, PathBuf::new())];
    let mut files = Vec::new();
    let mut file_count = 0_u64;
    let mut expanded_bytes = 0_u64;
    while let Some((directory, relative_directory)) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .await
            .map_err(|error| io_error("read OKF bundle directory", &directory, error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("read OKF bundle entry", &directory, error))?
        {
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                okf_package_error("OKF bundle paths must be valid UTF-8 on every platform.")
            })?;
            let relative = relative_directory.join(name);
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .await
                .map_err(|error| io_error("inspect OKF bundle entry", &path, error))?;
            if metadata.file_type().is_symlink() {
                return Err(okf_package_error(format!(
                    "OKF bundle entry '{}' cannot be a symbolic link.",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                pending.push((path, relative));
                continue;
            }
            if !metadata.is_file() {
                return Err(okf_package_error(format!(
                    "OKF bundle entry '{}' must be a regular file or directory.",
                    path.display()
                )));
            }
            file_count = file_count.checked_add(1).ok_or_else(okf_limit_error)?;
            expanded_bytes = expanded_bytes
                .checked_add(metadata.len())
                .ok_or_else(okf_limit_error)?;
            if file_count > surface.bundle.limits.max_files
                || expanded_bytes > surface.bundle.limits.max_expanded_bytes
            {
                return Err(okf_limit_error());
            }
            let relative = relative.to_str().ok_or_else(|| {
                okf_package_error("OKF bundle paths must be valid UTF-8 on every platform.")
            })?;
            let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
            let content = fs::read(&path)
                .await
                .map_err(|error| io_error("read OKF bundle file", &path, error))?;
            if content.len() as u64 != metadata.len() {
                return Err(UseError::new(
                    "use.extension.package_changed",
                    "An OKF bundle file changed while it was being inspected.",
                ));
            }
            files.push(OkfBundleFile::new(relative, content));
        }
    }
    let inspection = inspect_okf_bundle(
        surface.bundle.format_version,
        surface.bundle.limits.clone(),
        files,
    )?;
    surface.bundle.verify_inspection(&inspection)
}

fn okf_package_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.okf_bundle_invalid", message)
}

fn okf_limit_error() -> UseError {
    UseError::new(
        "use.okf.limit_exceeded",
        "The OKF bundle exceeds its declared conformance limits.",
    )
}

async fn validate_tool_task(
    task: &super::ToolTaskSurface,
    canonical_root: &Path,
    package_root: &Path,
) -> UseResult<()> {
    match &task.source {
        ToolTaskSource::Executable { executable } => {
            validate_surface_file(
                "Tool Task executable",
                canonical_root,
                &package_root.join(executable),
                true,
            )
            .await
        }
        ToolTaskSource::Release { release } => {
            let path = package_root.join(release);
            validate_surface_file("Tool Task release descriptor", canonical_root, &path, false)
                .await?;
            let descriptor = read_tool_release_descriptor("Tool Task", &path).await?;
            match descriptor.workload {
                ToolReleaseWorkload::Task { timeout_ms, .. } if timeout_ms == task.timeout_ms => {
                    Ok(())
                }
                ToolReleaseWorkload::Task { .. } => Err(release_binding_error(
                    "Tool Task",
                    &path,
                    "timeout_ms does not match the plugin manifest.",
                )),
                ToolReleaseWorkload::Service { .. } => Err(release_binding_error(
                    "Tool Task",
                    &path,
                    "must declare a Task workload.",
                )),
            }
        }
    }
}

async fn validate_tool_service(
    service: &super::ToolServiceSurface,
    canonical_root: &Path,
    package_root: &Path,
) -> UseResult<()> {
    let release_path = package_root.join(&service.release);
    validate_surface_file(
        "Tool Service release descriptor",
        canonical_root,
        &release_path,
        false,
    )
    .await?;
    let descriptor = read_tool_release_descriptor("Tool Service", &release_path).await?;
    let (base_path, api_contract_digest) = match descriptor.workload {
        ToolReleaseWorkload::Service {
            base_path,
            api_contract_digest,
            ..
        } => (base_path, api_contract_digest),
        ToolReleaseWorkload::Task { .. } => {
            return Err(release_binding_error(
                "Tool Service",
                &release_path,
                "must declare a Service workload.",
            ));
        }
    };
    if base_path != service.base_path {
        return Err(release_binding_error(
            "Tool Service",
            &release_path,
            "base_path does not match the plugin manifest.",
        ));
    }
    if let Some(contract) = &service.contract {
        let contract_path = package_root.join(contract);
        validate_text_asset(
            "use.extension.tool_contract_invalid",
            "Tool Service API contract",
            "JSON or YAML",
            canonical_root,
            &contract_path,
            MAX_TOOL_API_CONTRACT_BYTES,
        )
        .await?;
        let bytes = read_bounded_file(
            "Tool Service API contract",
            &contract_path,
            MAX_TOOL_API_CONTRACT_BYTES,
            "use.extension.tool_contract_invalid",
        )
        .await?;
        let digest = format!("sha256:{:x}", Sha256::digest(bytes));
        if api_contract_digest.as_deref() != Some(digest.as_str()) {
            return Err(release_binding_error(
                "Tool Service",
                &release_path,
                "api_contract_digest does not match the declared API contract.",
            ));
        }
    }
    Ok(())
}

async fn validate_ui_text_asset(
    label: &str,
    content_type: &str,
    canonical_root: &Path,
    path: &Path,
    max_bytes: u64,
) -> UseResult<()> {
    validate_text_asset(
        "use.extension.ui_asset_invalid",
        label,
        content_type,
        canonical_root,
        path,
        max_bytes,
    )
    .await
}

async fn read_tool_release_descriptor(
    label: &str,
    path: &Path,
) -> UseResult<ToolReleaseDescriptor> {
    let bytes = read_bounded_file(
        &format!("{label} release descriptor"),
        path,
        MAX_RELEASE_DESCRIPTOR_BYTES as u64,
        "use.extension.release_descriptor_invalid",
    )
    .await?;
    ToolReleaseDescriptor::from_json(&bytes)
        .map_err(|error| release_descriptor_error(label, path, error))
}

async fn read_bounded_file(
    label: &str,
    path: &Path,
    max_bytes: u64,
    error_code: &'static str,
) -> UseResult<Vec<u8>> {
    let metadata = fs::metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if metadata.len() == 0 || metadata.len() > max_bytes {
        return Err(UseError::new(
            error_code,
            format!(
                "{label} '{}' must contain between 1 byte and {max_bytes} bytes.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| io_error(&format!("read {label}"), path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        return Err(UseError::new(
            error_code,
            format!("{label} '{}' changed while it was read.", path.display()),
        ));
    }
    Ok(bytes)
}

fn release_descriptor_error(label: &str, path: &Path, error: UseError) -> UseError {
    UseError::new(
        "use.extension.release_descriptor_invalid",
        format!(
            "{label} release descriptor '{}' is invalid: {}",
            path.display(),
            error.message
        ),
    )
}

fn release_binding_error(label: &str, path: &Path, message: &str) -> UseError {
    UseError::new(
        "use.extension.release_descriptor_invalid",
        format!("{label} release descriptor '{}' {message}", path.display()),
    )
}
