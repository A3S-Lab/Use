use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_extension::ExtensionPaths;
use sha2::{Digest, Sha256};

use super::inventory::{
    expected_family, scan, scan_with_paths, summarize_families, validate_archived_path,
    validate_inventory_bounds,
};
use super::{
    canonical_json, sha256_digest, state_backup_exists, state_backup_invalid, state_backup_io,
    state_backup_limit, valid_digest, StateBackupAuthority, StateBackupEntry, StateBackupManifest,
    StateBackupRoot, A3S_USE_STATE_BACKUP_SCHEMA, MAX_STATE_BACKUP_FILES,
    MAX_STATE_BACKUP_MANIFEST_BYTES,
};

mod staging;

use staging::{
    ensure_candidate_root, file_matches_entry, prepare_candidate, set_candidate_permissions,
    validate_candidate_tree,
};

const ARCHIVE_MAGIC: &[u8] = b"A3S-USE-STATE-BACKUP-V1\n";
const MANIFEST_LENGTH_BYTES: usize = 8;
const MANIFEST_DIGEST_BYTES: usize = 32;
const COPY_BUFFER_BYTES: usize = 128 * 1024;

pub(super) fn create_backup(
    paths: &ExtensionPaths,
    destination: &Path,
    authority: StateBackupAuthority,
) -> UseResult<StateBackupManifest> {
    reject_existing_destination(destination)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let _directory_lock = super::retention::BackupDirectoryLock::acquire(parent)?;
    let scanned = scan_with_paths(paths)?;
    let entries = scanned
        .iter()
        .map(|(entry, _)| entry.clone())
        .collect::<Vec<_>>();
    let (file_count, byte_count) = validate_inventory_bounds(entries.iter())?;
    let inventory_digest = sha256_digest(&canonical_json(&entries)?);
    let families = summarize_families(&entries)?;
    let manifest = StateBackupManifest {
        schema: A3S_USE_STATE_BACKUP_SCHEMA.to_owned(),
        installation: paths.installation().clone(),
        use_version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        file_count,
        byte_count,
        inventory_digest,
        authority,
        families,
        entries,
    };
    validate_manifest(&manifest)?;
    let manifest_bytes = canonical_json(&manifest)?;
    if manifest_bytes.is_empty() || manifest_bytes.len() as u64 > MAX_STATE_BACKUP_MANIFEST_BYTES {
        return Err(state_backup_limit(
            "The canonical state backup manifest exceeds its byte bound.",
        ));
    }

    let mut temporary = tempfile::Builder::new()
        .prefix(".a3s-use-state-backup-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| {
            state_backup_io(format!(
                "The backup staging file cannot be created: {error}"
            ))
        })?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writer
            .write_all(ARCHIVE_MAGIC)
            .and_then(|_| writer.write_all(&(manifest_bytes.len() as u64).to_be_bytes()))
            .and_then(|_| writer.write_all(Sha256::digest(&manifest_bytes).as_slice()))
            .and_then(|_| writer.write_all(&manifest_bytes))
            .map_err(|error| {
                state_backup_io(format!("The backup header cannot be written: {error}"))
            })?;
        for (entry, path) in &scanned {
            copy_verified_file(&mut writer, path, entry)?;
        }
        writer.flush().map_err(|error| {
            state_backup_io(format!(
                "The backup staging file cannot be flushed: {error}"
            ))
        })?;
    }
    temporary.as_file().sync_all().map_err(|error| {
        state_backup_io(format!(
            "The backup staging file cannot be synchronized: {error}"
        ))
    })?;

    let after = scan(paths)?;
    if after != manifest.entries {
        return Err(super::state_backup_nonterminal(
            "Use-owned state changed between the initial and final backup inventories.",
        ));
    }
    a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, destination).map_err(
        |error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                state_backup_exists()
            } else {
                state_backup_io(format!(
                    "The backup staging file cannot be published: {}",
                    error
                ))
            }
        },
    )?;
    sync_parent(parent)?;
    Ok(manifest)
}

