//! Bridge Browser compatibility commands to the A3S component lifecycle.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use a3s_use_core::FirstUseInstallPolicy;

const USE_EXECUTABLE_ENV: &str = "A3S_USE_EXECUTABLE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoInstallAction {
    Ready,
    Install,
}

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

pub fn ensure_first_use_browser() -> Result<(), String> {
    let available = crate::native::cdp::chrome::find_chrome().is_some();
    let explicit_invalid = explicit_browser_provider_invalid();
    let policy = FirstUseInstallPolicy::from_env()
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    match automatic_install_action(available, explicit_invalid, policy)? {
        AutoInstallAction::Ready => Ok(()),
        AutoInstallAction::Install => {
            run_component_install_captured()?;
            crate::native::cdp::chrome::find_chrome()
                .is_some()
                .then_some(())
                .ok_or_else(|| {
                    "use.browser.install_failed: Browser installation completed without a usable Chrome executable."
                        .to_string()
                })
        }
    }
}

fn automatic_install_action(
    available: bool,
    explicit_invalid: bool,
    policy: FirstUseInstallPolicy,
) -> Result<AutoInstallAction, String> {
    if explicit_invalid {
        return Err(
            "use.browser.explicit_provider_invalid: The explicit Browser executable is not usable. Fix or unset it before retrying."
                .to_string(),
        );
    }
    if available {
        return Ok(AutoInstallAction::Ready);
    }
    if let Some(block) = policy.blocked_by() {
        return Err(format!(
            "use.browser.auto_install_disabled: No compatible browser is ready and first-use installation is disabled by {}. Run 'a3s install use/browser' explicitly while online.",
            block.reason()
        ));
    }
    Ok(AutoInstallAction::Install)
}

fn explicit_browser_provider_invalid() -> bool {
    [
        "A3S_USE_BROWSER_EXECUTABLE_PATH",
        "AGENT_BROWSER_EXECUTABLE_PATH",
        "A3S_BROWSER_EXECUTABLE",
        "CHROME",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
    .any(|path| !is_usable_executable(&path))
}

fn is_usable_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn run_component_install(force: bool, json: bool) -> i32 {
    let mut command = match component_install_command(force, json) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!(
                "Failed to launch A3S Use component lifecycle '{}': {error}",
                command.get_program().to_string_lossy()
            );
            1
        }
    }
}

fn run_component_install_captured() -> Result<(), String> {
    let mut command = component_install_command(false, true)?;
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!(
                "use.browser.install_failed: Failed to launch A3S Use component lifecycle '{}': {error}",
                command.get_program().to_string_lossy()
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("the component installer returned no diagnostic");
    Err(format!(
        "use.browser.install_failed: Browser first-use installation failed: {detail}"
    ))
}

fn component_install_command(force: bool, json: bool) -> Result<Command, String> {
    let executable = resolve_use_executable().ok_or_else(|| {
        format!(
            "Cannot find a3s-use for the Browser component lifecycle. Install or repair the A3S Use package, or set {USE_EXECUTABLE_ENV}."
        )
    })?;
    let mut command = Command::new(executable);
    command.args(component_install_args(force, json));
    Ok(command)
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
    fn first_use_installs_only_when_the_runtime_is_missing_and_policy_allows_it() {
        assert_eq!(
            automatic_install_action(true, false, FirstUseInstallPolicy::new(true, true)).unwrap(),
            AutoInstallAction::Ready
        );
        assert_eq!(
            automatic_install_action(false, false, FirstUseInstallPolicy::new(false, false))
                .unwrap(),
            AutoInstallAction::Install
        );
        for policy in [
            FirstUseInstallPolicy::new(true, false),
            FirstUseInstallPolicy::new(false, true),
        ] {
            let error = automatic_install_action(false, false, policy).unwrap_err();
            assert!(error.starts_with("use.browser.auto_install_disabled:"));
        }
        let explicit =
            automatic_install_action(false, true, FirstUseInstallPolicy::new(false, false))
                .unwrap_err();
        assert!(explicit.starts_with("use.browser.explicit_provider_invalid:"));
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
