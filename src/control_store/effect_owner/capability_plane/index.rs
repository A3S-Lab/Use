use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::ControlCapabilityIndexDocument;

const CAPABILITY_INDEX_DIRECTORY: &str = "capability-index";
const CAPABILITY_INDEX_LOCK: &str = ".mutation.lock";
const CAPABILITY_INDEX_STAGING: &str = ".staging";
const MAX_CAPABILITY_INDEX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(in crate::control_store) struct ControlCapabilityIndexStore {
    state_root: PathBuf,
    index_root: PathBuf,
}

impl ControlCapabilityIndexStore {
    pub(in crate::control_store) fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            index_root: state_root.join(CAPABILITY_INDEX_DIRECTORY),
            state_root,
        }
    }

    pub(in crate::control_store) async fn materialize(
        &self,
        document: &ControlCapabilityIndexDocument,
    ) -> UseResult<String> {
        let bytes = document.canonical_bytes()?;
        let receipt_digest = document.receipt_digest()?;
        let _guard = self.try_acquire_mutation().await?;
        let target = self.document_path(&receipt_digest)?;
        let parent = target.parent().ok_or_else(index_path_invalid)?;
        ensure_owned_directory_chain(&self.state_root, parent).await?;
        let staging_root = self.index_root.join(CAPABILITY_INDEX_STAGING);
        ensure_owned_directory_chain(&self.state_root, &staging_root).await?;
        let staging = staging_root.join(format!(
            "{}.tmp",
            receipt_digest
                .strip_prefix("sha256:")
                .ok_or_else(index_path_invalid)?
        ));
        if optional_regular_file(&target).await? {
            let existing = read_document(&target, &receipt_digest).await?;
            if existing != *document || existing.canonical_bytes()? != bytes {
                return Err(index_conflict());
            }
            // Make a replayed target durable before retiring the staging link
            // that may be its only other crash-stable directory entry.
            sync_directory(parent).await?;
            retire_owned_staging(&staging).await?;
            sync_directory(&staging_root).await?;
            return Ok(receipt_digest);
        }

        // A prior attempt that never published a target can leave only this
        // deterministic staging identity. The committed request remains the
        // authority, so recreate the same bytes under create-new semantics.
        retire_owned_staging(&staging).await?;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        configure_no_follow_async(&mut options);
        let mut file = options
            .open(&staging)
            .await
            .map_err(|error| index_io("create Capability Index staging file", &staging, error))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| index_io("write Capability Index staging file", &staging, error))?;
        file.sync_all()
            .await
            .map_err(|error| index_io("sync Capability Index staging file", &staging, error))?;
        drop(file);
        validate_regular_file(&staging).await?;
        // Persist the recovery link before attempting publication. A crash
        // before the target directory is synced can then replay from staging.
        sync_directory(&staging_root).await?;
        // A hard-link publication is create-if-absent on every supported
        // platform. Unlike rename on Unix it can never replace an immutable
        // document that appeared after the existence check above.
        match fs::hard_link(&staging, &target).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(index_io(
                    "publish immutable Capability Index document",
                    &target,
                    error,
                ))
            }
        }
        let published = read_document(&target, &receipt_digest).await?;
        if published != *document || published.canonical_bytes()? != bytes {
            return Err(index_conflict());
        }
        // The publication directory must reach stable storage before the
        // recovery link is removed; reversing these operations admits a crash
        // window in which neither directory entry survives.
        sync_directory(parent).await?;
        retire_owned_staging(&staging).await?;
        sync_directory(&staging_root).await?;
        Ok(receipt_digest)
    }

    pub(in crate::control_store) async fn read(
        &self,
        receipt_digest: &str,
    ) -> UseResult<ControlCapabilityIndexDocument> {
        read_document(&self.document_path(receipt_digest)?, receipt_digest).await
    }

    async fn try_acquire_mutation(&self) -> UseResult<IndexMutationGuard> {
        ensure_owned_directory_chain(&self.state_root, &self.index_root).await?;
        let path = self.index_root.join(CAPABILITY_INDEX_LOCK);
        match fs::symlink_metadata(&path).await {
            Ok(metadata)
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    || !metadata.is_file() =>
            {
                return Err(index_path_invalid())
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(index_io(
                    "inspect Capability Index mutation lock",
                    &path,
                    error,
                ))
            }
        }
        let error_path = path.clone();
        let acquired = tokio::task::spawn_blocking(move || -> io::Result<std::fs::File> {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            configure_no_follow_blocking(&mut options);
            let file = options.open(path)?;
            FileExt::try_lock_exclusive(&file)?;
            Ok(file)
        })
        .await
        .map_err(|error| {
            UseError::new(
                "use.control.capability_index_io",
                format!("Capability Index lock task failed: {error}"),
            )
        })?;
        match acquired {
            Ok(file) => {
                validate_regular_file(&error_path).await?;
                Ok(IndexMutationGuard(file))
            }
            Err(error) if lock_is_contended(&error) => Err(UseError::new(
                "use.control.capability_index_contended",
                "Another Capability Index materialization owns the mutation lock.",
            )),
            Err(error) => Err(index_io(
                "acquire Capability Index mutation lock",
                &error_path,
                error,
            )),
        }
    }

    fn document_path(&self, receipt_digest: &str) -> UseResult<PathBuf> {
        let digest = receipt_digest
            .strip_prefix("sha256:")
            .filter(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            })
            .ok_or_else(index_path_invalid)?;
        Ok(self
            .index_root
            .join("sha256")
            .join(&digest[..2])
            .join(format!("{digest}.json")))
    }
}

