use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::thread::{self, Thread};
use std::time::{Duration, Instant, SystemTime};

use a3s_use_core::{UseError, UseResult};
use notify::{Config, Event, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, oneshot};

const FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ACTIVE_REGISTRY_WATCHERS: usize = 64;
static ACTIVE_REGISTRY_WATCHERS: AtomicUsize = AtomicUsize::new(0);

pub(super) struct RegistryChangeWatcher {
    events: mpsc::Receiver<()>,
    failure: Arc<Mutex<Option<String>>>,
    cancelled: Arc<AtomicBool>,
    watcher_thread: Thread,
}

impl RegistryChangeWatcher {
    pub(super) async fn start(
        target: PathBuf,
        ownership_root: PathBuf,
        deadline: Instant,
    ) -> UseResult<Option<Self>> {
        let target = std::path::absolute(target).map_err(|error| {
            watch_error(format!(
                "Failed to resolve the extension Registry notification path: {error}"
            ))
        })?;
        let ownership_root = std::path::absolute(ownership_root).map_err(|error| {
            watch_error(format!(
                "Failed to resolve the extension Registry ownership root: {error}"
            ))
        })?;
        if !target.starts_with(&ownership_root) {
            return Err(watch_error(
                "The extension Registry notification path escapes its ownership root.",
            ));
        }
        let watch_root = nearest_existing_directory(
            target.parent().ok_or_else(|| {
                watch_error("The extension Registry notification path has no parent directory.")
            })?,
            &ownership_root,
        )?;
        let (event_sender, events) = mpsc::channel(1);
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        let worker_target = target.clone();
        let worker_probe_events = event_sender.clone();
        let (ready_sender, ready) = oneshot::channel();
        let capacity = RegistryWatcherCapacity::acquire()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = thread::Builder::new()
            .name("a3s-use-registry-watch".to_owned())
            .spawn(move || {
                let _capacity = capacity;
                let mut observed_target = target_fingerprint(&worker_target);
                let watcher = platform_watcher(
                    &watch_root,
                    worker_target.clone(),
                    event_sender,
                    worker_failure,
                );
                match watcher {
                    Ok(watcher) => {
                        if ready_sender.send(Ok(())).is_err()
                            || worker_cancelled.load(Ordering::Acquire)
                        {
                            return;
                        }
                        // RecommendedWatcher teardown can join a platform
                        // worker on macOS. Keep construction, ownership, and
                        // Drop on this detached thread so an async runtime
                        // worker is never blocked during cancellation.
                        while !worker_cancelled.load(Ordering::Acquire) {
                            thread::park_timeout(FALLBACK_POLL_INTERVAL);
                            if worker_cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            let current_target = target_fingerprint(&worker_target);
                            if current_target != observed_target {
                                observed_target = current_target;
                                let _ = worker_probe_events.try_send(());
                            }
                        }
                        drop(watcher);
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                    }
                }
            })
            .map_err(|error| {
                watch_error(format!(
                    "Failed to start the extension Registry notification worker: {error}"
                ))
            })?;
        let watcher_thread = worker.thread().clone();
        drop(worker);

        let mut watcher = Self {
            events,
            failure,
            cancelled,
            watcher_thread,
        };
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(None);
        };
        match tokio::time::timeout(remaining, ready).await {
            Ok(Ok(Ok(()))) => Ok(Some(watcher)),
            Ok(Ok(Err(error))) => Err(watch_error(error)),
            Ok(Err(_)) => Err(watch_error(
                "The extension Registry notification worker stopped during setup.",
            )),
            Err(_) => {
                watcher.stop();
                Ok(None)
            }
        }
    }

    pub(super) async fn changed(&mut self, deadline: Instant) -> UseResult<bool> {
        if let Some(error) = self.take_failure() {
            return Err(watch_error(error));
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(false);
        };
        match tokio::time::timeout(remaining, self.events.recv()).await {
            Ok(Some(())) => {
                if let Some(error) = self.take_failure() {
                    return Err(watch_error(error));
                }
                Ok(true)
            }
            Ok(None) => Err(watch_error(
                "The extension Registry notification worker stopped unexpectedly.",
            )),
            Err(_) => Ok(false),
        }
    }

    fn take_failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.watcher_thread.unpark();
    }
}

