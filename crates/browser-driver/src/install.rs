//! Discovery of A3S-managed Chrome and explicit Linux system dependencies.
//!
//! Browser archive download, verification, receipts, atomic activation, and
//! removal belong to `a3s-use-browser`. The driver must not maintain a second
//! installation layout.

use std::fs;
#[cfg(any(target_os = "linux", test))]
use std::io;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", test))]
use std::process::ExitStatus;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

const A3S_RECEIPT_FILE: &str = ".a3s-install.json";

pub fn get_browsers_dir() -> PathBuf {
    crate::product::data_root().join("chrome")
}

pub fn find_installed_chrome() -> Option<PathBuf> {
    let root = get_browsers_dir();
    let mut versions = fs::read_dir(&root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.path().is_dir()
                && !entry.file_name().to_string_lossy().starts_with('.')
                && entry.path().join(A3S_RECEIPT_FILE).is_file()
        })
        .collect::<Vec<_>>();
    versions.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    versions
        .into_iter()
        .find_map(|entry| chrome_binary_in_dir(&entry.path()))
}

fn chrome_binary_in_dir(dir: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    const CANDIDATES: &[&str] = &[
        "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
    ];
    #[cfg(target_os = "linux")]
    const CANDIDATES: &[&str] = &["chrome-linux64/chrome", "chrome"];
    #[cfg(target_os = "windows")]
    const CANDIDATES: &[&str] = &["chrome-win64/chrome.exe", "chrome.exe"];
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    const CANDIDATES: &[&str] = &[];

    CANDIDATES
        .iter()
        .map(|relative| dir.join(relative))
        .find(|path| path.is_file())
}

pub fn install_system_dependencies() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        install_linux_dependencies()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn install_linux_dependencies() -> Result<(), String> {
    if which_exists("apt-get") {
        return install_apt_dependencies();
    }
    if which_exists("dnf") {
        return install_rpm_dependencies("dnf", dnf_dependencies());
    }
    if which_exists("yum") {
        return install_rpm_dependencies("yum", yum_dependencies());
    }
    Err("no supported package manager found; expected apt-get, dnf, or yum".to_string())
}

#[cfg(target_os = "linux")]
fn install_apt_dependencies() -> Result<(), String> {
    eprintln!("Installing Browser system dependencies with apt-get...");
    let update = privileged_command("apt-get")
        .arg("update")
        .status()
        .map_err(|error| format!("could not run apt-get update: {error}"))?;
    if !update.success() {
        eprintln!("Warning: apt-get update failed; using the existing package index.");
    }

    let dependencies = resolve_apt_dependencies();
    let simulation = privileged_command("apt-get")
        .args(["install", "--simulate"])
        .args(&dependencies)
        .output()
        .map_err(|error| format!("could not simulate Browser dependency installation: {error}"))?;
    if !simulation.status.success() {
        return Err(format!(
            "apt-get rejected the Browser dependencies: {}",
            String::from_utf8_lossy(&simulation.stderr).trim()
        ));
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&simulation.stdout),
        String::from_utf8_lossy(&simulation.stderr)
    );
    let removals = combined
        .lines()
        .filter(|line| line.starts_with("Remv "))
        .collect::<Vec<_>>();
    if !removals.is_empty() {
        return Err(format!(
            "refusing Browser dependency installation because apt would remove {} package(s): {}",
            removals.len(),
            removals.into_iter().take(10).collect::<Vec<_>>().join(", ")
        ));
    }

    install_status_result(
        privileged_command("apt-get")
            .args(["install", "-y"])
            .args(&dependencies)
            .status(),
    )
}

#[cfg(target_os = "linux")]
fn install_rpm_dependencies(manager: &str, dependencies: &[&str]) -> Result<(), String> {
    eprintln!("Installing Browser system dependencies with {manager}...");
    install_status_result(
        privileged_command(manager)
            .args(["install", "-y"])
            .args(dependencies)
            .status(),
    )
}

#[cfg(target_os = "linux")]
fn privileged_command(program: &str) -> Command {
    if unsafe { libc::geteuid() } == 0 {
        Command::new(program)
    } else {
        let mut command = Command::new("sudo");
        command.arg(program);
        command
    }
}

#[cfg(any(target_os = "linux", test))]
fn install_status_result(status: io::Result<ExitStatus>) -> Result<(), String> {
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "dependency install command failed with exit code {}",
            status
                .code()
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        )),
        Err(error) => Err(format!("could not run install command: {error}")),
    }
}

