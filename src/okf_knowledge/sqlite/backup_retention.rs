use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path};

use a3s_use_core::{PlanScope, UseError, UseResult};
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::backup;

pub const OKF_KNOWLEDGE_BACKUP_RETENTION_PLAN_SCHEMA: &str =
    "a3s.use.okf-knowledge-backup-retention-plan.v1";
pub const OKF_KNOWLEDGE_BACKUP_RETENTION_RESULT_SCHEMA: &str =
    "a3s.use.okf-knowledge-backup-retention-result.v1";
pub const DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BACKUPS: u64 = 32;
pub const DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BYTES: u64 = 256 * 1024 * 1024 * 1024;
pub const MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BACKUPS: u64 = 4_096;
pub const MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BYTES: u64 = 256 * 1024 * 1024 * 1024 * 1024;

const BACKUP_SUFFIX: &str = ".a3s-okf-backup";
const DIRECTORY_LOCK_NAME: &str = ".a3s-okf-backup-retention.lock";
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_BACKUP_FILE_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeBackupRetentionPolicy {
    pub max_backups: u64,
    pub max_bytes: u64,
}

impl OkfKnowledgeBackupRetentionPolicy {
    pub fn new(max_backups: u64, max_bytes: u64) -> UseResult<Self> {
        let policy = Self {
            max_backups,
            max_bytes,
        };
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> UseResult<()> {
        if self.max_backups == 0
            || self.max_backups > MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BACKUPS
            || self.max_bytes == 0
            || self.max_bytes > MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BYTES
        {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_policy_invalid",
                format!(
                    "Knowledge backup retention requires 1..={MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BACKUPS} backups and 1..={MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BYTES} bytes."
                ),
            ));
        }
        Ok(())
    }
}

impl Default for OkfKnowledgeBackupRetentionPolicy {
    fn default() -> Self {
        Self {
            max_backups: DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BACKUPS,
            max_bytes: DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeBackupRetentionEntry {
    pub file_name: String,
    pub created_at_ms: u64,
    pub archive_bytes: u64,
    pub manifest_digest: String,
    pub database_bytes: u64,
    pub database_sha256: String,
}

impl OkfKnowledgeBackupRetentionEntry {
    fn validate(&self) -> UseResult<()> {
        let path = Path::new(&self.file_name);
        if self.file_name.is_empty()
            || self.file_name.len() > MAX_BACKUP_FILE_NAME_BYTES
            || !self.file_name.ends_with(BACKUP_SUFFIX)
            || !matches!(
                path.components().collect::<Vec<_>>().as_slice(),
                [Component::Normal(_)]
            )
            || self.created_at_ms == 0
            || self.archive_bytes == 0
            || self.database_bytes == 0
            || self.database_bytes > backup::MAX_BACKUP_DATABASE_BYTES
            || self.archive_bytes <= self.database_bytes
            || !valid_sha256(&self.manifest_digest)
            || !valid_sha256(&self.database_sha256)
        {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                "A Knowledge backup retention entry is invalid or exceeds its safety bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeBackupRetentionPlan {
    pub schema: String,
    pub scope: PlanScope,
    pub policy: OkfKnowledgeBackupRetentionPolicy,
    pub before_backup_count: u64,
    pub before_archive_bytes: u64,
    pub remove: Vec<OkfKnowledgeBackupRetentionEntry>,
    pub retain: Vec<OkfKnowledgeBackupRetentionEntry>,
    pub retained_backup_count: u64,
    pub retained_archive_bytes: u64,
}

impl OkfKnowledgeBackupRetentionPlan {
    fn new(
        scope: PlanScope,
        policy: OkfKnowledgeBackupRetentionPolicy,
        inventory: Vec<OkfKnowledgeBackupRetentionEntry>,
    ) -> UseResult<Self> {
        policy.validate()?;
        let before_backup_count = count(&inventory)?;
        let before_archive_bytes = total_bytes(&inventory)?;
        let (remove, retain) = partition_inventory(inventory, policy)?;
        let plan = Self {
            schema: OKF_KNOWLEDGE_BACKUP_RETENTION_PLAN_SCHEMA.to_owned(),
            scope,
            policy,
            before_backup_count,
            before_archive_bytes,
            retained_backup_count: count(&retain)?,
            retained_archive_bytes: total_bytes(&retain)?,
            remove,
            retain,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.policy.validate()?;
        if self.schema != OKF_KNOWLEDGE_BACKUP_RETENTION_PLAN_SCHEMA
            || !super::valid_machine_id(&self.scope.id)
        {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                "The Knowledge backup retention plan schema or scope is invalid.",
            ));
        }
        let mut inventory = self.remove.clone();
        inventory.extend(self.retain.clone());
        validate_inventory(&inventory)?;
        let (expected_remove, expected_retain) =
            partition_inventory(inventory.clone(), self.policy)?;
        if self.before_backup_count != count(&inventory)?
            || self.before_archive_bytes != total_bytes(&inventory)?
            || self.remove != expected_remove
            || self.retain != expected_retain
            || self.retained_backup_count != count(&self.retain)?
            || self.retained_archive_bytes != total_bytes(&self.retain)?
        {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                "The Knowledge backup retention plan inventory or accounting is inconsistent.",
            ));
        }
        Ok(())
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                format!("Failed to encode the canonical Knowledge backup retention plan: {error}"),
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeBackupRetentionResult {
    pub schema: String,
    pub scope: PlanScope,
    pub plan_digest: String,
    pub changed: bool,
    pub removed: Vec<OkfKnowledgeBackupRetentionEntry>,
    pub retained_backup_count: u64,
    pub retained_archive_bytes: u64,
}

pub(super) struct BackupDirectoryLock {
    file: File,
}

impl BackupDirectoryLock {
    pub(super) fn acquire(directory: &Path) -> UseResult<Self> {
        validate_directory(directory)?;
        let lock_path = directory.join(DIRECTORY_LOCK_NAME);
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&lock_path).map_err(|error| {
                    retention_io(format!(
                        "Failed to inspect Knowledge backup retention lock '{}': {error}",
                        lock_path.display()
                    ))
                })?;
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file()
                {
                    return Err(retention_error(
                        "use.okf.knowledge_backup_retention_directory_invalid",
                        "The Knowledge backup retention lock is not an owned regular file.",
                    ));
                }
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&lock_path)
                    .map_err(|error| {
                        retention_io(format!(
                            "Failed to open Knowledge backup retention lock '{}': {error}",
                            lock_path.display()
                        ))
                    })?
            }
            Err(error) => {
                return Err(retention_io(format!(
                    "Failed to create Knowledge backup retention lock '{}': {error}",
                    lock_path.display()
                )))
            }
        };
        file.try_lock_exclusive().map_err(|error| {
            retention_error(
                "use.okf.knowledge_backup_retention_busy",
                format!("Another Knowledge backup or retention operation owns the directory lock: {error}"),
            )
        })?;
        validate_directory(directory)?;
        Ok(Self { file })
    }
}

