//! Cancellation-safe browser and provider-child cleanup.

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::browser::CloseParams;
use tracing::{debug, warn};

const BROWSER_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const BROWSER_HANDLE_RELEASE_GRACE: Duration = Duration::from_millis(250);

pub(crate) async fn close_and_reap_browser(browser: Option<Arc<Browser>>) -> bool {
    let Some(browser) = browser else {
        return true;
    };

    let release_deadline = tokio::time::Instant::now() + BROWSER_HANDLE_RELEASE_GRACE;
    while Arc::strong_count(&browser) > 1 && tokio::time::Instant::now() < release_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let mut browser = match Arc::try_unwrap(browser) {
        Ok(browser) => browser,
        Err(browser) => {
            // A shared handle cannot expose chromiumoxide's mutable child
            // handle, but CDP close still tears down the browser tree.
            let strong_count = Arc::strong_count(&browser);
            match tokio::time::timeout(
                BROWSER_CLOSE_TIMEOUT,
                browser.execute(CloseParams::default()),
            )
            .await
            {
                Ok(Ok(_)) => debug!("Browser close requested through shared CDP handle"),
                Ok(Err(error)) => warn!("Failed to close shared browser handle: {error}"),
                Err(_) => warn!(
                    "Timed out closing shared browser handle ({strong_count} references remain)"
                ),
            }
            return false;
        }
    };

    let close_succeeded = match tokio::time::timeout(BROWSER_CLOSE_TIMEOUT, browser.close()).await {
        Ok(Ok(_)) => {
            debug!("Browser close requested");
            true
        }
        Ok(Err(error)) => {
            warn!("Failed to close browser gracefully: {error}");
            false
        }
        Err(_) => {
            warn!("Timed out closing browser gracefully");
            false
        }
    };

    if close_succeeded {
        match tokio::time::timeout(BROWSER_CLOSE_TIMEOUT, browser.wait()).await {
            Ok(Ok(_)) => {
                debug!("Browser process exited and was reaped");
                return true;
            }
            Ok(Err(error)) => warn!("Failed while waiting for browser exit: {error}"),
            Err(_) => warn!("Timed out waiting for browser process exit"),
        }
    }

    match tokio::time::timeout(BROWSER_CLOSE_TIMEOUT, browser.kill()).await {
        Ok(Some(Ok(()))) => debug!("Browser process killed"),
        Ok(Some(Err(error))) => warn!("Failed to kill browser process: {error}"),
        Ok(None) => debug!("Connected browser has no owned child process"),
        Err(_) => {
            warn!("Timed out killing browser process");
            return false;
        }
    }
    match tokio::time::timeout(BROWSER_CLOSE_TIMEOUT, browser.wait()).await {
        Ok(Ok(_)) => {
            debug!("Browser process reaped after kill");
            true
        }
        Ok(Err(error)) => {
            warn!("Failed to reap browser process after kill: {error}");
            false
        }
        Err(_) => {
            warn!("Timed out reaping browser process after kill");
            false
        }
    }
}

pub(crate) async fn kill_and_reap_child(child: Option<tokio::process::Child>) -> bool {
    let Some(mut child) = child else {
        return true;
    };
    match child.try_wait() {
        Ok(Some(status)) => {
            debug!("Browser child had already exited with {status}");
            return true;
        }
        Ok(None) => {}
        Err(error) => warn!("Failed to inspect browser child process: {error}"),
    }

    match tokio::time::timeout(BROWSER_CLOSE_TIMEOUT, child.kill()).await {
        Ok(Ok(())) => debug!("Browser child process killed"),
        Ok(Err(error)) => warn!("Failed to kill browser child process: {error}"),
        Err(_) => {
            warn!("Timed out killing browser child process");
            return false;
        }
    }
    match tokio::time::timeout(BROWSER_CLOSE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => {
            debug!("Browser child process reaped with {status}");
            true
        }
        Ok(Err(error)) => {
            warn!("Failed to reap browser child process: {error}");
            false
        }
        Err(_) => {
            warn!("Timed out reaping browser child process");
            false
        }
    }
}

#[cfg(feature = "lightpanda")]
pub(crate) async fn finish_child_cleanup(child: Option<tokio::process::Child>) {
    let cleanup = tokio::spawn(kill_and_reap_child(child));
    if let Err(error) = cleanup.await {
        warn!("Browser child cleanup task failed: {error}");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawned_child_is_killed_and_reaped() {
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sleep fixture");

        assert!(kill_and_reap_child(Some(child)).await);
    }
}
