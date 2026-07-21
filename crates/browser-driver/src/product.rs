//! A3S-owned product identity, environment compatibility, and filesystem roots.

#[cfg(test)]
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub const COMMAND_NAME: &str = "a3s use browser";
pub const SERVER_NAME: &str = "a3s-use-browser";
pub const PRIMARY_ENV_PREFIX: &str = "A3S_USE_BROWSER_";
pub const LEGACY_ENV_PREFIX: &str = "AGENT_BROWSER_";
pub const RESOLVED_DAEMON_ENV: &str = "A3S_USE_BROWSER_RESOLVED_DAEMON_ENV";

/// Initialize compatibility aliases unless this process is a daemon whose
/// environment was already resolved by the parent CLI.
///
/// The parent may override canonical A3S values with explicit CLI flags before
/// spawning the daemon. Re-promoting the inherited canonical values in that
/// child would silently undo those overrides.
pub fn initialize_process_environment() {
    if std::env::var_os(RESOLVED_DAEMON_ENV).is_none() {
        initialize_environment();
    }
}

/// Promote every A3S Browser environment variable to the compatibility name
/// consumed by the vendored engine. The A3S name always wins when both exist.
///
/// Keeping this translation at the process boundary preserves the upstream
/// implementation and its tests while making `A3S_USE_BROWSER_*` canonical.
pub fn initialize_environment() {
    let variables = std::env::vars_os().collect::<Vec<_>>();
    for (name, value) in &variables {
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(suffix) = name.strip_prefix(PRIMARY_ENV_PREFIX) else {
            continue;
        };
        std::env::set_var(format!("{LEGACY_ENV_PREFIX}{suffix}"), value);
    }
    for (name, value) in variables {
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(suffix) = name.strip_prefix(LEGACY_ENV_PREFIX) else {
            continue;
        };
        let primary = format!("{PRIMARY_ENV_PREFIX}{suffix}");
        if std::env::var_os(&primary).is_none() {
            std::env::set_var(primary, value);
        }
    }

    if std::env::var_os("AGENT_BROWSER_SOCKET_DIR").is_none() {
        std::env::set_var("AGENT_BROWSER_SOCKET_DIR", runtime_root());
    }
}

pub fn data_root() -> PathBuf {
    if let Some(path) = first_path(&["A3S_USE_BROWSER_HOME"]) {
        return absolute(path);
    }
    if let Some(path) = first_path(&["A3S_DATA_HOME"]) {
        return absolute(path).join("use").join("browser");
    }
    if let Some(path) = first_path(&["XDG_DATA_HOME"]) {
        return absolute(path).join("a3s").join("use").join("browser");
    }
    if cfg!(windows) {
        if let Some(path) = first_path(&["LOCALAPPDATA"]) {
            return absolute(path).join("a3s").join("use").join("browser");
        }
    }
    home_dir()
        .map(|home| home.join(".local/share/a3s/use/browser"))
        .unwrap_or_else(|| std::env::temp_dir().join("a3s/use/browser/data"))
}

pub fn state_root() -> PathBuf {
    if let Some(path) = first_path(&["A3S_USE_BROWSER_STATE_HOME"]) {
        return absolute(path);
    }
    if let Some(path) = first_path(&["A3S_USE_HOME"]) {
        return absolute(path).join("state").join("browser");
    }
    if let Some(path) = first_path(&["A3S_STATE_HOME"]) {
        return absolute(path).join("use").join("browser");
    }
    if let Some(path) = first_path(&["XDG_STATE_HOME"]) {
        return absolute(path).join("a3s").join("use").join("browser");
    }
    if cfg!(windows) {
        if let Some(path) = first_path(&["LOCALAPPDATA"]) {
            return absolute(path)
                .join("a3s")
                .join("use")
                .join("browser")
                .join("state");
        }
    }
    home_dir()
        .map(|home| home.join(".local/state/a3s/use/browser"))
        .unwrap_or_else(|| std::env::temp_dir().join("a3s/use/browser/state"))
}