impl Drop for BackupDirectoryLock {
    fn drop(&mut self) {
        // Close also releases advisory locks, but an explicit unlock makes the
        // handoff deterministic before a following operation opens the file.
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) fn plan(
    directory: &Path,
    scope: &PlanScope,
    policy: OkfKnowledgeBackupRetentionPolicy,
) -> UseResult<OkfKnowledgeBackupRetentionPlan> {
    let _lock = BackupDirectoryLock::acquire(directory)?;
    build_plan(directory, scope, policy)
}

pub(super) fn apply(
    directory: &Path,
    scope: &PlanScope,
    policy: OkfKnowledgeBackupRetentionPolicy,
    expected_plan_digest: &str,
) -> UseResult<OkfKnowledgeBackupRetentionResult> {
    if !valid_sha256(expected_plan_digest) {
        return Err(plan_mismatch(
            "Knowledge backup retention requires an exact canonical SHA-256 plan digest.",
        ));
    }
    let _lock = BackupDirectoryLock::acquire(directory)?;
    let current = build_plan(directory, scope, policy)?;
    let actual_digest = current.descriptor_digest()?;
    if actual_digest != expected_plan_digest {
        return Err(plan_mismatch(
            "The Knowledge backup directory changed after review; create and confirm a new retention plan.",
        ));
    }

    let mut removed = Vec::new();
    for entry in &current.remove {
        let path = directory.join(&entry.file_name);
        let actual = inspect_entry(&path, scope)?.ok_or_else(|| {
            plan_mismatch("A reviewed Knowledge backup no longer belongs to the selected scope.")
        })?;
        if &actual != entry {
            return Err(plan_mismatch(
                "A reviewed Knowledge backup changed before retention apply.",
            ));
        }
        if let Err(error) = fs::remove_file(&path) {
            return Err(retention_outcome_unknown(
                format!(
                    "Failed to remove reviewed Knowledge backup '{}': {error}",
                    entry.file_name
                ),
                &removed,
            ));
        }
        removed.push(entry.clone());
    }
    if !removed.is_empty() {
        sync_directory(directory).map_err(|error| {
            retention_outcome_unknown(
                format!(
                    "Knowledge backups were removed, but directory durability could not be confirmed: {}",
                    error.message
                ),
                &removed,
            )
        })?;
    }
    let after = inventory(directory, scope).map_err(|error| {
        retention_outcome_unknown(
            format!(
                "Knowledge backup retention changed files, but the retained inventory could not be verified: {}",
                error.message
            ),
            &removed,
        )
    })?;
    if after != current.retain {
        return Err(retention_outcome_unknown(
            "The Knowledge backup directory changed during retention apply.",
            &removed,
        ));
    }
    Ok(OkfKnowledgeBackupRetentionResult {
        schema: OKF_KNOWLEDGE_BACKUP_RETENTION_RESULT_SCHEMA.to_owned(),
        scope: scope.clone(),
        plan_digest: actual_digest,
        changed: !removed.is_empty(),
        removed,
        retained_backup_count: count(&after)?,
        retained_archive_bytes: total_bytes(&after)?,
    })
}

fn build_plan(
    directory: &Path,
    scope: &PlanScope,
    policy: OkfKnowledgeBackupRetentionPolicy,
) -> UseResult<OkfKnowledgeBackupRetentionPlan> {
    policy.validate()?;
    if !super::valid_machine_id(&scope.id) {
        return Err(retention_error(
            "use.okf.knowledge_backup_retention_scope_invalid",
            "Knowledge backup retention requires one valid complete User or Workspace scope.",
        ));
    }
    OkfKnowledgeBackupRetentionPlan::new(scope.clone(), policy, inventory(directory, scope)?)
}

fn inventory(
    directory: &Path,
    scope: &PlanScope,
) -> UseResult<Vec<OkfKnowledgeBackupRetentionEntry>> {
    validate_directory(directory)?;
    let mut entries = Vec::new();
    let mut inspected = 0_usize;
    for entry in fs::read_dir(directory).map_err(|error| {
        retention_io(format!(
            "Failed to list Knowledge backup directory '{}': {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            retention_io(format!(
                "Failed to read a Knowledge backup directory entry: {error}"
            ))
        })?;
        let file_name = match entry.file_name().into_string() {
            Ok(file_name) => file_name,
            Err(_) => continue,
        };
        if file_name == DIRECTORY_LOCK_NAME {
            continue;
        }
        inspected = inspected.checked_add(1).ok_or_else(|| {
            retention_error(
                "use.okf.knowledge_backup_retention_directory_invalid",
                "The Knowledge backup directory entry count overflowed.",
            )
        })?;
        if inspected > MAX_DIRECTORY_ENTRIES {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_directory_invalid",
                format!(
                    "The Knowledge backup directory exceeds the {MAX_DIRECTORY_ENTRIES}-entry inspection bound."
                ),
            ));
        }
        if !file_name.ends_with(BACKUP_SUFFIX) {
            continue;
        }
        if let Some(backup) = inspect_entry(&entry.path(), scope)? {
            entries.push(backup);
        }
    }
    entries.sort_by(entry_order);
    validate_inventory(&entries)?;
    Ok(entries)
}

