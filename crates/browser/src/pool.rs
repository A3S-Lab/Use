//! Chromiumoxide-backed Browser provider lifecycle.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(feature = "lightpanda")]
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, warn};

use a3s_use_core::{UseError, UseResult};

/// Selects which headless browser backend the pool uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BrowserBackend {
    /// Launch Chrome/Chromium locally.
    #[default]
    Chrome,

    /// Spawn a Lightpanda process and connect via CDP over WebSocket.
    #[cfg(feature = "lightpanda")]
    Lightpanda,
}

/// Explicit provider selection for a Browser pool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BrowserProvider {
    /// Use an already installed or previously managed Chrome executable.
    #[default]
    DiscoveredChrome,
    /// Permit A3S Use to download Chrome when no executable is available.
    ManagedChrome,
    /// Use this exact Chrome-compatible executable.
    ChromeExecutable(std::path::PathBuf),

    /// Use an already installed or previously managed Lightpanda executable.
    #[cfg(feature = "lightpanda")]
    DiscoveredLightpanda,
    /// Permit A3S Use to download Lightpanda when it is unavailable.
    #[cfg(feature = "lightpanda")]
    ManagedLightpanda,
    /// Use this exact Lightpanda executable.
    #[cfg(feature = "lightpanda")]
    LightpandaExecutable(std::path::PathBuf),
}

impl BrowserProvider {
    pub fn backend(&self) -> BrowserBackend {
        match self {
            Self::DiscoveredChrome | Self::ManagedChrome | Self::ChromeExecutable(_) => {
                BrowserBackend::Chrome
            }
            #[cfg(feature = "lightpanda")]
            Self::DiscoveredLightpanda
            | Self::ManagedLightpanda
            | Self::LightpandaExecutable(_) => BrowserBackend::Lightpanda,
        }
    }
}

/// Configuration for the browser pool.
#[derive(Debug, Clone)]
pub struct BrowserPoolConfig {
    /// Maximum number of concurrent browser tabs.
    pub max_tabs: usize,
    /// Whether to run Chrome in headless mode (ignored for Lightpanda).
    pub headless: bool,
    /// Typed provider selection. Downloads require a managed variant.
    pub provider: BrowserProvider,
    /// Proxy URL for the browser to use.
    pub proxy_url: Option<String>,
    /// Additional launch arguments for Chrome.
    pub launch_args: Vec<String>,
}

impl Default for BrowserPoolConfig {
    fn default() -> Self {
        Self {
            max_tabs: 4,
            headless: true,
            provider: BrowserProvider::default(),
            proxy_url: None,
            launch_args: Vec::new(),
        }
    }
}

/// A shared pool managing a single browser process with tab concurrency control.
///
/// The browser is lazily launched on the first `acquire_browser()` call.
/// A semaphore limits the number of concurrent tabs to prevent memory exhaustion.
pub struct BrowserPool {
    config: BrowserPoolConfig,
    chrome_profile_dir: std::path::PathBuf,
    closed: AtomicBool,
    runtime: Mutex<BrowserRuntime>,
    tab_semaphore: Arc<Semaphore>,
}

#[derive(Default)]
struct BrowserRuntime {
    browser: Option<Arc<Browser>>,
    child: Option<tokio::process::Child>,
}

impl BrowserPool {
    /// Creates a new browser pool with the given configuration.
    pub fn new(config: BrowserPoolConfig) -> Self {
        static NEXT_PROFILE_ID: AtomicU64 = AtomicU64::new(1);
        let max_tabs = config.max_tabs.max(1);
        let chrome_profile_dir = std::env::temp_dir().join(format!(
            "a3s-use-chrome-{}-{}",
            std::process::id(),
            NEXT_PROFILE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        Self {
            config,
            chrome_profile_dir,
            closed: AtomicBool::new(false),
            runtime: Mutex::new(BrowserRuntime::default()),
            tab_semaphore: Arc::new(Semaphore::new(max_tabs)),
        }
    }

    /// Returns the tab semaphore for acquiring permits before opening tabs.
    pub(crate) fn tab_semaphore(&self) -> &Arc<Semaphore> {
        &self.tab_semaphore
    }

    /// Returns the number of tabs that may be opened immediately.
    pub fn available_tab_permits(&self) -> usize {
        self.tab_semaphore.available_permits()
    }

    /// Starts the configured provider without exposing its implementation handle.
    pub async fn warm_up(&self) -> UseResult<()> {
        self.acquire_browser().await.map(|_| ())
    }

    /// Lazily acquires the browser, launching it on the first call.
    pub(crate) async fn acquire_browser(&self) -> UseResult<Arc<Browser>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(browser_error(
                "Browser pool has already been shut down".to_string(),
            ));
        }
        #[cfg(feature = "lightpanda")]
        if self.config.provider.backend() == BrowserBackend::Lightpanda {
            return self.acquire_lightpanda().await;
        }

        self.acquire_chrome().await
    }

