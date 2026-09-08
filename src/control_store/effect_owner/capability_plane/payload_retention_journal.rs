//! Durable cross-owner phase journal for Capability payload retention.
//!
//! The catalog and descriptor-snapshot owners each have a per-record journal,
//! but those journals cannot answer the coordinator question: did the first
//! owner finish before the process stopped?  This bounded append-only file
//! records the reviewed pair of plans and the catalog-complete checkpoint.
//! Replaying either child is idempotent; the coordinator file is retired only
//! after both owners have reached their reviewed retained sets.

use std::io;
use std::path::{Path, PathBuf};

use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    coordinator_invalid, coordinator_journal_io, coordinator_stale, valid_sha256,
    ControlCapabilityPayloadRetentionPlan, CONTROL_CAPABILITY_PAYLOAD_RETENTION_JOURNAL_SCHEMA,
};
use crate::control_store::{
    CAPABILITY_PAYLOAD_RETENTION_COORDINATOR_JOURNAL,
    CAPABILITY_PAYLOAD_RETENTION_COORDINATOR_JOURNAL_MAX_BYTES,
};

const CAPABILITY_GATEWAY_ROOT: &str = "capability-gateway";
// Leave room for the canonical journal envelope around the coordinator plan,
// whose own bound is eight MiB.
const MAX_JOURNAL_BYTES: u64 = CAPABILITY_PAYLOAD_RETENTION_COORDINATOR_JOURNAL_MAX_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalRecord {
    schema: String,
    sequence: u64,
    state: JournalState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum JournalState {
    Prepared {
        plan: Box<ControlCapabilityPayloadRetentionPlan>,
        plan_digest: String,
    },
    CatalogApplied {
        plan_digest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    CatalogApplied,
}

#[derive(Debug, Clone)]
struct JournalProgress {
    plan: ControlCapabilityPayloadRetentionPlan,
    plan_digest: String,
    phase: Phase,
    next_sequence: u64,
}

/// An opened and replayed coordinator journal.
#[derive(Debug, Clone)]
pub(super) struct RetentionCoordinatorJournal {
    path: PathBuf,
    progress: JournalProgress,
}

impl RetentionCoordinatorJournal {
    /// Check for a syntactically valid pending journal without repairing a
    /// torn tail. Shared owner operations use this read-only probe; recovery
    /// owns the exclusive maintenance fence and performs any repair.
    pub(super) async fn has_pending(root: &Path) -> super::UseResult<bool> {
        let coordinator_root = root.join(CAPABILITY_GATEWAY_ROOT);
        if !validate_optional_root(&coordinator_root).await? {
            return Ok(false);
        }
        let path = coordinator_root.join(CAPABILITY_PAYLOAD_RETENTION_COORDINATOR_JOURNAL);
        let Some(bytes) = read_journal(&path).await? else {
            return Ok(false);
        };
        let _ = replay(&bytes)?;
        Ok(true)
    }

    /// Load and, if necessary, repair one journal. A valid record without a
    /// final newline is completed; an invalid torn tail is truncated to the
    /// last complete record. No destructive owner operation occurs here.
    pub(super) async fn load_unbound(root: &Path) -> super::UseResult<Option<Self>> {
        let coordinator_root = root.join(CAPABILITY_GATEWAY_ROOT);
        if !validate_optional_root(&coordinator_root).await? {
            return Ok(None);
        }
        let path = coordinator_root.join(CAPABILITY_PAYLOAD_RETENTION_COORDINATOR_JOURNAL);
        let Some(bytes) = read_journal(&path).await? else {
            return Ok(None);
        };
        let (progress, complete_len, tail) = replay(&bytes)?;
        match tail {
            Tail::None => {}
            Tail::Valid => append_journal(&path, b"\n").await?,
            Tail::Invalid => truncate_journal(&path, complete_len).await?,
        }
        Ok(Some(Self { path, progress }))
    }

    pub(super) async fn create(
        root: &Path,
        plan: &ControlCapabilityPayloadRetentionPlan,
        plan_digest: &str,
    ) -> super::UseResult<Self> {
        plan.validate()?;
        if !valid_sha256(plan_digest) || plan.descriptor_digest()? != plan_digest {
            return Err(coordinator_stale(
                "The Capability payload retention journal cannot bind a different plan digest.",
            ));
        }
        let coordinator_root = root.join(CAPABILITY_GATEWAY_ROOT);
        ensure_root(&coordinator_root).await?;
        let path = coordinator_root.join(CAPABILITY_PAYLOAD_RETENTION_COORDINATOR_JOURNAL);
        let record = JournalRecord {
            schema: CONTROL_CAPABILITY_PAYLOAD_RETENTION_JOURNAL_SCHEMA.to_owned(),
            sequence: 0,
            state: JournalState::Prepared {
                plan: Box::new(plan.clone()),
                plan_digest: plan_digest.to_owned(),
            },
        };
        let line = encode_record(&record)?;
        create_journal(&path, &line).await?;
        Ok(Self {
            path,
            progress: JournalProgress {
                plan: plan.clone(),
                plan_digest: plan_digest.to_owned(),
                phase: Phase::Prepared,
                next_sequence: 1,
            },
        })
    }

    pub(super) fn plan(&self) -> &ControlCapabilityPayloadRetentionPlan {
        &self.progress.plan
    }

    pub(super) fn plan_digest(&self) -> &str {
        &self.progress.plan_digest
    }

    pub(super) fn is_prepared(&self) -> bool {
        matches!(self.progress.phase, Phase::Prepared)
    }

    pub(super) async fn mark_catalog_applied(&mut self) -> super::UseResult<()> {
        if !self.is_prepared() {
            return Ok(());
        }
        self.append(JournalState::CatalogApplied {
            plan_digest: self.progress.plan_digest.clone(),
        })
        .await
    }

    pub(super) async fn retire(&self) -> super::UseResult<()> {
        let metadata =
            match fs::symlink_metadata(&self.path).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(journal_io(
                    "inspect Capability payload retention coordinator journal before retirement",
                    &self.path,
                    error,
                )),
            };
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator journal is not an owned regular file.",
            ));
        }
        fs::remove_file(&self.path).await.map_err(|error| {
            journal_io(
                "retire Capability payload retention coordinator journal",
                &self.path,
                error,
            )
        })?;
        sync_directory(self.path.parent().ok_or_else(|| {
            coordinator_invalid(
                "The Capability payload retention coordinator journal has no parent.",
            )
        })?)
        .await
    }

    async fn append(&mut self, state: JournalState) -> super::UseResult<()> {
        let record = JournalRecord {
            schema: CONTROL_CAPABILITY_PAYLOAD_RETENTION_JOURNAL_SCHEMA.to_owned(),
            sequence: self.progress.next_sequence,
            state,
        };
        let mut candidate = Some(self.progress.clone());
        apply_record(&mut candidate, &record)?;
        let candidate = candidate.ok_or_else(|| {
            coordinator_invalid(
                "The Capability payload retention coordinator journal transition has no state.",
            )
        })?;
        let line = encode_record(&record)?;
        append_journal(&self.path, &line).await?;
        self.progress = candidate;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    None,
    Valid,
    Invalid,
}

