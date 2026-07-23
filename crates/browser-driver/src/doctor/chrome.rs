//! Check the Chrome install: binary path, version, cache dirs, user-data
//! dir, and the optional lightpanda engine.

use std::env;
use std::path::{Path, PathBuf};

use super::helpers::which_exists;
use super::{Check, Status};

pub(super) fn check(checks: &mut Vec<Check>) {
    let category = "Chrome";

    let chrome = crate::native::cdp::chrome::find_chrome();
    match chrome {
        Some(path) => {
            let label = path.display().to_string();
            match query_chrome_version(&path) {
                Some(version) => checks.push(Check::new(
                    "chrome.installed",
                    category,
                    Status::Pass,
                    format!("{} at {}", version, label),
                )),
                None => checks.push(Check::new(
                    "chrome.installed",
                    category,
                    Status::Pass,
                    format!(
                        "{} at {} (version unknown)",
                        browser_product_name(&path),
                        label
                    ),
                )),
            }
        }
        None => checks.push(
            Check::new(
                "chrome.installed",
                category,
                Status::Fail,
                "No Chrome binary found",
            )
            .with_fix("a3s install use/browser"),
        ),
    }

    let cache_dir = crate::install::get_browsers_dir();
    if cache_dir.exists() {
        checks.push(Check::new(
            "chrome.cache_dir",
            category,
            Status::Info,
            format!("Cache dir {}", cache_dir.display()),
        ));
    }

    if let Some(puppeteer_dir) = puppeteer_cache_dir() {
        if puppeteer_dir.exists() {
            checks.push(Check::new(
                "chrome.puppeteer_cache",
                category,
                Status::Info,
                format!(
                    "Puppeteer cache also present: {} (will be used as a fallback)",
                    puppeteer_dir.display()
                ),
            ));
        }
    }

    if let Some(user_data_dir) = crate::native::cdp::chrome::find_chrome_user_data_dir() {
        let profiles = crate::native::cdp::chrome::list_chrome_profiles(&user_data_dir);
        let count = profiles.len();
        let dir_label = user_data_dir.display().to_string();
        if count == 0 {
            checks.push(Check::new(
                "chrome.user_data_dir",
                category,
                Status::Info,
                format!(
                    "Chrome user data dir found ({}), no profiles parsed",
                    dir_label
                ),
            ));
        } else {
            checks.push(Check::new(
                "chrome.user_data_dir",
                category,
                Status::Info,
                format!("{} Chrome profile(s) at {}", count, dir_label),
            ));
        }
    }

    if let Ok(engine) = env::var("AGENT_BROWSER_ENGINE") {
        if engine == "lightpanda" {
            // Best-effort PATH lookup; absence is FAIL only when the user
            // explicitly opted into the lightpanda engine.
            if which_exists("lightpanda") {
                checks.push(Check::new(
                    "chrome.engine_lightpanda",
                    category,
                    Status::Pass,
                    "Lightpanda binary on PATH",
                ));
            } else {
                checks.push(
                    Check::new(
                        "chrome.engine_lightpanda",
                        category,
                        Status::Fail,
                        "A3S_USE_BROWSER_ENGINE=lightpanda but no lightpanda binary on PATH",
                    )
                    .with_fix("install lightpanda or unset A3S_USE_BROWSER_ENGINE"),
                );
            }
        }
    }
}

#[cfg(not(windows))]
fn query_chrome_version(path: &Path) -> Option<String> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    browser_version_output(path, &output.stdout)
}

#[cfg(windows)]
fn query_chrome_version(path: &Path) -> Option<String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };

    // Edge can treat `--version` as a normal browser launch on Windows and
    // never exit. Read the executable's version resource instead so Doctor is
    // bounded and does not leave a browser process tree behind.
    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut ignored_handle = 0;
    let size = unsafe { GetFileVersionInfoSizeW(wide_path.as_ptr(), &mut ignored_handle) };
    if size == 0 {
        return None;
    }

    let mut data = vec![0_u8; usize::try_from(size).ok()?];
    if unsafe {
        GetFileVersionInfoW(
            wide_path.as_ptr(),
            0,
            size,
            data.as_mut_ptr().cast::<c_void>(),
        )
    } == 0
    {
        return None;
    }

    let root = [b'\\' as u16, 0];
    let mut version_ptr = std::ptr::null_mut::<c_void>();
    let mut version_len = 0;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast::<c_void>(),
            root.as_ptr(),
            &mut version_ptr,
            &mut version_len,
        )
    } == 0
        || version_ptr.is_null()
        || usize::try_from(version_len).ok()? < std::mem::size_of::<VS_FIXEDFILEINFO>()
    {
        return None;
    }

    let version = unsafe { std::ptr::read_unaligned(version_ptr.cast::<VS_FIXEDFILEINFO>()) };
    Some(format_windows_file_version(
        path,
        version.dwFileVersionMS,
        version.dwFileVersionLS,
    ))
}

