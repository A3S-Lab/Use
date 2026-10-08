use std::fs;
use std::io::Read;
use std::path::Path;

use a3s_use_core::metadata_is_link_or_reparse_point;

use crate::spec::{PackageFile, Payload};
use crate::unpack::{prepare, MAX_ENTRIES, MAX_UNPACKED};
use crate::{PublishedSurface, ReconcileError};

impl PublishedSurface {
    /// Verify a surface published from `Payload::Files`, including its entry and
    /// MCP projection. Archive digests cannot be verified from unpacked files.
    /// This checks bytes at read time; it does not acquire an execution lease.
    pub fn verify_files_payload(&self) -> Result<(), ReconcileError> {
        self.verify_digest(&self.sha256)
    }

    /// Verify the unpacked content digest for either Files or ZIP publications.
    pub fn verify_content(&self) -> Result<(), ReconcileError> {
        self.verify_digest(&self.content_sha256)
    }

    fn verify_digest(&self, expected: &str) -> Result<(), ReconcileError> {
        let mut files = Vec::new();
        let mut entries = 0;
        let mut bytes = 0;
        collect(
            &self.directory,
            &self.directory,
            &mut files,
            &mut entries,
            &mut bytes,
        )?;
        let entry = self
            .entry
            .as_ref()
            .map(|entry| {
                entry
                    .strip_prefix(&self.directory)
                    .map_err(|_| ReconcileError::InvalidPath(entry.display().to_string()))
                    .and_then(relative_name)
            })
            .transpose()?;
        let prepared = prepare(
            self.kind,
            &self.id,
            expected,
            entry.as_deref(),
            &Payload::Files(files),
        )?;
        if prepared.mcp != self.mcp {
            return Err(ReconcileError::McpInvalid);
        }
        Ok(())
    }
}

fn collect(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PackageFile>,
    entries: &mut usize,
    bytes: &mut u64,
) -> Result<(), ReconcileError> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| ReconcileError::io("inspect surface", directory, error))?;
    if metadata_is_link_or_reparse_point(&metadata) {
        return Err(ReconcileError::LinkRefused {
            path: directory.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(ReconcileError::InvalidPath(directory.display().to_string()));
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| ReconcileError::io("read surface", directory, error))?
    {
        let entry =
            entry.map_err(|error| ReconcileError::io("read surface entry", directory, error))?;
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return Err(ReconcileError::PayloadTooLarge);
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| ReconcileError::io("inspect surface entry", &path, error))?;
        if metadata_is_link_or_reparse_point(&metadata) {
            return Err(ReconcileError::LinkRefused { path });
        }
        if metadata.is_dir() {
            collect(root, &path, files, entries, bytes)?;
            continue;
        }
        crate::store::ensure_real_file(&path)?;
        if metadata.len() > MAX_UNPACKED.saturating_sub(*bytes) {
            return Err(ReconcileError::PayloadTooLarge);
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| ReconcileError::InvalidPath(path.display().to_string()))?;
        let relative = relative_name(relative)?;
        let mut body = Vec::new();
        fs::File::open(&path)
            .map_err(|error| ReconcileError::io("open surface file", &path, error))?
            .take(MAX_UNPACKED.saturating_sub(*bytes).saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|error| ReconcileError::io("read surface file", &path, error))?;
        *bytes += body.len() as u64;
        if *bytes > MAX_UNPACKED {
            return Err(ReconcileError::PayloadTooLarge);
        }
        files.push(PackageFile {
            path: relative,
            bytes: body,
        });
    }
    Ok(())
}

fn relative_name(relative: &Path) -> Result<String, ReconcileError> {
    relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| ReconcileError::InvalidPath(relative.display().to_string()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}