pub(super) fn verify_backup(path: &Path) -> UseResult<StateBackupManifest> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        state_backup_io(format!("The state backup cannot be inspected: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(state_backup_invalid(
            "The state backup is not an owned regular file.",
        ));
    }
    let file = File::open(path)
        .map_err(|error| state_backup_io(format!("The state backup cannot be opened: {error}")))?;
    let opened_metadata = file.metadata().map_err(|error| {
        state_backup_io(format!(
            "The open state backup cannot be inspected: {error}"
        ))
    })?;
    if !opened_metadata.is_file() || opened_metadata.len() != metadata.len() {
        return Err(state_backup_invalid(
            "The state backup changed while it was opened.",
        ));
    }
    let opened_modified = opened_metadata.modified().ok();
    let mut reader = BufReader::new(file);
    let mut magic = vec![0u8; ARCHIVE_MAGIC.len()];
    read_exact_invalid(&mut reader, &mut magic)?;
    if magic != ARCHIVE_MAGIC {
        return Err(state_backup_invalid(
            "The state backup archive magic is invalid.",
        ));
    }
    let mut length_bytes = [0u8; MANIFEST_LENGTH_BYTES];
    read_exact_invalid(&mut reader, &mut length_bytes)?;
    let manifest_length = u64::from_be_bytes(length_bytes);
    if manifest_length == 0 || manifest_length > MAX_STATE_BACKUP_MANIFEST_BYTES {
        return Err(state_backup_invalid(
            "The state backup manifest length is outside its bound.",
        ));
    }
    let mut expected_manifest_digest = [0u8; MANIFEST_DIGEST_BYTES];
    read_exact_invalid(&mut reader, &mut expected_manifest_digest)?;
    let mut manifest_bytes = vec![0u8; manifest_length as usize];
    read_exact_invalid(&mut reader, &mut manifest_bytes)?;
    if Sha256::digest(&manifest_bytes).as_slice() != expected_manifest_digest {
        return Err(state_backup_invalid(
            "The state backup manifest digest does not match its header.",
        ));
    }
    let manifest: StateBackupManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
        state_backup_invalid("The state backup manifest is not valid schema-v1 JSON.")
    })?;
    validate_manifest(&manifest).map_err(|error| {
        state_backup_invalid(format!(
            "The state backup manifest failed validation: {}",
            error.message
        ))
    })?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err(state_backup_invalid(
            "The state backup manifest is not in canonical JSON form.",
        ));
    }
    let expected_archive_length = (ARCHIVE_MAGIC.len() as u64)
        .checked_add(MANIFEST_LENGTH_BYTES as u64)
        .and_then(|value| value.checked_add(MANIFEST_DIGEST_BYTES as u64))
        .and_then(|value| value.checked_add(manifest_length))
        .and_then(|value| value.checked_add(manifest.byte_count))
        .ok_or_else(|| state_backup_invalid("The state backup archive length overflowed."))?;
    if opened_metadata.len() != expected_archive_length {
        return Err(state_backup_invalid(
            "The state backup archive length does not match its manifest.",
        ));
    }
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    for entry in &manifest.entries {
        let mut remaining = entry.length;
        let mut hasher = Sha256::new();
        while remaining > 0 {
            let requested = usize::try_from(remaining.min(buffer.len() as u64))
                .map_err(|_| state_backup_invalid("A state backup payload length is invalid."))?;
            read_exact_invalid(&mut reader, &mut buffer[..requested])?;
            hasher.update(&buffer[..requested]);
            remaining -= requested as u64;
        }
        let digest = format!("sha256:{:x}", hasher.finalize());
        if digest != entry.sha256 {
            return Err(state_backup_invalid(
                "A state backup payload digest does not match its manifest entry.",
            ));
        }
    }
    let mut trailing = [0u8; 1];
    if reader
        .read(&mut trailing)
        .map_err(|error| state_backup_io(format!("The state backup cannot be finished: {error}")))?
        != 0
    {
        return Err(state_backup_invalid(
            "The state backup contains trailing unaccounted bytes.",
        ));
    }
    let after = std::fs::symlink_metadata(path).map_err(|error| {
        state_backup_io(format!("The state backup cannot be reinspected: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != opened_metadata.len()
        || opened_modified.is_some_and(|modified| after.modified().ok() != Some(modified))
    {
        return Err(state_backup_invalid(
            "The state backup changed while it was verified.",
        ));
    }
    Ok(manifest)
}

pub(super) fn stage_restore_entries(
    archive_path: &Path,
    expected_manifest: &StateBackupManifest,
    selected_entries: &[StateBackupEntry],
    data_candidate_root: &Path,
    state_candidate_root: &Path,
) -> UseResult<()> {
    expected_manifest.validate()?;
    let verified = verify_backup(archive_path)?;
    if &verified != expected_manifest {
        return Err(state_backup_invalid(
            "The restore archive differs from the reviewed backup manifest.",
        ));
    }

    let selected = selected_restore_entries(expected_manifest, selected_entries)?;

    for (root, candidate_root) in [
        (StateBackupRoot::Data, data_candidate_root),
        (StateBackupRoot::State, state_candidate_root),
    ] {
        if selected.keys().any(|(entry_root, _)| *entry_root == root) {
            ensure_candidate_root(candidate_root)?;
        } else if std::fs::symlink_metadata(candidate_root).is_ok() {
            return Err(state_backup_invalid(
                "An unneeded restore candidate root already exists.",
            ));
        }
    }

    let metadata = std::fs::symlink_metadata(archive_path).map_err(|error| {
        state_backup_io(format!(
            "The state restore archive cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(state_backup_invalid(
            "The state restore archive is not an owned regular file.",
        ));
    }
    let file = File::open(archive_path).map_err(|error| {
        state_backup_io(format!(
            "The state restore archive cannot be opened: {error}"
        ))
    })?;
    let opened = file.metadata().map_err(|error| {
        state_backup_io(format!(
            "The open state restore archive cannot be inspected: {error}"
        ))
    })?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err(state_backup_invalid(
            "The state restore archive changed while it was opened.",
        ));
    }
    let opened_modified = opened.modified().ok();
    let mut reader = BufReader::new(file);
    let mut magic = vec![0u8; ARCHIVE_MAGIC.len()];
    read_exact_invalid(&mut reader, &mut magic)?;
    if magic != ARCHIVE_MAGIC {
        return Err(state_backup_invalid(
            "The state restore archive magic is invalid.",
        ));
    }
    let mut length_bytes = [0u8; MANIFEST_LENGTH_BYTES];
    read_exact_invalid(&mut reader, &mut length_bytes)?;
    let manifest_length = u64::from_be_bytes(length_bytes);
    if manifest_length == 0 || manifest_length > MAX_STATE_BACKUP_MANIFEST_BYTES {
        return Err(state_backup_invalid(
            "The state restore manifest length is outside its bound.",
        ));
    }
    let mut expected_manifest_digest = [0u8; MANIFEST_DIGEST_BYTES];
    read_exact_invalid(&mut reader, &mut expected_manifest_digest)?;
    let mut manifest_bytes = vec![0u8; manifest_length as usize];
    read_exact_invalid(&mut reader, &mut manifest_bytes)?;
    if Sha256::digest(&manifest_bytes).as_slice() != expected_manifest_digest
        || canonical_json(expected_manifest)? != manifest_bytes
    {
        return Err(state_backup_invalid(
            "The staged restore manifest differs from the exact reviewed manifest.",
        ));
    }

    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    for entry in &expected_manifest.entries {
        let candidate_root = match entry.root {
            StateBackupRoot::Data => data_candidate_root,
            StateBackupRoot::State => state_candidate_root,
        };
        let selected_entry = selected.get(&(entry.root, entry.path.clone()));
        let destination = selected_entry.map(|_| candidate_root.join(&entry.path));
        let mut pending = destination
            .as_deref()
            .map(|destination| prepare_candidate(destination, candidate_root, entry))
            .transpose()?
            .flatten();

        let mut remaining = entry.length;
        let mut hasher = Sha256::new();
        while remaining > 0 {
            let requested = remaining.min(buffer.len() as u64) as usize;
            read_exact_invalid(&mut reader, &mut buffer[..requested])?;
            hasher.update(&buffer[..requested]);
            if let Some(candidate) = &mut pending {
                candidate
                    .file
                    .write_all(&buffer[..requested])
                    .map_err(|error| {
                        state_backup_io(format!(
                            "A restore candidate payload cannot be written: {error}"
                        ))
                    })?;
            }
            remaining -= requested as u64;
        }
        if format!("sha256:{:x}", hasher.finalize()) != entry.sha256 {
            return Err(state_backup_invalid(
                "A staged restore payload digest differs from its reviewed entry.",
            ));
        }
        if let Some(mut candidate) = pending {
            candidate.file.flush().map_err(|error| {
                state_backup_io(format!("A restore candidate cannot be flushed: {error}"))
            })?;
            candidate.file.sync_all().map_err(|error| {
                state_backup_io(format!(
                    "A restore candidate cannot be synchronized: {error}"
                ))
            })?;
            drop(candidate.file);
            set_candidate_permissions(&candidate.partial, entry)?;
            if !file_matches_entry(&candidate.partial, entry)? {
                return Err(state_backup_invalid(
                    "A staged restore candidate differs from its reviewed evidence.",
                ));
            }
            a3s_use_extension::rename_path_with_windows_retry_blocking(
                &candidate.partial,
                &candidate.destination,
            )
            .map_err(|error| {
                state_backup_io(format!("A restore candidate cannot be activated: {error}"))
            })?;
            sync_parent(&candidate.destination)?;
        }
        if let Some(destination) = &destination {
            if !file_matches_entry(destination, entry)? {
                return Err(state_backup_invalid(
                    "An activated restore candidate differs from its reviewed evidence.",
                ));
            }
        }
    }
    let mut trailing = [0u8; 1];
    if reader
        .read(&mut trailing)
        .map_err(|error| state_backup_io(format!("The restore archive cannot finish: {error}")))?
        != 0
    {
        return Err(state_backup_invalid(
            "The restore archive contains trailing unaccounted bytes.",
        ));
    }
    let after = std::fs::symlink_metadata(archive_path).map_err(|error| {
        state_backup_io(format!(
            "The state restore archive cannot be reinspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != opened.len()
        || opened_modified.is_some_and(|modified| after.modified().ok() != Some(modified))
    {
        return Err(state_backup_invalid(
            "The state restore archive changed while candidates were staged.",
        ));
    }
    validate_candidate_tree(data_candidate_root, StateBackupRoot::Data, &selected)?;
    validate_candidate_tree(state_candidate_root, StateBackupRoot::State, &selected)?;
    Ok(())
}

pub(super) fn validate_restore_entries(
    expected_manifest: &StateBackupManifest,
    selected_entries: &[StateBackupEntry],
    data_candidate_root: &Path,
    state_candidate_root: &Path,
) -> UseResult<()> {
    expected_manifest.validate()?;
    let selected = selected_restore_entries(expected_manifest, selected_entries)?;
    validate_candidate_tree(data_candidate_root, StateBackupRoot::Data, &selected)?;
    validate_candidate_tree(state_candidate_root, StateBackupRoot::State, &selected)
}

fn selected_restore_entries(
    expected_manifest: &StateBackupManifest,
    selected_entries: &[StateBackupEntry],
) -> UseResult<BTreeMap<(StateBackupRoot, String), StateBackupEntry>> {
    let manifest_entries = expected_manifest
        .entries
        .iter()
        .map(|entry| ((entry.root, entry.path.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeMap::new();
    for entry in selected_entries {
        let key = (entry.root, entry.path.clone());
        if manifest_entries
            .get(&(entry.root, entry.path.as_str()))
            .is_none_or(|manifest_entry| *manifest_entry != entry)
            || selected.insert(key, entry.clone()).is_some()
        {
            return Err(state_backup_invalid(
                "The restore candidate selection is not an exact subset of the reviewed backup.",
            ));
        }
    }
    Ok(selected)
}

pub(super) fn validate_manifest(manifest: &StateBackupManifest) -> UseResult<()> {
    if manifest.schema != A3S_USE_STATE_BACKUP_SCHEMA
        || manifest.installation.validate().is_err()
        || !valid_identity(&manifest.use_version)
        || !valid_identity(&manifest.os)
        || !valid_identity(&manifest.architecture)
    {
        return Err(state_backup_invalid(
            "The state backup manifest identity is invalid or unsupported.",
        ));
    }
    if !valid_digest(&manifest.inventory_digest)
        || !valid_digest(&manifest.authority.registry_digest)
    {
        return Err(state_backup_invalid(
            "The state backup manifest contains an invalid digest.",
        ));
    }
    if manifest.authority.packages.len() as u64 > MAX_STATE_BACKUP_FILES {
        return Err(state_backup_invalid(
            "The state backup package authority exceeds its bound.",
        ));
    }
    let mut prior_package = None;
    for package in &manifest.authority.packages {
        if a3s_use_core::PluginPackageId::parse(package.package_id.clone()).is_err()
            || !valid_digest(&package.receipt_digest)
            || prior_package.is_some_and(|prior| prior >= package.package_id.as_str())
        {
            return Err(state_backup_invalid(
                "The state backup package authority is invalid or unsorted.",
            ));
        }
        prior_package = Some(package.package_id.as_str());
    }

    let mut prior_entry: Option<(&super::StateBackupRoot, &str)> = None;
    let mut portable_paths = BTreeSet::new();
    for entry in &manifest.entries {
        validate_archived_path(entry.root, &entry.path)?;
        if !valid_digest(&entry.sha256) || expected_family(entry.root, &entry.path)? != entry.family
        {
            return Err(state_backup_invalid(
                "The state backup manifest contains an invalid entry.",
            ));
        }
        let key = (&entry.root, entry.path.as_str());
        if prior_entry.is_some_and(|prior| prior >= key) {
            return Err(state_backup_invalid(
                "The state backup manifest entries are not uniquely sorted.",
            ));
        }
        if entry.unix_mode.is_some_and(|mode| mode > 0o7777)
            || !portable_paths.insert((entry.root, entry.path.to_ascii_lowercase()))
        {
            return Err(state_backup_invalid(
                "The state backup manifest contains an invalid mode or portable path collision.",
            ));
        }
        prior_entry = Some(key);
    }
    let (file_count, byte_count) = validate_inventory_bounds(manifest.entries.iter())?;
    if manifest.file_count != file_count || manifest.byte_count != byte_count {
        return Err(state_backup_invalid(
            "The state backup manifest accounting does not match its entries.",
        ));
    }
    if sha256_digest(&canonical_json(&manifest.entries)?) != manifest.inventory_digest {
        return Err(state_backup_invalid(
            "The state backup inventory digest does not match its entries.",
        ));
    }
    if summarize_families(&manifest.entries)? != manifest.families {
        return Err(state_backup_invalid(
            "The state backup family summaries do not match its entries.",
        ));
    }
    Ok(())
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
}

fn copy_verified_file(
    writer: &mut impl Write,
    path: &Path,
    entry: &StateBackupEntry,
) -> UseResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        state_backup_io(format!(
            "A Use-owned backup payload cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != entry.length
    {
        return Err(super::state_backup_nonterminal(
            "A Use-owned backup payload changed before it was copied.",
        ));
    }
    let mut reader = BufReader::new(File::open(path).map_err(|error| {
        state_backup_io(format!(
            "A Use-owned backup payload cannot be opened: {error}"
        ))
    })?);
    let mut remaining = entry.length;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..requested]).map_err(|error| {
            state_backup_io(format!(
                "A Use-owned backup payload cannot be read: {error}"
            ))
        })?;
        if read == 0 {
            return Err(super::state_backup_nonterminal(
                "A Use-owned backup payload was truncated while it was copied.",
            ));
        }
        writer.write_all(&buffer[..read]).map_err(|error| {
            state_backup_io(format!("A backup payload cannot be written: {error}"))
        })?;
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut trailing = [0u8; 1];
    if reader.read(&mut trailing).map_err(|error| {
        state_backup_io(format!(
            "A Use-owned backup payload cannot be finished: {error}"
        ))
    })? != 0
    {
        return Err(super::state_backup_nonterminal(
            "A Use-owned backup payload grew while it was copied.",
        ));
    }
    let digest = format!("sha256:{:x}", hasher.finalize());
    if digest != entry.sha256 {
        return Err(super::state_backup_nonterminal(
            "A Use-owned backup payload changed while it was copied.",
        ));
    }
    Ok(())
}

fn reject_existing_destination(destination: &Path) -> UseResult<()> {
    match std::fs::symlink_metadata(destination) {
        Ok(_) => Err(state_backup_exists()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(state_backup_io(format!(
            "The backup destination cannot be inspected: {error}"
        ))),
    }
}

fn read_exact_invalid(reader: &mut impl Read, buffer: &mut [u8]) -> UseResult<()> {
    reader
        .read_exact(buffer)
        .map_err(|_| state_backup_invalid("The state backup archive is truncated."))
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> UseResult<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            state_backup_io(format!(
                "The backup directory cannot be synchronized: {error}"
            ))
        })
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}
