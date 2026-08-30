use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::{
    artifact_store_error, validate_real_directory, validate_sha256, ArtifactKind,
    ArtifactMutationLock, ArtifactReferenceAdmission, ArtifactStorageWrite, ArtifactStore,
    ARTIFACT_STAGING_PREFIX, BLOBS_DIRECTORY, CONTENT_DIRECTORY, MAX_ARTIFACT_CONTAINER_ENTRIES,
    MUTATION_LOCK, SHA256_DIRECTORY,
};
use crate::package::{
    io_error, remove_file_with_windows_retry, sync_parent_directory, unique_suffix,
};

/// One verified handle to immutable bytes in the global Artifact Store.
///
/// The handle, rather than its path, remains the authority while callers copy
/// bytes into an operation-local staging directory. Global blobs are never
/// removed by Registry-source pruning or installation lifecycle operations.
#[derive(Debug)]
pub(crate) struct ArtifactBlob {
    path: PathBuf,
    file: fs::File,
    expected_length: u64,
    sha256: String,
}

impl ArtifactBlob {
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) async fn stage_into(&mut self, output: &Path) -> UseResult<()> {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut destination = options
            .open(output)
            .await
            .map_err(|error| io_error("create global artifact blob staging file", output, error))?;
        let result = verify_open_file(
            &mut self.file,
            Some(&mut destination),
            &self.path,
            self.expected_length,
            &self.sha256,
        )
        .await;
        if let Err(error) = result {
            drop(destination);
            let _ = fs::remove_file(output).await;
            return Err(error);
        }
        if let Err(error) = destination.sync_all().await {
            drop(destination);
            let _ = fs::remove_file(output).await;
            return Err(io_error(
                "sync global artifact blob staging file",
                output,
                error,
            ));
        }
        Ok(())
    }
}

impl ArtifactStore {
    /// Resolve one raw immutable blob from its canonical digest.
    pub fn blob_path(&self, digest: &str) -> UseResult<PathBuf> {
        let sha256 = digest.strip_prefix("sha256:").ok_or_else(|| {
            artifact_store_error(
                "use.artifact_store.digest_invalid",
                "An artifact blob digest must use the 'sha256:' prefix.",
            )
        })?;
        validate_sha256(sha256)?;
        Ok(self.blob_path_from_sha256(sha256))
    }

    pub(crate) fn blob_path_from_sha256(&self, sha256: &str) -> PathBuf {
        self.blob_container(sha256).join(CONTENT_DIRECTORY)
    }

    /// Open and reverify an existing global blob without following links.
    pub(crate) async fn open_blob(
        &self,
        admission: &ArtifactReferenceAdmission,
        sha256: &str,
        expected_length: u64,
    ) -> UseResult<Option<ArtifactBlob>> {
        admission.ensure_store(self)?;
        validate_blob_evidence(sha256, expected_length)?;
        let path = self.blob_path_from_sha256(sha256);
        if !self
            .validate_optional_blob_path(sha256, expected_length)
            .await?
        {
            return Ok(None);
        }
        open_blob_path(path, expected_length, sha256)
            .await
            .map(Some)
    }

    /// Observe only the owned path and exact length of a global blob.
    ///
    /// This is diagnostic evidence, not admission authority, and deliberately
    /// does not rehash the bytes. Every staging or commit path rehashes them.
    pub(crate) async fn observe_blob(&self, sha256: &str, expected_length: u64) -> UseResult<bool> {
        validate_blob_evidence(sha256, expected_length)?;
        self.validate_optional_blob_path(sha256, expected_length)
            .await
    }

