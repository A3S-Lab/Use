use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::package::{io_error, remove_file_with_windows_retry, sync_parent_directory};

use super::{
    acquire_target_cache_lock, ensure_cache_directory, record, secure_file, target_cache_error,
    validated_evidence, verify_open_file, TargetCacheLock,
};
use crate::artifact_store::ArtifactBlob;
use crate::remote::target_cache_inventory::admit_target_write;
use crate::remote::VerifiedTargetCachePolicy;
use crate::ArtifactStore;

const PARTIAL_PREFIX: &str = ".target-";
const PARTIAL_SUFFIX: &str = ".part";

#[cfg(test)]
type BeforeBlobCommitHook = Box<dyn FnOnce(&Path) + Send>;

#[cfg(test)]
static BEFORE_BLOB_COMMIT_HOOKS: std::sync::Mutex<Vec<(PathBuf, BeforeBlobCommitHook)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(all(test, unix))]
fn install_before_blob_commit_hook(path: PathBuf, hook: BeforeBlobCommitHook) {
    BEFORE_BLOB_COMMIT_HOOKS.lock().unwrap().push((path, hook));
}

#[cfg(test)]
fn run_before_blob_commit_hook(path: &Path) {
    let hook = {
        let mut hooks = BEFORE_BLOB_COMMIT_HOOKS.lock().unwrap();
        hooks
            .iter()
            .position(|(hook_path, _)| hook_path == path)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook(path);
    }
}

#[cfg(not(test))]
fn run_before_blob_commit_hook(_path: &Path) {}

/// One exclusive, digest-bound target-cache download transaction.
///
/// The partial path is deterministic so a later process can resume it. The
/// cache lock prevents GC or another installer from changing the same cache
/// while bytes are admitted, appended, verified, globally committed, and staged.
pub(in crate::remote) struct ResumableTarget {
    _lock: TargetCacheLock,
    artifact_store: ArtifactStore,
    cache_directory: PathBuf,
    partial_path: PathBuf,
    expected_length: u64,
    expected_sha256: String,
    offset: u64,
    partial: Option<fs::File>,
    verified: Option<ArtifactBlob>,
    ready: bool,
}

impl ResumableTarget {
    pub(in crate::remote) async fn begin(
        datastore: &Path,
        artifact_store: &ArtifactStore,
        expected_length: u64,
        expected_sha256: &str,
        policy: VerifiedTargetCachePolicy,
    ) -> UseResult<Self> {
        let expected_sha256 = validated_evidence(expected_length, expected_sha256)?;
        let artifact_admission = artifact_store.acquire_reference_admission().await?;
        let lock = acquire_target_cache_lock(datastore, true)?;
        let cache_directory = ensure_cache_directory(datastore).await?;
        let partial_path = cache_directory.join(partial_name(&expected_sha256));
        let observation =
            record::read_observation(&cache_directory, &expected_sha256, expected_length).await?;
        let global_blob = artifact_store
            .open_blob(&artifact_admission, &expected_sha256, expected_length)
            .await?;
        if observation.is_some() && global_blob.is_none() {
            return Err(target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "A Registry target observation references a missing global artifact blob.",
            ));
        }
        if let Some(verified) = global_blob {
            admit_target_write(
                &cache_directory,
                &expected_sha256,
                expected_length,
                policy,
                false,
            )
            .await?;
            record::write_observation(&cache_directory, &expected_sha256, expected_length).await?;
            if let Some(metadata) = optional_metadata(&partial_path).await? {
                validate_partial_metadata(&partial_path, &metadata, expected_length)?;
                remove_partial(&partial_path, &cache_directory).await?;
            }
            return Ok(Self {
                _lock: lock,
                artifact_store: artifact_store.clone(),
                cache_directory,
                partial_path,
                expected_length,
                expected_sha256,
                offset: expected_length,
                partial: None,
                verified: Some(verified),
                ready: true,
            });
        }