    async fn acquire_chrome(&self) -> UseResult<Arc<Browser>> {
        let mut runtime = self.runtime.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(browser_error(
                "Browser pool has already been shut down".to_string(),
            ));
        }

        if let Some(ref browser) = runtime.browser {
            return Ok(Arc::clone(browser));
        }

        debug!("Launching Chrome headless browser");

        let mut builder = BrowserConfig::builder().user_data_dir(&self.chrome_profile_dir);

        if self.config.headless {
            builder = builder.arg("--headless=new");
        }

        let chrome_path = match &self.config.provider {
            BrowserProvider::DiscoveredChrome => crate::chrome::resolve_chrome()?,
            BrowserProvider::ManagedChrome => crate::chrome::ensure_chrome().await?,
            BrowserProvider::ChromeExecutable(path) => path.clone(),
            #[cfg(feature = "lightpanda")]
            _ => {
                return Err(browser_error(
                    "The selected provider is not Chrome-compatible.",
                ))
            }
        };
        debug!("Using Chrome at: {}", chrome_path.display());
        builder = builder.chrome_executable(chrome_path);

        builder = builder.arg(
            "--user-agent=Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        );

        builder = builder.arg("--disable-blink-features=AutomationControlled");

        builder = builder
            .arg("--disable-gpu")
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-extensions")
            .arg("--disable-background-networking")
            .arg("--disable-default-apps")
            .arg("--disable-sync")
            .arg("--disable-translate")
            .arg("--mute-audio")
            .arg("--no-first-run");

        if let Some(ref proxy) = self.config.proxy_url {
            builder = builder.arg(format!("--proxy-server={}", proxy));
        }

        for arg in &self.config.launch_args {
            builder = builder.arg(arg);
        }

        let browser_config = builder
            .build()
            .map_err(|e| browser_error(format!("Failed to build browser config: {}", e)))?;

        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| browser_error(format!("Failed to launch Chrome: {}", e)))?;

        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    warn!("Chrome CDP handler error: {}", e);
                }
            }
            debug!("Chrome CDP handler exited");
        });

        let browser = Arc::new(browser);
        runtime.browser = Some(Arc::clone(&browser));

        Ok(browser)
    }

    #[cfg(feature = "lightpanda")]
    async fn acquire_lightpanda(&self) -> UseResult<Arc<Browser>> {
        let mut runtime = self.runtime.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(browser_error(
                "Browser pool has already been shut down".to_string(),
            ));
        }

        if let Some(ref browser) = runtime.browser {
            return Ok(Arc::clone(browser));
        }

        debug!("Launching Lightpanda browser");

        let lp_path = match &self.config.provider {
            BrowserProvider::DiscoveredLightpanda => crate::lightpanda::resolve_lightpanda()?,
            BrowserProvider::ManagedLightpanda => crate::lightpanda::ensure_lightpanda().await?,
            BrowserProvider::LightpandaExecutable(path) => path.clone(),
            _ => {
                return Err(browser_error(
                    "The selected provider is not Lightpanda-compatible.",
                ))
            }
        };

        let port = find_free_port()?;

        let child = tokio::process::Command::new(&lp_path)
            .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                browser_error(format!(
                    "Failed to spawn Lightpanda ({}): {}",
                    lp_path.display(),
                    e
                ))
            })?;
        runtime.child = Some(child);

        if let Err(error) = wait_for_cdp_ready("127.0.0.1", port, Duration::from_secs(10)).await {
            crate::cleanup::finish_child_cleanup(runtime.child.take()).await;
            return Err(error);
        }

        let ws_url = format!("ws://127.0.0.1:{}", port);
        debug!("Connecting to Lightpanda CDP at {}", ws_url);

        let (browser, mut handler) = match Browser::connect(&ws_url).await {
            Ok(connected) => connected,
            Err(error) => {
                crate::cleanup::finish_child_cleanup(runtime.child.take()).await;
                return Err(browser_error(format!(
                    "Failed to connect to Lightpanda: {}",
                    error
                )));
            }
        };

        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    warn!("Lightpanda CDP handler error: {}", e);
                }
            }
            debug!("Lightpanda CDP handler exited");
        });

        let browser = Arc::new(browser);
        runtime.browser = Some(Arc::clone(&browser));

        Ok(browser)
    }

    /// Shuts down the browser and reaps its spawned child process.
    ///
    /// Runtime ownership is detached into a cleanup task before this method
    /// awaits process termination. If the caller itself is cancelled, cleanup
    /// therefore continues instead of dropping Chrome's parent and orphaning
    /// its renderer processes. Callers should release every `Arc<Browser>`
    /// returned by [`Self::acquire_browser`] first. If a shared handle remains,
    /// shutdown can request CDP close but final reaping is deferred until the
    /// last external handle is dropped.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.tab_semaphore.close();
        let runtime = {
            let mut guard = self.runtime.lock().await;
            std::mem::take(&mut *guard)
        };
        let profile_dir = self.chrome_profile_dir.clone();
        let cleanup = tokio::spawn(async move {
            let browser_reaped = crate::cleanup::close_and_reap_browser(runtime.browser).await;
            let _ = crate::cleanup::kill_and_reap_child(runtime.child).await;
            if browser_reaped {
                match tokio::fs::remove_dir_all(&profile_dir).await {
                    Ok(()) => debug!("Removed browser profile {}", profile_dir.display()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => warn!(
                        "Failed to remove browser profile {}: {}",
                        profile_dir.display(),
                        error
                    ),
                }
            }
        });
        if let Err(error) = cleanup.await {
            warn!("Browser cleanup task failed: {}", error);
        }
    }
}