fn replay(bytes: &[u8]) -> super::UseResult<(JournalProgress, u64, Tail)> {
    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index.saturating_add(1))
        .unwrap_or(0);
    let (complete, tail_bytes) = bytes.split_at(complete_len);
    let mut progress = None;
    let mut lines = complete.split(|byte| *byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() {
            if lines.peek().is_none() {
                continue;
            }
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator journal contains an empty record.",
            ));
        }
        let record = decode_record(line)?;
        apply_record(&mut progress, &record)?;
    }
    let tail = if tail_bytes.is_empty() {
        Tail::None
    } else if let Ok(record) = decode_record(tail_bytes) {
        apply_record(&mut progress, &record)?;
        Tail::Valid
    } else {
        Tail::Invalid
    };
    let progress = progress.ok_or_else(|| {
        coordinator_invalid(
            "The Capability payload retention coordinator journal does not contain a prepared record.",
        )
    })?;
    Ok((
        progress,
        u64::try_from(complete_len).map_err(|_| {
            coordinator_invalid(
                "The Capability payload retention coordinator journal offset exceeds the platform range.",
            )
        })?,
        tail,
    ))
}

fn apply_record(
    progress: &mut Option<JournalProgress>,
    record: &JournalRecord,
) -> super::UseResult<()> {
    if record.schema != CONTROL_CAPABILITY_PAYLOAD_RETENTION_JOURNAL_SCHEMA {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal schema is unsupported.",
        ));
    }
    let Some(current) = progress.as_mut() else {
        let JournalState::Prepared { plan, plan_digest } = &record.state else {
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator journal must begin with a prepared record.",
            ));
        };
        if record.sequence != 0 {
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator journal sequence does not begin at zero.",
            ));
        }
        plan.validate()?;
        if !valid_sha256(plan_digest) || plan.descriptor_digest()? != *plan_digest {
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator journal prepared digest is invalid.",
            ));
        }
        *progress = Some(JournalProgress {
            plan: (**plan).clone(),
            plan_digest: plan_digest.clone(),
            phase: Phase::Prepared,
            next_sequence: 1,
        });
        return Ok(());
    };
    if record.sequence != current.next_sequence {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal sequence is not contiguous.",
        ));
    }
    match &record.state {
        JournalState::Prepared { .. } => {
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator journal contains multiple prepared records.",
            ));
        }
        JournalState::CatalogApplied { plan_digest } => {
            if !matches!(current.phase, Phase::Prepared) || plan_digest != &current.plan_digest {
                return Err(coordinator_invalid(
                    "The Capability payload retention coordinator journal catalog checkpoint is invalid.",
                ));
            }
            current.phase = Phase::CatalogApplied;
        }
    }
    current.next_sequence = current.next_sequence.checked_add(1).ok_or_else(|| {
        coordinator_invalid(
            "The Capability payload retention coordinator journal sequence overflowed.",
        )
    })?;
    Ok(())
}

