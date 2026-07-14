//! Lifecycle and diagnostics for Browser provider runtimes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use a3s_use_core::UseResult;

use crate::pool::browser_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedBrowser {
    Chrome,
    Lightpanda,
}

impl ManagedBrowser {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Lightpanda => "lightpanda",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BrowserInstallSource {
    Environment,
    System,
    ManagedCache,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserRuntimeStatus {
    pub browser: ManagedBrowser,
    pub available: bool,
    pub source: BrowserInstallSource,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub cache_dir: Option<PathBuf>,
    pub detail: String,
}

pub fn browser_status(browser: ManagedBrowser) -> BrowserRuntimeStatus {
    match browser {
        ManagedBrowser::Chrome => chrome_status(),
        ManagedBrowser::Lightpanda => lightpanda_status(),
    }
}

pub fn browser_statuses() -> Vec<BrowserRuntimeStatus> {
    [ManagedBrowser::Chrome, ManagedBrowser::Lightpanda]
        .into_iter()
        .map(browser_status)
        .collect()
}

pub async fn install_browser(browser: ManagedBrowser) -> UseResult<BrowserRuntimeStatus> {
    install_or_update_browser(browser, false).await
}

pub async fn update_browser(browser: ManagedBrowser) -> UseResult<BrowserRuntimeStatus> {
    install_or_update_browser(browser, true).await
}

pub async fn repair_browser(browser: ManagedBrowser) -> UseResult<BrowserRuntimeStatus> {
    let status = browser_status(browser);
    if status.available {
        Ok(status)
    } else {
        update_browser(browser).await
    }
}

async fn install_or_update_browser(
    browser: ManagedBrowser,
    force_latest: bool,
) -> UseResult<BrowserRuntimeStatus> {
    match browser {
        ManagedBrowser::Chrome => {
            if force_latest {
                crate::chrome::download_latest_chrome().await?;
            } else {
                crate::chrome::ensure_chrome().await?;
            }
        }
        ManagedBrowser::Lightpanda => {
            #[cfg(feature = "lightpanda")]
            if force_latest {
                crate::lightpanda::download_latest_lightpanda().await?;
            } else {
                crate::lightpanda::ensure_lightpanda().await?;
            }
            #[cfg(not(feature = "lightpanda"))]
            return Err(browser_error(
                "Lightpanda support is not compiled into this build".to_string(),
            ));
        }
    }
    let status = browser_status(browser);
    status.available.then_some(status).ok_or_else(|| {
        browser_error(format!(
            "{} installation completed without a usable executable",
            browser.as_str()
        ))
    })
}

fn chrome_status() -> BrowserRuntimeStatus {
    let cache_dir = crate::chrome::managed_cache_dir().ok();
    if let Some(path) =
        env_executable("A3S_BROWSER_EXECUTABLE").or_else(|| env_executable("CHROME"))
    {
        return available_status(
            ManagedBrowser::Chrome,
            BrowserInstallSource::Environment,
            path,
            cache_dir,
        );
    }
    if let Some(path) = crate::chrome::detect_chrome() {
        return available_status(
            ManagedBrowser::Chrome,
            BrowserInstallSource::System,
            path,
            cache_dir,
        );
    }
    if let Ok(path) = crate::chrome::find_managed_chrome() {
        return available_status(
            ManagedBrowser::Chrome,
            BrowserInstallSource::ManagedCache,
            path,
            cache_dir,
        );
    }
    missing_status(ManagedBrowser::Chrome, cache_dir, "not installed")
}

fn lightpanda_status() -> BrowserRuntimeStatus {
    #[cfg(feature = "lightpanda")]
    {
        let cache_dir = crate::lightpanda::managed_cache_dir().ok();
        if let Some(path) =
            env_executable("A3S_LIGHTPANDA_EXECUTABLE").or_else(|| env_executable("LIGHTPANDA"))
        {
            return available_status(
                ManagedBrowser::Lightpanda,
                BrowserInstallSource::Environment,
                path,
                cache_dir,
            );
        }
        if let Some(path) = crate::lightpanda::detect_lightpanda() {
            return available_status(
                ManagedBrowser::Lightpanda,
                BrowserInstallSource::System,
                path,
                cache_dir,
            );
        }
        if let Ok(path) = crate::lightpanda::find_managed_lightpanda() {
            return available_status(
                ManagedBrowser::Lightpanda,
                BrowserInstallSource::ManagedCache,
                path,
                cache_dir,
            );
        }
        missing_status(ManagedBrowser::Lightpanda, cache_dir, "not installed")
    }
    #[cfg(not(feature = "lightpanda"))]
    {
        BrowserRuntimeStatus {
            browser: ManagedBrowser::Lightpanda,
            available: false,
            source: BrowserInstallSource::Unsupported,
            path: None,
            version: None,
            cache_dir: None,
            detail: "support is not compiled into this build".to_string(),
        }
    }
}

fn env_executable(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn available_status(
    browser: ManagedBrowser,
    source: BrowserInstallSource,
    path: PathBuf,
    cache_dir: Option<PathBuf>,
) -> BrowserRuntimeStatus {
    if !is_usable_executable(&path) {
        return BrowserRuntimeStatus {
            browser,
            available: false,
            source,
            path: Some(path),
            version: None,
            cache_dir,
            detail: "executable is not runnable; use the repair operation".to_string(),
        };
    }
    let version = (source == BrowserInstallSource::ManagedCache)
        .then(|| {
            let cache = cache_dir.as_deref()?;
            path.strip_prefix(cache)
                .ok()?
                .components()
                .next()?
                .as_os_str()
                .to_str()
                .map(str::to_string)
        })
        .flatten();
    BrowserRuntimeStatus {
        browser,
        available: true,
        source,
        detail: "ready".to_string(),
        path: Some(path),
        version,
        cache_dir,
    }
}

fn is_usable_executable(path: &std::path::Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn missing_status(
    browser: ManagedBrowser,
    cache_dir: Option<PathBuf>,
    detail: &str,
) -> BrowserRuntimeStatus {
    BrowserRuntimeStatus {
        browser,
        available: false,
        source: BrowserInstallSource::Missing,
        path: None,
        version: None,
        cache_dir,
        detail: detail.to_string(),
    }
}

/// Removes only Browser runtimes installed under the A3S Use data root.
pub async fn uninstall_managed_browsers() -> UseResult<bool> {
    let root = crate::chrome::browser_data_root()?;
    uninstall_managed_browsers_at(&root).await
}

async fn uninstall_managed_browsers_at(root: &std::path::Path) -> UseResult<bool> {
    let mut changed = false;
    for directory in [root.join("chrome"), root.join("lightpanda")] {
        match tokio::fs::remove_dir_all(&directory).await {
            Ok(()) => changed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(browser_error(format!(
                    "Failed to remove managed Browser runtime '{}': {error}",
                    directory.display()
                )))
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uninstall_is_idempotent_and_preserves_unowned_files() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(temp.path().join("chrome/v1"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(temp.path().join("lightpanda/v1"))
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("keep"), b"unowned")
            .await
            .unwrap();

        assert!(uninstall_managed_browsers_at(temp.path()).await.unwrap());
        assert!(!uninstall_managed_browsers_at(temp.path()).await.unwrap());
        assert!(tokio::fs::try_exists(temp.path().join("keep"))
            .await
            .unwrap());
    }

    #[cfg(not(feature = "lightpanda"))]
    #[test]
    fn unsupported_lightpanda_status_is_explicit_without_the_feature() {
        let status = browser_status(ManagedBrowser::Lightpanda);
        assert_eq!(status.source, BrowserInstallSource::Unsupported);
        assert!(!status.available);
    }
}
