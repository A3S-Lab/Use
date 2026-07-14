//! Automatic Chrome/Chromium detection and installation.
//!
//! Detects an existing Chrome/Chromium installation or explicitly downloads
//! Chrome for Testing from Google's official CDN.
//!
//! Downloaded binaries are stored under the A3S Use Browser data root.

use std::path::{Path, PathBuf};

use tracing::{debug, info};

use a3s_use_core::{UseError, UseResult};

use crate::pool::browser_error;

/// Well-known Chrome/Chromium executable paths per platform.
#[cfg(target_os = "macos")]
const KNOWN_PATHS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
];

#[cfg(all(unix, not(target_os = "macos")))]
const KNOWN_PATHS: &[&str] = &[
    "/opt/google/chrome/chrome",
    "/opt/chromium.org/chromium/chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/snap/bin/chromium",
];

#[cfg(target_os = "windows")]
const KNOWN_PATHS: &[&str] = &[
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
];

/// Well-known command names to search in PATH.
const KNOWN_COMMANDS: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];

/// Returns the platform identifier for Chrome for Testing downloads.
pub(crate) fn platform_id() -> UseResult<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok("mac-arm64")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Ok("mac-x64")
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok("linux64")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok("win64")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86"))]
    {
        Ok("win32")
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86"),
    )))]
    {
        Err(browser_error(
            "Unsupported platform for automatic Chrome download".to_string(),
        ))
    }
}

/// Returns the relative path to the Chrome executable inside the extracted zip.
#[cfg(target_os = "macos")]
pub(crate) fn chrome_executable_in_zip(platform: &str) -> String {
    format!(
        "chrome-{}/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        platform
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn chrome_executable_in_zip(platform: &str) -> String {
    format!("chrome-{}/chrome", platform)
}

#[cfg(target_os = "windows")]
pub(crate) fn chrome_executable_in_zip(platform: &str) -> String {
    format!("chrome-{}\\chrome.exe", platform)
}

/// Base directory for cached Chrome downloads.
pub(crate) fn managed_cache_dir() -> UseResult<PathBuf> {
    Ok(browser_data_root()?.join("chrome"))
}

pub(crate) fn browser_data_root() -> UseResult<PathBuf> {
    if let Some(value) = std::env::var_os("A3S_USE_BROWSER_HOME") {
        return absolute(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("A3S_DATA_HOME") {
        return Ok(absolute(PathBuf::from(value))?.join("use/browser"));
    }
    if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(absolute(PathBuf::from(value))?.join("a3s/use/browser"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return Ok(absolute(home)?.join(".local/share/a3s/use/browser"));
    }
    #[cfg(windows)]
    if let Some(value) = std::env::var_os("LOCALAPPDATA") {
        return Ok(absolute(PathBuf::from(value))?.join("a3s/use/browser"));
    }
    Err(browser_error(
        "Cannot determine the A3S Use Browser data directory.",
    ))
}

fn absolute(path: PathBuf) -> UseResult<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| browser_error(format!("Failed to resolve browser data path: {error}")))
}

/// Detect an existing Chrome/Chromium installation on the system.
///
/// Checks:
/// 1. `CHROME` environment variable
/// 2. Well-known command names in PATH
/// 3. Well-known filesystem paths
///
/// Returns `Some(path)` if found, `None` otherwise.
pub fn detect_chrome() -> Option<PathBuf> {
    // 1. Check explicit environment overrides.
    for name in ["A3S_BROWSER_EXECUTABLE", "CHROME"] {
        if let Some(path) = std::env::var_os(name).map(PathBuf::from) {
            if path.is_file() {
                debug!("Chrome found via {name}: {}", path.display());
                return Some(path);
            }
        }
    }

    // 2. Check well-known commands in PATH
    for cmd in KNOWN_COMMANDS {
        if let Ok(path) = which::which(cmd) {
            debug!("Chrome found in PATH: {}", path.display());
            return Some(path);
        }
    }

    // 3. Check well-known filesystem paths
    for path_str in KNOWN_PATHS {
        let p = Path::new(path_str);
        if p.exists() {
            debug!("Chrome found at known path: {}", path_str);
            return Some(p.to_path_buf());
        }
    }

    None
}

/// Ensure Chrome is available, downloading it if necessary.
///
/// 1. If Chrome is already installed on the system, returns its path.
/// 2. If a cached download exists in `the A3S Use data directory/`, returns that path.
/// 3. Otherwise, downloads Chrome for Testing and caches it.
///
/// Returns the path to the Chrome executable.
pub async fn ensure_chrome() -> UseResult<PathBuf> {
    // 1. Check system installation
    if let Some(path) = detect_chrome() {
        info!("Using system Chrome: {}", path.display());
        return Ok(path);
    }

    // 2. Check cached download
    if let Ok(path) = find_managed_chrome() {
        info!("Using cached Chrome: {}", path.display());
        return Ok(path);
    }

    // 3. Download Chrome for Testing
    info!("No Chrome installation found, downloading Chrome for Testing...");
    download_latest_chrome().await
}

/// Resolve Chrome without downloading or mutating the machine.
pub(crate) fn resolve_chrome() -> UseResult<PathBuf> {
    detect_chrome()
        .or_else(|| find_managed_chrome().ok())
        .ok_or_else(|| {
            UseError::new(
                "use.browser.runtime_missing",
                "No compatible Chrome executable is installed.",
            )
            .with_suggestion(
                "Run 'a3s install use/browser' or select BrowserProvider::ManagedChrome.",
            )
        })
}

/// Look for a previously downloaded Chrome in the cache directory.
pub(crate) fn find_managed_chrome() -> UseResult<PathBuf> {
    let base = managed_cache_dir()?;
    if !base.exists() {
        return Err(browser_error("No cached Chrome found".to_string()));
    }

    // Find the latest version directory
    let mut versions: Vec<_> = std::fs::read_dir(&base)
        .map_err(|e| browser_error(format!("Failed to read cache dir: {}", e)))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .collect();

    // Sort by name descending (latest version first)
    versions.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

    let platform = platform_id()?;
    for version_dir in versions {
        let Some(version) = version_dir.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !crate::install::has_complete_receipt(&version_dir.path(), "chrome", &version) {
            continue;
        }
        let exe_path = version_dir.path().join(chrome_executable_in_zip(platform));
        if exe_path.is_file() {
            return Ok(exe_path);
        }
    }

    Err(browser_error("No cached Chrome found".to_string()))
}

/// Download Chrome for Testing from Google's official CDN.
///
/// Downloads the stable version for the current platform and extracts it
/// to `the A3S Use data directory/<version>/`.
pub async fn download_latest_chrome() -> UseResult<PathBuf> {
    crate::chrome_install::download_latest_chrome().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_browser_candidates_are_defined() {
        assert!(!KNOWN_PATHS.is_empty());
        assert!(!KNOWN_COMMANDS.is_empty());
    }
}