        let mut partial = open_existing_partial(&partial_path).await?;
        let mut existing_length = match partial.as_ref() {
            Some(partial) => {
                let metadata = partial.metadata().await.map_err(|error| {
                    io_error("inspect opened resumable target", &partial_path, error)
                })?;
                validate_partial_metadata(&partial_path, &metadata, expected_length)?;
                metadata.len()
            }
            None => 0,
        };
        admit_target_write(
            &cache_directory,
            &expected_sha256,
            expected_length,
            policy,
            true,
        )
        .await?;

        if existing_length == expected_length && existing_length > 0 {
            let valid = match partial.as_mut() {
                Some(partial) => verify_open_file(
                    partial,
                    &partial_path,
                    expected_length,
                    &expected_sha256,
                    "use.extension.registry_target_invalid",
                )
                .await
                .is_ok(),
                None => false,
            };
            if valid {
                let mut partial = partial.take().ok_or_else(|| {
                    target_cache_error(
                        "use.extension.registry_target_cache_invalid",
                        "The verified resumable target handle is unavailable.",
                    )
                })?;
                run_before_blob_commit_hook(&partial_path);
                let verified = artifact_store
                    .commit_blob(
                        &artifact_admission,
                        &mut partial,
                        expected_length,
                        &expected_sha256,
                    )
                    .await?;
                record::write_observation(&cache_directory, &expected_sha256, expected_length)
                    .await?;
                drop(partial);
                remove_partial(&partial_path, &cache_directory).await?;
                return Ok(Self {
                    _lock: lock,
                    artifact_store: artifact_store.clone(),
                    cache_directory,
                    partial_path,
                    expected_length,
                    expected_sha256,
                    offset: expected_length,
                    partial: None,
                    verified: Some(verified),
                    ready: true,
                });
            }
            drop(partial.take());
            remove_partial(&partial_path, &cache_directory).await?;
            admit_target_write(
                &cache_directory,
                &expected_sha256,
                expected_length,
                policy,
                true,
            )
            .await?;
            existing_length = 0;
        }

