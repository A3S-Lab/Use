//! Bridge Browser compatibility commands to the A3S component lifecycle.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const USE_EXECUTABLE_ENV: &str = "A3S_USE_EXECUTABLE";

pub fn install(with_dependencies: bool, json: bool) -> i32 {
    if with_dependencies {
        if let Err(error) = crate::install::install_system_dependencies() {
            eprintln!("Failed to install Browser system dependencies: {error}");
            return 1;
        }
    } else if cfg!(target_os = "linux") {
        eprintln!(
            "Browser system packages are not changed by default; use '{} install --with-deps' when required.",
            crate::product::COMMAND_NAME
        );
    }
    run_component_install(false, json)
}

pub fn upgrade(json: bool) -> i32 {
    run_component_install(true, json)
}

fn run_component_install(force: bool, json: bool) -> i32 {
    let executable = match resolve_use_executable() {
        Some(executable) => executable,
        None => {
            eprintln!(
                "Cannot find a3s-use for the Browser component lifecycle. Install or repair the A3S Use package, or set {USE_EXECUTABLE_ENV}."
            );
            return 1;
        }
    };
    let mut command = Command::new(&executable);
    command.args(component_install_args(force, json));
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!(
                "Failed to launch A3S Use component lifecycle '{}': {error}",
                executable.display()
            );
            1
        }
    }
}

fn component_install_args(force: bool, json: bool) -> Vec<&'static str> {
    let mut arguments = vec!["component", "install", "browser"];
    if force {
        arguments.push("--force");
    }
    if json {
        arguments.push("--json");
    }
    arguments
}

fn resolve_use_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(USE_EXECUTABLE_ENV).map(PathBuf::from) {
        return usable_executable(path);
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            if let Some(path) = usable_executable(parent.join(platform_use_name())) {
                return Some(path);
            }
        }
    }
    find_on_path(platform_use_name())
}

fn platform_use_name() -> &'static str {
    if cfg!(windows) {
        "a3s-use.exe"
    } else {
        "a3s-use"
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find_map(usable_executable)
}

fn usable_executable(path: PathBuf) -> Option<PathBuf> {
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn lifecycle_arguments_are_owned_by_a3s_use() {
        assert_eq!(
            component_install_args(false, false),
            ["component", "install", "browser"]
        );
        assert_eq!(
            component_install_args(true, true),
            ["component", "install", "browser", "--force", "--json"]
        );
    }

    #[test]
    fn explicit_lifecycle_executable_must_be_a_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(usable_executable(temp.path().to_path_buf()).is_none());
    }

    #[test]
    fn executable_name_is_platform_specific() {
        use std::ffi::OsStr;

        assert_eq!(
            Path::new(platform_use_name())
                .extension()
                .and_then(OsStr::to_str),
            cfg!(windows).then_some("exe")
        );
    }
}