fn inspect_entry(
    path: &Path,
    scope: &PlanScope,
) -> UseResult<Option<OkfKnowledgeBackupRetentionEntry>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        retention_io(format!(
            "Failed to inspect Knowledge backup candidate '{}': {error}",
            path.display()
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(retention_error(
            "use.okf.knowledge_backup_retention_directory_invalid",
            "A managed Knowledge backup candidate is not an owned regular file.",
        ));
    }
    let manifest = backup::verify(path, None)?;
    if &manifest.scope != scope {
        return Ok(None);
    }
    let manifest_digest = canonical_digest(&manifest)?;
    let entry = OkfKnowledgeBackupRetentionEntry {
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                retention_error(
                    "use.okf.knowledge_backup_retention_directory_invalid",
                    "A managed Knowledge backup file name is not valid UTF-8.",
                )
            })?
            .to_owned(),
        created_at_ms: manifest.created_at_ms,
        archive_bytes: metadata.len(),
        manifest_digest,
        database_bytes: manifest.database_bytes,
        database_sha256: manifest.database_sha256,
    };
    entry.validate()?;
    Ok(Some(entry))
}

fn partition_inventory(
    inventory: Vec<OkfKnowledgeBackupRetentionEntry>,
    policy: OkfKnowledgeBackupRetentionPolicy,
) -> UseResult<(
    Vec<OkfKnowledgeBackupRetentionEntry>,
    Vec<OkfKnowledgeBackupRetentionEntry>,
)> {
    policy.validate()?;
    validate_inventory(&inventory)?;
    let mut retained_count = count(&inventory)?;
    let mut retained_bytes = total_bytes(&inventory)?;
    let mut remove_count = 0_usize;
    while retained_count > policy.max_backups || retained_bytes > policy.max_bytes {
        if retained_count <= 1 {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_policy_unsatisfied",
                "The newest Knowledge backup alone exceeds the byte policy; retention never removes the last verified scope backup.",
            ));
        }
        let entry = inventory.get(remove_count).ok_or_else(|| {
            retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                "The Knowledge backup retention partition exceeded its inventory.",
            )
        })?;
        retained_count -= 1;
        retained_bytes = retained_bytes
            .checked_sub(entry.archive_bytes)
            .ok_or_else(|| {
                retention_error(
                    "use.okf.knowledge_backup_retention_plan_invalid",
                    "The Knowledge backup retention byte accounting underflowed.",
                )
            })?;
        remove_count += 1;
    }
    let mut inventory = inventory;
    let retain = inventory.split_off(remove_count);
    Ok((inventory, retain))
}