impl Drop for RegistryChangeWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

struct RegistryWatcherCapacity;

impl RegistryWatcherCapacity {
    fn acquire() -> UseResult<Self> {
        ACTIVE_REGISTRY_WATCHERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_ACTIVE_REGISTRY_WATCHERS).then_some(active + 1)
            })
            .map_err(|_| {
                watch_error(format!(
                    "At most {MAX_ACTIVE_REGISTRY_WATCHERS} extension Registry watchers may be active in one process."
                ))
            })?;
        Ok(Self)
    }
}

impl Drop for RegistryWatcherCapacity {
    fn drop(&mut self) {
        ACTIVE_REGISTRY_WATCHERS.fetch_sub(1, Ordering::AcqRel);
    }
}

enum PlatformWatcher {
    Recommended { _watcher: RecommendedWatcher },
    Poll { _watcher: PollWatcher },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetFingerprint {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    is_file: bool,
    is_directory: bool,
    is_link_or_reparse: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn target_fingerprint(path: &Path) -> Option<TargetFingerprint> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return None,
    };
    Some(TargetFingerprint {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        is_file: metadata.is_file(),
        is_directory: metadata.is_dir(),
        is_link_or_reparse: a3s_use_core::metadata_is_link_or_reparse_point(&metadata),
        #[cfg(unix)]
        device: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.dev()
        },
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino()
        },
    })
}

fn platform_watcher(
    watch_root: &Path,
    target: PathBuf,
    events: mpsc::Sender<()>,
    failure: Arc<Mutex<Option<String>>>,
) -> Result<PlatformWatcher, String> {
    let recommended_events = events.clone();
    let recommended_root = watch_root.to_path_buf();
    let recommended_target = target.clone();
    let recommended_failure = Arc::clone(&failure);
    let recommended_error = match notify::recommended_watcher(move |event| {
        signal_event(
            event,
            &recommended_root,
            &recommended_target,
            &recommended_events,
            &recommended_failure,
        );
    }) {
        Ok(mut watcher) => match watcher.watch(watch_root, RecursiveMode::NonRecursive) {
            Ok(()) => {
                return Ok(PlatformWatcher::Recommended { _watcher: watcher });
            }
            Err(error) => error.to_string(),
        },
        Err(error) => error.to_string(),
    };

    // The native watcher is gone before the fallback is created. Discard any
    // callback error it emitted while registration was failing so a healthy
    // polling backend does not inherit stale failure state.
    *failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;

    let mut watcher = PollWatcher::new(
        {
            let watch_root = watch_root.to_path_buf();
            move |event| {
                signal_event(event, &watch_root, &target, &events, &failure);
            }
        },
        Config::default()
            .with_poll_interval(FALLBACK_POLL_INTERVAL)
            .with_compare_contents(false),
    )
    .map_err(|error| {
        format!(
            "No filesystem notification backend is available; the native backend failed with \
             '{recommended_error}' and the polling fallback failed with '{error}'."
        )
    })?;
    watcher
        .watch(watch_root, RecursiveMode::NonRecursive)
        .map_err(|error| {
            format!(
                "Failed to subscribe to Registry changes; the native backend failed with \
                 '{recommended_error}' and the polling fallback failed with '{error}'."
            )
        })?;
    Ok(PlatformWatcher::Poll { _watcher: watcher })
}

fn signal_event(
    event: notify::Result<Event>,
    watch_root: &Path,
    target: &Path,
    events: &mpsc::Sender<()>,
    failure: &Mutex<Option<String>>,
) {
    match event {
        Ok(event) if event_affects_target(&event, watch_root, target) => {
            let _ = events.try_send(());
        }
        Ok(_) => {}
        Err(error) => {
            *failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!(
                "The extension Registry notification backend failed: {error}"
            ));
            let _ = events.try_send(());
        }
    }
}

