//! Crash-recoverable progress journal for catalog retention.
//!
//! Retention removes immutable files one at a time.  A single snapshot marker
//! cannot tell whether a process stopped before or after the last unlink, so
//! this module uses a bounded, append-only journal.  Every record is canonical
//! JSON and is fsynced before the corresponding destructive step.  A torn final
//! record is repaired on the next open; a complete but non-canonical record is
//! rejected.

use std::io;
use std::path::{Path, PathBuf};

use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    CapabilityGatewayCatalogRetentionEntry, CapabilityGatewayCatalogRetentionPlan,
    CAPABILITY_GATEWAY_CATALOG_RETENTION_JOURNAL_SCHEMA, CATALOG_RETENTION_JOURNAL,
    MAX_RETENTION_JOURNAL_BYTES,
};

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
        plan: CapabilityGatewayCatalogRetentionPlan,
        plan_digest: String,
    },
    Removing {
        index: u64,
        digest: String,
    },
    Removed {
        index: u64,
        digest: String,
    },
    Completed {
        removed_count: u64,
    },
}

#[derive(Debug, Clone)]
struct JournalProgress {
    plan: CapabilityGatewayCatalogRetentionPlan,
    plan_digest: String,
    removed_count: usize,
    in_flight: Option<usize>,
    completed: bool,
    next_sequence: u64,
}

/// An opened retention journal with validated progress.
#[derive(Debug, Clone)]
pub(super) struct RetentionJournal {
    path: PathBuf,
    progress: JournalProgress,
}

impl RetentionJournal {
    pub(super) async fn load_unbound(root: &Path) -> super::UseResult<Option<Self>> {
        let path = root.join(CATALOG_RETENTION_JOURNAL);
        let Some(bytes) = read_journal(&path).await? else {
            return Ok(None);
        };
        let (progress, complete_len, tail) = replay(&bytes)?;
        match tail {
            Tail::None => {}
            Tail::Valid => append_newline(&path).await?,
            Tail::Invalid => truncate_journal(&path, complete_len).await?,
        }
        Ok(Some(Self { path, progress }))
    }

    pub(super) async fn load(
        root: &Path,
        plan: &CapabilityGatewayCatalogRetentionPlan,
        plan_digest: &str,
    ) -> super::UseResult<Option<Self>> {
        let Some(journal) = Self::load_unbound(root).await? else {
            return Ok(None);
        };
        let progress = &journal.progress;
        if progress.plan != *plan || progress.plan_digest != plan_digest {
            return Err(super::retention_stale(
                "A durable catalog-retention journal belongs to another plan.",
            ));
        }
        // Revalidate the caller's exact binding after replay.  This also
        // protects against a theoretically valid journal carrying a malformed
        // plan digest representation.
        if progress.plan.descriptor_digest()? != plan_digest {
            return Err(super::retention_stale(
                "The durable catalog-retention journal plan digest is stale.",
            ));
        }
        Ok(Some(journal))
    }

