//! Process boundary for the complete interactive Browser automation driver.
//!
//! Search and embedded Rust callers continue to use `a3s-use-browser`
//! directly. The driver is a sibling executable because it must be able to
//! restart itself as an owned per-session daemon without turning the facade
//! process into a daemon or coupling Search to CLI state.

use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};

const DRIVER_ENV: &str = "A3S_USE_BROWSER_DRIVER";
const DRIVER_NAME: &str = "a3s-use-browser-driver";

pub(crate) async fn run(args: &[String]) -> UseResult<u8> {
    let driver = resolve_driver()?;
    let mut command = tokio::process::Command::new(&driver);
    command.args(args).kill_on_drop(true);
    apply_a3s_environment(&mut command)?;
    let status = command.status().await.map_err(|error| {
        UseError::new(
            "use.browser.driver_launch_failed",
            format!(
                "Failed to launch Browser driver '{}': {error}",
                driver.display()
            ),
        )
    })?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

fn resolve_driver() -> UseResult<PathBuf> {
    if let Some(path) = std::env::var_os(DRIVER_ENV).map(PathBuf::from) {
        return usable_driver(path).ok_or_else(|| driver_missing(Some(DRIVER_ENV)));
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            let sibling = parent.join(platform_driver_name());
            if let Some(path) = usable_driver(sibling) {
                return Ok(path);
            }
        }
    }

    if let Some(path) = find_on_path(platform_driver_name()) {
        return Ok(path);
    }

    Err(driver_missing(None))
}

pub(crate) fn is_available() -> bool {
    resolve_driver().is_ok()
}

pub(crate) async fn primary_skill_surface() -> Option<(PathBuf, PathBuf)> {
    let mut roots = Vec::new();
    if let Some(skills_dir) = std::env::var_os("A3S_USE_BROWSER_SKILLS_DIR").map(PathBuf::from) {
        if let Some(root) = skills_dir.parent() {
            roots.push(root.to_path_buf());
        }
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(root) = executable.parent() {
            roots.push(root.to_path_buf());
        }
    }
    roots.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates")
            .join("browser-driver"),
    );

    for root in roots {
        let skill = root.join("skills/a3s-use-browser/SKILL.md");
        let Ok(root) = tokio::fs::canonicalize(root).await else {
            continue;
        };
        let Ok(skill) = tokio::fs::canonicalize(skill).await else {
            continue;
        };
        if skill.starts_with(&root) {
            return Some((root, skill));
        }
    }
    None
}

fn platform_driver_name() -> &'static str {
    if cfg!(windows) {
        "a3s-use-browser-driver.exe"
    } else {
        DRIVER_NAME
    }
}

fn usable_driver(path: PathBuf) -> Option<PathBuf> {
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

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find_map(usable_driver)
}

fn driver_missing(explicit: Option<&str>) -> UseError {
    let message = match explicit {
        Some(variable) => format!("{variable} does not point to a usable Browser driver."),
        None => "The complete A3S Use Browser driver is not installed beside a3s-use.".to_string(),
    };
    UseError::new("use.browser.driver_missing", message).with_suggestion(
        "Install or repair the Use component, or build all workspace binaries with 'cargo build --workspace --bins'.",
    )
}

fn apply_a3s_environment(command: &mut tokio::process::Command) -> UseResult<()> {
    if no_environment_value(&[
        "A3S_USE_BROWSER_RUNTIME_DIR",
        "A3S_USE_BROWSER_SOCKET_DIR",
        "AGENT_BROWSER_SOCKET_DIR",
    ]) {
        command.env("A3S_USE_BROWSER_RUNTIME_DIR", runtime_dir()?);
    }
    if no_environment_value(&["A3S_USE_BROWSER_NAMESPACE", "AGENT_BROWSER_NAMESPACE"]) {
        command.env("A3S_USE_BROWSER_NAMESPACE", "a3s-use");
    }
    if no_environment_value(&["A3S_USE_BROWSER_SKILLS_DIR", "AGENT_BROWSER_SKILLS_DIR"]) {
        if let Some(path) = source_skills_dir() {
            command.env("A3S_USE_BROWSER_SKILLS_DIR", path);
        }
    }
    if no_environment_value(&[
        "A3S_USE_BROWSER_EXECUTABLE_PATH",
        "AGENT_BROWSER_EXECUTABLE_PATH",
    ]) {
        if let Some(path) = managed_browser_executable() {
            command.env("A3S_USE_BROWSER_EXECUTABLE_PATH", path);
        }
    }
    Ok(())
}

fn no_environment_value(names: &[&str]) -> bool {
    names
        .iter()
        .all(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()))
}

fn runtime_dir() -> UseResult<PathBuf> {
    if let Some(path) = std::env::var_os("A3S_USE_BROWSER_RUNTIME_DIR") {
        return absolute(PathBuf::from(path), "A3S_USE_BROWSER_RUNTIME_DIR");
    }
    if let Some(path) = std::env::var_os("A3S_USE_RUNTIME_DIR") {
        return absolute(PathBuf::from(path), "A3S_USE_RUNTIME_DIR")
            .map(|path| path.join("browser-driver"));
    }
    if let Some(path) = std::env::var_os("XDG_RUNTIME_DIR") {
        return absolute(PathBuf::from(path), "XDG_RUNTIME_DIR")
            .map(|path| path.join("a3s-use").join("browser-driver"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            UseError::new(
                "use.browser.runtime_dir_missing",
                "Cannot resolve a private Browser driver runtime directory.",
            )
            .with_suggestion("Set A3S_USE_BROWSER_RUNTIME_DIR to an absolute local directory.")
        })?;
    absolute(home, "home directory").map(|path| {
        path.join(".a3s")
            .join("use")
            .join("run")
            .join("browser-driver")
    })
}

fn absolute(path: PathBuf, source: &str) -> UseResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(UseError::new(
            "use.browser.runtime_dir_invalid",
            format!("{source} must be an absolute path."),
        ))
    }
}

fn source_skills_dir() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates")
        .join("browser-driver")
        .join("skill-data");
    path.is_dir().then_some(path)
}

fn managed_browser_executable() -> Option<PathBuf> {
    let status = a3s_use_browser::browser_status(a3s_use_browser::ManagedBrowser::Chrome);
    status.available.then_some(status.path).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_is_owned_by_a3s_use() {
        let path = runtime_dir().unwrap();
        assert!(path.is_absolute());
        assert!(path.to_string_lossy().contains("a3s"));
        assert!(path.ends_with("browser-driver"));
    }

    #[test]
    fn source_skills_are_available_to_development_builds() {
        assert!(source_skills_dir().unwrap().join("core/SKILL.md").is_file());
    }

    #[tokio::test]
    async fn primary_skill_is_resolved_inside_its_package_root() {
        let (root, skill) = primary_skill_surface().await.unwrap();
        assert!(root.is_absolute());
        assert!(skill.starts_with(root));
        assert!(skill.ends_with("skills/a3s-use-browser/SKILL.md"));
    }
}
