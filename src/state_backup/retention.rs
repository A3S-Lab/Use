use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::ExtensionPaths;
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    archive, canonical_or_absolute, valid_digest, StateBackupManifest, MAX_STATE_BACKUP_BYTES,
    MAX_STATE_BACKUP_FILES, MAX_STATE_BACKUP_MANIFEST_BYTES,
};

pub const A3S_USE_STATE_BACKUP_RETENTION_PLAN_SCHEMA: &str =
    "a3s.use.state-backup-retention-plan.v1";
pub const A3S_USE_STATE_BACKUP_RETENTION_RESULT_SCHEMA: &str =
    "a3s.use.state-backup-retention-result.v1";
pub const DEFAULT_STATE_BACKUP_RETENTION_MAX_BACKUPS: u64 = 32;
pub const DEFAULT_STATE_BACKUP_RETENTION_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;
pub const MIN_STATE_BACKUP_RETENTION_BACKUPS: u64 = 2;
pub const MAX_STATE_BACKUP_RETENTION_BACKUPS: u64 = 4_096;
pub const MAX_STATE_BACKUP_RETENTION_BYTES: u64 = 256 * 1024 * 1024 * 1024 * 1024;

const BACKUP_SUFFIX: &str = ".a3s-use-state-backup";
const DIRECTORY_LOCK_NAME: &str = ".a3s-use-state-backup-retention.lock";
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_BACKUP_FILE_NAME_BYTES: usize = 255;
const MAX_ARCHIVE_BYTES: u64 = MAX_STATE_BACKUP_BYTES + MAX_STATE_BACKUP_MANIFEST_BYTES + 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupRetentionPolicy {
    pub max_backups: u64,
    pub max_bytes: u64,
}

