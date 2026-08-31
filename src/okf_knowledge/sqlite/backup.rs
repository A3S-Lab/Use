use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use a3s_use_core::{
    OkfKnowledgeObservedState, PlanQualifiedSurfaceRef, PlanScope, UseError, UseResult,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::audit;
use super::policy::OkfKnowledgeStoragePolicy;
use super::projection::{database_io, load_projection, observation, selected_generation};
use super::schema;
use super::storage::OkfKnowledgeStorageUsage;

pub const OKF_KNOWLEDGE_BACKUP_SCHEMA: &str = "a3s.use.okf-knowledge-backup.v1";

const BACKUP_MAGIC: &[u8] = b"A3S-OKF-BACKUP\n";
const MANIFEST_DIGEST_BYTES: usize = 32;
const MAX_BACKUP_MANIFEST_BYTES: usize = 64 * 1024;
pub(crate) const MAX_BACKUP_DATABASE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub(crate) const MAX_BACKUP_ARCHIVE_BYTES: u64 = MAX_BACKUP_DATABASE_BYTES
    + MAX_BACKUP_MANIFEST_BYTES as u64
    + BACKUP_MAGIC.len() as u64
    + 4
    + MANIFEST_DIGEST_BYTES as u64;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OkfKnowledgeRestoreInventory {
    pub(crate) bindings: Vec<crate::okf_knowledge::OkfKnowledgeBinding>,
    pub(crate) selected: Vec<(PlanQualifiedSurfaceRef, u64)>,
}

#[derive(Debug)]
pub(crate) struct VerifiedOkfKnowledgeBackup {
    pub(crate) manifest: OkfKnowledgeBackupManifest,
    pub(crate) bindings: Vec<crate::okf_knowledge::OkfKnowledgeBinding>,
    pub(crate) selected: Vec<(PlanQualifiedSurfaceRef, u64)>,
    pub(crate) database_path: PathBuf,
    _temporary: tempfile::TempDir,
}

/// Self-describing receipt for one consistent scope-local SQLite snapshot.
///
/// The digest detects corruption and the embedded receipts retain exact scope
/// and package evidence. This is not a signature and does not replace a backup
/// of Registry, package, Grant, binding, or lifecycle authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeBackupManifest {
    pub schema: String,
    pub scope: PlanScope,
    pub created_at_ms: u64,
    pub database_bytes: u64,
    pub database_sha256: String,
    pub storage: OkfKnowledgeStorageUsage,
}

impl OkfKnowledgeBackupManifest {
    pub(crate) fn validate(&self) -> UseResult<OkfKnowledgeStoragePolicy> {
        if self.schema != OKF_KNOWLEDGE_BACKUP_SCHEMA
            || self.scope.validate().is_err()
            || self.created_at_ms == 0
            || self.database_bytes == 0
            || self.database_bytes > MAX_BACKUP_DATABASE_BYTES
            || !valid_sha256(&self.database_sha256)
            || self.storage.scope != self.scope
            || self.storage.database_bytes != self.database_bytes
            || self.storage.reclaimable_database_bytes > self.storage.database_bytes
            || self.storage.retained_projections > self.storage.max_scope_projections
            || self.storage.retained_expanded_bytes > self.storage.max_scope_expanded_bytes
            || self.storage.removed_tombstones > self.storage.max_scope_tombstones
        {
            return Err(backup_invalid(
                "The OKF Knowledge backup manifest is inconsistent or exceeds its safety bounds.",
            ));
        }
        OkfKnowledgeStoragePolicy::new(
            self.storage.max_scope_expanded_bytes,
            self.storage.max_scope_projections,
            self.storage.max_surface_generations,
            self.storage.max_scope_tombstones,
        )
        .map_err(|_| backup_invalid("The OKF Knowledge backup carries an invalid storage policy."))
    }
}