    pub(super) async fn create(
        root: &Path,
        plan: &CapabilityGatewayCatalogRetentionPlan,
        plan_digest: &str,
    ) -> super::UseResult<Self> {
        if plan.descriptor_digest()? != plan_digest {
            return Err(super::retention_stale(
                "The durable catalog-retention journal cannot bind a different plan digest.",
            ));
        }
        let path = root.join(CATALOG_RETENTION_JOURNAL);
        let record = JournalRecord {
            schema: CAPABILITY_GATEWAY_CATALOG_RETENTION_JOURNAL_SCHEMA.to_owned(),
            sequence: 0,
            state: JournalState::Prepared {
                plan: plan.clone(),
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
                removed_count: 0,
                in_flight: None,
                completed: false,
                next_sequence: 1,
            },
        })
    }

    pub(super) fn plan(&self) -> &CapabilityGatewayCatalogRetentionPlan {
        &self.progress.plan
    }

    pub(super) fn plan_digest(&self) -> &str {
        &self.progress.plan_digest
    }

    pub(super) fn removed_count(&self) -> usize {
        self.progress.removed_count
    }

    pub(super) fn in_flight(&self) -> Option<usize> {
        self.progress.in_flight
    }

    pub(super) fn is_completed(&self) -> bool {
        self.progress.completed
    }

    pub(super) fn next_index(&self) -> Option<usize> {
        if self.progress.completed || self.progress.removed_count >= self.progress.plan.remove.len()
        {
            None
        } else {
            Some(self.progress.removed_count)
        }
    }

    pub(super) fn removed_entries(&self) -> Vec<CapabilityGatewayCatalogRetentionEntry> {
        self.progress
            .plan
            .remove
            .get(..self.progress.removed_count)
            .unwrap_or_default()
            .to_vec()
    }

    /// Return the inventory that must be present for the current checkpoint.
    pub(super) fn expected_entries(&self) -> Vec<CapabilityGatewayCatalogRetentionEntry> {
        let mut entries = self
            .progress
            .plan
            .remove
            .get(self.progress.removed_count..)
            .unwrap_or_default()
            .to_vec();
        entries.extend(self.progress.plan.retain.iter().cloned());
        entries.sort_by(|left, right| left.digest.cmp(&right.digest));
        entries
    }

    /// Return the expected inventory after the in-flight unlink completed.
    pub(super) fn expected_entries_after_in_flight(
        &self,
    ) -> Option<Vec<CapabilityGatewayCatalogRetentionEntry>> {
        let index = self.progress.in_flight?;
        let mut entries = self.expected_entries();
        let digest = self.progress.plan.remove.get(index)?.digest.as_str();
        entries.retain(|entry| entry.digest != digest);
        Some(entries)
    }

    pub(super) async fn begin_removal(&mut self, index: usize) -> super::UseResult<()> {
        let entry = self.progress.plan.remove.get(index).ok_or_else(|| {
            super::retention_invalid("The retention journal removal index is out of bounds.")
        })?;
        self.append(JournalState::Removing {
            index: u64::try_from(index).map_err(|_| {
                super::retention_invalid(
                    "The retention journal removal index exceeds the platform range.",
                )
            })?,
            digest: entry.digest.clone(),
        })
        .await
    }

    pub(super) async fn mark_removed(&mut self, index: usize) -> super::UseResult<()> {
        let entry = self.progress.plan.remove.get(index).ok_or_else(|| {
            super::retention_invalid("The retention journal removal index is out of bounds.")
        })?;
        self.append(JournalState::Removed {
            index: u64::try_from(index).map_err(|_| {
                super::retention_invalid(
                    "The retention journal removal index exceeds the platform range.",
                )
            })?,
            digest: entry.digest.clone(),
        })
        .await
    }

    pub(super) async fn complete(&mut self) -> super::UseResult<()> {
        self.append(JournalState::Completed {
            removed_count: u64::try_from(self.progress.plan.remove.len()).map_err(|_| {
                super::retention_invalid(
                    "The retention journal removal count exceeds the platform range.",
                )
            })?,
        })
        .await
    }

    pub(super) async fn retire(&self) -> super::UseResult<()> {
        let metadata = match fs::symlink_metadata(&self.path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(journal_io(
                    "inspect retention journal before retirement",
                    &self.path,
                    error,
                ))
            }
        };
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
            return Err(super::retention_invalid(
                "The catalog-retention journal is not an owned regular file.",
            ));
        }
        fs::remove_file(&self.path)
            .await
            .map_err(|error| journal_io("retire catalog-retention journal", &self.path, error))?;
        let parent = self.path.parent().ok_or_else(|| {
            super::retention_invalid("The catalog-retention journal has no parent.")
        })?;
        super::super::sync_directory(parent).await
    }

    async fn append(&mut self, state: JournalState) -> super::UseResult<()> {
        let record = JournalRecord {
            schema: CAPABILITY_GATEWAY_CATALOG_RETENTION_JOURNAL_SCHEMA.to_owned(),
            sequence: self.progress.next_sequence,
            state,
        };
        let mut candidate = self.progress.clone();
        let mut candidate_option = Some(candidate.clone());
        apply_record(&mut candidate_option, &record)?;
        candidate = candidate_option.ok_or_else(|| {
            super::retention_invalid("The catalog-retention journal transition has no state.")
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
            // Only the final split item after a trailing newline is empty.
            if lines.peek().is_none() {
                continue;
            }
            return Err(super::retention_invalid(
                "The catalog-retention journal contains an empty record.",
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
        super::retention_invalid(
            "The catalog-retention journal does not contain a complete prepared record.",
        )
    })?;
    Ok((
        progress,
        u64::try_from(complete_len).map_err(|_| {
            super::retention_invalid(
                "The catalog-retention journal offset exceeds the platform range.",
            )
        })?,
        tail,
    ))
}

fn apply_record(
    progress: &mut Option<JournalProgress>,
    record: &JournalRecord,
) -> super::UseResult<()> {
    if record.schema != CAPABILITY_GATEWAY_CATALOG_RETENTION_JOURNAL_SCHEMA {
        return Err(super::retention_invalid(
            "The catalog-retention journal schema is unsupported.",
        ));
    }
    let Some(current) = progress.as_mut() else {
        let JournalState::Prepared { plan, plan_digest } = &record.state else {
            return Err(super::retention_invalid(
                "The catalog-retention journal must begin with a prepared record.",
            ));
        };
        if record.sequence != 0 {
            return Err(super::retention_invalid(
                "The catalog-retention journal sequence does not begin at zero.",
            ));
        }
        plan.validate()?;
        super::super::validate_digest(plan_digest)?;
        if plan.descriptor_digest()? != *plan_digest {
            return Err(super::retention_invalid(
                "The catalog-retention journal prepared digest is invalid.",
            ));
        }
        *progress = Some(JournalProgress {
            plan: plan.clone(),
            plan_digest: plan_digest.clone(),
            removed_count: 0,
            in_flight: None,
            completed: false,
            next_sequence: 1,
        });
        return Ok(());
    };
    if record.sequence != current.next_sequence {
        return Err(super::retention_invalid(
            "The catalog-retention journal sequence is not contiguous.",
        ));
    }
    if current.completed {
        return Err(super::retention_invalid(
            "The catalog-retention journal has records after completion.",
        ));
    }
    match &record.state {
        JournalState::Prepared { .. } => {
            return Err(super::retention_invalid(
                "The catalog-retention journal contains multiple prepared records.",
            ));
        }
        JournalState::Removing { index, digest } => {
            if current.in_flight.is_some() {
                return Err(super::retention_invalid(
                    "The catalog-retention journal contains overlapping removals.",
                ));
            }
            let index = checked_index(*index, current.plan.remove.len())?;
            let expected = current.plan.remove.get(index).ok_or_else(|| {
                super::retention_invalid("The catalog-retention journal removal index is invalid.")
            })?;
            if index != current.removed_count || digest != &expected.digest {
                return Err(super::retention_invalid(
                    "The catalog-retention journal removal order is invalid.",
                ));
            }
            current.in_flight = Some(index);
        }
        JournalState::Removed { index, digest } => {
            let index = checked_index(*index, current.plan.remove.len())?;
            let expected = current.plan.remove.get(index).ok_or_else(|| {
                super::retention_invalid("The catalog-retention journal removal index is invalid.")
            })?;
            if current.in_flight != Some(index) || digest != &expected.digest {
                return Err(super::retention_invalid(
                    "The catalog-retention journal completion order is invalid.",
                ));
            }
            current.in_flight = None;
            current.removed_count = current.removed_count.saturating_add(1);
        }
        JournalState::Completed { removed_count } => {
            let removed_count = checked_index(*removed_count, current.plan.remove.len())?;
            if current.in_flight.is_some()
                || current.removed_count != current.plan.remove.len()
                || removed_count != current.removed_count
            {
                return Err(super::retention_invalid(
                    "The catalog-retention journal completed before every removal was recorded.",
                ));
            }
            current.completed = true;
        }
    }
    current.next_sequence = current.next_sequence.checked_add(1).ok_or_else(|| {
        super::retention_invalid("The catalog-retention journal sequence overflowed.")
    })?;
    Ok(())
}

fn checked_index(value: u64, bound: usize) -> super::UseResult<usize> {
    let index = usize::try_from(value).map_err(|_| {
        super::retention_invalid("The catalog-retention journal index exceeds the platform range.")
    })?;
    if index > bound {
        return Err(super::retention_invalid(
            "The catalog-retention journal index exceeds the plan bound.",
        ));
    }
    Ok(index)
}

fn encode_record(record: &JournalRecord) -> super::UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    record.serialize(&mut serializer).map_err(|error| {
        super::retention_invalid(format!(
            "The catalog-retention journal record cannot be encoded: {error}"
        ))
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_RETENTION_JOURNAL_BYTES {
        return Err(super::retention_invalid(
            "The catalog-retention journal record exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

fn decode_record(bytes: &[u8]) -> super::UseResult<JournalRecord> {
    let record: JournalRecord = serde_json::from_slice(bytes).map_err(|_| {
        super::retention_invalid("The catalog-retention journal contains invalid JSON.")
    })?;
    let canonical = encode_record(&record)?;
    if canonical.get(..canonical.len().saturating_sub(1)) != Some(bytes) {
        return Err(super::retention_invalid(
            "The catalog-retention journal record is not canonical.",
        ));
    }
    Ok(record)
}

async fn read_journal(path: &Path) -> super::UseResult<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(journal_io("inspect catalog-retention journal", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RETENTION_JOURNAL_BYTES
    {
        return Err(super::retention_invalid(
            "The catalog-retention journal is not a bounded owned regular file.",
        ));
    }
    let before = super::super::file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    super::super::configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| journal_io("open catalog-retention journal", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| journal_io("inspect opened catalog-retention journal", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
        || super::super::file_identity(&opened) != before
    {
        return Err(super::retention_invalid(
            "The catalog-retention journal changed while it was opened.",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_RETENTION_JOURNAL_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| journal_io("read catalog-retention journal", path, error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| journal_io("reinspect catalog-retention journal", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || super::super::file_identity(&after) != before
        || bytes.len() as u64 != opened.len()
    {
        return Err(super::retention_invalid(
            "The catalog-retention journal changed while it was read.",
        ));
    }
    Ok(Some(bytes))
}

async fn create_journal(path: &Path, bytes: &[u8]) -> super::UseResult<()> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    super::super::configure_no_follow_async(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| journal_io("create catalog-retention journal", path, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(path).await;
        return Err(journal_io("write catalog-retention journal", path, error));
    }
    drop(file);
    super::super::sync_directory(
        path.parent().ok_or_else(|| {
            super::retention_invalid("The catalog-retention journal has no parent.")
        })?,
    )
    .await
}

async fn append_journal(path: &Path, bytes: &[u8]) -> super::UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        journal_io(
            "inspect catalog-retention journal before append",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len().saturating_add(bytes.len() as u64) > MAX_RETENTION_JOURNAL_BYTES
    {
        return Err(super::retention_invalid(
            "The catalog-retention journal is not an appendable bounded regular file.",
        ));
    }
    let before = super::super::file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.append(true).write(true);
    super::super::configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| journal_io("open catalog-retention journal for append", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| journal_io("inspect appendable catalog-retention journal", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || super::super::file_identity(&opened) != before
    {
        return Err(super::retention_invalid(
            "The catalog-retention journal changed before append.",
        ));
    }
    file.write_all(bytes)
        .await
        .map_err(|error| journal_io("append catalog-retention journal", path, error))?;
    file.flush()
        .await
        .map_err(|error| journal_io("flush catalog-retention journal", path, error))?;
    file.sync_all()
        .await
        .map_err(|error| journal_io("sync catalog-retention journal", path, error))?;
    let after = file
        .metadata()
        .await
        .map_err(|error| journal_io("inspect appended catalog-retention journal", path, error))?;
    if after.len() != metadata.len().saturating_add(bytes.len() as u64) {
        return Err(journal_io(
            "verify catalog-retention journal append",
            path,
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "journal length changed unexpectedly",
            ),
        ));
    }
    Ok(())
}

async fn append_newline(path: &Path) -> super::UseResult<()> {
    append_journal(path, b"\n").await
}

async fn truncate_journal(path: &Path, length: u64) -> super::UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        journal_io(
            "inspect catalog-retention journal before repair",
            path,
            error,
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || length > metadata.len()
    {
        return Err(super::retention_invalid(
            "The catalog-retention journal cannot be repaired safely.",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.write(true);
    super::super::configure_no_follow_async(&mut options);
    let file = options
        .open(path)
        .await
        .map_err(|error| journal_io("open catalog-retention journal for repair", path, error))?;
    file.set_len(length)
        .await
        .map_err(|error| journal_io("truncate torn catalog-retention journal", path, error))?;
    file.sync_all()
        .await
        .map_err(|error| journal_io("sync repaired catalog-retention journal", path, error))
}

fn journal_io(action: &str, path: &Path, error: io::Error) -> super::UseError {
    super::retention_journal_io(format!("Failed to {action} '{}': {error}", path.display()))
}
