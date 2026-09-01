use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use tokio::fs;

use crate::package::{io_error, lock_is_contended};

mod audit;
mod blob;
mod garbage_collection;
mod inventory;
mod package_read;
mod quarantine;
mod quota;
mod reachability;
mod rehydration;

pub use audit::{
    ArtifactDigestAuditEntry, ArtifactDigestAuditStatus, ArtifactStoreDigestAudit,
    ARTIFACT_STORE_DIGEST_AUDIT_SCHEMA,
};
pub(crate) use blob::ArtifactBlob;
pub use garbage_collection::{
    ArtifactGarbageCollectionEntry, ArtifactGarbageCollectionLifecycle,
    ArtifactGarbageCollectionPlan, ArtifactGarbageCollectionPolicy,
    ArtifactGarbageCollectionRecord, ArtifactGarbageCollectionResult,
    ArtifactGarbageCollectionTarget, ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA,
    ARTIFACT_GARBAGE_COLLECTION_RECORD_SCHEMA, ARTIFACT_GARBAGE_COLLECTION_RESULT_SCHEMA,
    MAX_ARTIFACT_GARBAGE_COLLECTION_TARGETS,
};
pub use inventory::{
    ArtifactInventoryEntry, ArtifactKind, ArtifactPhysicalState, ArtifactStoreInventory,
    ARTIFACT_STORE_INVENTORY_SCHEMA, MAX_ARTIFACT_STORE_INVENTORY_ENTRIES,
};
pub use package_read::VerifiedArtifactPackage;
pub use quarantine::{
    ArtifactQuarantinePlan, ArtifactQuarantineRecord, ArtifactQuarantineResult,
    ARTIFACT_QUARANTINE_PLAN_SCHEMA, ARTIFACT_QUARANTINE_RECORD_SCHEMA,
    ARTIFACT_QUARANTINE_RESULT_SCHEMA,
};
pub(crate) use quota::{ArtifactStorageAdmission, ArtifactStorageWrite};
pub use quota::{
    ArtifactStorageQuotaAction, ArtifactStorageQuotaMutation, ArtifactStorageQuotaPolicy,
    ArtifactStorageQuotaSnapshot, ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION,
    MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS,
};
pub use reachability::{ArtifactCollectionGuard, ArtifactReferenceAdmission};
pub use rehydration::{
    ArtifactRehydrationPlan, ArtifactRehydrationRecord, ArtifactRehydrationResult,
    ARTIFACT_REHYDRATION_PLAN_SCHEMA, ARTIFACT_REHYDRATION_RECORD_SCHEMA,
    ARTIFACT_REHYDRATION_RESULT_SCHEMA,
};