        let mut partial = match partial {
            Some(partial) => partial,
            None => create_partial(&partial_path).await?,
        };
        let opened = partial
            .metadata()
            .await
            .map_err(|error| io_error("inspect opened resumable target", &partial_path, error))?;
        validate_partial_metadata(&partial_path, &opened, expected_length)?;
        if opened.len() != existing_length {
            return Err(target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "The resumable Registry target changed while it was opened.",
            ));
        }
        partial
            .seek(SeekFrom::End(0))
            .await
            .map_err(|error| io_error("seek resumable Registry target", &partial_path, error))?;
        secure_file(&partial, &partial_path).await?;

        Ok(Self {
            _lock: lock,
            artifact_store: artifact_store.clone(),
            cache_directory,
            partial_path,
            expected_length,
            expected_sha256,
            offset: existing_length,
            partial: Some(partial),
            verified: None,
            ready: false,
        })
    }

    pub(in crate::remote) const fn is_ready(&self) -> bool {
        self.ready
    }

    pub(in crate::remote) const fn offset(&self) -> u64 {
        self.offset
    }

    pub(in crate::remote) const fn expected_length(&self) -> u64 {
        self.expected_length
    }

    pub(in crate::remote) async fn reset(&mut self) -> UseResult<()> {
        let path = self.partial_path.clone();
        let partial = self.partial_mut()?;
        partial
            .set_len(0)
            .await
            .map_err(|error| io_error("truncate resumable Registry target", &path, error))?;
        partial
            .seek(SeekFrom::Start(0))
            .await
            .map_err(|error| io_error("seek resumable Registry target", &path, error))?;
        partial
            .sync_all()
            .await
            .map_err(|error| io_error("sync resumable Registry target", &path, error))?;
        self.offset = 0;
        Ok(())
    }

    pub(in crate::remote) async fn append(&mut self, bytes: &[u8]) -> UseResult<()> {
        let next = self.offset.checked_add(bytes.len() as u64).ok_or_else(|| {
            target_cache_error(
                "use.extension.registry_target_invalid",
                "The resumable Registry target length overflowed.",
            )
        })?;
        if next > self.expected_length {
            return Err(target_cache_error(
                "use.extension.registry_target_invalid",
                "The resumable Registry target exceeds its signed length.",
            ));
        }
        let path = self.partial_path.clone();
        let partial = self.partial_mut()?;
        partial
            .write_all(bytes)
            .await
            .map_err(|error| io_error("append resumable Registry target", &path, error))?;
        self.offset = next;
        Ok(())
    }

    pub(in crate::remote) async fn checkpoint(&mut self) -> UseResult<()> {
        let path = self.partial_path.clone();
        let partial = self.partial_mut()?;
        partial
            .sync_data()
            .await
            .map_err(|error| io_error("checkpoint resumable Registry target", &path, error))
    }

    pub(in crate::remote) async fn commit(&mut self, error_code: &'static str) -> UseResult<()> {
        if self.ready {
            return Ok(());
        }
        if self.offset != self.expected_length {
            return Err(target_cache_error(
                error_code,
                "The resumable Registry target ended before its signed length.",
            ));
        }
        if let Some(partial) = self.partial.as_mut() {
            partial.sync_all().await.map_err(|error| {
                io_error("sync resumable Registry target", &self.partial_path, error)
            })?;
        }
        let mut partial = self.partial.take().ok_or_else(|| {
            target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "The resumable Registry target is not writable.",
            )
        })?;
        if let Err(error) = verify_open_file(
            &mut partial,
            &self.partial_path,
            self.expected_length,
            &self.expected_sha256,
            error_code,
        )
        .await
        {
            drop(partial);
            remove_partial(&self.partial_path, &self.cache_directory).await?;
            return Err(error);
        }
        run_before_blob_commit_hook(&self.partial_path);
        let artifact_admission = match self.artifact_store.acquire_reference_admission().await {
            Ok(admission) => admission,
            Err(error) => {
                self.partial = Some(partial);
                return Err(error);
            }
        };
        let verified = match self
            .artifact_store
            .commit_blob(
                &artifact_admission,
                &mut partial,
                self.expected_length,
                &self.expected_sha256,
            )
            .await
        {
            Ok(verified) => verified,
            Err(error) => {
                self.partial = Some(partial);
                return Err(error);
            }
        };
        if let Err(error) = record::write_observation(
            &self.cache_directory,
            &self.expected_sha256,
            self.expected_length,
        )
        .await
        {
            self.partial = Some(partial);
            return Err(error);
        }
        drop(partial);
        self.verified = Some(verified);
        self.ready = true;
        remove_partial(&self.partial_path, &self.cache_directory).await?;
        Ok(())
    }

    pub(in crate::remote) async fn discard(&mut self) -> UseResult<()> {
        drop(self.partial.take());
        if optional_metadata(&self.partial_path).await?.is_some() {
            remove_partial(&self.partial_path, &self.cache_directory).await?;
        }
        self.offset = 0;
        Ok(())
    }

    pub(in crate::remote) async fn stage_into(&mut self, output: &Path) -> UseResult<()> {
        if !self.ready {
            return Err(target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "An unverified resumable target cannot be staged.",
            ));
        }
        let verified = self.verified.as_mut().ok_or_else(|| {
            target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "The verified resumable target handle is unavailable.",
            )
        })?;
        verified.stage_into(output).await
    }

    fn partial_mut(&mut self) -> UseResult<&mut fs::File> {
        self.partial.as_mut().ok_or_else(|| {
            target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "The resumable Registry target is not writable.",
            )
        })
    }
}

fn partial_name(digest: &str) -> String {
    format!("{PARTIAL_PREFIX}{digest}{PARTIAL_SUFFIX}")
}

async fn optional_metadata(path: &Path) -> UseResult<Option<std::fs::Metadata>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("inspect Registry target cache entry", path, error)),
    }
}

