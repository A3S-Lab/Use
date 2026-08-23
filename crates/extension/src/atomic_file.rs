use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
const WINDOWS_PERSIST_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(windows)]
const WINDOWS_PERSIST_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

/// Atomically replace `target` with an already-synced temporary file.
///
/// This function may sleep while Windows releases a transient scanner or
/// sharing lock, so callers must run it on a blocking worker.
#[doc(hidden)]
pub fn persist_temporary_replace_blocking(temporary: PathBuf, target: &Path) -> io::Result<()> {
    let temporary = tempfile::TempPath::try_from_path(temporary)?;
    persist_temporary_path_blocking(temporary, target, replace)
}

/// Atomically publish a temporary path without replacing an existing target.
///
/// This function may sleep while Windows releases a transient scanner or
/// sharing lock, so callers must run it on a blocking worker.
#[doc(hidden)]
pub fn persist_temporary_noclobber_blocking(temporary: PathBuf, target: &Path) -> io::Result<()> {
    let temporary = tempfile::TempPath::try_from_path(temporary)?;
    persist_temporary_path_blocking(temporary, target, persist_noclobber)
}

/// Atomically publish an already-synced named temporary file without replacing
/// an existing target.
///
/// This function may sleep while Windows releases a transient scanner or
/// sharing lock, so callers must run it on a blocking worker.
#[doc(hidden)]
pub fn persist_named_temporary_noclobber_blocking(
    temporary: tempfile::NamedTempFile,
    target: &Path,
) -> io::Result<()> {
    persist_temporary_path_blocking(temporary.into_temp_path(), target, persist_noclobber)
}

/// Rename a file or directory, retrying transient Windows sharing failures.
///
/// Standard rename semantics are retained. Callers that require a missing
/// target must validate and serialize that invariant before calling. This
/// function may sleep, so callers must run it on a blocking worker.
#[doc(hidden)]
pub fn rename_path_with_windows_retry_blocking(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let started = std::time::Instant::now();
        loop {
            match std::fs::rename(source, target) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if !windows_path_mutation_error_is_retryable(&error)
                        || started.elapsed() >= WINDOWS_PERSIST_RETRY_TIMEOUT
                    {
                        return Err(error);
                    }
                    let remaining = WINDOWS_PERSIST_RETRY_TIMEOUT.saturating_sub(started.elapsed());
                    std::thread::sleep(WINDOWS_PERSIST_RETRY_DELAY.min(remaining));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(source, target)
    }
}

/// Remove a file, retrying transient Windows sharing failures.
///
/// This function may sleep while Windows releases a transient scanner or
/// sharing lock, so callers must run it on a blocking worker.
pub(crate) fn remove_file_with_windows_retry_blocking(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let started = std::time::Instant::now();
        loop {
            match std::fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if !windows_path_mutation_error_is_retryable(&error)
                        || started.elapsed() >= WINDOWS_PERSIST_RETRY_TIMEOUT
                    {
                        return Err(error);
                    }
                    let remaining = WINDOWS_PERSIST_RETRY_TIMEOUT.saturating_sub(started.elapsed());
                    std::thread::sleep(WINDOWS_PERSIST_RETRY_DELAY.min(remaining));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::remove_file(path)
    }
}

