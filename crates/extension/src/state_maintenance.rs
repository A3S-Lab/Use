use std::collections::HashMap;
use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use tokio::fs;
use tokio::time::sleep;

pub const ACTIVE_STATE_RESTORE_MARKER: &str = ".maintenance.restore.json";

#[derive(Debug, Clone, Copy)]
enum MaintenanceMode {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Copy)]
enum LockBehavior {
    Wait,
    Try,
}

/// Cross-process boundary between ordinary state mutation and coordinated
/// maintenance such as restore.
///
/// Lifecycle owners take a shared guard around every state family that must
/// advance together. A restore takes the exclusive guard before validating
/// authority and keeps it through publication, so it cannot observe or create
/// a split across Registry, Grant, lifecycle, binding, and database evidence.
///
/// Nested shared acquires in the same process reuse one OS lock. Opening a
/// second shared handle on `.maintenance.lock` deadlocks on Windows when the
/// first shared fence is still held (Control drain under CPM install).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMaintenanceLock {
    state_root: PathBuf,
}

#[derive(Debug)]
pub struct StateMaintenanceGuard {
    file: Option<StdFile>,
    state_root: PathBuf,
    mode: MaintenanceMode,
}

struct SharedHold {
    /// OS lock handle; released when the last nested shared guard drops.
    #[allow(dead_code)]
    file: StdFile,
    depth: usize,
}

fn shared_holds() -> &'static Mutex<HashMap<PathBuf, SharedHold>> {
    static HOLDS: OnceLock<Mutex<HashMap<PathBuf, SharedHold>>> = OnceLock::new();
    HOLDS.get_or_init(|| Mutex::new(HashMap::new()))
}

impl StateMaintenanceGuard {
    /// Prove that this guard owns a shared maintenance lock for the exact
    /// configured state root.
    ///
    /// Shared guards are the ordinary mutation boundary. Exposing the mode
    /// check keeps cross-store coordinators from relying on a comment that a
    /// caller already holds the right file lock.
    pub fn is_shared_for(&self, state_root: &Path) -> bool {
        matches!(self.mode, MaintenanceMode::Shared) && self.state_root == state_root
    }

    /// Prove that this unforgeable guard owns the exclusive maintenance lock
    /// for the exact configured state root.
    pub fn is_exclusive_for(&self, state_root: &Path) -> bool {
        matches!(self.mode, MaintenanceMode::Exclusive) && self.state_root == state_root
    }
}

impl Drop for StateMaintenanceGuard {
    fn drop(&mut self) {
        if matches!(self.mode, MaintenanceMode::Shared) {
            let Ok(mut holds) = shared_holds().lock() else {
                return;
            };
            if let Some(hold) = holds.get_mut(&self.state_root) {
                hold.depth = hold.depth.saturating_sub(1);
                if hold.depth == 0 {
                    holds.remove(&self.state_root);
                }
            }
            // Nested guards never own the OS handle; the map entry does.
            self.file.take();
        }
    }
}

