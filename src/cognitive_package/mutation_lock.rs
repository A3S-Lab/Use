use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use tokio::fs;

const INSTALLATION_MUTATION_LOCK_FILE: &str = ".installation-mutation.lock";

/// Conservative cross-process writer fence for one explicit installation.
///
/// Holding this guard from live-state inspection through terminal graph
/// persistence gives every install, upgrade, uninstall, enablement, and
/// recovery mutation in that installation one serial order without blocking
/// independent installations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstallationMutationLock {
    state_root: PathBuf,
}

#[derive(Debug)]
pub(super) struct InstallationMutationGuard {
    _file: StdFile,
}

impl InstallationMutationLock {
    pub(super) fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }

    pub(super) async fn acquire(&self) -> UseResult<InstallationMutationGuard> {
        fs::create_dir_all(&self.state_root)
            .await
            .map_err(|error| mutation_io("create state root", &self.state_root, error))?;
        require_owned_directory(&self.state_root).await?;

        let lock_path = self.state_root.join(INSTALLATION_MUTATION_LOCK_FILE);
        match fs::symlink_metadata(&lock_path).await {
            Ok(metadata)
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    || !metadata.is_file() =>
            {
                return Err(mutation_path_invalid())
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(mutation_io(
                    "inspect installation mutation lock",
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
            FileExt::lock_exclusive(&file)?;
            Ok(file)
        })
        .await
        .map_err(|error| {
            UseError::new(
                "use.plugin.installation_mutation_lock_failed",
                format!(
                    "Failed to acquire installation mutation lock '{}': blocking task failed: {error}",
                    error_path.display()
                ),
            )
        })?
        .map_err(|error| mutation_io("acquire installation mutation lock", &error_path, error))?;

        require_owned_directory(&self.state_root).await?;
        let metadata = fs::symlink_metadata(&error_path).await.map_err(|error| {
            mutation_io("reinspect installation mutation lock", &error_path, error)
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
            return Err(mutation_path_invalid());
        }
        Ok(InstallationMutationGuard { _file: file })
    }
}

async fn require_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| mutation_io("inspect installation state root", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(mutation_path_invalid());
    }
    Ok(())
}

fn mutation_path_invalid() -> UseError {
    UseError::new(
        "use.plugin.installation_mutation_lock_invalid",
        "The installation mutation state root or lock is not an owned directory or regular file.",
    )
}

fn mutation_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.plugin.installation_mutation_lock_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::time::Duration;

    use super::*;

    const CHILD_ROOT_ENV: &str = "A3S_USE_TEST_INSTALLATION_MUTATION_LOCK_ROOT";

    fn assert_send_sync<T: Send + Sync>() {}

    #[tokio::test]
    async fn independent_instances_serialize_one_installation_domain() {
        assert_send_sync::<InstallationMutationLock>();
        assert_send_sync::<InstallationMutationGuard>();

        let temporary = tempfile::tempdir().unwrap();
        let first = InstallationMutationLock::new(temporary.path())
            .acquire()
            .await
            .unwrap();
        let second_lock = InstallationMutationLock::new(temporary.path());
        let mut second = tokio::spawn(async move { second_lock.acquire().await.unwrap() });
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err());

        drop(first);
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn independent_processes_serialize_one_installation_domain() {
        let temporary = tempfile::tempdir().unwrap();
        let started_path = temporary.path().join("child-started");
        let acquired_path = temporary.path().join("child-acquired");
        let parent = InstallationMutationLock::new(temporary.path())
            .acquire()
            .await
            .unwrap();
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .arg("installation_mutation_lock_subprocess_child")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(CHILD_ROOT_ENV, temporary.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            while !started_path.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!acquired_path.exists());
        assert!(child.try_wait().unwrap().is_none());

        drop(parent);
        let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "mutation-lock child failed: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(acquired_path.exists());
    }

    #[tokio::test]
    #[ignore = "subprocess helper for installation mutation locking"]
    async fn installation_mutation_lock_subprocess_child() {
        let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
            return;
        };
        fs::write(root.join("child-started"), b"started")
            .await
            .unwrap();
        let guard = InstallationMutationLock::new(&root)
            .acquire()
            .await
            .unwrap();
        fs::write(root.join("child-acquired"), b"acquired")
            .await
            .unwrap();
        drop(guard);
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn linked_mutation_lock_fails_without_following_it() {
        let temporary = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("sentinel"), b"outside").unwrap();
        crate::test_filesystem::create_directory_link(
            external.path(),
            &temporary.path().join(INSTALLATION_MUTATION_LOCK_FILE),
        );

        let error = InstallationMutationLock::new(temporary.path())
            .acquire()
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.installation_mutation_lock_invalid");
        assert_eq!(
            std::fs::read(external.path().join("sentinel")).unwrap(),
            b"outside"
        );
    }
}