#[cfg(windows)]
fn format_windows_file_version(path: &Path, version_ms: u32, version_ls: u32) -> String {
    format!(
        "{} {}.{}.{}.{}",
        browser_product_name(path),
        version_ms >> 16,
        version_ms & 0xffff,
        version_ls >> 16,
        version_ls & 0xffff
    )
}

#[cfg(any(not(windows), test))]
fn browser_version_output(path: &Path, stdout: &[u8]) -> Option<String> {
    if let Ok(version) = std::str::from_utf8(stdout) {
        let version = version.trim();
        return (!version.is_empty()).then(|| version.to_string());
    }

    // Chromium browsers on localized Windows installations can write
    // `--version` using the active OEM code page rather than UTF-8. Preserve
    // the ASCII version number and derive the product label from the selected
    // executable instead of exposing replacement-character mojibake.
    let version = stdout
        .split(|byte| !byte.is_ascii_digit() && *byte != b'.')
        .filter(|candidate| {
            candidate.contains(&b'.')
                && candidate.first().is_some_and(u8::is_ascii_digit)
                && candidate.last().is_some_and(u8::is_ascii_digit)
        })
        .max_by_key(|candidate| candidate.len())
        .and_then(|candidate| std::str::from_utf8(candidate).ok())?;
    Some(format!("{} {version}", browser_product_name(path)))
}

fn browser_product_name(path: &Path) -> &'static str {
    // Tests and diagnostics may inspect a path produced on a different host
    // platform, so recognize both path separators before removing the common
    // Windows executable suffix.
    let path_text = path.to_string_lossy();
    let file_name = path_text
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let executable = file_name.strip_suffix(".exe").unwrap_or(&file_name);
    match executable {
        "msedge" => "Microsoft Edge",
        "chrome" | "google-chrome" | "google-chrome-stable" => "Google Chrome",
        "chromium" | "chromium-browser" => "Chromium",
        "brave" | "brave-browser" | "brave-browser-stable" => "Brave",
        _ => "Chromium browser",
    }
}

pub(super) fn puppeteer_cache_dir() -> Option<PathBuf> {
    if let Ok(p) = env::var("PUPPETEER_CACHE_DIR") {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(".cache").join("puppeteer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_windows_version_output_never_surfaces_mojibake() {
        let output = [
            0xce, 0xa2, 0xc8, 0xed, 0xd1, 0xc5, 0xba, 0xcb, b' ', b'1', b'3', b'8', b'.', b'0',
            b'.', b'3', b'3', b'5', b'1', b'.', b'8', b'3', b'\r', b'\n',
        ];

        assert_eq!(
            browser_version_output(
                Path::new(r"C:\Program Files\Microsoft\Edge\msedge.exe"),
                &output
            ),
            Some("Microsoft Edge 138.0.3351.83".to_string())
        );
    }

    #[test]
    fn utf8_browser_version_output_is_preserved() {
        assert_eq!(
            browser_version_output(
                Path::new("/Applications/Google Chrome"),
                b"Google Chrome 138.0.0.0\n"
            ),
            Some("Google Chrome 138.0.0.0".to_string())
        );
    }

    #[test]
    fn edge_product_name_survives_versionless_localized_output() {
        assert_eq!(
            browser_product_name(Path::new(
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
            )),
            "Microsoft Edge"
        );
        assert_eq!(
            browser_version_output(
                Path::new(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
                &[0xce, 0xa2, 0xc8, 0xed]
            ),
            None
        );
        assert_eq!(
            browser_product_name(Path::new("/usr/bin/google-chrome-stable")),
            "Google Chrome"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_version_uses_selected_browser_product_name() {
        assert_eq!(
            format_windows_file_version(
                Path::new(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
                150 << 16,
                (4078 << 16) | 83,
            ),
            "Microsoft Edge 150.0.4078.83"
        );
    }

    #[test]
    fn test_puppeteer_cache_dir_returns_sensible_default() {
        // When PUPPETEER_CACHE_DIR is unset, we fall back to
        // ~/.cache/puppeteer. Mutating env vars here would race with other
        // tests, so just verify the fallback path is shaped correctly.
        if env::var("PUPPETEER_CACHE_DIR").is_err() {
            let dir = puppeteer_cache_dir().expect("home dir should resolve in tests");
            let s = dir.to_string_lossy();
            assert!(s.contains(".cache"));
            assert!(s.ends_with("puppeteer"));
        }
    }
}