impl StateMaintenanceLock {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }

    pub async fn acquire_shared(&self) -> UseResult<StateMaintenanceGuard> {
        self.acquire(MaintenanceMode::Shared, LockBehavior::Wait)
            .await?
            .ok_or_else(maintenance_lock_failed)
    }

    /// Attempt to join ordinary state access without waiting behind an
    /// exclusive maintenance operation. A successful guard still rejects a
    /// durable active-restore marker.
    pub async fn try_acquire_shared(&self) -> UseResult<Option<StateMaintenanceGuard>> {
        self.acquire(MaintenanceMode::Shared, LockBehavior::Try)
            .await
    }

    pub async fn acquire_exclusive(&self) -> UseResult<StateMaintenanceGuard> {
        self.acquire(MaintenanceMode::Exclusive, LockBehavior::Wait)
            .await?
            .ok_or_else(maintenance_lock_failed)
    }

    async fn acquire(
        &self,
        mode: MaintenanceMode,
        behavior: LockBehavior,
    ) -> UseResult<Option<StateMaintenanceGuard>> {
        fs::create_dir_all(&self.state_root)
            .await
            .map_err(|error| maintenance_io("create state root", &self.state_root, error))?;
        require_owned_directory(&self.state_root).await?;

        if matches!(mode, MaintenanceMode::Shared) {
            if let Some(guard) = self.try_nested_shared()? {
                reject_active_restore(&self.state_root).await?;
                return Ok(Some(guard));
            }
        } else {
            // Wait until this process releases every nested shared hold for the
            // root. Exclusive still serializes against other processes via flock.
            loop {
                let depth = {
                    let holds = shared_holds()
                        .lock()
                        .map_err(|_| maintenance_lock_poisoned())?;
                    holds
                        .get(&self.state_root)
                        .map(|hold| hold.depth)
                        .unwrap_or(0)
                };
                if depth == 0 {
                    break;
                }
                if matches!(behavior, LockBehavior::Try) {
                    return Ok(None);
                }
                sleep(Duration::from_millis(10)).await;
            }
        }

        let lock_path = self.state_root.join(".maintenance.lock");
        match fs::symlink_metadata(&lock_path).await {
            Ok(metadata)
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    || !metadata.is_file() =>
            {
                return Err(maintenance_path_invalid())
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(maintenance_io(
                    "inspect maintenance lock",
                    &lock_path,
                    error,
                ))
            }
        }
        let error_path = lock_path.clone();
        let file = tokio::task::spawn_blocking(move || -> io::Result<StdFile> {
            let file = StdOpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)?;
            match (mode, behavior) {
                (MaintenanceMode::Shared, LockBehavior::Wait) => FileExt::lock_shared(&file)?,
                (MaintenanceMode::Shared, LockBehavior::Try) => FileExt::try_lock_shared(&file)?,
                (MaintenanceMode::Exclusive, LockBehavior::Wait) => FileExt::lock_exclusive(&file)?,
                (MaintenanceMode::Exclusive, LockBehavior::Try) => {
                    FileExt::try_lock_exclusive(&file)?
                }
            }
            Ok(file)
        })
        .await
        .map_err(|error| {
            UseError::new(
                "use.state.maintenance_lock_failed",
                format!(
                    "Failed to acquire state maintenance lock '{}': blocking task failed: {error}",
                    error_path.display()
                ),
            )
        })?;
        let file = match file {
            Ok(file) => file,
            Err(error) if matches!(behavior, LockBehavior::Try) && lock_is_contended(&error) => {
                return Ok(None)
            }
            Err(error) => {
                return Err(maintenance_io(
                    "acquire maintenance lock",
                    &error_path,
                    error,
                ))
            }
        };
        require_owned_directory(&self.state_root).await?;
        let metadata = fs::symlink_metadata(&error_path)
            .await
            .map_err(|error| maintenance_io("reinspect maintenance lock", &error_path, error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
            return Err(maintenance_path_invalid());
        }
        if matches!(mode, MaintenanceMode::Shared) {
            reject_active_restore(&self.state_root).await?;
            let mut holds = shared_holds()
                .lock()
                .map_err(|_| maintenance_lock_poisoned())?;
            // Another task may have published a hold between the nested check
            // and this flock; prefer the existing hold and drop the new handle.
            if let Some(hold) = holds.get_mut(&self.state_root) {
                hold.depth = hold.depth.saturating_add(1);
                drop(file);
                return Ok(Some(StateMaintenanceGuard {
                    file: None,
                    state_root: self.state_root.clone(),
                    mode,
                }));
            }
            holds.insert(self.state_root.clone(), SharedHold { file, depth: 1 });
            return Ok(Some(StateMaintenanceGuard {
                file: None,
                state_root: self.state_root.clone(),
                mode,
            }));
        }
        Ok(Some(StateMaintenanceGuard {
            file: Some(file),
            state_root: self.state_root.clone(),
            mode,
        }))
    }

    fn try_nested_shared(&self) -> UseResult<Option<StateMaintenanceGuard>> {
        let mut holds = shared_holds()
            .lock()
            .map_err(|_| maintenance_lock_poisoned())?;
        let Some(hold) = holds.get_mut(&self.state_root) else {
            return Ok(None);
        };
        hold.depth = hold.depth.saturating_add(1);
        Ok(Some(StateMaintenanceGuard {
            file: None,
            state_root: self.state_root.clone(),
            mode: MaintenanceMode::Shared,
        }))
    }
}

fn maintenance_lock_poisoned() -> UseError {
    UseError::new(
        "use.state.maintenance_lock_failed",
        "The in-process shared maintenance hold map is poisoned.",
    )
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    false
}