pub(super) fn create(
    live_database: &Path,
    scope: &PlanScope,
    policy: &OkfKnowledgeStoragePolicy,
    destination: &Path,
    created_at_ms: u64,
    max_archive_bytes: u64,
) -> UseResult<OkfKnowledgeBackupManifest> {
    if max_archive_bytes == 0 || max_archive_bytes > MAX_BACKUP_ARCHIVE_BYTES {
        return Err(backup_invalid(
            "The OKF Knowledge backup archive bound is invalid.",
        ));
    }
    validate_new_destination(destination)?;
    let temporary_snapshot = tempfile::tempdir().map_err(|error| {
        backup_io(format!(
            "Failed to create a temporary Knowledge backup directory: {error}"
        ))
    })?;
    let snapshot_path = temporary_snapshot.path().join("knowledge.sqlite3");
    let snapshot_path_text = snapshot_path
        .to_str()
        .ok_or_else(|| backup_io("The temporary Knowledge backup path is not valid UTF-8."))?;

    let live = schema::open(live_database, false)
        .map_err(|error| database_io("open Knowledge database for backup", error))?;
    audit::audit(&live, scope, policy)?;
    live.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| database_io("checkpoint Knowledge database for backup", error))?;
    live.execute("VACUUM INTO ?1", params![snapshot_path_text])
        .map_err(|error| database_io("create consistent Knowledge backup snapshot", error))?;
    drop(live);

    let snapshot = schema::open(&snapshot_path, false)
        .map_err(|error| database_io("open Knowledge backup snapshot", error))?;
    let report = audit::audit(&snapshot, scope, policy)?;
    snapshot
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| database_io("checkpoint Knowledge backup snapshot", error))?;
    drop(snapshot);

    let database_bytes = regular_file_length(&snapshot_path)?;
    if database_bytes == 0 || database_bytes > MAX_BACKUP_DATABASE_BYTES {
        return Err(backup_invalid(
            "The OKF Knowledge backup database is empty or exceeds its safety bound.",
        ));
    }
    if report.storage.database_bytes != database_bytes {
        return Err(backup_invalid(
            "The OKF Knowledge backup page accounting does not match its snapshot length.",
        ));
    }
    let database_sha256 = hash_file(&snapshot_path, database_bytes)?;
    let manifest = OkfKnowledgeBackupManifest {
        schema: OKF_KNOWLEDGE_BACKUP_SCHEMA.to_owned(),
        scope: scope.clone(),
        created_at_ms,
        database_bytes,
        database_sha256,
        storage: report.storage,
    };
    manifest.validate()?;
    write_archive(destination, &manifest, &snapshot_path, max_archive_bytes)?;
    Ok(manifest)
}

pub(super) fn verify(
    backup_path: &Path,
    expected_scope: Option<&PlanScope>,
) -> UseResult<OkfKnowledgeBackupManifest> {
    Ok(load_verified(backup_path, expected_scope)?.manifest)
}

pub(super) fn inspect(
    backup_path: &Path,
    expected_scope: &PlanScope,
) -> UseResult<VerifiedOkfKnowledgeBackup> {
    load_verified(backup_path, Some(expected_scope))
}