struct IndexMutationGuard(std::fs::File);

impl Drop for IndexMutationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

async fn read_document(
    path: &Path,
    receipt_digest: &str,
) -> UseResult<ControlCapabilityIndexDocument> {
    let metadata = validate_regular_file(path).await?;
    if metadata.len() == 0 || metadata.len() > MAX_CAPABILITY_INDEX_FILE_BYTES {
        return Err(index_conflict());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| index_io("open Capability Index document", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| index_io("inspect opened Capability Index document", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
    {
        return Err(index_conflict());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_CAPABILITY_INDEX_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| index_io("read Capability Index document", path, error))?;
    let after = validate_regular_file(path).await?;
    if after.len() != opened.len() || bytes.len() as u64 != opened.len() {
        return Err(index_conflict());
    }
    let document =
        ControlCapabilityIndexDocument::from_bytes(&bytes).map_err(|_| index_conflict())?;
    if document.receipt_digest()? != receipt_digest {
        return Err(index_conflict());
    }
    Ok(document)
}

async fn retire_owned_staging(path: &Path) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            fs::remove_file(path).await.map_err(|error| {
                index_io("retire incomplete Capability Index staging", path, error)
            })
        }
        Ok(_) => Err(index_path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(index_io("inspect Capability Index staging", path, error)),
    }
}

async fn optional_regular_file(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(true)
        }
        Ok(_) => Err(index_path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(index_io(
            "inspect optional Capability Index document",
            path,
            error,
        )),
    }
}

async fn validate_regular_file(path: &Path) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| index_io("inspect Capability Index file", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(index_path_invalid());
    }
    Ok(metadata)
}

async fn ensure_owned_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(index_path_invalid());
    }
    validate_owned_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| index_path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(index_path_invalid());
        };
        let parent = current.clone();
        current.push(segment);
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(index_path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(index_io(
                            "create Capability Index directory",
                            &current,
                            error,
                        ))
                    }
                }
                validate_owned_directory(&current).await?;
                // Persist each newly created directory entry before creating
                // descendants that depend on it.
                sync_directory(&parent).await?;
            }
            Err(error) => {
                return Err(index_io(
                    "inspect Capability Index directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(())
}

async fn validate_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| index_io("inspect Capability Index directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(index_path_invalid());
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| index_io("open Capability Index directory for sync", path, error))?
        .sync_all()
        .await
        .map_err(|error| index_io("sync Capability Index directory", path, error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn configure_no_follow_async(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

fn configure_no_follow_blocking(options: &mut std::fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
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

fn index_conflict() -> UseError {
    UseError::new(
        "use.control.capability_index_conflict",
        "The immutable Capability Index document differs from its receipt identity.",
    )
}

fn index_path_invalid() -> UseError {
    UseError::new(
        "use.control.capability_index_path_invalid",
        "A Capability Index path is outside its owned link-free layout.",
    )
}

fn index_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.control.capability_index_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