#[cfg(target_os = "linux")]
fn apt_dependency_specs() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("libxcb-shm0", None),
        ("libx11-xcb1", None),
        ("libx11-6", None),
        ("libxcb1", None),
        ("libxext6", None),
        ("libxrandr2", None),
        ("libxcomposite1", None),
        ("libxcursor1", None),
        ("libxdamage1", None),
        ("libxfixes3", None),
        ("libxi6", None),
        ("libgtk-3-0", Some("libgtk-3-0t64")),
        ("libpangocairo-1.0-0", Some("libpangocairo-1.0-0t64")),
        ("libpango-1.0-0", Some("libpango-1.0-0t64")),
        ("libatk1.0-0", Some("libatk1.0-0t64")),
        ("libcairo-gobject2", Some("libcairo-gobject2t64")),
        ("libcairo2", Some("libcairo2t64")),
        ("libgdk-pixbuf-2.0-0", Some("libgdk-pixbuf-2.0-0t64")),
        ("libxrender1", None),
        ("libasound2", Some("libasound2t64")),
        ("libfreetype6", None),
        ("libfontconfig1", None),
        ("libdbus-1-3", Some("libdbus-1-3t64")),
        ("libnss3", None),
        ("libnspr4", None),
        ("libatk-bridge2.0-0", Some("libatk-bridge2.0-0t64")),
        ("libdrm2", None),
        ("libxkbcommon0", None),
        ("libatspi2.0-0", Some("libatspi2.0-0t64")),
        ("libcups2", Some("libcups2t64")),
        ("libxshmfence1", None),
        ("libgbm1", None),
        ("fonts-noto-color-emoji", None),
        ("fonts-noto-cjk", None),
        ("fonts-freefont-ttf", None),
    ]
}

#[cfg(target_os = "linux")]
fn resolve_apt_dependencies_with<F>(mut package_exists: F) -> Vec<&'static str>
where
    F: FnMut(&str) -> bool,
{
    apt_dependency_specs()
        .iter()
        .map(|(base, t64)| {
            t64.filter(|package| package_exists(package))
                .unwrap_or(base)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn resolve_apt_dependencies() -> Vec<&'static str> {
    resolve_apt_dependencies_with(package_exists_apt)
}

#[cfg(target_os = "linux")]
fn package_exists_apt(package: &str) -> bool {
    Command::new("apt-cache")
        .args(["show", package])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "linux")]
fn dnf_dependencies() -> &'static [&'static str] {
    &[
        "nss",
        "nspr",
        "atk",
        "at-spi2-atk",
        "cups-libs",
        "libdrm",
        "libXcomposite",
        "libXdamage",
        "libXrandr",
        "mesa-libgbm",
        "pango",
        "alsa-lib",
        "libxkbcommon",
        "libxcb",
        "libX11-xcb",
        "libX11",
        "libXext",
        "libXcursor",
        "libXfixes",
        "libXi",
        "gtk3",
        "cairo-gobject",
        "google-noto-cjk-fonts",
        "google-noto-emoji-color-fonts",
        "liberation-fonts",
    ]
}

#[cfg(target_os = "linux")]
fn yum_dependencies() -> &'static [&'static str] {
    &[
        "nss",
        "nspr",
        "atk",
        "at-spi2-atk",
        "cups-libs",
        "libdrm",
        "libXcomposite",
        "libXdamage",
        "libXrandr",
        "mesa-libgbm",
        "pango",
        "alsa-lib",
        "libxkbcommon",
        "google-noto-cjk-fonts",
        "liberation-fonts",
    ]
}

#[cfg(target_os = "linux")]
fn which_exists(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed_exit_status() -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(1 << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(1)
        }
    }

    #[test]
    fn install_status_rejects_failure_and_spawn_errors() {
        assert!(install_status_result(Ok(failed_exit_status())).is_err());
        assert!(install_status_result(Err(io::Error::new(
            io::ErrorKind::NotFound,
            "missing command"
        )))
        .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn apt_resolution_prefers_available_t64_packages() {
        let dependencies = resolve_apt_dependencies_with(|package| {
            matches!(package, "libasound2t64" | "libgtk-3-0t64")
        });
        assert!(dependencies.contains(&"libasound2t64"));
        assert!(!dependencies.contains(&"libasound2"));
        assert!(dependencies.contains(&"libgtk-3-0t64"));
        assert!(dependencies.contains(&"libnss3"));
    }

    #[test]
    fn managed_discovery_requires_an_a3s_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let guard = crate::test_utils::EnvGuard::new(&["A3S_USE_BROWSER_HOME"]);
        guard.set(
            "A3S_USE_BROWSER_HOME",
            temp.path().to_str().expect("temporary path is UTF-8"),
        );
        let version = temp.path().join("chrome/123");
        fs::create_dir_all(&version).unwrap();
        let relative = if cfg!(target_os = "macos") {
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        } else if cfg!(target_os = "windows") {
            "chrome-win64/chrome.exe"
        } else {
            "chrome-linux64/chrome"
        };
        let executable = version.join(relative);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"fixture").unwrap();
        assert!(find_installed_chrome().is_none());
        fs::write(version.join(A3S_RECEIPT_FILE), b"{}").unwrap();
        assert_eq!(find_installed_chrome(), Some(executable));
    }
}
