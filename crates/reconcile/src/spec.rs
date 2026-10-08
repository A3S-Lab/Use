use sha2::{Digest, Sha256};

use crate::error::ReconcileError;

/// Disk record written only after every declared surface verifies.
pub const SCHEMA: &str = "a3s.use.package-reconcile.v2";

/// Surfaces one package generation may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SurfaceKind {
    Skill,
    Okf,
    Ui,
    Mcp,
    Tool,
    Flow,
}

impl SurfaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Okf => "okf",
            Self::Ui => "ui",
            Self::Mcp => "mcp",
            Self::Tool => "tool",
            Self::Flow => "flow",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "skill" => Some(Self::Skill),
            "okf" => Some(Self::Okf),
            "ui" => Some(Self::Ui),
            "mcp" => Some(Self::Mcp),
            "tool" => Some(Self::Tool),
            "flow" => Some(Self::Flow),
            _ => None,
        }
    }
}

/// One regular file inside a surface payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Bytes the host pinned for one surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Files(Vec<PackageFile>),
    Zip(Vec<u8>),
}

/// One declared surface. `sha256` is `sha256:` plus 64 lowercase hex digits,
/// or the 64 digits alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceSpec {
    pub kind: SurfaceKind,
    pub id: String,
    pub sha256: String,
    pub entry: Option<String>,
    pub payload: Payload,
}

/// The package to publish as one generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub package_id: String,
    pub surfaces: Vec<SurfaceSpec>,
}

pub(crate) fn parse_package_id(raw: &str) -> Result<(), ReconcileError> {
    let segments: Vec<&str> = raw.split('/').collect();
    if segments.len() < 2 || raw.len() > 200 || segments.iter().any(|segment| !segment_ok(segment))
    {
        return Err(ReconcileError::InvalidPackageId(raw.to_string()));
    }
    Ok(())
}

pub(crate) fn parse_surface_id(raw: &str) -> Result<(), ReconcileError> {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return Err(ReconcileError::InvalidSurfaceId(raw.to_string()));
    };
    if raw.len() > 64
        || !first.is_ascii_alphanumeric()
        || !chars.all(|character| {
            character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.'
        })
    {
        return Err(ReconcileError::InvalidSurfaceId(raw.to_string()));
    }
    Ok(())
}

fn segment_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.'
        })
}

pub(crate) fn normalize_sha256(raw: &str) -> Result<String, ReconcileError> {
    let hex = raw.strip_prefix("sha256:").unwrap_or(raw);
    if hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(format!("sha256:{hex}"))
    } else {
        Err(ReconcileError::DigestMismatch)
    }
}

/// Digest of a file payload. Paths are sorted. The encoding is path, NUL,
/// little-endian length, then bytes.
pub fn files_sha256(files: &[PackageFile]) -> Result<String, ReconcileError> {
    if files.is_empty() {
        return Err(ReconcileError::EmptyPayload);
    }
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut seen = None;
    let mut hasher = Sha256::new();
    for file in &ordered {
        validate_relative(&file.path)?;
        if seen == Some(file.path.as_str()) {
            return Err(ReconcileError::InvalidPath(file.path.clone()));
        }
        seen = Some(file.path.as_str());
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update((file.bytes.len() as u64).to_le_bytes());
        hasher.update(&file.bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Digest the host pins. Zip payloads hash the archive bytes.
pub fn payload_sha256(payload: &Payload) -> Result<String, ReconcileError> {
    match payload {
        Payload::Files(files) => files_sha256(files),
        Payload::Zip(bytes) => {
            if bytes.is_empty() {
                return Err(ReconcileError::EmptyPayload);
            }
            Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
        }
    }
}

pub(crate) fn validate_relative(path: &str) -> Result<(), ReconcileError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err(ReconcileError::InvalidPath(path.to_string()));
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(ReconcileError::InvalidPath(path.to_string()));
        }
    }
    Ok(())
}