async fn reject_active_restore(state_root: &Path) -> UseResult<()> {
    let marker = state_root.join(ACTIVE_STATE_RESTORE_MARKER);
    match fs::symlink_metadata(&marker).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() > 0 =>
        {
            Err(UseError::new(
                "use.state.maintenance_restore_active",
                "A durable state restore is incomplete; ordinary state access remains blocked until the exact restore operation is resumed.",
            )
            .with_suggestion(
                "Resume the reviewed restore with its exact plan digest before retrying this operation.",
            ))
        }
        Ok(_) => Err(maintenance_path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(maintenance_io("inspect active restore marker", &marker, error)),
    }
}

async fn require_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| maintenance_io("inspect state root", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(maintenance_path_invalid());
    }
    Ok(())
}

fn maintenance_path_invalid() -> UseError {
    UseError::new(
        "use.state.maintenance_path_invalid",
        "The state maintenance root or lock is not an owned directory or regular file.",
    )
}

fn maintenance_lock_failed() -> UseError {
    UseError::new(
        "use.state.maintenance_lock_failed",
        "The blocking state maintenance lock did not return an acquired guard.",
    )
}

fn maintenance_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.state.maintenance_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn nested_shared_maintenance_reuses_one_os_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let lock = StateMaintenanceLock::new(temporary.path());
        let outer = lock.acquire_shared().await.unwrap();
        let inner = lock.acquire_shared().await.unwrap();
        assert!(outer.is_shared_for(temporary.path()));
        assert!(inner.is_shared_for(temporary.path()));
        drop(inner);
        assert!(outer.is_shared_for(temporary.path()));
        let exclusive_lock = lock.clone();
        let mut exclusive =
            tokio::spawn(async move { exclusive_lock.acquire_exclusive().await.unwrap() });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut exclusive)
                .await
                .is_err()
        );
        drop(outer);
        tokio::time::timeout(Duration::from_secs(1), exclusive)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn exclusive_maintenance_waits_for_shared_and_blocks_new_shared_guards() {
        let temporary = tempfile::tempdir().unwrap();
        let lock = StateMaintenanceLock::new(temporary.path());
        let shared = lock.acquire_shared().await.unwrap();
        assert!(shared.is_shared_for(temporary.path()));
        assert!(!shared.is_exclusive_for(temporary.path()));
        let exclusive_lock = lock.clone();
        let mut exclusive =
            tokio::spawn(async move { exclusive_lock.acquire_exclusive().await.unwrap() });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut exclusive)
                .await
                .is_err()
        );
        drop(shared);
        let exclusive = tokio::time::timeout(Duration::from_secs(1), exclusive)
            .await
            .unwrap()
            .unwrap();
        assert!(exclusive.is_exclusive_for(temporary.path()));
        assert!(!exclusive.is_shared_for(temporary.path()));
        assert!(!exclusive.is_exclusive_for(&temporary.path().join("other")));

        assert!(lock.try_acquire_shared().await.unwrap().is_none());
        let shared_lock = lock.clone();
        let mut blocked_shared =
            tokio::spawn(async move { shared_lock.acquire_shared().await.unwrap() });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut blocked_shared)
                .await
                .is_err()
        );
        drop(exclusive);
        tokio::time::timeout(Duration::from_secs(1), blocked_shared)
            .await
            .unwrap()
            .unwrap();
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn maintenance_rejects_a_linked_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"outside").unwrap();
        crate::test_filesystem::create_directory_link(
            &target,
            &temporary.path().join(".maintenance.lock"),
        );
        let error = StateMaintenanceLock::new(temporary.path())
            .acquire_exclusive()
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.state.maintenance_path_invalid");
        assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"outside");
    }

    #[tokio::test]
    async fn shared_maintenance_rejects_a_durable_restore_marker() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join(ACTIVE_STATE_RESTORE_MARKER),
            b"active",
        )
        .unwrap();

        let lock = StateMaintenanceLock::new(temporary.path());
        let error = lock.acquire_shared().await.unwrap_err();
        assert_eq!(error.code, "use.state.maintenance_restore_active");
        let error = lock.try_acquire_shared().await.unwrap_err();
        assert_eq!(error.code, "use.state.maintenance_restore_active");
        lock.acquire_exclusive().await.unwrap();
    }
}