fn validate_inventory(entries: &[OkfKnowledgeBackupRetentionEntry]) -> UseResult<()> {
    if entries.len() > MAX_DIRECTORY_ENTRIES {
        return Err(retention_error(
            "use.okf.knowledge_backup_retention_plan_invalid",
            "The Knowledge backup retention inventory exceeds its entry bound.",
        ));
    }
    let mut names = BTreeSet::new();
    for entry in entries {
        entry.validate()?;
        if !names.insert(entry.file_name.as_str()) {
            return Err(retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                "The Knowledge backup retention inventory repeats a file name.",
            ));
        }
    }
    if !entries
        .windows(2)
        .all(|pair| entry_order(&pair[0], &pair[1]).is_le())
    {
        return Err(retention_error(
            "use.okf.knowledge_backup_retention_plan_invalid",
            "The Knowledge backup retention inventory is not canonically ordered.",
        ));
    }
    total_bytes(entries)?;
    Ok(())
}

fn entry_order(
    left: &OkfKnowledgeBackupRetentionEntry,
    right: &OkfKnowledgeBackupRetentionEntry,
) -> std::cmp::Ordering {
    (left.created_at_ms, left.file_name.as_str())
        .cmp(&(right.created_at_ms, right.file_name.as_str()))
}

fn count(entries: &[OkfKnowledgeBackupRetentionEntry]) -> UseResult<u64> {
    u64::try_from(entries.len()).map_err(|_| {
        retention_error(
            "use.okf.knowledge_backup_retention_plan_invalid",
            "The Knowledge backup retention entry count exceeds the platform range.",
        )
    })
}

fn total_bytes(entries: &[OkfKnowledgeBackupRetentionEntry]) -> UseResult<u64> {
    entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.archive_bytes).ok_or_else(|| {
            retention_error(
                "use.okf.knowledge_backup_retention_plan_invalid",
                "The Knowledge backup retention byte accounting overflowed.",
            )
        })
    })
}

fn canonical_digest(value: &impl Serialize) -> UseResult<String> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        retention_error(
            "use.okf.knowledge_backup_retention_directory_invalid",
            format!("Failed to encode canonical Knowledge backup evidence: {error}"),
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn validate_directory(directory: &Path) -> UseResult<()> {
    if directory.as_os_str().is_empty() {
        return Err(retention_error(
            "use.okf.knowledge_backup_retention_directory_invalid",
            "Knowledge backup retention requires an explicit directory.",
        ));
    }
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        retention_io(format!(
            "Failed to inspect Knowledge backup directory '{}': {error}",
            directory.display()
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(retention_error(
            "use.okf.knowledge_backup_retention_directory_invalid",
            "The Knowledge backup retention path is not an owned directory.",
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn plan_mismatch(message: impl Into<String>) -> UseError {
    retention_error("use.okf.knowledge_backup_retention_plan_mismatch", message)
}

fn retention_outcome_unknown(
    message: impl Into<String>,
    removed: &[OkfKnowledgeBackupRetentionEntry],
) -> UseError {
    retention_error(
        "use.okf.knowledge_backup_retention_outcome_unknown",
        message,
    )
    .with_detail("removedBackups", serde_json::json!(removed))
    .with_suggestion(
        "Inspect the directory, verify every remaining backup, and create a new retention plan; never recreate or overwrite a removed backup path by assumption.",
    )
}

fn retention_io(message: impl Into<String>) -> UseError {
    retention_error("use.okf.knowledge_backup_retention_io", message)
}

fn retention_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> UseResult<()> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            retention_io(format!(
                "Failed to sync Knowledge backup directory '{}': {error}",
                directory.display()
            ))
        })
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> UseResult<()> {
    Ok(())
}