pub fn cache_root() -> PathBuf {
    if let Some(path) = first_path(&["A3S_USE_BROWSER_CACHE_HOME"]) {
        return absolute(path);
    }
    if let Some(path) = first_path(&["A3S_USE_HOME"]) {
        return absolute(path).join("cache").join("browser");
    }
    if let Some(path) = first_path(&["A3S_CACHE_HOME"]) {
        return absolute(path).join("use").join("browser");
    }
    if let Some(path) = first_path(&["XDG_CACHE_HOME"]) {
        return absolute(path).join("a3s").join("use").join("browser");
    }
    if cfg!(windows) {
        if let Some(path) = first_path(&["LOCALAPPDATA"]) {
            return absolute(path)
                .join("a3s")
                .join("use")
                .join("browser")
                .join("cache");
        }
    }
    home_dir()
        .map(|home| home.join(".cache/a3s/use/browser"))
        .unwrap_or_else(|| std::env::temp_dir().join("a3s/use/browser/cache"))
}

pub fn runtime_root() -> PathBuf {
    if let Some(path) = first_path(&["A3S_USE_BROWSER_RUNTIME_DIR", "A3S_USE_BROWSER_SOCKET_DIR"]) {
        return absolute(path);
    }
    if let Some(path) = first_path(&["A3S_USE_RUNTIME_DIR"]) {
        return absolute(path).join("browser-driver");
    }
    if let Some(path) = first_path(&["XDG_RUNTIME_DIR"]) {
        return absolute(path).join("a3s-use").join("browser-driver");
    }
    state_root().join("run")
}

pub fn user_config_path() -> PathBuf {
    if let Some(path) = first_path(&["A3S_CONFIG_HOME"]) {
        return absolute(path).join("use/browser/config.acl");
    }
    home_dir()
        .map(|home| home.join(".a3s/use/browser/config.acl"))
        .unwrap_or_else(|| state_root().join("config.acl"))
}

pub fn project_config_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".a3s/use/browser.acl")
}

pub fn legacy_user_config_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".agent-browser/config.json"))
}

pub fn legacy_project_config_path() -> PathBuf {
    PathBuf::from("agent-browser.json")
}

fn first_path(names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    })
}

fn home_dir() -> Option<PathBuf> {
    first_path(&["HOME", "USERPROFILE"]).or_else(dirs::home_dir)
}

fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|current| current.join(&path))
        .unwrap_or(path)
}

pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
fn primary_env_name(legacy: &str) -> OsString {
    legacy
        .strip_prefix(LEGACY_ENV_PREFIX)
        .map(|suffix| format!("{PRIMARY_ENV_PREFIX}{suffix}").into())
        .unwrap_or_else(|| OsStr::new(legacy).to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::EnvGuard;

    #[test]
    fn product_paths_are_a3s_owned() {
        for path in [data_root(), state_root(), cache_root(), runtime_root()] {
            assert!(path.is_absolute());
            assert!(path.to_string_lossy().contains("a3s"));
            assert!(!path.to_string_lossy().contains(".agent-browser"));
        }
    }

    #[test]
    fn primary_environment_name_is_derived_from_legacy_alias() {
        assert_eq!(
            primary_env_name("AGENT_BROWSER_SESSION"),
            "A3S_USE_BROWSER_SESSION"
        );
    }

    #[test]
    fn resolved_daemon_environment_preserves_parent_cli_overrides() {
        let guard = EnvGuard::new(&[
            RESOLVED_DAEMON_ENV,
            "A3S_USE_BROWSER_NAMESPACE",
            "AGENT_BROWSER_NAMESPACE",
        ]);
        guard.set(RESOLVED_DAEMON_ENV, "1");
        guard.set("A3S_USE_BROWSER_NAMESPACE", "inherited-default");
        guard.set("AGENT_BROWSER_NAMESPACE", "explicit-cli-value");

        initialize_process_environment();

        assert_eq!(
            std::env::var("AGENT_BROWSER_NAMESPACE").unwrap(),
            "explicit-cli-value"
        );
    }
}
