use std::path::PathBuf;

/// Closed failure set for tool acquisition.
#[derive(Debug, thiserror::Error)]
pub enum ToolAcquireError {
    #[error("invalid tool name: {0}")]
    InvalidName(String),
    #[error("invalid tool spec: {0}")]
    InvalidSpec(String),
    #[error("invalid tool version: {0}")]
    InvalidVersion(String),
    #[error("unknown tool backend: {0}")]
    UnknownBackend(String),
    #[error("tool spec is missing a checksum")]
    ChecksumRequired,
    #[error("tool checksum does not match the payload")]
    ChecksumMismatch,
    #[error("installed version {version} already has a different checksum")]
    VersionConflict { version: String },
    #[error("refusing to follow a link at {}", path.display())]
    LinkRefused { path: PathBuf },
    #[error("payload is not a regular file: {}", path.display())]
    PayloadInvalid { path: PathBuf },
    #[error("staged tool is not a regular executable file")]
    NotExecutable,
    #[error("receipt for {name} does not match the current version")]
    ReceiptDisagrees { name: String },
    #[error("{action} failed at {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tool install task failed: {0}")]
    Task(String),
    #[error("tool asset is missing")]
    AssetMissing,
    #[error("tool asset is ambiguous")]
    AssetAmbiguous,
    #[error("tool download was rejected")]
    DownloadRejected,
    #[error("tool latest version is unavailable")]
    LatestUnavailable,
    #[error("tool runtime is missing: {0}")]
    RuntimeMissing(String),
}

impl ToolAcquireError {
    /// Stable code for logs and host IPC.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidName(_) => "use.tool_acquire.invalid_name",
            Self::InvalidSpec(_) => "use.tool_acquire.invalid_spec",
            Self::InvalidVersion(_) => "use.tool_acquire.invalid_version",
            Self::UnknownBackend(_) => "use.tool_acquire.unknown_backend",
            Self::ChecksumRequired => "use.tool_acquire.checksum_required",
            Self::ChecksumMismatch => "use.tool_acquire.checksum_mismatch",
            Self::VersionConflict { .. } => "use.tool_acquire.version_conflict",
            Self::LinkRefused { .. } => "use.tool_acquire.link_refused",
            Self::PayloadInvalid { .. } => "use.tool_acquire.payload_invalid",
            Self::NotExecutable => "use.tool_acquire.not_executable",
            Self::ReceiptDisagrees { .. } => "use.tool_acquire.receipt_disagrees",
            Self::Io { .. } => "use.tool_acquire.io",
            Self::Task(_) => "use.tool_acquire.task",
            Self::AssetMissing => "use.tool_acquire.asset_missing",
            Self::AssetAmbiguous => "use.tool_acquire.asset_ambiguous",
            Self::DownloadRejected => "use.tool_acquire.download_rejected",
            Self::LatestUnavailable => "use.tool_acquire.latest_unavailable",
            Self::RuntimeMissing(_) => "use.tool_acquire.runtime_missing",
        }
    }

    pub fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
