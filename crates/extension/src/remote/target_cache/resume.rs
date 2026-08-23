use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::package::{activate_temporary_file, io_error, sync_parent_directory};

use super::{
    acquire_target_cache_lock, ensure_cache_directory, open_verified_file, secure_file,
    target_cache_error, validate_regular_metadata, validated_evidence, verify_open_file,
    TargetCacheLock,
};
use crate::remote::target_cache_inventory::admit_target_write;
use crate::remote::VerifiedTargetCachePolicy;

const PARTIAL_PREFIX: &str = ".target-";
const PARTIAL_SUFFIX: &str = ".part";

#[cfg(test)]
type BeforePromotionHook = Box<dyn FnOnce(&Path) + Send>;

#[cfg(test)]
static BEFORE_PROMOTION_HOOKS: std::sync::Mutex<Vec<(PathBuf, BeforePromotionHook)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn install_before_promotion_hook(path: PathBuf, hook: BeforePromotionHook) {
    BEFORE_PROMOTION_HOOKS.lock().unwrap().push((path, hook));
}

#[cfg(test)]
fn run_before_promotion_hook(path: &Path) {
    let hook = {
        let mut hooks = BEFORE_PROMOTION_HOOKS.lock().unwrap();
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
fn run_before_promotion_hook(_path: &Path) {}

/// One exclusive, digest-bound target-cache download transaction.
///
/// The partial path is deterministic so a later process can resume it. The
/// cache lock prevents GC or another installer from changing the same cache
/// while bytes are admitted, appended, verified, promoted, and staged.
pub(in crate::remote) struct ResumableTarget {
    _lock: TargetCacheLock,
    cache_directory: PathBuf,
    target_path: PathBuf,
    partial_path: PathBuf,
    expected_length: u64,
    expected_sha256: String,
    offset: u64,
    partial: Option<fs::File>,
    verified: Option<fs::File>,
    ready: bool,
}

impl ResumableTarget {
    pub(in crate::remote) async fn begin(
        datastore: &Path,
        expected_length: u64,
        expected_sha256: &str,
        policy: VerifiedTargetCachePolicy,
    ) -> UseResult<Self> {
        let expected_sha256 = validated_evidence(expected_length, expected_sha256)?;
        let lock = acquire_target_cache_lock(datastore, true)?;
        let cache_directory = ensure_cache_directory(datastore).await?;
        let target_path = cache_directory.join(&expected_sha256);
        let partial_path = cache_directory.join(partial_name(&expected_sha256));

        if let Some(metadata) = optional_metadata(&target_path).await? {
            validate_regular_metadata(
                &target_path,
                &metadata,
                expected_length,
                "use.extension.registry_target_cache_invalid",
                "The verified target cache entry is not a bounded regular file.",
            )?;
            let mut verified = open_verified_file(
                &target_path,
                expected_length,
                "use.extension.registry_target_cache_invalid",
            )
            .await?;
            verify_open_file(
                &mut verified,
                None,
                &target_path,
                expected_length,
                &expected_sha256,
                "use.extension.registry_target_cache_invalid",
            )
            .await?;
            admit_target_write(&cache_directory, &expected_sha256, expected_length, policy).await?;
            return Ok(Self {
                _lock: lock,
                cache_directory,
                target_path,
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
        admit_target_write(&cache_directory, &expected_sha256, expected_length, policy).await?;

        if existing_length == expected_length && existing_length > 0 {
            let valid = match partial.as_mut() {
                Some(partial) => verify_open_file(
                    partial,
                    None,
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
                let partial = partial.take().ok_or_else(|| {
                    target_cache_error(
                        "use.extension.registry_target_cache_invalid",
                        "The verified resumable target handle is unavailable.",
                    )
                })?;
                let verified = promote_verified_partial(
                    partial,
                    partial_path.clone(),
                    target_path.clone(),
                    &cache_directory,
                    expected_length,
                    &expected_sha256,
                    "use.extension.registry_target_invalid",
                )
                .await?;
                return Ok(Self {
                    _lock: lock,
                    cache_directory,
                    target_path,
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
            admit_target_write(&cache_directory, &expected_sha256, expected_length, policy).await?;
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
            cache_directory,
            target_path,
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
            None,
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
        let verified = promote_verified_partial(
            partial,
            self.partial_path.clone(),
            self.target_path.clone(),
            &self.cache_directory,
            self.expected_length,
            &self.expected_sha256,
            error_code,
        )
        .await?;
        self.verified = Some(verified);
        self.ready = true;
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
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut destination = options
            .open(output)
            .await
            .map_err(|error| io_error("create verified target staging file", output, error))?;
        let verified = self.verified.as_mut().ok_or_else(|| {
            target_cache_error(
                "use.extension.registry_target_cache_invalid",
                "The verified resumable target handle is unavailable.",
            )
        })?;
        verify_open_file(
            verified,
            Some(&mut destination),
            &self.target_path,
            self.expected_length,
            &self.expected_sha256,
            "use.extension.registry_target_cache_invalid",
        )
        .await?;
        destination
            .sync_all()
            .await
            .map_err(|error| io_error("sync verified target staging file", output, error))
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

async fn promote_verified_partial(
    partial: fs::File,
    partial_path: PathBuf,
    target_path: PathBuf,
    cache_directory: &Path,
    expected_length: u64,
    expected_sha256: &str,
    error_code: &'static str,
) -> UseResult<fs::File> {
    secure_file(&partial, &partial_path).await?;
    drop(partial);
    run_before_promotion_hook(&partial_path);
    activate_temporary_file(
        partial_path,
        target_path.clone(),
        "activate resumed verified target cache",
    )
    .await?;
    let mut verified = open_verified_file(&target_path, expected_length, error_code).await?;
    verify_open_file(
        &mut verified,
        None,
        &target_path,
        expected_length,
        expected_sha256,
        error_code,
    )
    .await?;
    sync_parent_directory(cache_directory, "verified target cache").await?;
    Ok(verified)
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
    fs::remove_file(path)
        .await
        .map_err(|error| io_error("remove resumable Registry target", path, error))?;
    sync_parent_directory(cache_directory, "verified target cache").await
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    #[cfg(windows)]
    fn open_reading_scanner_without_delete_share(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;

        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(path)
            .unwrap()
    }

    #[tokio::test]
    async fn complete_partial_is_verified_and_promoted_without_a_request() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        std::fs::create_dir_all(&cache).unwrap();
        let body = b"complete signed target";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        std::fs::write(&partial, body).unwrap();
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

        let target = ResumableTarget::begin(datastore, body.len() as u64, &digest, policy)
            .await
            .unwrap();

        assert!(target.is_ready());
        assert!(!partial.exists());
        assert_eq!(std::fs::read(cache.join(digest)).unwrap(), body);
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

        let error = ResumableTarget::begin(datastore, 2, &digest, policy)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
    }

    #[tokio::test]
    async fn replacement_after_verification_is_not_published_as_verified() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        let body = b"trusted!";
        let replacement = b"attacker";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();
        let mut target = ResumableTarget::begin(datastore, body.len() as u64, &digest, policy)
            .await
            .unwrap();
        target.append(body).await.unwrap();
        install_before_promotion_hook(
            partial.clone(),
            Box::new(move |path| {
                std::fs::remove_file(path).unwrap();
                std::fs::write(path, replacement).unwrap();
            }),
        );

        let error = target
            .commit("use.extension.registry_target_invalid")
            .await
            .expect_err("a post-verification replacement must fail closed");

        assert_eq!(error.code, "use.extension.registry_target_invalid");
        assert!(!target.is_ready());
        assert_eq!(std::fs::read(cache.join(&digest)).unwrap(), replacement);
        drop(target);

        let error = match ResumableTarget::begin(
            datastore,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        {
            Ok(_) => panic!("the replaced cache target must remain untrusted"),
            Err(error) => error,
        };
        assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verified_handle_stages_original_after_target_path_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let body = b"trusted!";
        let replacement = b"attacker";
        let digest = format!("{:x}", Sha256::digest(body));
        let target_path = datastore.join("verified-targets/sha256").join(&digest);
        let mut target = ResumableTarget::begin(
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
    async fn active_verified_target_allows_read_but_denies_external_write_and_removal() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let body = b"trusted!";
        let digest = format!("{:x}", Sha256::digest(body));
        let target_path = datastore.join("verified-targets/sha256").join(&digest);
        let mut target = ResumableTarget::begin(
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
            "active verified target accepted an external writer: {write_error}"
        );
        let remove_error = std::fs::remove_file(&target_path).unwrap_err();
        assert!(
            matches!(remove_error.raw_os_error(), Some(5 | 32 | 33)),
            "active verified target accepted external removal: {remove_error}"
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

        let target = ResumableTarget::begin(datastore, 16, &digest, policy)
            .await
            .unwrap();
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
    async fn transient_scanner_lock_releases_into_verified_promotion() {
        let temporary = tempfile::tempdir().unwrap();
        let datastore = temporary.path();
        let cache = datastore.join("verified-targets/sha256");
        let body = b"scanner retained target";
        let digest = format!("{:x}", Sha256::digest(body));
        let partial = cache.join(partial_name(&digest));
        let target_path = cache.join(&digest);
        let mut target = ResumableTarget::begin(
            datastore,
            body.len() as u64,
            &digest,
            VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap(),
        )
        .await
        .unwrap();
        target.append(body).await.unwrap();
        let scanner = open_reading_scanner_without_delete_share(&partial);
        let mut promotion = Box::pin(target.commit("use.extension.registry_target_invalid"));

        tokio::select! {
            result = &mut promotion => {
                panic!("promotion completed while the scanner denied delete sharing: {result:?}")
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }

        drop(scanner);
        promotion.await.unwrap();
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
        let target_path = cache.join(&digest);
        let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();
        let mut target = ResumableTarget::begin(datastore, body.len() as u64, &digest, policy)
            .await
            .unwrap();
        target.append(body).await.unwrap();
        let scanner = open_reading_scanner_without_delete_share(&partial);

        let started = std::time::Instant::now();
        let error = target
            .commit("use.extension.registry_target_invalid")
            .await
            .expect_err("a persistent scanner lock must stop at the retry bound");
        let elapsed = started.elapsed();

        assert_eq!(error.code, "use.extension.io");
        assert!(elapsed >= std::time::Duration::from_secs(2));
        assert!(elapsed < std::time::Duration::from_secs(10));
        assert!(!target.is_ready());
        assert_eq!(std::fs::read(&partial).unwrap(), body);
        assert!(!target_path.exists());

        drop(scanner);
        drop(target);
        let mut recovered = ResumableTarget::begin(
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

        let error = ResumableTarget::begin(datastore, 2, &digest, policy)
            .await
            .err()
            .unwrap();

        assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
        assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"x");
    }
}
