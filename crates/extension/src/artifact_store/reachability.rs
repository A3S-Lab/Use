use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use a3s_use_core::UseResult;
use fs2::FileExt;

use super::{artifact_store_error, open_lock_file, ArtifactStore};
use crate::package::{io_error, lock_is_contended};

const REACHABILITY_LOCK: &str = ".reachability.lock";
const REACHABILITY_LOCK_WAIT: Duration = Duration::from_secs(2);
const REACHABILITY_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Shared cross-process guard for publishing a new durable artifact reference.
///
/// Acquire this guard before any Registry-source, installation, or operation
/// lock. A collector takes the exclusive counterpart, so it can derive one
/// stable reachability view without racing a newly admitted reference.
#[derive(Debug)]
#[must_use = "dropping the admission allows artifact collection to resume"]
pub struct ArtifactReferenceAdmission {
    file: File,
    root: PathBuf,
}

/// Exclusive cross-process guard for global Artifact Store maintenance.
///
/// Holding this guard freezes new durable references. Reference retirement may
/// continue because it can only make a collection plan more conservative.
#[derive(Debug)]
#[must_use = "dropping the guard allows durable artifact references to resume"]
pub struct ArtifactCollectionGuard {
    file: File,
}

#[derive(Debug, Clone, Copy)]
enum ReachabilityLockMode {
    Shared,
    Exclusive,
}

impl ArtifactStore {
    /// Enter the global boundary for publishing a durable artifact reference.
    pub async fn acquire_reference_admission(&self) -> UseResult<ArtifactReferenceAdmission> {
        self.ensure_container(self.root(), "global artifact reachability")
            .await?;
        acquire_reachability_lock(self, ReachabilityLockMode::Shared)
            .await
            .map(|file| ArtifactReferenceAdmission {
                file,
                root: self.root().to_path_buf(),
            })
    }

    /// Freeze new durable references while deriving or applying maintenance.
    pub async fn acquire_collection(&self) -> UseResult<ArtifactCollectionGuard> {
        self.ensure_container(self.root(), "global artifact reachability")
            .await?;
        acquire_reachability_lock(self, ReachabilityLockMode::Exclusive)
            .await
            .map(|file| ArtifactCollectionGuard { file })
    }
}

impl ArtifactReferenceAdmission {
    pub(crate) fn ensure_store(&self, store: &ArtifactStore) -> UseResult<()> {
        if self.root != store.root() {
            return Err(artifact_store_error(
                "use.artifact_store.admission_mismatch",
                "An artifact reference admission belongs to a different global store.",
            ));
        }
        Ok(())
    }
}

impl Drop for ArtifactReferenceAdmission {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl Drop for ArtifactCollectionGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

async fn acquire_reachability_lock(
    store: &ArtifactStore,
    mode: ReachabilityLockMode,
) -> UseResult<File> {
    let path = store.root().join(REACHABILITY_LOCK);
    let file = open_lock_file(&path, "global artifact reachability")?;
    let deadline = tokio::time::Instant::now() + REACHABILITY_LOCK_WAIT;
    loop {
        let acquired = match mode {
            ReachabilityLockMode::Shared => FileExt::try_lock_shared(&file),
            ReachabilityLockMode::Exclusive => FileExt::try_lock_exclusive(&file),
        };
        match acquired {
            Ok(()) => return Ok(file),
            Err(error) if lock_is_contended(&error) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(artifact_store_error(
                        "use.artifact_store.busy",
                        "Another process owns the global artifact reachability boundary.",
                    ));
                }
                tokio::time::sleep(
                    REACHABILITY_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(error) => {
                return Err(io_error(
                    "acquire global artifact reachability lock",
                    &path,
                    error,
                ));
            }
        }
    }
}
