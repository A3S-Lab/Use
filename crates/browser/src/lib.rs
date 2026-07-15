#[cfg(not(feature = "chrome"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{Artifact, DomainDiagnostic, Readiness};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;

pub use a3s_use_core::{UseError, UseResult};

#[cfg(feature = "chrome")]
mod chrome;
#[cfg(feature = "chrome")]
mod chrome_install;
#[cfg(feature = "chrome")]
mod cleanup;
#[cfg(feature = "chrome")]
mod install;
#[cfg(feature = "chrome")]
mod management;
#[cfg(feature = "chrome")]
mod pool;
#[cfg(feature = "chrome")]
mod renderer;
#[cfg(feature = "chrome")]
mod session;

#[cfg(feature = "lightpanda")]
mod lightpanda;

#[cfg(feature = "chrome")]
pub use chrome::{detect_chrome, ensure_chrome};
#[cfg(feature = "chrome")]
pub use management::{
    browser_status, browser_statuses, install_browser, repair_browser, uninstall_managed_browsers,
    update_browser, BrowserInstallSource, BrowserRuntimeStatus, ManagedBrowser,
};
#[cfg(feature = "chrome")]
pub use pool::{BrowserBackend, BrowserPool, BrowserPoolConfig, BrowserProvider};
#[cfg(feature = "chrome")]
pub use session::{
    BrowserActionResult, BrowserSessionInfo, BrowserSessions, BrowserSnapshot, OpenSessionRequest,
    SnapshotElement,
};

#[cfg(feature = "lightpanda")]
pub use lightpanda::{detect_lightpanda, ensure_lightpanda};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitCondition {
    Load,
    DomContentLoaded,
    NetworkIdle { idle_ms: u64 },
    Selector { css: String, timeout_ms: u64 },
    Delay { ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub url: Url,
    pub timeout_ms: u64,
    pub wait: WaitCondition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_path: Option<PathBuf>,
}

impl RenderRequest {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            timeout_ms: 30_000,
            wait: WaitCondition::DomContentLoaded,
            user_agent: None,
            screenshot_path: None,
        }
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedPage {
    pub requested_url: Url,
    pub final_url: Url,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    pub html: String,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

#[async_trait]
pub trait PageRenderer: Send + Sync {
    async fn render(&self, request: RenderRequest) -> UseResult<RenderedPage>;
}

#[derive(Clone)]
pub struct BrowserRuntime {
    renderer: Arc<dyn PageRenderer>,
}

impl BrowserRuntime {
    pub fn new(renderer: Arc<dyn PageRenderer>) -> Self {
        Self { renderer }
    }

    pub async fn render(&self, request: RenderRequest) -> UseResult<RenderedPage> {
        self.renderer.render(request).await
    }
}

/// Initial provider used until the proven Chrome implementation is extracted
/// from A3S Search.
pub struct UnavailableRenderer;

#[async_trait]
impl PageRenderer for UnavailableRenderer {
    async fn render(&self, _request: RenderRequest) -> UseResult<RenderedPage> {
        Err(UseError::new(
            "use.browser.runtime_missing",
            "No compatible browser provider is configured.",
        )
        .with_suggestion("Run 'a3s install use/browser' or configure a system browser."))
    }
}

pub fn doctor() -> DomainDiagnostic {
    #[cfg(feature = "chrome")]
    {
        let statuses = browser_statuses();
        if let Some(status) = statuses.iter().find(|status| status.available) {
            return DomainDiagnostic {
                domain: "browser".to_string(),
                readiness: Readiness::Ready,
                provider: Some(status.browser.as_str().to_string()),
                version: status.version.clone(),
                path: status.path.clone(),
                message: format!("The {} browser provider is ready.", status.browser.as_str()),
                suggestions: Vec::new(),
            };
        }
        DomainDiagnostic {
            domain: "browser".to_string(),
            readiness: Readiness::Missing,
            provider: None,
            version: None,
            path: None,
            message: "No compatible browser provider was found.".to_string(),
            suggestions: vec!["Run 'a3s install use/browser'.".to_string()],
        }
    }
    #[cfg(not(feature = "chrome"))]
    match discover_system_browser() {
        Some(path) => DomainDiagnostic {
            domain: "browser".to_string(),
            readiness: Readiness::Ready,
            provider: Some("system".to_string()),
            version: None,
            path: Some(path),
            message: "A compatible system browser is available.".to_string(),
            suggestions: Vec::new(),
        },
        None => DomainDiagnostic {
            domain: "browser".to_string(),
            readiness: Readiness::Missing,
            provider: None,
            version: None,
            path: None,
            message: "No compatible system browser was found.".to_string(),
            suggestions: vec!["Run 'a3s install use/browser'.".to_string()],
        },
    }
}

pub fn discover_system_browser() -> Option<PathBuf> {
    #[cfg(feature = "chrome")]
    {
        detect_chrome()
    }
    #[cfg(not(feature = "chrome"))]
    {
        if let Some(path) = std::env::var_os("A3S_BROWSER_EXECUTABLE").map(PathBuf::from) {
            if executable(&path) {
                return Some(path);
            }
        }
        let candidates = if cfg!(target_os = "macos") {
            vec![
                PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
                PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            ]
        } else {
            Vec::new()
        };
        if let Some(path) = candidates.into_iter().find(|path| executable(path)) {
            return Some(path);
        }
        let path = std::env::var_os("PATH")?;
        for directory in std::env::split_paths(&path) {
            for name in ["google-chrome", "chromium", "chromium-browser"] {
                let candidate = directory.join(name);
                if executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

#[cfg(not(feature = "chrome"))]
fn executable(path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRenderer;

    #[async_trait]
    impl PageRenderer for FakeRenderer {
        async fn render(&self, request: RenderRequest) -> UseResult<RenderedPage> {
            Ok(RenderedPage {
                requested_url: request.url.clone(),
                final_url: request.url,
                status: Some(200),
                content_type: Some("text/html".to_string()),
                html: "<main>fixture</main>".to_string(),
                elapsed_ms: 1,
                artifacts: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn renderer_is_injectable_without_a_cli_or_service() {
        let runtime = BrowserRuntime::new(Arc::new(FakeRenderer));
        let page = runtime
            .render(RenderRequest::new(
                Url::parse("https://example.com").unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(page.status, Some(200));
        assert!(page.html.contains("fixture"));
    }

    #[test]
    fn public_runtime_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BrowserRuntime>();
    }
}