fn nearest_existing_directory(path: &Path, ownership_root: &Path) -> UseResult<PathBuf> {
    let mut candidate = path;
    let watch_root = loop {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() =>
            {
                break candidate.to_path_buf();
            }
            Ok(_) => {
                return Err(watch_error(
                    "The extension Registry notification root is not an owned directory.",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                candidate = candidate.parent().ok_or_else(|| {
                    watch_error("No existing extension Registry notification root was found.")
                })?;
            }
            Err(error) => {
                return Err(watch_error(format!(
                    "Failed to inspect the extension Registry notification root: {error}"
                )))
            }
        }
    };

    if watch_root.starts_with(ownership_root) {
        validate_owned_directory_chain(ownership_root, &watch_root)?;
    } else if !ownership_root.starts_with(&watch_root) {
        return Err(watch_error(
            "The extension Registry notification root escapes its ownership root.",
        ));
    }
    Ok(watch_root)
}

fn validate_owned_directory_chain(ownership_root: &Path, directory: &Path) -> UseResult<()> {
    let relative = directory.strip_prefix(ownership_root).map_err(|_| {
        watch_error("The extension Registry notification root escapes its ownership root.")
    })?;
    let mut current = ownership_root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            watch_error(format!(
                "Failed to inspect the extension Registry ownership chain: {error}"
            ))
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(watch_error(
                "The extension Registry notification root is not an owned directory.",
            ));
        }
    }
    Ok(())
}

fn event_affects_target(event: &Event, watch_root: &Path, target: &Path) -> bool {
    if event.need_rescan() {
        return true;
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                watch_root.join(path)
            };
            path == target || target.starts_with(path)
        })
}

fn watch_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_watch_failed", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_filter_ignores_access_and_unrelated_siblings() {
        let root = std::path::absolute("target/registry-watch-filter").unwrap();
        let target = root.join("installation/registry.json");
        let access =
            Event::new(EventKind::Access(notify::event::AccessKind::Read)).add_path(target.clone());
        let sibling = Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join("other/registry.json"));
        let ancestor = Event::new(EventKind::Create(notify::event::CreateKind::Folder))
            .add_path(root.join("installation"));
        let relative_ancestor = Event::new(EventKind::Create(notify::event::CreateKind::Folder))
            .add_path(PathBuf::from("installation"));

        assert!(!event_affects_target(&access, &root, &target));
        assert!(!event_affects_target(&sibling, &root, &target));
        assert!(event_affects_target(&ancestor, &root, &target));
        assert!(event_affects_target(&relative_ancestor, &root, &target));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn nearest_existing_directory_rejects_a_link_inside_the_owned_chain() {
        let temporary = tempfile::tempdir().unwrap();
        let ownership_root = temporary.path().join("state");
        let external = temporary.path().join("external");
        std::fs::create_dir(&ownership_root).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::create_dir(external.join("installation")).unwrap();
        let link = ownership_root.join("scope");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&external, &link).unwrap();
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(&link)
                .arg(&external)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "mklink /J failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let target_parent = link.join("installation");
        let error = nearest_existing_directory(&target_parent, &ownership_root).unwrap_err();

        assert_eq!(error.code, "use.extension.registry_watch_failed");
        assert!(error.message.contains("not an owned directory"));
    }

    #[tokio::test]
    async fn watcher_observes_atomic_creation_below_a_missing_state_root() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary
            .path()
            .join("state/installations/user/example/registry.json");
        let ownership_root = temporary.path().join("state");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut watcher = RegistryChangeWatcher::start(target.clone(), ownership_root, deadline)
            .await
            .unwrap()
            .unwrap();

        tokio::fs::create_dir_all(target.parent().unwrap())
            .await
            .unwrap();
        let staging = target.with_extension("tmp");
        tokio::fs::write(&staging, b"generation").await.unwrap();
        tokio::fs::rename(staging, target).await.unwrap();

        assert!(watcher.changed(deadline).await.unwrap());
    }

    #[tokio::test]
    async fn watcher_ignores_staging_and_observes_atomic_target_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("registry.json");
        tokio::fs::write(&target, b"before").await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut watcher =
            RegistryChangeWatcher::start(target.clone(), temporary.path().to_path_buf(), deadline)
                .await
                .unwrap()
                .unwrap();

        let staging = temporary.path().join(".registry-staging.tmp");
        tokio::fs::write(&staging, b"after").await.unwrap();
        #[cfg(windows)]
        tokio::fs::remove_file(&target).await.unwrap();
        tokio::fs::rename(staging, target).await.unwrap();

        assert!(watcher.changed(deadline).await.unwrap());
    }
}