const BLOBS_DIRECTORY: &str = "blobs";
const EXPANDED_PACKAGES_DIRECTORY: &str = "expanded-packages";
const SHA256_DIRECTORY: &str = "sha256";
const CONTENT_DIRECTORY: &str = "content";
const MUTATION_LOCK: &str = ".mutation.lock";
const REACHABILITY_LOCK: &str = ".reachability.lock";
#[cfg(test)]
mod audit_tests;
#[cfg(all(test, any(unix, windows)))]
mod garbage_collection_recovery_tests;
#[cfg(test)]
mod garbage_collection_tests;
#[cfg(test)]
mod package_read_tests;
#[cfg(test)]
mod quarantine_tests;
#[cfg(test)]
mod quota_tests;
#[cfg(test)]
mod rehydration_tests;
pub(crate) const ARTIFACT_STAGING_PREFIX: &str = ".artifact-staging-";
pub(crate) const MAX_ARTIFACT_CONTAINER_ENTRIES: usize = 128;
pub(crate) const MAX_ARTIFACT_TREE_ENTRIES: usize = crate::package::MAX_PACKAGE_FILES * 2;
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
        self.ensure_container_not_quarantined(
            &self.expanded_package_container(sha256),
            ArtifactKind::ExpandedPackage,
            sha256,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn acquire_expanded_package_mutation(
        &self,
        admission: &ArtifactReferenceAdmission,
        storage: &ArtifactStorageAdmission,
        sha256: &str,
    ) -> UseResult<ArtifactMutationLock> {
        admission.ensure_store(self)?;
        storage.ensure_store(self)?;
        validate_sha256(sha256)?;
        let container = self.expanded_package_container(sha256);
        self.ensure_container(&container, "expanded-package artifact")
            .await?;
        self.ensure_container_not_quarantined(&container, ArtifactKind::ExpandedPackage, sha256)
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
    open_lock_file_with_create(path, label, true)
}

pub(super) fn open_existing_lock_file(path: &Path, label: &str) -> UseResult<File> {
    open_lock_file_with_create(path, label, false)
}

fn open_lock_file_with_create(path: &Path, label: &str, create: bool) -> UseResult<File> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        validate_lock_metadata(path, &metadata, label)?;
    }
    let mut options = OpenOptions::new();
    options
        .create(create)
        .truncate(false)
        .read(true)
        .write(true);
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
        assert_send_sync::<ArtifactReferenceAdmission>();
        assert_send_sync::<ArtifactCollectionGuard>();
        assert_send_sync::<ArtifactStoreInventory>();
        assert_send_sync::<ArtifactInventoryEntry>();
        assert_send_sync::<ArtifactGarbageCollectionPolicy>();
        assert_send_sync::<ArtifactGarbageCollectionPlan>();
    }

    #[tokio::test]
    async fn collection_excludes_reference_admission_until_the_guard_is_released() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
        let admission = store.acquire_reference_admission().await.unwrap();

        let error = store.acquire_collection().await.unwrap_err();
        assert_eq!(error.code, "use.artifact_store.busy");

        drop(admission);
        let _collection = store.acquire_collection().await.unwrap();
    }

    #[tokio::test]
    async fn reference_admissions_share_the_global_reachability_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));

        let first = store.acquire_reference_admission().await.unwrap();
        let second = store.acquire_reference_admission().await.unwrap();
        drop((first, second));

        let collection = store.acquire_collection().await.unwrap();
        let error = store.acquire_reference_admission().await.unwrap_err();
        assert_eq!(error.code, "use.artifact_store.busy");
        drop(collection);
    }

    #[tokio::test]
    async fn reference_admission_is_bound_to_one_artifact_store() {
        let temporary = tempfile::tempdir().unwrap();
        let first = ArtifactStore::from_data_root(&temporary.path().join("first"));
        let second = ArtifactStore::from_data_root(&temporary.path().join("second"));
        let admission = first.acquire_reference_admission().await.unwrap();

        let error = admission.ensure_store(&second).unwrap_err();
        assert_eq!(error.code, "use.artifact_store.admission_mismatch");
    }

    #[tokio::test]
    async fn physical_inventory_is_path_free_deterministic_and_kind_aware() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
        let blob_sha256 = "a".repeat(64);
        let package_sha256 = "b".repeat(64);
        let blob = store.blob_path(&format!("sha256:{blob_sha256}")).unwrap();
        let package = store
            .expanded_package_path(&format!("sha256:{package_sha256}"))
            .unwrap();
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::write(&blob, b"blob").unwrap();
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("a3s-use-extension.acl"), b"extension").unwrap();
        std::fs::create_dir_all(package.join("nested")).unwrap();
        std::fs::write(package.join("nested/README.md"), b"readme").unwrap();
        let collection = store.acquire_collection().await.unwrap();

        let inventory = store.inspect_inventory(&collection).await.unwrap();

        assert_eq!(inventory.schema, ARTIFACT_STORE_INVENTORY_SCHEMA);
        assert_eq!(inventory.entries.len(), 2);
        assert_eq!(inventory.entries[0].kind, ArtifactKind::Blob);
        assert_eq!(inventory.entries[0].digest, format!("sha256:{blob_sha256}"));
        assert_eq!(inventory.entries[0].state, ArtifactPhysicalState::Complete);
        assert_eq!(inventory.entries[0].content_bytes, 4);
        assert_eq!(inventory.entries[0].content_files, 1);
        assert_eq!(inventory.entries[1].kind, ArtifactKind::ExpandedPackage);
        assert_eq!(
            inventory.entries[1].digest,
            format!("sha256:{package_sha256}")
        );
        assert_eq!(inventory.entries[1].state, ArtifactPhysicalState::Complete);
        assert_eq!(inventory.entries[1].content_bytes, 15);
        assert_eq!(inventory.entries[1].content_files, 2);
        let json = serde_json::to_string(&inventory).unwrap();
        let temporary_path = temporary.path().to_string_lossy();
        assert!(!json.contains(temporary_path.as_ref() as &str));
    }

    #[tokio::test]
    async fn physical_inventory_reports_bounded_abandoned_staging_without_promoting_it() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
        let sha256 = "c".repeat(64);
        let content = store.blob_path(&format!("sha256:{sha256}")).unwrap();
        let container = content.parent().unwrap();
        std::fs::create_dir_all(container).unwrap();
        std::fs::write(container.join(".artifact-staging-test.tmp"), b"partial").unwrap();
        let collection = store.acquire_collection().await.unwrap();

        let inventory = store.inspect_inventory(&collection).await.unwrap();

        assert_eq!(inventory.entries.len(), 1);
        assert_eq!(
            inventory.entries[0].state,
            ArtifactPhysicalState::Incomplete
        );
        assert_eq!(inventory.entries[0].content_bytes, 0);
        assert_eq!(inventory.entries[0].content_files, 0);
        assert_eq!(inventory.entries[0].staging_entries, 1);
        assert_eq!(inventory.entries[0].staging_bytes, 7);
    }

    #[tokio::test]
    async fn physical_inventory_requires_the_exact_collection_store() {
        let temporary = tempfile::tempdir().unwrap();
        let first = ArtifactStore::from_data_root(&temporary.path().join("first"));
        let second = ArtifactStore::from_data_root(&temporary.path().join("second"));
        let collection = first.acquire_collection().await.unwrap();

        let error = second.inspect_inventory(&collection).await.unwrap_err();

        assert_eq!(error.code, "use.artifact_store.collection_mismatch");
    }

    #[tokio::test]
    async fn physical_inventory_rejects_unowned_container_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
        let sha256 = "d".repeat(64);
        let content = store.blob_path(&format!("sha256:{sha256}")).unwrap();
        let container = content.parent().unwrap();
        std::fs::create_dir_all(container).unwrap();
        std::fs::write(container.join("unexpected"), b"unowned").unwrap();
        let collection = store.acquire_collection().await.unwrap();

        let error = store.inspect_inventory(&collection).await.unwrap_err();

        assert_eq!(error.code, "use.artifact_store.ownership_invalid");
    }

    #[tokio::test]
    async fn physical_inventory_rejects_an_unbounded_digest_container() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
        let sha256 = "f".repeat(64);
        let content = store.blob_path(&format!("sha256:{sha256}")).unwrap();
        let container = content.parent().unwrap();
        std::fs::create_dir_all(container).unwrap();
        for index in 0..=MAX_ARTIFACT_CONTAINER_ENTRIES {
            std::fs::write(
                container.join(format!("{ARTIFACT_STAGING_PREFIX}{index}.tmp")),
                b"partial",
            )
            .unwrap();
        }
        let collection = store.acquire_collection().await.unwrap();

        let error = store.inspect_inventory(&collection).await.unwrap_err();

        assert_eq!(error.code, "use.artifact_store.inventory_limit_exceeded");
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn physical_inventory_rejects_links_in_expanded_content() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
        let sha256 = "e".repeat(64);
        let content = store
            .expanded_package_path(&format!("sha256:{sha256}"))
            .unwrap();
        let external = temporary.path().join("external");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("payload"), b"outside").unwrap();
        crate::test_filesystem::create_directory_link(&external, &content.join("linked"));
        let collection = store.acquire_collection().await.unwrap();

        let error = store.inspect_inventory(&collection).await.unwrap_err();

        assert_eq!(error.code, "use.artifact_store.ownership_invalid");
    }
}