#[allow(dead_code)]
fn find_free_port() -> UseResult<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| browser_error(format!("Failed to find a free port: {}", e)))?;
    let port = listener
        .local_addr()
        .map_err(|e| browser_error(format!("Failed to read assigned port: {}", e)))?
        .port();
    Ok(port)
}

#[cfg(feature = "lightpanda")]
async fn wait_for_cdp_ready(host: &str, port: u16, timeout: Duration) -> UseResult<()> {
    let addr = format!("{}:{}", host, port);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(browser_error(format!(
                "Timed out waiting for CDP server at {} to become ready",
                addr
            )));
        }

        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            debug!("CDP server at {} is ready", addr);
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub(crate) fn browser_error(message: impl Into<String>) -> UseError {
    UseError::new("use.browser.provider_failed", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_discovers_chrome_without_authorizing_downloads() {
        let config = BrowserPoolConfig::default();
        assert_eq!(config.provider, BrowserProvider::DiscoveredChrome);
        assert_eq!(config.provider.backend(), BrowserBackend::Chrome);
    }

    #[test]
    fn zero_tab_configuration_is_clamped_to_one() {
        let pool = BrowserPool::new(BrowserPoolConfig {
            max_tabs: 0,
            ..BrowserPoolConfig::default()
        });
        assert_eq!(pool.available_tab_permits(), 1);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_and_prevents_restart() {
        let pool = BrowserPool::new(BrowserPoolConfig::default());
        pool.shutdown().await;
        pool.shutdown().await;

        assert!(pool.tab_semaphore().try_acquire().is_err());
        let error = pool.warm_up().await.unwrap_err();
        assert_eq!(error.code, "use.browser.provider_failed");
        assert!(error.message.contains("shut down"));
    }
}
