use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;

use super::super::{artifact_store_error, open_lock_file, ArtifactStore};
use crate::package::{io_error, lock_is_contended};

pub(in crate::artifact_store) const STORAGE_QUOTA_LOCK: &str = ".storage-quota.lock";

const STORAGE_QUOTA_LOCK_WAIT: Duration = Duration::from_secs(30);
const STORAGE_QUOTA_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub(super) struct StorageQuotaLock {
    file: File,
    root: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum StorageQuotaLockMode {
    Shared,
    Exclusive,
}

impl StorageQuotaLock {
    pub(super) fn ensure_store(&self, store: &ArtifactStore) -> UseResult<()> {
        if self.root != store.root() {
            return Err(artifact_store_error(
                "use.artifact_store.quota_admission_mismatch",
                "An artifact storage admission belongs to a different global store.",
            ));
        }
        Ok(())
    }
}

impl Drop for StorageQuotaLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) async fn acquire_storage_quota_lock(
    store: &ArtifactStore,
    mode: StorageQuotaLockMode,
) -> UseResult<StorageQuotaLock> {
    store
        .ensure_container(store.root(), "storage quota")
        .await?;
    let path = store.root().join(STORAGE_QUOTA_LOCK);
    let file = open_lock_file(&path, "Artifact Store quota")?;
    let deadline = tokio::time::Instant::now() + STORAGE_QUOTA_LOCK_WAIT;
    loop {
        let acquired = match mode {
            StorageQuotaLockMode::Shared => FileExt::try_lock_shared(&file),
            StorageQuotaLockMode::Exclusive => FileExt::try_lock_exclusive(&file),
        };
        match acquired {
            Ok(()) => {
                return Ok(StorageQuotaLock {
                    file,
                    root: store.root().to_path_buf(),
                })
            }
            Err(error) if lock_is_contended(&error) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(UseError::new(
                        "use.artifact_store.quota_busy",
                        "Another process owns the global Artifact Store quota boundary.",
                    )
                    .with_suggestion(
                        "Retry after the active Artifact Store publication completes.",
                    ));
                }
                tokio::time::sleep(
                    STORAGE_QUOTA_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(error) => return Err(io_error("acquire Artifact Store quota lock", &path, error)),
        }
    }
}