impl StateBackupRetentionPolicy {
    pub fn new(max_backups: u64, max_bytes: u64) -> UseResult<Self> {
        let policy = Self {
            max_backups,
            max_bytes,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.max_backups < MIN_STATE_BACKUP_RETENTION_BACKUPS
            || self.max_backups > MAX_STATE_BACKUP_RETENTION_BACKUPS
            || self.max_bytes == 0
            || self.max_bytes > MAX_STATE_BACKUP_RETENTION_BYTES
        {
            return Err(retention_error(
                "use.state_backup_retention_policy_invalid",
                format!(
                    "State backup retention requires {MIN_STATE_BACKUP_RETENTION_BACKUPS}..={MAX_STATE_BACKUP_RETENTION_BACKUPS} backups and 1..={MAX_STATE_BACKUP_RETENTION_BYTES} bytes."
                ),
            ));
        }
        Ok(())
    }
}

impl Default for StateBackupRetentionPolicy {
    fn default() -> Self {
        Self {
            max_backups: DEFAULT_STATE_BACKUP_RETENTION_MAX_BACKUPS,
            max_bytes: DEFAULT_STATE_BACKUP_RETENTION_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupRetentionEntry {
    pub file_name: String,
    pub modified_at_ns: u64,
    pub archive_bytes: u64,
    pub manifest_digest: String,
    pub inventory_digest: String,
    pub registry_generation: u64,
    pub registry_digest: String,
    pub file_count: u64,
    pub payload_bytes: u64,
}

impl StateBackupRetentionEntry {
    fn validate(&self) -> UseResult<()> {
        let components = Path::new(&self.file_name).components().collect::<Vec<_>>();
        if self.file_name.is_empty()
            || self.file_name.len() > MAX_BACKUP_FILE_NAME_BYTES
            || !self.file_name.ends_with(BACKUP_SUFFIX)
            || !matches!(components.as_slice(), [Component::Normal(_)])
            || !portable_file_name(&self.file_name)
            || self.archive_bytes == 0
            || self.archive_bytes > MAX_ARCHIVE_BYTES
            || self.archive_bytes <= self.payload_bytes
            || self.payload_bytes > MAX_STATE_BACKUP_BYTES
            || self.file_count > MAX_STATE_BACKUP_FILES
            || !valid_digest(&self.manifest_digest)
            || !valid_digest(&self.inventory_digest)
            || !valid_digest(&self.registry_digest)
        {
            return Err(plan_invalid(
                "A state backup retention entry is invalid or exceeds its safety bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupRetentionPlan {
    pub schema: String,
    pub policy: StateBackupRetentionPolicy,
    pub before_backup_count: u64,
    pub before_archive_bytes: u64,
    pub remove: Vec<StateBackupRetentionEntry>,
    pub retain: Vec<StateBackupRetentionEntry>,
    pub retained_backup_count: u64,
    pub retained_archive_bytes: u64,
}

impl StateBackupRetentionPlan {
    fn new(
        policy: StateBackupRetentionPolicy,
        inventory: Vec<StateBackupRetentionEntry>,
    ) -> UseResult<Self> {
        policy.validate()?;
        let before_backup_count = count(&inventory)?;
        let before_archive_bytes = total_bytes(&inventory)?;
        let (remove, retain) = partition_inventory(inventory, policy)?;
        let plan = Self {
            schema: A3S_USE_STATE_BACKUP_RETENTION_PLAN_SCHEMA.to_owned(),
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
        if self.schema != A3S_USE_STATE_BACKUP_RETENTION_PLAN_SCHEMA {
            return Err(plan_invalid(
                "The state backup retention plan schema is invalid.",
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
            return Err(plan_invalid(
                "The state backup retention plan inventory or accounting is inconsistent.",
            ));
        }
        Ok(())
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        canonical_digest(self, "plan")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupRetentionResult {
    pub schema: String,
    pub plan_digest: String,
    pub changed: bool,
    pub removed: Vec<StateBackupRetentionEntry>,
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
                        "The state backup retention lock cannot be inspected: {error}"
                    ))
                })?;
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file()
                {
                    return Err(directory_invalid(
                        "The state backup retention lock is not an owned regular file.",
                    ));
                }
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&lock_path)
                    .map_err(|error| {
                        retention_io(format!(
                            "The state backup retention lock cannot be opened: {error}"
                        ))
                    })?
            }
            Err(error) => {
                return Err(retention_io(format!(
                    "The state backup retention lock cannot be created: {error}"
                )))
            }
        };
        file.try_lock_exclusive().map_err(|error| {
            retention_error(
                "use.state_backup_retention_busy",
                format!(
                    "Another coordinated backup or retention operation owns the directory lock: {error}"
                ),
            )
        })?;
        validate_directory(directory)?;
        Ok(Self { file })
    }
}

impl Drop for BackupDirectoryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) fn resolve_directory(directory: &Path, paths: &ExtensionPaths) -> UseResult<PathBuf> {
    if directory.as_os_str().is_empty() {
        return Err(directory_invalid(
            "State backup retention requires an explicit directory.",
        ));
    }
    validate_directory(directory)?;
    let directory = fs::canonicalize(directory).map_err(|error| {
        retention_io(format!(
            "The state backup retention directory cannot be resolved: {error}"
        ))
    })?;
    validate_directory(&directory)?;
    for owned_root in [paths.data_root(), paths.state_root()] {
        let owned_root = canonical_or_absolute(owned_root)?;
        if directory == owned_root
            || directory.starts_with(&owned_root)
            || owned_root.starts_with(&directory)
        {
            return Err(directory_invalid(
                "The state backup retention directory must not overlap Use-owned data or state.",
            ));
        }
    }
    Ok(directory)
}

pub(super) fn plan(
    directory: &Path,
    policy: StateBackupRetentionPolicy,
) -> UseResult<StateBackupRetentionPlan> {
    let _lock = BackupDirectoryLock::acquire(directory)?;
    build_plan(directory, policy)
}

pub(super) fn apply(
    directory: &Path,
    policy: StateBackupRetentionPolicy,
    expected_plan_digest: &str,
) -> UseResult<StateBackupRetentionResult> {
    if !valid_digest(expected_plan_digest) {
        return Err(plan_mismatch(
            "State backup retention requires an exact canonical SHA-256 plan digest.",
        ));
    }
    let _lock = BackupDirectoryLock::acquire(directory)?;
    let current = build_plan(directory, policy)?;
    let actual_digest = current.descriptor_digest()?;
    if actual_digest != expected_plan_digest {
        return Err(plan_mismatch(
            "The state backup directory changed after review; create and confirm a new retention plan.",
        ));
    }

    for expected in &current.remove {
        let actual = inspect_entry(&directory.join(&expected.file_name))?;
        if &actual != expected {
            return Err(plan_mismatch(
                "A reviewed state backup changed before retention apply.",
            ));
        }
    }

    let mut removed = Vec::new();
    for entry in &current.remove {
        if let Err(error) = fs::remove_file(directory.join(&entry.file_name)) {
            return Err(retention_outcome_unknown(
                format!(
                    "A reviewed state backup could not be removed after retention began: {error}"
                ),
                &removed,
            ));
        }
        removed.push(entry.clone());
        if let Err(error) = sync_directory(directory) {
            return Err(retention_outcome_unknown(
                format!(
                    "A state backup was removed, but directory durability could not be confirmed: {}",
                    error.message
                ),
                &removed,
            ));
        }
    }
    let after = inventory(directory).map_err(|error| {
        retention_outcome_unknown(
            format!(
                "State backup retention changed files, but the retained inventory could not be verified: {}",
                error.message
            ),
            &removed,
        )
    })?;
    if after != current.retain {
        return Err(retention_outcome_unknown(
            "The state backup directory changed during retention apply.",
            &removed,
        ));
    }
    Ok(StateBackupRetentionResult {
        schema: A3S_USE_STATE_BACKUP_RETENTION_RESULT_SCHEMA.to_owned(),
        plan_digest: actual_digest,
        changed: !removed.is_empty(),
        removed,
        retained_backup_count: count(&after)?,
        retained_archive_bytes: total_bytes(&after)?,
    })
}

fn build_plan(
    directory: &Path,
    policy: StateBackupRetentionPolicy,
) -> UseResult<StateBackupRetentionPlan> {
    policy.validate()?;
    StateBackupRetentionPlan::new(policy, inventory(directory)?)
}

fn inventory(directory: &Path) -> UseResult<Vec<StateBackupRetentionEntry>> {
    validate_directory(directory)?;
    let mut entries = Vec::new();
    let mut inspected = 0usize;
    for entry in fs::read_dir(directory).map_err(|error| {
        retention_io(format!(
            "The state backup retention directory cannot be listed: {error}"
        ))
    })? {
        let entry = entry.map_err(|error| {
            retention_io(format!(
                "A state backup retention directory entry cannot be read: {error}"
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
            directory_invalid("The state backup directory entry count overflowed.")
        })?;
        if inspected > MAX_DIRECTORY_ENTRIES {
            return Err(directory_invalid(format!(
                "The state backup directory exceeds the {MAX_DIRECTORY_ENTRIES}-entry inspection bound."
            )));
        }
        if file_name.ends_with(BACKUP_SUFFIX) {
            entries.push(inspect_entry(&entry.path())?);
        }
    }
    entries.sort_by(entry_order);
    validate_inventory(&entries)?;
    Ok(entries)
}

fn inspect_entry(path: &Path) -> UseResult<StateBackupRetentionEntry> {
    let before = fs::symlink_metadata(path).map_err(|error| {
        retention_io(format!(
            "A managed state backup candidate cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&before) || !before.is_file() {
        return Err(directory_invalid(
            "A managed state backup candidate is not an owned regular file.",
        ));
    }
    let before_modified = modified_at_ns(&before)?;
    let manifest = archive::verify_backup(path)?;
    let after = fs::symlink_metadata(path).map_err(|error| {
        retention_io(format!(
            "A verified state backup candidate cannot be reinspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != before.len()
        || modified_at_ns(&after)? != before_modified
    {
        return Err(plan_mismatch(
            "A state backup candidate changed while retention inspected it.",
        ));
    }
    entry_from_manifest(path, &after, before_modified, &manifest)
}

fn entry_from_manifest(
    path: &Path,
    metadata: &fs::Metadata,
    modified_at_ns: u64,
    manifest: &StateBackupManifest,
) -> UseResult<StateBackupRetentionEntry> {
    let entry = StateBackupRetentionEntry {
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                directory_invalid("A managed state backup file name is not valid UTF-8.")
            })?
            .to_owned(),
        modified_at_ns,
        archive_bytes: metadata.len(),
        manifest_digest: canonical_digest(manifest, "manifest")?,
        inventory_digest: manifest.inventory_digest.clone(),
        registry_generation: manifest.authority.registry_generation,
        registry_digest: manifest.authority.registry_digest.clone(),
        file_count: manifest.file_count,
        payload_bytes: manifest.byte_count,
    };
    entry.validate()?;
    Ok(entry)
}

fn partition_inventory(
    inventory: Vec<StateBackupRetentionEntry>,
    policy: StateBackupRetentionPolicy,
) -> UseResult<(
    Vec<StateBackupRetentionEntry>,
    Vec<StateBackupRetentionEntry>,
)> {
    policy.validate()?;
    validate_inventory(&inventory)?;
    let mut retained_count = count(&inventory)?;
    let mut retained_bytes = total_bytes(&inventory)?;
    let mut remove_count = 0usize;
    while retained_count > policy.max_backups || retained_bytes > policy.max_bytes {
        if retained_count <= MIN_STATE_BACKUP_RETENTION_BACKUPS {
            return Err(retention_error(
                "use.state_backup_retention_policy_unsatisfied",
                "The newest two verified state backups exceed the policy; coordinated retention never removes either recovery generation.",
            ));
        }
        let entry = inventory.get(remove_count).ok_or_else(|| {
            plan_invalid("The state backup retention partition exceeded its inventory.")
        })?;
        retained_count -= 1;
        retained_bytes = retained_bytes
            .checked_sub(entry.archive_bytes)
            .ok_or_else(|| {
                plan_invalid("The state backup retention byte accounting underflowed.")
            })?;
        remove_count += 1;
    }
    let mut inventory = inventory;
    let retain = inventory.split_off(remove_count);
    Ok((inventory, retain))
}

fn validate_inventory(entries: &[StateBackupRetentionEntry]) -> UseResult<()> {
    if entries.len() > MAX_DIRECTORY_ENTRIES {
        return Err(plan_invalid(
            "The state backup retention inventory exceeds its entry bound.",
        ));
    }
    let mut names = BTreeSet::new();
    for entry in entries {
        entry.validate()?;
        if !names.insert(entry.file_name.as_str()) {
            return Err(plan_invalid(
                "The state backup retention inventory repeats a file name.",
            ));
        }
    }
    if !entries
        .windows(2)
        .all(|pair| entry_order(&pair[0], &pair[1]).is_le())
    {
        return Err(plan_invalid(
            "The state backup retention inventory is not canonically ordered.",
        ));
    }
    total_bytes(entries)?;
    Ok(())
}

fn entry_order(
    left: &StateBackupRetentionEntry,
    right: &StateBackupRetentionEntry,
) -> std::cmp::Ordering {
    (left.modified_at_ns, left.file_name.as_str())
        .cmp(&(right.modified_at_ns, right.file_name.as_str()))
}

fn count(entries: &[StateBackupRetentionEntry]) -> UseResult<u64> {
    u64::try_from(entries.len()).map_err(|_| {
        plan_invalid("The state backup retention entry count exceeds the platform range.")
    })
}

fn total_bytes(entries: &[StateBackupRetentionEntry]) -> UseResult<u64> {
    entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.archive_bytes)
            .ok_or_else(|| plan_invalid("The state backup retention byte accounting overflowed."))
    })
}

fn canonical_digest(value: &impl Serialize, label: &str) -> UseResult<String> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        plan_invalid(format!(
            "The canonical state backup retention {label} cannot be encoded: {error}"
        ))
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn modified_at_ns(metadata: &fs::Metadata) -> UseResult<u64> {
    let modified = metadata.modified().map_err(|error| {
        directory_invalid(format!(
            "A managed state backup modification time is unavailable: {error}"
        ))
    })?;
    let elapsed = modified.duration_since(UNIX_EPOCH).map_err(|_| {
        directory_invalid("A managed state backup modification time predates the Unix epoch.")
    })?;
    u64::try_from(elapsed.as_nanos()).map_err(|_| {
        directory_invalid("A managed state backup modification time exceeds its numeric bound.")
    })
}

fn portable_file_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem.strip_prefix("COM").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
        || stem.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        });
    !reserved
        && !value.ends_with([' ', '.'])
        && value.bytes().all(|byte| {
            byte >= 0x20
                && byte != 0x7f
                && !matches!(
                    byte,
                    b'<' | b'>' | b':' | b'"' | b'/' | b'\\' | b'|' | b'?' | b'*'
                )
        })
}

fn validate_directory(directory: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        retention_io(format!(
            "The state backup retention directory cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(directory_invalid(
            "The state backup retention path is not an owned directory.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> UseResult<()> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            retention_io(format!(
                "The state backup retention directory cannot be synchronized: {error}"
            ))
        })
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> UseResult<()> {
    Ok(())
}

fn directory_invalid(message: impl Into<String>) -> UseError {
    retention_error("use.state_backup_retention_directory_invalid", message)
}

fn plan_invalid(message: impl Into<String>) -> UseError {
    retention_error("use.state_backup_retention_plan_invalid", message)
}

fn plan_mismatch(message: impl Into<String>) -> UseError {
    retention_error("use.state_backup_retention_plan_mismatch", message)
}

fn retention_outcome_unknown(
    message: impl Into<String>,
    removed: &[StateBackupRetentionEntry],
) -> UseError {
    retention_error("use.state_backup_retention_outcome_unknown", message)
        .with_detail("removedBackups", serde_json::json!(removed))
        .with_suggestion(
            "Inspect the directory, verify every remaining archive, and create a new retention plan; never recreate a removed backup path by assumption.",
        )
}

pub(super) fn retention_io(message: impl Into<String>) -> UseError {
    retention_error("use.state_backup_retention_io", message)
}

fn retention_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
