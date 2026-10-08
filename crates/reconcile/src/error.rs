use std::path::PathBuf;

/// Closed failure set for package reconcile.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("invalid selection id: {0}")]
    InvalidSelectionId(String),
    #[error("selection exceeds its package, surface, receipt or identity bound")]
    SelectionTooLarge,
    #[error("selection repeats package {0}")]
    DuplicateSelectionPackage(String),
    #[error("selection cursor is stale or withdrawn")]
    StaleSelection,
    #[error("unsupported or invalid selection snapshot; clean unsupported selection state and republish")]
    InvalidSelectionSnapshot,
    #[error("package publication generation is exhausted")]
    GenerationExhausted,
    #[error("package publication cursor is stale or withdrawn")]
    StalePublication,
    #[error(
        "unsupported or invalid reconcile snapshot; clean unsupported package state and reinstall"
    )]
    InvalidSnapshot,
    #[error("invalid package id: {0}")]
    InvalidPackageId(String),
    #[error("invalid surface id: {0}")]
    InvalidSurfaceId(String),
    #[error("invalid surface kind: {0}")]
    InvalidSurfaceKind(String),
    #[error("package declares no surfaces")]
    EmptyPackage,
    #[error("duplicate surface {kind} {id}")]
    DuplicateSurface { kind: String, id: String },
    #[error("surface {kind}/{id} is missing an entry path")]
    EntryRequired { kind: String, id: String },
    #[error("invalid relative path: {0}")]
    InvalidPath(String),
    #[error("surface digest does not match the payload")]
    DigestMismatch,
    #[error("skill surface has no SKILL.md")]
    SkillMissing,
    #[error("mcp server entry is invalid")]
    McpInvalid,
    #[error("payload is empty")]
    EmptyPayload,
    #[error("archive has too many entries or bytes")]
    ArchiveTooLarge,
    #[error("surface payload has too many entries or bytes")]
    PayloadTooLarge,
    #[error("archive entry escapes its directory")]
    ArchiveEscape,
    #[error("archive entry is a link")]
    ArchiveLink,
    #[error("archive is not a zip")]
    ArchiveInvalid,
    #[error("refusing to follow a link at {}", path.display())]
    LinkRefused { path: PathBuf },
    #[error("{action} failed at {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("reconcile task failed: {0}")]
    Task(String),
}

impl ReconcileError {
    /// Stable code for logs and host IPC.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidSelectionId(_) => "use.reconcile.invalid_selection_id",
            Self::SelectionTooLarge => "use.reconcile.selection_too_large",
            Self::DuplicateSelectionPackage(_) => "use.reconcile.duplicate_selection_package",
            Self::StaleSelection => "use.reconcile.stale_selection",
            Self::InvalidSelectionSnapshot => "use.reconcile.invalid_selection_snapshot",
            Self::GenerationExhausted => "use.reconcile.generation_exhausted",
            Self::StalePublication => "use.reconcile.stale_publication",
            Self::InvalidSnapshot => "use.reconcile.invalid_snapshot",
            Self::InvalidPackageId(_) => "use.reconcile.invalid_package_id",
            Self::InvalidSurfaceId(_) => "use.reconcile.invalid_surface_id",
            Self::InvalidSurfaceKind(_) => "use.reconcile.invalid_surface_kind",
            Self::EmptyPackage => "use.reconcile.empty_package",
            Self::DuplicateSurface { .. } => "use.reconcile.duplicate_surface",
            Self::EntryRequired { .. } => "use.reconcile.entry_required",
            Self::InvalidPath(_) => "use.reconcile.invalid_path",
            Self::DigestMismatch => "use.reconcile.digest_mismatch",
            Self::SkillMissing => "use.reconcile.skill_missing",
            Self::McpInvalid => "use.reconcile.mcp_invalid",
            Self::EmptyPayload => "use.reconcile.empty_payload",
            Self::ArchiveTooLarge => "use.reconcile.archive_too_large",
            Self::PayloadTooLarge => "use.reconcile.payload_too_large",
            Self::ArchiveEscape => "use.reconcile.archive_escape",
            Self::ArchiveLink => "use.reconcile.archive_link",
            Self::ArchiveInvalid => "use.reconcile.archive_invalid",
            Self::LinkRefused { .. } => "use.reconcile.link_refused",
            Self::Io { .. } => "use.reconcile.io",
            Self::Task(_) => "use.reconcile.task",
        }
    }

    pub(crate) fn io(action: &'static str, path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}
