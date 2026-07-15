use std::time::{Duration, Instant};

use a3s_use_core::{Artifact, UseError, UseResult};
use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;
use sha2::{Digest, Sha256};
use tracing::warn;
use url::Url;

use crate::pool::{browser_error, BrowserPool};
use crate::{PageRenderer, RenderRequest, RenderedPage, WaitCondition};

#[async_trait]
impl PageRenderer for BrowserPool {
    async fn render(&self, request: RenderRequest) -> UseResult<RenderedPage> {
        let timeout = request.timeout();
        match tokio::time::timeout(timeout, self.render_inner(request)).await {
            Ok(result) => result,
            Err(_) => Err(UseError::new(
                "use.browser.timeout",
                format!("Browser rendering exceeded {} ms.", timeout.as_millis()),
            )),
        }
    }
}

impl BrowserPool {
    async fn render_inner(&self, request: RenderRequest) -> UseResult<RenderedPage> {
        let started = Instant::now();
        let _permit = self
            .tab_semaphore()
            .acquire()
            .await
            .map_err(|error| browser_error(format!("Tab limit is closed: {error}")))?;
        let browser = self.acquire_browser().await?;
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|error| browser_error(format!("Failed to open browser tab: {error}")))?;
        let guard = PageGuard::new(page);
        let page = guard.page()?;

        if let Some(user_agent) = &request.user_agent {
            page.set_user_agent(SetUserAgentOverrideParams::new(user_agent))
                .await
                .map_err(|error| {
                    browser_error(format!("Failed to set browser user agent: {error}"))
                })?;
        }

        page.goto(request.url.as_str())
            .await
            .map_err(|error| browser_error(format!("Browser navigation failed: {error}")))?;
        apply_wait_condition(page, &request.wait).await?;
        let html = page
            .content()
            .await
            .map_err(|error| browser_error(format!("Failed to read rendered HTML: {error}")))?;
        let final_url = page
            .url()
            .await
            .ok()
            .flatten()
            .and_then(|value| Url::parse(&value).ok())
            .unwrap_or_else(|| request.url.clone());
        let artifacts = match &request.screenshot_path {
            Some(path) => vec![capture_screenshot(page, path).await?],
            None => Vec::new(),
        };
        guard.close().await;

        Ok(RenderedPage {
            requested_url: request.url,
            final_url,
            status: None,
            content_type: Some("text/html".to_string()),
            html,
            elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            artifacts,
        })
    }
}

pub(crate) async fn apply_wait_condition(
    page: &chromiumoxide::Page,
    condition: &WaitCondition,
) -> UseResult<()> {
    match condition {
        // `Page::goto` resolves after the requested page is loaded.
        WaitCondition::Load | WaitCondition::DomContentLoaded => {}
        WaitCondition::NetworkIdle { idle_ms } => {
            tokio::time::sleep(Duration::from_millis(*idle_ms)).await;
        }
        WaitCondition::Selector { css, timeout_ms } => {
            match tokio::time::timeout(
                Duration::from_millis(*timeout_ms),
                page.find_element(css.as_str()),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    return Err(browser_error(format!(
                        "Browser selector '{css}' failed: {error}"
                    )))
                }
                Err(_) => {
                    return Err(UseError::new(
                        "use.browser.wait_timeout",
                        format!("Selector '{css}' was not found within {timeout_ms} ms."),
                    ))
                }
            }
        }
        WaitCondition::Delay { ms } => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
        }
    }
    Ok(())
}

pub(crate) async fn capture_screenshot(
    page: &chromiumoxide::Page,
    path: &std::path::Path,
) -> UseResult<Artifact> {
    let bytes = page
        .save_screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .full_page(true)
                .build(),
            path,
        )
        .await
        .map_err(|error| browser_error(format!("Failed to save browser screenshot: {error}")))?;
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(Artifact {
        path: path.to_path_buf(),
        media_type: "image/png".to_string(),
        size: bytes.len().try_into().unwrap_or(u64::MAX),
        sha256,
    })
}

struct PageGuard {
    page: Option<chromiumoxide::Page>,
}

impl PageGuard {
    fn new(page: chromiumoxide::Page) -> Self {
        Self { page: Some(page) }
    }

    fn page(&self) -> UseResult<&chromiumoxide::Page> {
        self.page.as_ref().ok_or_else(|| {
            UseError::new(
                "use.browser.page_closed",
                "The browser page was closed before rendering completed.",
            )
        })
    }

    async fn close(mut self) {
        if let Some(page) = self.page.take() {
            if let Err(error) = page.close().await {
                warn!("Failed to close browser tab: {error}");
            }
        }
    }
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        if let Some(page) = self.page.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    runtime.spawn(async move {
                        if let Err(error) = page.close().await {
                            warn!("Failed to close browser tab after cancellation: {error}");
                        }
                    });
                }
                Err(error) => warn!("Cannot schedule browser tab cleanup: {error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserPoolConfig, BrowserProvider};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovered_chrome_renders_a_network_free_page_when_available() {
        let Some(executable) = crate::detect_chrome() else {
            return;
        };
        let pool = BrowserPool::new(BrowserPoolConfig {
            provider: BrowserProvider::ChromeExecutable(executable),
            ..BrowserPoolConfig::default()
        });
        let request = RenderRequest {
            url: Url::parse("data:text/html,<main id='fixture'>a3s-use</main>").unwrap(),
            timeout_ms: 10_000,
            wait: WaitCondition::Load,
            user_agent: Some("a3s-use-browser-test".to_string()),
            screenshot_path: None,
        };

        let rendered = pool.render(request).await;
        pool.shutdown().await;

        let rendered = rendered.unwrap();
        assert!(rendered.html.contains("a3s-use"));
        assert_eq!(rendered.content_type.as_deref(), Some("text/html"));
    }
}