async fn open_existing_partial(path: &Path) -> UseResult<Option<fs::File>> {
    let options = partial_open_options();
    match options.open(path).await {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            if let Ok(metadata) = fs::symlink_metadata(path).await {
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file()
                {
                    return Err(target_cache_error(
                        "use.extension.registry_target_cache_invalid",
                        "The resumable Registry target is not a bounded regular file.",
                    )
                    .with_detail("path", path.display().to_string()));
                }
            }
            Err(io_error("open resumable Registry target", path, error))
        }
    }
}

async fn create_partial(path: &Path) -> UseResult<fs::File> {
    let mut options = partial_open_options();
    options.create_new(true);
    options
        .open(path)
        .await
        .map_err(|error| io_error("create resumable Registry target", path, error))
}

fn partial_open_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).truncate(false);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        // Open the final path itself and keep external writers or replacers out
        // while this download transaction owns the partial. Readers such as
        // scanners and diagnostics remain permitted.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    options
}

fn validate_partial_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    expected_length: u64,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() > expected_length
    {
        return Err(target_cache_error(
            "use.extension.registry_target_cache_invalid",
            "The resumable Registry target is not a bounded regular file.",
        )
        .with_detail("path", path.display().to_string())
        .with_detail("length", metadata.len().to_string())
        .with_detail("expectedLength", expected_length.to_string()));
    }
    Ok(())
}