fn load_verified(
    backup_path: &Path,
    expected_scope: Option<&PlanScope>,
) -> UseResult<VerifiedOkfKnowledgeBackup> {
    validate_regular_backup_file(backup_path)?;
    let archive_bytes = regular_file_length(backup_path)?;
    if archive_bytes == 0 || archive_bytes > MAX_BACKUP_ARCHIVE_BYTES {
        return Err(backup_invalid(
            "The Knowledge backup archive exceeds its safety bound.",
        ));
    }
    let mut reader = BufReader::new(File::open(backup_path).map_err(|error| {
        backup_io(format!(
            "Failed to open Knowledge backup '{}': {error}",
            backup_path.display()
        ))
    })?);
    let mut magic = vec![0_u8; BACKUP_MAGIC.len()];
    reader
        .read_exact(&mut magic)
        .map_err(|error| backup_io(format!("Failed to read Knowledge backup header: {error}")))?;
    if magic != BACKUP_MAGIC {
        return Err(backup_invalid(
            "The file is not an A3S OKF Knowledge backup.",
        ));
    }
    let mut manifest_length = [0_u8; 4];
    reader.read_exact(&mut manifest_length).map_err(|error| {
        backup_io(format!(
            "Failed to read Knowledge backup manifest length: {error}"
        ))
    })?;
    let manifest_length = usize::try_from(u32::from_be_bytes(manifest_length))
        .map_err(|_| backup_invalid("The Knowledge backup manifest length is invalid."))?;
    if manifest_length == 0 || manifest_length > MAX_BACKUP_MANIFEST_BYTES {
        return Err(backup_invalid(
            "The Knowledge backup manifest exceeds its safety bound.",
        ));
    }
    let mut manifest_digest = [0_u8; MANIFEST_DIGEST_BYTES];
    reader.read_exact(&mut manifest_digest).map_err(|error| {
        backup_io(format!(
            "Failed to read Knowledge backup manifest digest: {error}"
        ))
    })?;
    let mut manifest_bytes = vec![0_u8; manifest_length];
    reader
        .read_exact(&mut manifest_bytes)
        .map_err(|error| backup_io(format!("Failed to read Knowledge backup manifest: {error}")))?;
    if Sha256::digest(&manifest_bytes).as_slice() != manifest_digest {
        return Err(backup_invalid(
            "The Knowledge backup manifest digest does not match its bytes.",
        ));
    }
    let manifest: OkfKnowledgeBackupManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            backup_invalid(format!("The Knowledge backup manifest is invalid: {error}"))
        })?;
    manifest.validate()?;
    if expected_scope.is_some_and(|scope| scope != &manifest.scope) {
        return Err(UseError::new(
            "use.okf.knowledge_backup_scope_mismatch",
            "The OKF Knowledge backup belongs to a different complete User or Workspace scope.",
        ));
    }
    let header_bytes =
        u64::try_from(BACKUP_MAGIC.len() + 4 + MANIFEST_DIGEST_BYTES + manifest_length)
            .map_err(|_| backup_invalid("The Knowledge backup header length overflowed."))?;
    let expected_archive_bytes = header_bytes
        .checked_add(manifest.database_bytes)
        .ok_or_else(|| backup_invalid("The Knowledge backup length overflowed."))?;
    if archive_bytes != expected_archive_bytes {
        return Err(backup_invalid(
            "The Knowledge backup length does not match its manifest.",
        ));
    }

    let temporary_snapshot = tempfile::tempdir().map_err(|error| {
        backup_io(format!(
            "Failed to create a temporary Knowledge verification directory: {error}"
        ))
    })?;
    let snapshot_path = temporary_snapshot.path().join("knowledge.sqlite3");
    let mut snapshot = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&snapshot_path)
        .map_err(|error| {
            backup_io(format!(
                "Failed to stage Knowledge backup verification: {error}"
            ))
        })?;
    let digest = copy_and_hash(&mut reader, &mut snapshot, manifest.database_bytes)?;
    snapshot.sync_all().map_err(|error| {
        backup_io(format!(
            "Failed to sync Knowledge backup verification: {error}"
        ))
    })?;
    drop(snapshot);
    if digest != manifest.database_sha256 {
        return Err(backup_invalid(
            "The Knowledge backup database digest does not match its manifest.",
        ));
    }

    let inventory = inspect_restore_database(&snapshot_path, &manifest)?;
    Ok(VerifiedOkfKnowledgeBackup {
        manifest,
        bindings: inventory.bindings,
        selected: inventory.selected,
        database_path: snapshot_path,
        _temporary: temporary_snapshot,
    })
}