fn encode_record(record: &JournalRecord) -> super::UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    record.serialize(&mut serializer).map_err(|error| {
        coordinator_invalid(format!(
            "The Capability payload retention coordinator journal record cannot be encoded: {error}"
        ))
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal record exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

fn decode_record(bytes: &[u8]) -> super::UseResult<JournalRecord> {
    let record: JournalRecord = serde_json::from_slice(bytes).map_err(|_| {
        coordinator_invalid(
            "The Capability payload retention coordinator journal contains invalid JSON.",
        )
    })?;
    let canonical = encode_record(&record)?;
    if canonical.get(..canonical.len().saturating_sub(1)) != Some(bytes) {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal record is not canonical.",
        ));
    }
    Ok(record)
}

async fn validate_optional_root(root: &Path) -> super::UseResult<bool> {
    match fs::symlink_metadata(root).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(coordinator_invalid(
            "The Capability payload retention coordinator root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(journal_io(
            "inspect Capability payload retention coordinator root",
            root,
            error,
        )),
    }
}

async fn ensure_root(root: &Path) -> super::UseResult<()> {
    if !validate_optional_root(root).await? {
        fs::create_dir_all(root).await.map_err(|error| {
            journal_io(
                "create Capability payload retention coordinator root",
                root,
                error,
            )
        })?;
        if !validate_optional_root(root).await? {
            return Err(coordinator_invalid(
                "The Capability payload retention coordinator root disappeared after creation.",
            ));
        }
    }
    Ok(())
}