async fn remove_partial(path: &Path, cache_directory: &Path) -> UseResult<()> {
    remove_file_with_windows_retry(path.to_path_buf(), "remove resumable Registry target").await?;
    sync_parent_directory(cache_directory, "verified target cache").await
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    async fn begin_target(
        datastore: &Path,
        expected_length: u64,
        digest: &str,
        policy: VerifiedTargetCachePolicy,
    ) -> UseResult<ResumableTarget> {
        let artifact_store = test_artifact_store(datastore);
        ResumableTarget::begin(datastore, &artifact_store, expected_length, digest, policy).await
    }

    fn test_artifact_store(datastore: &Path) -> ArtifactStore {
        ArtifactStore::from_data_root(&datastore.join("global-data"))
    }

    fn test_blob_path(datastore: &Path, digest: &str) -> PathBuf {
        test_artifact_store(datastore)
            .blob_path(&format!("sha256:{digest}"))
            .unwrap()
    }

    #[tokio::test]
    async fn global_collection_blocks_new_target_reference_publication() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let artifact_store = test_artifact_store(datastore);
        let collection = artifact_store.acquire_collection().await.unwrap();
        let digest = "a".repeat(64);
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();
        let mut publication = Box::pin(ResumableTarget::begin(
            datastore,
            &artifact_store,
            1,
            &digest,
            policy,
        ));

        tokio::select! {
            _ = &mut publication => {
                panic!("target publication crossed the active collection boundary")
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        }
        assert!(!datastore.join("verified-targets").exists());

        drop(collection);
        let target = publication.await.unwrap();
        assert!(!target.is_ready());
    }

    #[tokio::test]
    async fn incomplete_download_releases_admission_until_atomic_blob_commit() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let artifact_store = test_artifact_store(datastore);
        let body = b"bounded admission";
        let digest = format!("{:x}", Sha256::digest(body));
        let mut target = ResumableTarget::begin(
            datastore,
            &artifact_store,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();
        target.append(body).await.unwrap();

        let collection = artifact_store.acquire_collection().await.unwrap();
        let mut publication = Box::pin(target.commit("use.extension.registry_target_invalid"));
        tokio::select! {
            result = &mut publication => {
                panic!("blob commit crossed the active collection boundary: {result:?}")
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        }

        drop(collection);
        publication.await.unwrap();
        assert!(target.is_ready());
        assert!(
            record::observation_path(&datastore.join("verified-targets/sha256"), &digest,)
                .is_file()
        );
    }

    #[tokio::test]
    async fn complete_partial_is_committed_without_a_request() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        std::fs::create_dir_all(&cache).unwrap();
        let body = b"complete signed target";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        std::fs::write(&partial, body).unwrap();
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

        let target = begin_target(datastore, body.len() as u64, &digest, policy)
            .await
            .unwrap();

        assert!(target.is_ready());
        assert!(!partial.exists());
        assert_eq!(
            std::fs::read(test_blob_path(datastore, &digest)).unwrap(),
            body
        );
        assert!(record::observation_path(&cache, &digest).is_file());
    }

    #[tokio::test]
    async fn partial_larger_than_signed_length_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        std::fs::create_dir_all(&cache).unwrap();
        let digest = "a".repeat(64);
        std::fs::write(cache.join(partial_name(&digest)), b"too long").unwrap();
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

        let error = begin_target(datastore, 2, &digest, policy)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verified_partial_handle_commits_original_bytes_after_path_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        let body = b"trusted!";
        let replacement = b"attacker";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();
        let mut target = begin_target(datastore, body.len() as u64, &digest, policy)
            .await
            .unwrap();
        target.append(body).await.unwrap();
        install_before_blob_commit_hook(
            partial.clone(),
            Box::new(move |path| {
                std::fs::remove_file(path).unwrap();
                std::fs::write(path, replacement).unwrap();
            }),
        );

        target
            .commit("use.extension.registry_target_invalid")
            .await
            .unwrap();

        assert!(target.is_ready());
        assert!(!partial.exists());
        assert_eq!(
            std::fs::read(test_blob_path(datastore, &digest)).unwrap(),
            body
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verified_handle_stages_original_after_target_path_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let body = b"trusted!";
        let replacement = b"attacker";
        let digest = format!("{:x}", Sha256::digest(body));
        let target_path = test_blob_path(datastore, &digest);
        let mut target = begin_target(
            datastore,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();
        target.append(body).await.unwrap();
        target
            .commit("use.extension.registry_target_invalid")
            .await
            .unwrap();

        std::fs::remove_file(&target_path).unwrap();
        std::fs::write(&target_path, replacement).unwrap();
        let staged = temporary.path().join("staged");
        target.stage_into(&staged).await.unwrap();

        assert_eq!(std::fs::read(staged).unwrap(), body);
        assert_eq!(std::fs::read(target_path).unwrap(), replacement);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn active_global_blob_allows_read_but_denies_external_write_and_removal() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let body = b"trusted!";
        let digest = format!("{:x}", Sha256::digest(body));
        let target_path = test_blob_path(datastore, &digest);
        let mut target = begin_target(
            datastore,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();
        target.append(body).await.unwrap();
        target
            .commit("use.extension.registry_target_invalid")
            .await
            .unwrap();

        assert_eq!(std::fs::read(&target_path).unwrap(), body);
        let write_error = std::fs::OpenOptions::new()
            .write(true)
            .open(&target_path)
            .unwrap_err();
        assert!(
            matches!(write_error.raw_os_error(), Some(5 | 32 | 33)),
            "active global blob accepted an external writer: {write_error}"
        );
        let remove_error = std::fs::remove_file(&target_path).unwrap_err();
        assert!(
            matches!(remove_error.raw_os_error(), Some(5 | 32 | 33)),
            "active global blob accepted external removal: {remove_error}"
        );
        let staged = temporary.path().join("staged");
        target.stage_into(&staged).await.unwrap();
        assert_eq!(std::fs::read(staged).unwrap(), body);

        drop(target);
        std::fs::write(&target_path, b"released").unwrap();
        assert_eq!(std::fs::read(target_path).unwrap(), b"released");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn active_partial_allows_read_but_denies_external_write_and_removal() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        std::fs::create_dir_all(&cache).unwrap();
        let digest = "c".repeat(64);
        let partial = cache.join(partial_name(&digest));
        std::fs::write(&partial, b"retained").unwrap();
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

        let target = begin_target(datastore, 16, &digest, policy).await.unwrap();
        assert_eq!(target.offset(), 8);

        let write_error = std::fs::OpenOptions::new()
            .write(true)
            .open(&partial)
            .unwrap_err();
        assert!(
            matches!(write_error.raw_os_error(), Some(5 | 32 | 33)),
            "active partial accepted an external writer: {write_error}"
        );
        let remove_error = std::fs::remove_file(&partial).unwrap_err();
        assert!(
            matches!(remove_error.raw_os_error(), Some(5 | 32 | 33)),
            "active partial accepted external removal: {remove_error}"
        );
        assert_eq!(std::fs::read(&partial).unwrap(), b"retained");

        drop(target);
        std::fs::write(&partial, b"released").unwrap();
        assert_eq!(std::fs::read(&partial).unwrap(), b"released");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn transient_scanner_lock_releases_into_blob_commit_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        let body = b"scanner retained target";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        let target_path = test_blob_path(datastore, &digest);
        let mut target = begin_target(
            datastore,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();
        target.append(body).await.unwrap();
        let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&partial);
        let mut commit = Box::pin(target.commit("use.extension.registry_target_invalid"));

        tokio::select! {
            result = &mut commit => {
                panic!("commit completed while the scanner denied delete sharing: {result:?}")
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }

        drop(scanner);
        commit.await.unwrap();
        assert!(target.is_ready());
        assert!(!partial.exists());
        assert_eq!(std::fs::read(&target_path).unwrap(), body);

        let staged = temporary.path().join("staged");
        target.stage_into(&staged).await.unwrap();
        assert_eq!(std::fs::read(staged).unwrap(), body);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn persistent_scanner_lock_preserves_complete_partial_for_offline_retry() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        let body = b"scanner retained target";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        let target_path = test_blob_path(datastore, &digest);
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();
        let mut target = begin_target(datastore, body.len() as u64, &digest, policy)
            .await
            .unwrap();
        target.append(body).await.unwrap();
        let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&partial);

        let started = std::time::Instant::now();
        let error = target
            .commit("use.extension.registry_target_invalid")
            .await
            .expect_err("a persistent scanner lock must stop at the retry bound");
        let elapsed = started.elapsed();

        assert_eq!(error.code, "use.extension.io");
        assert!(elapsed >= std::time::Duration::from_secs(2));
        assert!(elapsed < std::time::Duration::from_secs(10));
        assert!(target.is_ready());
        assert_eq!(std::fs::read(&partial).unwrap(), body);
        assert_eq!(std::fs::read(&target_path).unwrap(), body);

        drop(scanner);
        drop(target);
        let mut recovered = begin_target(
            datastore,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();

        assert!(recovered.is_ready());
        assert_eq!(recovered.offset(), body.len() as u64);
        assert!(!partial.exists());
        assert_eq!(std::fs::read(&target_path).unwrap(), body);
        let staged = temporary.path().join("recovered-staging");
        recovered.stage_into(&staged).await.unwrap();
        assert_eq!(std::fs::read(staged).unwrap(), body);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn invalid_partial_cleanup_waits_for_a_transient_scanner_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        let expected = b"trusted";
        let corrupt = b"corrupt";
        let digest = format!("{:x}", Sha256::digest(expected));
        let partial = cache.join(partial_name(&digest));
        let target_path = test_blob_path(datastore, &digest);
        let mut target = begin_target(
            datastore,
            expected.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();
        target.append(corrupt).await.unwrap();
        let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&partial);
        let mut commit = Box::pin(target.commit("use.extension.registry_target_invalid"));

        tokio::select! {
            result = &mut commit => {
                panic!("invalid-partial cleanup completed while the scanner denied delete sharing: {result:?}")
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }

        drop(scanner);
        let error = commit
            .await
            .expect_err("the corrupt partial must remain untrusted");
        assert_eq!(error.code, "use.extension.registry_target_invalid");
        assert!(!target.is_ready());
        assert!(!partial.exists());
        assert!(!target_path.exists());
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn linked_partial_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        std::fs::create_dir_all(&cache).unwrap();
        let digest = "b".repeat(64);
        let outside = temporary.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), b"x").unwrap();
        crate::test_filesystem::create_directory_link(&outside, &cache.join(partial_name(&digest)));
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

        let error = begin_target(datastore, 2, &digest, policy)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
        assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"x");
    }
}
