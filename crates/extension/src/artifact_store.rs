use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use tokio::fs;

use crate::package::{io_error, lock_is_contended};

mod blob;

pub(crate) use blob::ArtifactBlob;

const EXPANDED_PACKAGES_DIRECTORY: &str = "expanded-packages";
const SHA256_DIRECTORY: &str = "sha256";
const CONTENT_DIRECTORY: &str = "content";
const MUTATION_LOCK: &str = ".mutation.lock";
const MUTATION_LOCK_WAIT: Duration = Duration::from_secs(2);
const MUTATION_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Global owner of immutable, content-addressed package bytes.
///
/// Installation selection, enablement, lifecycle generation, and publication
/// never belong to this store. Those remain scoped by `InstallationId`; this
/// store only deduplicates bytes whose digest has already been verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub(crate) fn from_data_root(data_root: &Path) -> Self {
        Self {
            root: data_root.join("artifacts"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve one expanded-package artifact from its canonical digest.
    pub fn expanded_package_path(&self, digest: &str) -> UseResult<PathBuf> {
        let sha256 = digest.strip_prefix("sha256:").ok_or_else(|| {
            artifact_store_error(
                "use.artifact_store.digest_invalid",
                "An expanded-package artifact digest must use the 'sha256:' prefix.",
            )
        })?;
        validate_sha256(sha256)?;
        Ok(self.expanded_package_path_from_sha256(sha256))
    }

    pub(crate) fn expanded_package_path_from_sha256(&self, sha256: &str) -> PathBuf {
        self.expanded_package_container(sha256)
            .join(CONTENT_DIRECTORY)
    }

    pub(crate) async fn validate_expanded_package_path(
        &self,
        sha256: &str,
        path: &Path,
    ) -> UseResult<()> {
        validate_sha256(sha256)?;
        let expected = self.expanded_package_path_from_sha256(sha256);
        if path != expected {
            return Err(artifact_store_error(
                "use.artifact_store.ownership_invalid",
                "An expanded-package path does not match its content digest.",
            ));
        }
        let relative = expected.strip_prefix(&self.root).map_err(|_| {
            artifact_store_error(
                "use.artifact_store.ownership_invalid",
                "An expanded-package path escapes the Artifact Store.",
            )
        })?;
        let mut current = self.root.clone();
        validate_real_directory(&current, "Artifact Store root").await?;
        for component in relative.components() {
            current.push(component.as_os_str());
            validate_real_directory(&current, "expanded-package Artifact Store directory").await?;
        }
        Ok(())
    }

    pub(crate) async fn acquire_expanded_package_mutation(
        &self,
        sha256: &str,
    ) -> UseResult<ArtifactMutationLock> {
        validate_sha256(sha256)?;
        let container = self.expanded_package_container(sha256);
        self.ensure_container(&container, "expanded-package artifact")
            .await?;
        ArtifactMutationLock::acquire(&container.join(MUTATION_LOCK), "expanded-package artifact")
            .await
    }

    fn expanded_package_container(&self, sha256: &str) -> PathBuf {
        let shard = sha256.get(..2).unwrap_or_default();
        self.root
            .join(EXPANDED_PACKAGES_DIRECTORY)
            .join(SHA256_DIRECTORY)
            .join(shard)
            .join(sha256)
    }

    pub(super) async fn ensure_container(&self, container: &Path, label: &str) -> UseResult<()> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|error| io_error("create Artifact Store root", &self.root, error))?;
        validate_real_directory(&self.root, "Artifact Store root").await?;

        let relative = container.strip_prefix(&self.root).map_err(|_| {
            artifact_store_error(
                "use.artifact_store.ownership_invalid",
                "An expanded-package artifact path escapes the Artifact Store.",
            )
        })?;
        let mut current = self.root.clone();
        for component in relative.components() {
            current.push(component.as_os_str());
            match fs::create_dir(&current).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(io_error(
                        &format!("create {label} Artifact Store directory"),
                        &current,
                        error,
                    ))
                }
            }
            validate_real_directory(&current, &format!("{label} Artifact Store directory")).await?;
        }
        Ok(())
    }
}

pub(super) struct ArtifactMutationLock(File);

impl ArtifactMutationLock {
    pub(super) async fn acquire(path: &Path, label: &str) -> UseResult<Self> {
        let file = open_lock_file(path, label)?;
        let deadline = tokio::time::Instant::now() + MUTATION_LOCK_WAIT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self(file)),
                Err(error) if lock_is_contended(&error) => {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(artifact_store_error(
                            "use.artifact_store.busy",
                            format!("Another process is committing the same {label}."),
                        ));
                    }
                    tokio::time::sleep(
                        MUTATION_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                    )
                    .await;
                }
                Err(error) => return Err(io_error(&format!("acquire {label} lock"), path, error)),
            }
        }
    }
}

impl Drop for ArtifactMutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn open_lock_file(path: &Path, label: &str) -> UseResult<File> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        validate_lock_metadata(path, &metadata, label)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options
        .open(path)
        .map_err(|error| io_error(&format!("open {label} lock"), path, error))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("inspect {label} lock"), path, error))?;
    validate_lock_metadata(path, &metadata, label)?;
    Ok(file)
}

fn validate_lock_metadata(path: &Path, metadata: &std::fs::Metadata, label: &str) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata) || !metadata.is_file() {
        return Err(artifact_store_error(
            "use.artifact_store.ownership_invalid",
            format!(
                "The {label} lock '{}' must be an owned regular file.",
                path.display()
            ),
        ));
    }
    Ok(())
}

async fn validate_real_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(artifact_store_error(
            "use.artifact_store.ownership_invalid",
            format!(
                "The {label} '{}' must be an owned directory.",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(super) fn validate_sha256(sha256: &str) -> UseResult<()> {
    if sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(artifact_store_error(
            "use.artifact_store.digest_invalid",
            "An Artifact Store digest must contain exactly 64 lowercase hexadecimal characters.",
        ))
    }
}

pub(super) fn artifact_store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_package_paths_are_typed_and_sharded() {
        let store = ArtifactStore::from_data_root(Path::new("/data/use"));
        let sha256 = "ab".repeat(32);
        assert_eq!(
            store
                .expanded_package_path(&format!("sha256:{sha256}"))
                .unwrap(),
            PathBuf::from(format!(
                "/data/use/artifacts/expanded-packages/sha256/ab/{sha256}/content"
            ))
        );
        assert_eq!(
            store.expanded_package_path(&sha256).unwrap_err().code,
            "use.artifact_store.digest_invalid"
        );
        assert_eq!(
            store
                .expanded_package_path(&format!("sha256:{}", "A".repeat(64)))
                .unwrap_err()
                .code,
            "use.artifact_store.digest_invalid"
        );
    }

    #[test]
    fn artifact_store_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ArtifactStore>();
    }
}