async fn read_journal(path: &Path) -> super::UseResult<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(journal_io(
                "inspect Capability payload retention coordinator journal",
                path,
                error,
            ))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal is not a bounded owned regular file.",
        ));
    }
    let before = file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow(&mut options);
    let mut file = options.open(path).await.map_err(|error| {
        journal_io(
            "open Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    let opened = file.metadata().await.map_err(|error| {
        journal_io(
            "inspect opened Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
        || file_identity(&opened) != before
    {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal changed while it was opened.",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_JOURNAL_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            journal_io(
                "read Capability payload retention coordinator journal",
                path,
                error,
            )
        })?;
    let after = fs::symlink_metadata(path).await.map_err(|error| {
        journal_io(
            "reinspect Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || bytes.len() as u64 != opened.len()
    {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal changed while it was read.",
        ));
    }
    Ok(Some(bytes))
}

async fn create_journal(path: &Path, bytes: &[u8]) -> super::UseResult<()> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    configure_no_follow(&mut options);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(path).await.map_err(|error| {
        journal_io(
            "create Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(path).await;
        return Err(journal_io(
            "write Capability payload retention coordinator journal",
            path,
            error,
        ));
    }
    drop(file);
    sync_directory(path.parent().ok_or_else(|| {
        coordinator_invalid("The Capability payload retention coordinator journal has no parent.")
    })?)
    .await
}

async fn append_journal(path: &Path, bytes: &[u8]) -> super::UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        journal_io(
            "inspect Capability payload retention coordinator journal before append",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len().saturating_add(bytes.len() as u64) > MAX_JOURNAL_BYTES
    {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal is not appendable.",
        ));
    }
    let before = file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.append(true).write(true);
    configure_no_follow(&mut options);
    let mut file = options.open(path).await.map_err(|error| {
        journal_io(
            "open Capability payload retention coordinator journal for append",
            path,
            error,
        )
    })?;
    let opened = file.metadata().await.map_err(|error| {
        journal_io(
            "inspect appendable Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || file_identity(&opened) != before
    {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal changed before append.",
        ));
    }
    file.write_all(bytes).await.map_err(|error| {
        journal_io(
            "append Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    file.flush().await.map_err(|error| {
        journal_io(
            "flush Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    file.sync_all().await.map_err(|error| {
        journal_io(
            "sync Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    let after = file.metadata().await.map_err(|error| {
        journal_io(
            "inspect appended Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    if after.len() != metadata.len().saturating_add(bytes.len() as u64) {
        return Err(coordinator_journal_io(format!(
            "Failed to verify Capability payload retention coordinator journal append '{}': length changed unexpectedly",
            path.display()
        )));
    }
    Ok(())
}

async fn truncate_journal(path: &Path, length: u64) -> super::UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        journal_io(
            "inspect Capability payload retention coordinator journal before repair",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || length > metadata.len()
    {
        return Err(coordinator_invalid(
            "The Capability payload retention coordinator journal cannot be repaired safely.",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.write(true);
    configure_no_follow(&mut options);
    let file = options.open(path).await.map_err(|error| {
        journal_io(
            "open Capability payload retention coordinator journal for repair",
            path,
            error,
        )
    })?;
    file.set_len(length).await.map_err(|error| {
        journal_io(
            "truncate Capability payload retention coordinator journal",
            path,
            error,
        )
    })?;
    file.sync_all().await.map_err(|error| {
        journal_io(
            "sync repaired Capability payload retention coordinator journal",
            path,
            error,
        )
    })
}

fn journal_io(action: &str, path: &Path, error: io::Error) -> super::UseError {
    coordinator_journal_io(format!("Failed to {action} '{}': {error}", path.display()))
}

fn configure_no_follow(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> super::UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| {
            journal_io(
                "open Capability payload retention coordinator directory for sync",
                path,
                error,
            )
        })?
        .sync_all()
        .await
        .map_err(|error| {
            journal_io(
                "sync Capability payload retention coordinator directory",
                path,
                error,
            )
        })
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> super::UseResult<()> {
    Ok(())
}
