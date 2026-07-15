use std::path::{Path, PathBuf};

use a3s_use_core::{DomainDiagnostic, Readiness, UseError, UseResult};

const BOX_EXECUTABLE_ENV: &str = "A3S_USE_BOX_EXECUTABLE";

pub(crate) fn box_diagnostic() -> DomainDiagnostic {
    match box_executable() {
        Ok(path) => DomainDiagnostic {
            domain: "box".to_string(),
            readiness: Readiness::Ready,
            provider: Some("a3s-box".to_string()),
            version: None,
            path: Some(path),
            message: "The component-backed Box route is ready.".to_string(),
            suggestions: Vec::new(),
        },
        Err(error) => DomainDiagnostic {
            domain: "box".to_string(),
            readiness: Readiness::Missing,
            provider: None,
            version: None,
            path: None,
            message: error.message,
            suggestions: vec![error
                .suggestion
                .unwrap_or_else(|| "Run 'a3s install box'.".to_string())],
        },
    }
}

pub(crate) async fn run_box(args: &[String]) -> UseResult<u8> {
    let executable = box_executable()?;
    let status = tokio::process::Command::new(&executable)
        .args(args)
        .status()
        .await
        .map_err(|error| {
            UseError::new(
                "use.box.launch_failed",
                format!(
                    "Failed to launch Box at '{}': {error}",
                    executable.display()
                ),
            )
        })?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

fn box_executable() -> UseResult<PathBuf> {
    let path = std::env::var_os(BOX_EXECUTABLE_ENV)
        .map(PathBuf::from)
        .ok_or_else(missing_box)?;
    if !path.is_absolute() {
        return Err(invalid_box_path(
            &path,
            "the configured path is not absolute",
        ));
    }
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| invalid_box_path(&path, &error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_box_path(
            &path,
            "the configured path is not a regular file",
        ));
    }
    if !is_executable(&metadata) {
        return Err(invalid_box_path(
            &path,
            "the configured file is not executable",
        ));
    }
    Ok(path)
}

fn missing_box() -> UseError {
    UseError::new(
        "use.box.missing",
        "The Box component path was not provided to A3S Use.",
    )
    .with_suggestion("Run through 'a3s use box ...' or install Box with 'a3s install box'.")
}

fn invalid_box_path(path: &Path, reason: &str) -> UseError {
    UseError::new(
        "use.box.path_invalid",
        format!("Invalid Box component path '{}': {reason}.", path.display()),
    )
    .with_suggestion("Repair or reinstall the component with 'a3s install box --force'.")
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}