pub(super) fn inspect_restore_database(
    database_path: &Path,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<OkfKnowledgeRestoreInventory> {
    let policy = manifest.validate()?;
    validate_regular_backup_file(database_path)?;
    let (bytes, sha256) = file_evidence(database_path)?;
    if bytes != manifest.database_bytes || sha256 != manifest.database_sha256 {
        return Err(backup_invalid(
            "The staged Knowledge restore database differs from its reviewed backup manifest.",
        ));
    }
    let connection = schema::open(database_path, false)
        .map_err(|_| backup_invalid("The Knowledge restore database schema is unsupported."))?;
    let report = audit::audit(&connection, &manifest.scope, &policy).map_err(|_| {
        backup_invalid("The Knowledge restore database failed integrity validation.")
    })?;
    if report.storage != manifest.storage {
        return Err(backup_invalid(
            "The Knowledge restore database storage evidence does not match its manifest.",
        ));
    }
    let inventory = restore_inventory(&connection)?;
    drop(connection);
    Ok(inventory)
}

fn restore_inventory(connection: &rusqlite::Connection) -> UseResult<OkfKnowledgeRestoreInventory> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, surface_id, generation
             FROM knowledge_projections
             ORDER BY package_id, surface_id, generation",
        )
        .map_err(|error| database_io("prepare Knowledge backup restore inventory", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| database_io("query Knowledge backup restore inventory", error))?;
    let mut bindings = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| database_io("read Knowledge backup restore inventory", error))?
    {
        let package_id = row
            .get::<_, String>(0)
            .map_err(|error| database_io("read restore package identity", error))?;
        let surface_id = row
            .get::<_, String>(1)
            .map_err(|error| database_io("read restore surface identity", error))?;
        let generation = row
            .get::<_, i64>(2)
            .map_err(|error| database_io("read restore generation", error))?;
        let generation = u64::try_from(generation)
            .map_err(|_| backup_invalid("A restore generation is outside its valid range."))?;
        let stored = load_projection(connection, &package_id, &surface_id, generation)?
            .ok_or_else(|| backup_invalid("A restore projection disappeared during inspection."))?;
        let selected = selected_generation(connection, &package_id, &surface_id)?;
        let observed = observation(
            &stored.receipt,
            stored.state,
            &stored.index_digest,
            stored.observed_at_ms,
            selected,
        )?;
        bindings.push(crate::okf_knowledge::OkfKnowledgeBinding::new(
            stored.receipt,
            observed,
        )?);
    }
    drop(rows);
    drop(statement);

    let mut statement = connection
        .prepare(
            "SELECT package_id, surface_id, generation
             FROM knowledge_selection
             ORDER BY package_id, surface_id",
        )
        .map_err(|error| database_io("prepare Knowledge backup selections", error))?;
    let selected = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| database_io("query Knowledge backup selections", error))?
        .map(|row| {
            let (package_id, surface_id, generation) =
                row.map_err(|error| database_io("read Knowledge backup selection", error))?;
            let generation = u64::try_from(generation).map_err(|_| {
                backup_invalid("A selected restore generation is outside its valid range.")
            })?;
            let binding = bindings
                .iter()
                .find(|binding| {
                    binding.receipt.surface.package_id == package_id
                        && binding.receipt.surface.surface.id == surface_id
                        && binding.receipt.generation == generation
                })
                .ok_or_else(|| {
                    backup_invalid(
                        "A selected restore generation has no retained projection receipt.",
                    )
                })?;
            if binding.observation.state != OkfKnowledgeObservedState::Promoted {
                return Err(backup_invalid(
                    "A selected restore generation is not promoted.",
                ));
            }
            Ok((binding.receipt.surface.clone(), generation))
        })
        .collect::<UseResult<Vec<_>>>()?;
    Ok(OkfKnowledgeRestoreInventory { bindings, selected })
}