    /// Commit already verified bytes into the global content-addressed tier.
    ///
    /// The caller retains its source handle throughout this call. The bytes are
    /// copied, hashed again, synchronized, and atomically published under a
    /// cross-process digest lock. Existing content is reverified and is never
    /// overwritten, including when corruption is detected.
    pub(crate) async fn commit_blob(
        &self,
        admission: &ArtifactReferenceAdmission,
        source: &mut fs::File,
        expected_length: u64,
        sha256: &str,
    ) -> UseResult<ArtifactBlob> {
        admission.ensure_store(self)?;
        validate_blob_evidence(sha256, expected_length)?;
        let _storage = self
            .acquire_storage_admission(
                admission,
                ArtifactStorageWrite::blob(sha256, expected_length)?,
            )
            .await?;
        let container = self.blob_container(sha256);
        self.ensure_container(&container, "blob artifact").await?;
        self.ensure_container_not_quarantined(&container, ArtifactKind::Blob, sha256)
            .await?;
        let _lock =
            ArtifactMutationLock::acquire(&container.join(MUTATION_LOCK), "blob artifact").await?;
        reclaim_abandoned_staging(&container).await?;

        let target = self.blob_path_from_sha256(sha256);
        if self
            .validate_optional_blob_path(sha256, expected_length)
            .await?
        {
            return open_blob_path(target, expected_length, sha256).await;
        }

        validate_source_handle(source, expected_length).await?;
        let staging = container.join(format!("{ARTIFACT_STAGING_PREFIX}{}.tmp", unique_suffix()));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut output = options
            .open(&staging)
            .await
            .map_err(|error| io_error("create global artifact blob staging", &staging, error))?;
        if let Err(error) = secure_blob_file(&output, &staging).await {
            drop(output);
            let _ = fs::remove_file(&staging).await;
            return Err(error);
        }
        if let Err(error) =
            verify_open_file(source, Some(&mut output), &staging, expected_length, sha256).await
        {
            drop(output);
            let _ = fs::remove_file(&staging).await;
            return Err(error);
        }
        if let Err(error) = output.sync_all().await {
            drop(output);
            let _ = fs::remove_file(&staging).await;
            return Err(io_error(
                "sync global artifact blob staging",
                &staging,
                error,
            ));
        }
        drop(output);
        let staging_for_error = staging.clone();
        let target_for_worker = target.clone();
        let published = match tokio::task::spawn_blocking(move || {
            crate::atomic_file::persist_temporary_noclobber_blocking(staging, &target_for_worker)
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                let _ = fs::remove_file(&staging_for_error).await;
                return Err(artifact_store_error(
                    "use.artifact_store.blob_invalid",
                    format!("Global artifact blob publication did not complete: {error}"),
                ));
            }
        };
        if let Err(error) = published {
            let _ = fs::remove_file(&staging_for_error).await;
            return Err(io_error("publish global artifact blob", &target, error));
        }
        sync_parent_directory(&container, "global artifact blob").await?;
        open_blob_path(target, expected_length, sha256).await
    }

    pub(super) fn blob_container(&self, sha256: &str) -> PathBuf {
        let shard = sha256.get(..2).unwrap_or_default();
        self.root()
            .join(BLOBS_DIRECTORY)
            .join(SHA256_DIRECTORY)
            .join(shard)
            .join(sha256)
    }

    async fn validate_optional_blob_path(
        &self,
        sha256: &str,
        expected_length: u64,
    ) -> UseResult<bool> {
        let container = self.blob_container(sha256);
        let relative = container.strip_prefix(self.root()).map_err(|_| {
            artifact_store_error(
                "use.artifact_store.ownership_invalid",
                "An artifact blob path escapes the Artifact Store.",
            )
        })?;
        let mut current = self.root().to_path_buf();
        if !optional_real_directory(&current, "Artifact Store root").await? {
            return Ok(false);
        }
        for component in relative.components() {
            current.push(component.as_os_str());
            if !optional_real_directory(&current, "blob Artifact Store directory").await? {
                return Ok(false);
            }
        }
        self.ensure_container_not_quarantined(&current, ArtifactKind::Blob, sha256)
            .await?;
        let content = current.join(CONTENT_DIRECTORY);
        let metadata = match fs::symlink_metadata(&content).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_error("inspect global artifact blob", &content, error)),
        };
        validate_blob_metadata(&content, &metadata, expected_length)?;
        Ok(true)
    }
}

async fn open_blob_path(
    path: PathBuf,
    expected_length: u64,
    sha256: &str,
) -> UseResult<ArtifactBlob> {
    let file = blob_open_options()
        .open(&path)
        .await
        .map_err(|error| io_error("open global artifact blob", &path, error))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened global artifact blob", &path, error))?;
    validate_blob_metadata(&path, &metadata, expected_length)?;
    let mut blob = ArtifactBlob {
        path,
        file,
        expected_length,
        sha256: sha256.to_owned(),
    };
    verify_open_file(&mut blob.file, None, &blob.path, expected_length, sha256).await?;
    Ok(blob)
}

async fn validate_source_handle(source: &fs::File, expected_length: u64) -> UseResult<()> {
    let metadata = source.metadata().await.map_err(|error| {
        artifact_store_error(
            "use.artifact_store.blob_invalid",
            format!("Failed to inspect the verified blob source handle: {error}"),
        )
    })?;
    if !metadata.is_file() || metadata.len() != expected_length {
        return Err(artifact_store_error(
            "use.artifact_store.blob_invalid",
            "The verified blob source handle does not match its expected length.",
        ));
    }
    Ok(())
}

