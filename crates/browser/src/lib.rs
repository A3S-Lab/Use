use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{Artifact, DomainDiagnostic, Readiness, UseError, UseResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitCondition {
    Load,
    DomContentLoaded,
    NetworkIdle,
    Selector(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub url: Url,
    pub timeout_ms: u64,
    pub wait: WaitCondition,
    pub capture_screenshot: bool,
}

impl RenderRequest {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            timeout_ms: 30_000,
            wait: WaitCondition::DomContentLoaded,
            capture_screenshot: false,
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