fn persist_temporary_path_blocking(
    temporary: tempfile::TempPath,
    target: &Path,
    persist: fn(tempfile::TempPath, &Path) -> Result<(), tempfile::PathPersistError>,
) -> io::Result<()> {
    #[cfg(windows)]
    {
        let started = std::time::Instant::now();
        let mut temporary = temporary;
        loop {
            match persist(temporary, target) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if !windows_path_mutation_error_is_retryable(&error.error)
                        || started.elapsed() >= WINDOWS_PERSIST_RETRY_TIMEOUT
                    {
                        return Err(error.error);
                    }
                    temporary = error.path;
                    let remaining = WINDOWS_PERSIST_RETRY_TIMEOUT.saturating_sub(started.elapsed());
                    std::thread::sleep(WINDOWS_PERSIST_RETRY_DELAY.min(remaining));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        persist(temporary, target).map_err(|error| error.error)
    }
}

fn replace(temporary: tempfile::TempPath, target: &Path) -> Result<(), tempfile::PathPersistError> {
    temporary.persist(target)
}

fn persist_noclobber(
    temporary: tempfile::TempPath,
    target: &Path,
) -> Result<(), tempfile::PathPersistError> {
    temporary.persist_noclobber(target)
}

#[cfg(windows)]
fn windows_path_mutation_error_is_retryable(error: &io::Error) -> bool {
    // MoveFileEx and DeleteFile report transient scanner, sharing, and
    // byte-range locks as access denied, sharing violations, or lock violations.
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_an_existing_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.json");
        let temporary = directory.path().join(".state.tmp");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();

        persist_temporary_replace_blocking(temporary, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new".to_vec());
    }

    #[test]
    fn noclobber_rejects_an_existing_target_without_changing_it() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.json");
        let temporary = directory.path().join(".state.tmp");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();

        let error = persist_temporary_noclobber_blocking(temporary.clone(), &target).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"old".to_vec());
        assert!(!temporary.exists());
    }

    #[test]
    fn path_rename_moves_a_directory_without_leaving_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("state.json"), b"state").unwrap();

        rename_path_with_windows_retry_blocking(&source, &target).unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read(target.join("state.json")).unwrap(), b"state");
    }

    #[test]
    fn path_rename_preserves_read_only_file_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.json");
        let target = directory.path().join("target.json");
        std::fs::write(&source, b"state").unwrap();
        let original_permissions = std::fs::metadata(&source).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&source, permissions).unwrap();

        rename_path_with_windows_retry_blocking(&source, &target).unwrap();

        assert!(!source.exists());
        assert!(std::fs::metadata(&target).unwrap().permissions().readonly());
        assert_eq!(std::fs::read(&target).unwrap(), b"state");
        std::fs::set_permissions(&target, original_permissions).unwrap();
    }

    #[test]
    fn file_removal_deletes_the_exact_path() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.json");
        std::fs::write(&target, b"state").unwrap();

        remove_file_with_windows_retry_blocking(&target).unwrap();

        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn retries_while_the_windows_target_is_temporarily_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.json");
        let temporary = directory.path().join(".state.tmp");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();

        let locked_target = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .unwrap();
        let activation_target = target.clone();
        let activation = std::thread::spawn(move || {
            persist_temporary_replace_blocking(temporary, &activation_target)
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!activation.is_finished());

        drop(locked_target);
        activation.join().unwrap().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new".to_vec());
    }

    #[cfg(windows)]
    #[test]
    fn noclobber_retries_while_the_windows_temporary_file_is_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.json");
        let temporary = directory.path().join(".state.tmp");
        std::fs::write(&temporary, b"new").unwrap();

        let locked_temporary = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&temporary)
            .unwrap();
        let activation_target = target.clone();
        let activation = std::thread::spawn(move || {
            persist_temporary_noclobber_blocking(temporary, &activation_target)
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!activation.is_finished());

        drop(locked_temporary);
        activation.join().unwrap().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new".to_vec());
    }

    #[cfg(windows)]
    #[test]
    fn stops_retrying_a_persistent_windows_lock_at_the_bound() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.json");
        let temporary = directory.path().join(".state.tmp");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();
        let locked_target = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .unwrap();

        let started = std::time::Instant::now();
        let error = persist_temporary_replace_blocking(temporary.clone(), &target).unwrap_err();
        let elapsed = started.elapsed();

        assert!(windows_path_mutation_error_is_retryable(&error));
        assert!(elapsed >= WINDOWS_PERSIST_RETRY_TIMEOUT);
        assert!(elapsed < WINDOWS_PERSIST_RETRY_TIMEOUT + std::time::Duration::from_secs(8));
        assert!(!temporary.exists());
        drop(locked_target);
        assert_eq!(std::fs::read(&target).unwrap(), b"old".to_vec());
    }

    #[cfg(windows)]
    #[test]
    fn path_rename_retries_while_a_windows_directory_is_temporarily_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        std::fs::create_dir(&source).unwrap();
        let locked_source = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&source)
            .unwrap();
        let rename_source = source.clone();
        let rename_target = target.clone();
        let rename = std::thread::spawn(move || {
            rename_path_with_windows_retry_blocking(&rename_source, &rename_target)
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!rename.is_finished());

        drop(locked_source);
        rename.join().unwrap().unwrap();
        assert!(!source.exists());
        assert!(target.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn path_rename_stops_at_the_bound_and_preserves_a_locked_directory() {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        std::fs::create_dir(&source).unwrap();
        let locked_source = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&source)
            .unwrap();

        let started = std::time::Instant::now();
        let error = rename_path_with_windows_retry_blocking(&source, &target).unwrap_err();
        let elapsed = started.elapsed();

        assert!(windows_path_mutation_error_is_retryable(&error));
        assert!(elapsed >= WINDOWS_PERSIST_RETRY_TIMEOUT);
        assert!(elapsed < WINDOWS_PERSIST_RETRY_TIMEOUT + std::time::Duration::from_secs(8));
        assert!(source.is_dir());
        assert!(!target.exists());
        drop(locked_source);
    }
}