async fn verify_open_file(
    input: &mut fs::File,
    mut output: Option<&mut fs::File>,
    path: &Path,
    expected_length: u64,
    expected_sha256: &str,
) -> UseResult<()> {
    let metadata = input
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened artifact blob", path, error))?;
    if !metadata.is_file() || metadata.len() != expected_length {
        return Err(blob_invalid(
            path,
            "The opened artifact blob does not match its expected length.",
        ));
    }
    input
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|error| io_error("seek opened artifact blob", path, error))?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("read opened artifact blob", path, error))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| blob_invalid(path, "The opened artifact blob length overflowed."))?;
        if length > expected_length {
            return Err(blob_invalid(
                path,
                "The opened artifact blob exceeds its expected length.",
            ));
        }
        digest.update(&buffer[..read]);
        if let Some(destination) = output.as_deref_mut() {
            destination
                .write_all(&buffer[..read])
                .await
                .map_err(|error| io_error("write global artifact blob", path, error))?;
        }
    }
    let actual_sha256 = format!("{:x}", digest.finalize());
    if length != expected_length || actual_sha256 != expected_sha256 {
        return Err(blob_invalid(
            path,
            "The artifact blob does not match its expected length and SHA-256 digest.",
        )
        .with_detail("expectedLength", expected_length.to_string())
        .with_detail("actualLength", length.to_string())
        .with_detail("expectedSha256", expected_sha256.to_owned())
        .with_detail("actualSha256", actual_sha256));
    }
    Ok(())
}

async fn reclaim_abandoned_staging(container: &Path) -> UseResult<()> {
    let mut entries = fs::read_dir(container)
        .await
        .map_err(|error| io_error("read artifact blob container", container, error))?;
    let mut entries_seen = 0_usize;
    let mut removed = false;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("read artifact blob entry", container, error))?
    {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_ARTIFACT_CONTAINER_ENTRIES {
            return Err(artifact_store_error(
                "use.artifact_store.inventory_limit_exceeded",
                "An artifact blob container exceeds its bounded entry inventory.",
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            blob_invalid(
                &entry.path(),
                "An artifact blob container contains a non-UTF-8 entry name.",
            )
        })?;
        if matches!(name, MUTATION_LOCK | CONTENT_DIRECTORY) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .await
            .map_err(|error| io_error("inspect artifact blob staging", &path, error))?;
        if !name.starts_with(ARTIFACT_STAGING_PREFIX)
            || a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || !metadata.is_file()
        {
            return Err(blob_invalid(
                &path,
                "An artifact blob container contains an unowned entry.",
            ));
        }
        remove_file_with_windows_retry(path, "remove abandoned artifact blob staging").await?;
        removed = true;
    }
    if removed {
        sync_parent_directory(container, "artifact blob").await?;
    }
    Ok(())
}

async fn optional_real_directory(path: &Path, label: &str) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(_) => {
            validate_real_directory(path, label).await?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(&format!("inspect {label}"), path, error)),
    }
}

pub(super) fn blob_open_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    options
}

#[cfg(unix)]
async fn secure_blob_file(file: &fs::File, path: &Path) -> UseResult<()> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(|error| io_error("secure global artifact blob staging", path, error))
}

#[cfg(not(unix))]
async fn secure_blob_file(_file: &fs::File, _path: &Path) -> UseResult<()> {
    Ok(())
}

fn validate_blob_evidence(sha256: &str, expected_length: u64) -> UseResult<()> {
    validate_sha256(sha256)?;
    if expected_length == 0 {
        return Err(artifact_store_error(
            "use.artifact_store.blob_invalid",
            "An artifact blob must have a non-zero expected length.",
        ));
    }
    Ok(())
}

fn validate_blob_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    expected_length: u64,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() != expected_length
    {
        return Err(blob_invalid(
            path,
            "The global artifact blob is not an owned regular file with the expected length.",
        ));
    }
    Ok(())
}

fn blob_invalid(path: &Path, message: impl Into<String>) -> a3s_use_core::UseError {
    artifact_store_error("use.artifact_store.blob_invalid", message)
        .with_detail("path", path.display().to_string())
}

#[cfg(test)]
#[path = "blob_tests.rs"]
mod tests;