fn write_archive(
    destination: &Path,
    manifest: &OkfKnowledgeBackupManifest,
    snapshot_path: &Path,
    max_archive_bytes: u64,
) -> UseResult<()> {
    let manifest_bytes = serde_json::to_vec(manifest).map_err(|error| {
        backup_invalid(format!(
            "Failed to encode Knowledge backup manifest: {error}"
        ))
    })?;
    if manifest_bytes.is_empty() || manifest_bytes.len() > MAX_BACKUP_MANIFEST_BYTES {
        return Err(backup_invalid(
            "The encoded Knowledge backup manifest exceeds its safety bound.",
        ));
    }
    let manifest_length = u32::try_from(manifest_bytes.len())
        .map_err(|_| backup_invalid("The Knowledge backup manifest length overflowed."))?;
    let header_bytes = u64::try_from(BACKUP_MAGIC.len() + 4 + MANIFEST_DIGEST_BYTES)
        .map_err(|_| backup_invalid("The Knowledge backup header length overflowed."))?
        .checked_add(u64::from(manifest_length))
        .ok_or_else(|| backup_invalid("The Knowledge backup header length overflowed."))?;
    let archive_bytes = header_bytes
        .checked_add(manifest.database_bytes)
        .ok_or_else(|| backup_invalid("The Knowledge backup archive length overflowed."))?;
    if archive_bytes > max_archive_bytes {
        return Err(backup_invalid(
            "The Knowledge backup archive exceeds its caller-provided byte bound.",
        ));
    }
    let manifest_digest = Sha256::digest(&manifest_bytes);
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let _directory_lock = super::backup_retention::BackupDirectoryLock::acquire(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        backup_io(format!(
            "Failed to create a temporary Knowledge backup beside '{}': {error}",
            destination.display()
        ))
    })?;
    {
        let mut writer = BufWriter::new(temporary.as_file());
        writer
            .write_all(BACKUP_MAGIC)
            .and_then(|_| writer.write_all(&manifest_length.to_be_bytes()))
            .and_then(|_| writer.write_all(&manifest_digest))
            .and_then(|_| writer.write_all(&manifest_bytes))
            .map_err(|error| {
                backup_io(format!("Failed to write Knowledge backup header: {error}"))
            })?;
        let mut snapshot = BufReader::new(File::open(snapshot_path).map_err(|error| {
            backup_io(format!(
                "Failed to reopen Knowledge backup snapshot: {error}"
            ))
        })?);
        let copied = std::io::copy(&mut snapshot, &mut writer).map_err(|error| {
            backup_io(format!(
                "Failed to write Knowledge backup database: {error}"
            ))
        })?;
        if copied != manifest.database_bytes {
            return Err(backup_invalid(
                "The Knowledge backup snapshot length changed while it was written.",
            ));
        }
        writer
            .flush()
            .map_err(|error| backup_io(format!("Failed to flush Knowledge backup: {error}")))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| backup_io(format!("Failed to sync Knowledge backup: {error}")))?;
    a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, destination).map_err(
        |error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                backup_exists(destination)
            } else {
                backup_io(format!(
                    "Failed to publish Knowledge backup '{}': {}",
                    destination.display(),
                    error
                ))
            }
        },
    )?;
    sync_parent(parent).map_err(|error| {
        UseError::new(
            "use.okf.knowledge_backup_outcome_unknown",
            format!(
                "The Knowledge backup was written to '{}', but directory durability could not be confirmed: {}",
                destination.display(), error.message
            ),
        )
        .with_detail("backupWritten", serde_json::json!(true))
        .with_suggestion(
            "Verify the existing backup before retrying; the command never overwrites it.",
        )
    })?;
    Ok(())
}

