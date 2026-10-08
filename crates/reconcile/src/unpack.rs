use std::io::{Cursor, Read};

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::error::ReconcileError;
use crate::spec::{validate_relative, PackageFile, Payload};

pub(crate) const MAX_ENTRIES: usize = 512;
pub(crate) const MAX_UNPACKED: u64 = 64 * 1024 * 1024;
const MAX_ZIP_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct PreparedSurface {
    pub files: Vec<PackageFile>,
    pub sha256: String,
    pub mcp: Option<serde_json::Value>,
}

pub(crate) fn prepare(
    kind: crate::spec::SurfaceKind,
    id: &str,
    expected: &str,
    entry: Option<&str>,
    payload: &Payload,
) -> Result<PreparedSurface, ReconcileError> {
    let files = match payload {
        Payload::Files(files) => {
            if files.is_empty() {
                return Err(ReconcileError::EmptyPayload);
            }
            let mut copied = files.clone();
            copied.sort_by(|left, right| left.path.cmp(&right.path));
            for file in &copied {
                validate_relative(&file.path)?;
            }
            copied
        }
        Payload::Zip(bytes) => unpack_zip(bytes)?,
    };
    let sha256 = match payload {
        Payload::Files(files) => crate::spec::files_sha256(files)?,
        Payload::Zip(bytes) => format!("sha256:{:x}", Sha256::digest(bytes)),
    };
    if sha256 != expected {
        return Err(ReconcileError::DigestMismatch);
    }
    match kind {
        crate::spec::SurfaceKind::Skill => {
            if !files
                .iter()
                .any(|file| file.path == "SKILL.md" || file.path.ends_with("/SKILL.md"))
            {
                return Err(ReconcileError::SkillMissing);
            }
        }
        crate::spec::SurfaceKind::Mcp => {
            return Ok(PreparedSurface {
                mcp: Some(mcp_server(&files)?),
                files,
                sha256,
            });
        }
        crate::spec::SurfaceKind::Ui
        | crate::spec::SurfaceKind::Tool
        | crate::spec::SurfaceKind::Flow => {
            let entry = entry.ok_or_else(|| ReconcileError::EntryRequired {
                kind: kind.as_str().to_string(),
                id: id.to_string(),
            })?;
            validate_relative(entry)?;
            if !files.iter().any(|file| file.path == entry) {
                return Err(ReconcileError::InvalidPath(entry.to_string()));
            }
        }
        crate::spec::SurfaceKind::Okf => {}
    }
    Ok(PreparedSurface {
        files,
        sha256,
        mcp: None,
    })
}

fn unpack_zip(bytes: &[u8]) -> Result<Vec<PackageFile>, ReconcileError> {
    if bytes.is_empty() {
        return Err(ReconcileError::EmptyPayload);
    }
    if bytes.len() > MAX_ZIP_BYTES {
        return Err(ReconcileError::ArchiveTooLarge);
    }
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|_| ReconcileError::ArchiveInvalid)?;
    if archive.len() > MAX_ENTRIES {
        return Err(ReconcileError::ArchiveTooLarge);
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| ReconcileError::ArchiveInvalid)?;
        if entry.is_symlink() {
            return Err(ReconcileError::ArchiveLink);
        }
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry.enclosed_name() else {
            return Err(ReconcileError::ArchiveEscape);
        };
        let path = name.to_string_lossy();
        validate_relative(&path)?;
        let mut bytes = Vec::new();
        let limit = MAX_UNPACKED.saturating_sub(total).saturating_add(1);
        entry
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|_| ReconcileError::ArchiveInvalid)?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_UNPACKED {
            return Err(ReconcileError::ArchiveTooLarge);
        }
        files.push(PackageFile {
            path: path.into_owned(),
            bytes,
        });
    }
    if files.is_empty() {
        return Err(ReconcileError::EmptyPayload);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn mcp_server(files: &[PackageFile]) -> Result<serde_json::Value, ReconcileError> {
    let Some(file) = files.iter().find(|file| file.path == "server.json") else {
        return Err(ReconcileError::McpInvalid);
    };
    let value: serde_json::Value =
        serde_json::from_slice(&file.bytes).map_err(|_| ReconcileError::McpInvalid)?;
    let object = value.as_object().ok_or(ReconcileError::McpInvalid)?;
    match object.get("type").and_then(serde_json::Value::as_str) {
        Some("stdio") => {
            let command = object.get("command").and_then(serde_json::Value::as_str);
            if command.is_some_and(|command| !command.is_empty() && !command.contains('\0')) {
                Ok(value)
            } else {
                Err(ReconcileError::McpInvalid)
            }
        }
        Some("http") | Some("sse") => {
            let url = object.get("url").and_then(serde_json::Value::as_str);
            if url.is_some_and(|url| url.starts_with("https://") || url.starts_with("http://")) {
                Ok(value)
            } else {
                Err(ReconcileError::McpInvalid)
            }
        }
        _ => Err(ReconcileError::McpInvalid),
    }
}
