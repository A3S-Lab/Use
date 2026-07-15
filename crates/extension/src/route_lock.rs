use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;

use super::package::io_error;

const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct RouteDrainGuard {
    file: File,
}

impl Drop for RouteDrainGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) fn open_route_lock(path: &Path) -> UseResult<File> {
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.extension.lock_invalid",
            "The extension route lock has no parent directory.",
        )
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| io_error("create extension route lock directory", parent, error))?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error("open extension route lock", path, error))
}

pub(super) async fn acquire_drain_lock(
    path: &Path,
    timeout: Duration,
) -> UseResult<RouteDrainGuard> {
    let file = open_route_lock(path)?;
    let deadline = deadline_after(timeout)?;
    loop {
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(RouteDrainGuard { file }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let now = Instant::now();
                if now >= deadline {
                    let timeout_ms = timeout.as_millis().min(u64::MAX as u128) as u64;
                    return Err(UseError::new(
                        "use.extension.drain_timeout",
                        "The extension route was disabled, but accepted calls did not drain before the timeout.",
                    )
                    .with_detail("routeDisabled", true)
                    .with_detail("timeoutMs", timeout_ms)
                    .with_suggestion(
                        "Wait for in-flight calls to finish, then retry the lifecycle operation.",
                    ));
                }
                tokio::time::sleep(
                    DRAIN_POLL_INTERVAL.min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(error) => {
                return Err(io_error(
                    "acquire exclusive extension drain lock",
                    path,
                    error,
                ))
            }
        }
    }
}

pub(super) fn deadline_after(timeout: Duration) -> UseResult<Instant> {
    Instant::now().checked_add(timeout).ok_or_else(|| {
        UseError::new(
            "use.extension.timeout_invalid",
            "The requested extension lifecycle timeout is too large for this platform.",
        )
    })
}