pub(super) fn validate_new_destination(destination: &Path) -> UseResult<()> {
    if destination.as_os_str().is_empty() || destination.file_name().is_none() {
        return Err(backup_invalid(
            "The Knowledge backup destination must name a file.",
        ));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
        backup_io(format!(
            "Failed to inspect Knowledge backup directory '{}': {error}",
            parent.display()
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&parent_metadata)
        || !parent_metadata.is_dir()
    {
        return Err(backup_invalid(
            "The Knowledge backup destination parent is not an owned directory.",
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(backup_exists(destination)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(backup_io(format!(
            "Failed to inspect Knowledge backup destination '{}': {error}",
            destination.display()
        ))),
    }
}

fn validate_regular_backup_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        backup_io(format!(
            "Failed to inspect Knowledge backup '{}': {error}",
            path.display()
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(backup_invalid(
            "The Knowledge backup path is not a regular file.",
        ));
    }
    Ok(())
}

fn regular_file_length(path: &Path) -> UseResult<u64> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| backup_io(format!("Failed to inspect '{}': {error}", path.display())))
}

pub(super) fn file_evidence(path: &Path) -> UseResult<(u64, String)> {
    let bytes = regular_file_length(path)?;
    if bytes == 0 || bytes > MAX_BACKUP_DATABASE_BYTES {
        return Err(backup_invalid(
            "The Knowledge database is empty or exceeds the restore safety bound.",
        ));
    }
    Ok((bytes, hash_file(path, bytes)?))
}

pub(super) fn archive_file_evidence(
    path: &Path,
    max_archive_bytes: u64,
) -> UseResult<(u64, String)> {
    validate_regular_backup_file(path)?;
    let bytes = regular_file_length(path)?;
    if bytes == 0 || bytes > max_archive_bytes || bytes > MAX_BACKUP_ARCHIVE_BYTES {
        return Err(backup_invalid(
            "The Knowledge backup archive exceeds its caller-provided byte bound.",
        ));
    }
    Ok((bytes, hash_file(path, bytes)?))
}

pub(super) fn optional_file_evidence(path: &Path) -> UseResult<Option<(u64, String)>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(backup_io(format!(
                "Failed to inspect '{}': {error}",
                path.display()
            )));
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_BACKUP_DATABASE_BYTES
    {
        return Err(backup_invalid(
            "A Knowledge database sidecar is not a bounded owned regular file.",
        ));
    }
    Ok(Some((metadata.len(), hash_file(path, metadata.len())?)))
}

fn hash_file(path: &Path, expected_bytes: u64) -> UseResult<String> {
    let mut reader = BufReader::new(
        File::open(path)
            .map_err(|error| backup_io(format!("Failed to hash '{}': {error}", path.display())))?,
    );
    let mut sink = std::io::sink();
    copy_and_hash(&mut reader, &mut sink, expected_bytes)
}

fn copy_and_hash(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected_bytes: u64,
) -> UseResult<String> {
    let mut remaining = expected_bytes;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
            .map_err(|_| backup_invalid("The Knowledge backup copy length overflowed."))?;
        let count = reader.read(&mut buffer[..requested]).map_err(|error| {
            backup_io(format!("Failed to read Knowledge backup bytes: {error}"))
        })?;
        if count == 0 {
            return Err(backup_invalid(
                "The Knowledge backup ended before its declared database length.",
            ));
        }
        writer.write_all(&buffer[..count]).map_err(|error| {
            backup_io(format!("Failed to write Knowledge backup bytes: {error}"))
        })?;
        digest.update(&buffer[..count]);
        remaining -= u64::try_from(count)
            .map_err(|_| backup_invalid("The Knowledge backup copy count overflowed."))?;
    }
    let mut extra = [0_u8; 1];
    if reader
        .read(&mut extra)
        .map_err(|error| backup_io(format!("Failed to recheck Knowledge backup bytes: {error}")))?
        != 0
    {
        return Err(backup_invalid(
            "The Knowledge backup source grew beyond its declared database length.",
        ));
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn backup_exists(path: &Path) -> UseError {
    UseError::new(
        "use.okf.knowledge_backup_exists",
        format!(
            "The Knowledge backup destination '{}' already exists; no file was overwritten.",
            path.display()
        ),
    )
}

fn backup_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_backup_invalid", message)
}

fn backup_io(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_backup_io", message)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> UseResult<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            backup_io(format!(
                "Failed to sync Knowledge backup directory '{}': {error}",
                parent.display()
            ))
        })
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}
