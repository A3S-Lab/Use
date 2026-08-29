use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use a3s_use_core::UseResult;
use a3s_use_extension::{ExtensionPaths, ACTIVE_STATE_RESTORE_MARKER};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, sha256_digest, state_backup_invalid, state_backup_layout_unsupported,
    state_backup_limit, state_backup_nonterminal, state_backup_path_invalid, StateBackupEntry,
    StateBackupFamily, StateBackupFamilySummary, StateBackupRoot, MAX_STATE_BACKUP_BYTES,
    MAX_STATE_BACKUP_ENTRIES, MAX_STATE_BACKUP_FILES, MAX_STATE_BACKUP_FILE_BYTES,
    MAX_STATE_BACKUP_PATH_BYTES,
};
use crate::installation_state_layout;

const MAX_STATE_BACKUP_DEPTH: usize = 32;
const READ_BUFFER_BYTES: usize = 128 * 1024;

#[derive(Debug)]
struct ScannedFile {
    entry: StateBackupEntry,
    absolute_path: PathBuf,
}

pub(super) fn reject_active_restore(state_root: &Path) -> UseResult<()> {
    let marker = state_root.join(ACTIVE_STATE_RESTORE_MARKER);
    match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(state_backup_path_invalid(
                    "The active restore marker is not an owned regular file.",
                ));
            }
            Err(state_backup_nonterminal(
                "A durable state restore is still active.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(super::state_backup_io(format!(
            "The active restore marker cannot be inspected: {error}"
        ))),
    }
}

pub(super) fn scan(paths: &ExtensionPaths) -> UseResult<Vec<StateBackupEntry>> {
    Ok(scan_files(paths, None)?
        .into_iter()
        .map(|file| file.entry)
        .collect())
}

pub(super) fn scan_with_paths(
    paths: &ExtensionPaths,
) -> UseResult<Vec<(StateBackupEntry, PathBuf)>> {
    Ok(scan_files(paths, None)?
        .into_iter()
        .map(|file| (file.entry, file.absolute_path))
        .collect())
}

pub(crate) fn scan_for_state_restore(
    paths: &ExtensionPaths,
    active_plan_digest: Option<&str>,
) -> UseResult<Vec<StateBackupEntry>> {
    Ok(scan_files(paths, active_plan_digest)?
        .into_iter()
        .map(|file| file.entry)
        .collect())
}

fn scan_files(
    paths: &ExtensionPaths,
    active_plan_digest: Option<&str>,
) -> UseResult<Vec<ScannedFile>> {
    if active_plan_digest.is_none() {
        reject_active_restore(paths.state_root())?;
    }
    let mut files = Vec::new();
    let mut visited_entries = 0u64;
    let mut portable_paths = BTreeSet::new();
    scan_root(
        paths.data_root(),
        StateBackupRoot::Data,
        &mut files,
        &mut visited_entries,
        &mut portable_paths,
        active_plan_digest,
    )?;
    scan_root(
        paths.state_root(),
        StateBackupRoot::State,
        &mut files,
        &mut visited_entries,
        &mut portable_paths,
        active_plan_digest,
    )?;
    files.sort_by(|left, right| left.entry.cmp_key().cmp(&right.entry.cmp_key()));
    validate_inventory_bounds(files.iter().map(|file| &file.entry))?;
    Ok(files)
}

fn scan_root(
    root: &Path,
    kind: StateBackupRoot,
    files: &mut Vec<ScannedFile>,
    visited_entries: &mut u64,
    portable_paths: &mut BTreeSet<(StateBackupRoot, String)>,
    active_plan_digest: Option<&str>,
) -> UseResult<()> {
    let root_metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(super::state_backup_io(format!(
                "A Use-owned backup root cannot be inspected: {error}"
            )))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&root_metadata) || !root_metadata.is_dir() {
        return Err(state_backup_path_invalid(
            "A Use-owned backup root is not an owned directory.",
        ));
    }

    let mut stack = vec![(root.to_path_buf(), PathBuf::new(), 0usize)];
    while let Some((directory, relative_directory, depth)) = stack.pop() {
        if depth > MAX_STATE_BACKUP_DEPTH {
            return Err(state_backup_limit(
                "The Use state tree exceeds the supported directory depth.",
            ));
        }
        let directory_entries = std::fs::read_dir(&directory).map_err(|error| {
            super::state_backup_io(format!(
                "A Use-owned backup directory cannot be read: {error}"
            ))
        })?;
        let mut entries = Vec::new();
        for entry in directory_entries {
            let entry = entry.map_err(|error| {
                super::state_backup_io(format!(
                    "A Use-owned backup directory entry cannot be read: {error}"
                ))
            })?;
            *visited_entries = visited_entries.checked_add(1).ok_or_else(|| {
                state_backup_limit("The Use state entry count overflowed its bound.")
            })?;
            if *visited_entries > MAX_STATE_BACKUP_ENTRIES {
                return Err(state_backup_limit(
                    "The Use state tree exceeds its filesystem-entry bound.",
                ));
            }
            entries.push(entry);
        }
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for directory_entry in entries.into_iter().rev() {
            let name = directory_entry.file_name().into_string().map_err(|_| {
                state_backup_path_invalid("Use-owned backup paths must be valid UTF-8.")
            })?;
            validate_portable_segment(&name)?;
            let relative = relative_directory.join(&name);
            let portable = portable_path(&relative)?;
            if !portable_paths.insert((kind, portable.to_ascii_lowercase())) {
                return Err(state_backup_path_invalid(
                    "Use-owned backup state contains case-insensitive path collisions.",
                ));
            }
            let absolute = directory_entry.path();
            let metadata = std::fs::symlink_metadata(&absolute).map_err(|error| {
                super::state_backup_io(format!(
                    "A Use-owned backup entry cannot be inspected: {error}"
                ))
            })?;
            if excluded_active_restore_entry(kind, &relative, &metadata, active_plan_digest)? {
                continue;
            }
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(state_backup_path_invalid(
                    "Use-owned backup state contains a link or reparse point.",
                ));
            }
            validate_layout(kind, &relative, metadata.is_dir())?;
            if is_nonterminal(kind, &relative, &absolute, &metadata)? {
                return Err(state_backup_nonterminal(
                    "Use-owned state contains temporary, partial, active, or artifact-staging evidence.",
                ));
            }
            if metadata.is_dir() {
                stack.push((absolute, relative, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                return Err(state_backup_path_invalid(
                    "Use-owned backup state contains a special filesystem entry.",
                ));
            }
            if excluded_lock(kind, &relative) {
                continue;
            }
            let family = expected_family(kind, &portable)?;
            let (length, digest) = hash_file(&absolute, &metadata)?;
            files.push(ScannedFile {
                entry: StateBackupEntry {
                    root: kind,
                    path: portable,
                    family,
                    length,
                    sha256: digest,
                    read_only: metadata.permissions().readonly(),
                    unix_mode: unix_mode(&metadata),
                },
                absolute_path: absolute,
            });
            if files.len() as u64 > MAX_STATE_BACKUP_FILES {
                return Err(state_backup_limit(
                    "The Use state inventory exceeds its file-count bound.",
                ));
            }
        }
    }
    Ok(())
}

fn excluded_active_restore_entry(
    root: StateBackupRoot,
    relative: &Path,
    metadata: &std::fs::Metadata,
    active_plan_digest: Option<&str>,
) -> UseResult<bool> {
    let Some(digest) = active_plan_digest else {
        return Ok(false);
    };
    let digest = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| state_backup_invalid("The active state restore plan digest is invalid."))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(state_backup_invalid(
            "The active state restore plan digest is invalid.",
        ));
    }
    let parts = relative
        .components()
        .map(|component| normal_component(Some(component)))
        .collect::<UseResult<Vec<_>>>()?;
    let candidate_name = format!(".state-restore-{digest}");
    let expected = match (root, parts.as_slice()) {
        (StateBackupRoot::Data | StateBackupRoot::State, [name]) if *name == candidate_name => {
            Some(true)
        }
        (StateBackupRoot::State, [name]) if *name == ACTIVE_STATE_RESTORE_MARKER => Some(false),
        (StateBackupRoot::State, ["operations", "state-restores", operation])
            if *operation == digest =>
        {
            Some(true)
        }
        _ => None,
    };
    let Some(directory) = expected else {
        return Ok(false);
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || directory != metadata.is_dir()
        || (!directory && !metadata.is_file())
    {
        return Err(state_backup_path_invalid(
            "Active state restore evidence is not an owned file or directory.",
        ));
    }
    Ok(true)
}

fn validate_layout(root: StateBackupRoot, relative: &Path, directory: bool) -> UseResult<()> {
    if root == StateBackupRoot::Data {
        return Err(state_backup_layout_unsupported(
            "Installation data payloads are not portable authority; immutable package bytes belong to the global Artifact Store.",
        ));
    }
    let mut components = relative.components();
    let first = normal_component(components.next())?;
    if components.next().is_none()
        && !installation_state_layout::supported_root_entry(first, directory)
    {
        return Err(state_backup_layout_unsupported(
            "The Use state root contains an unknown top-level state family.",
        ));
    }
    let parts = relative
        .components()
        .map(|component| normal_component(Some(component)))
        .collect::<UseResult<Vec<_>>>()?;
    if parts.first() == Some(&"operations")
        && parts.len() >= 2
        && !installation_state_layout::supported_operation_directory(parts[1])
    {
        return Err(state_backup_layout_unsupported(
            "The Use operations root contains an unknown state family.",
        ));
    }
    if parts.first() == Some(&"bindings")
        && parts.len() >= 2
        && !installation_state_layout::supported_binding_directory(parts[1])
    {
        return Err(state_backup_layout_unsupported(
            "The Use binding root contains an unknown state family.",
        ));
    }
    Ok(())
}

fn normal_component(component: Option<Component<'_>>) -> UseResult<&str> {
    match component {
        Some(Component::Normal(value)) => value.to_str().ok_or_else(|| {
            state_backup_path_invalid("Use-owned backup paths must be valid UTF-8.")
        }),
        _ => Err(state_backup_path_invalid(
            "Use-owned backup paths must remain relative and normalized.",
        )),
    }
}

fn excluded_lock(root: StateBackupRoot, relative: &Path) -> bool {
    if root != StateBackupRoot::State {
        return false;
    }
    let Some(name) = relative.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if relative.components().count() == 1 {
        return installation_state_layout::excluded_root_lock(name);
    }
    name.ends_with(".lock")
}

fn is_nonterminal(
    root: StateBackupRoot,
    relative: &Path,
    absolute: &Path,
    metadata: &std::fs::Metadata,
) -> UseResult<bool> {
    let Some(name) = relative.file_name().and_then(|value| value.to_str()) else {
        return Ok(true);
    };
    let temporary = name.ends_with(".tmp")
        || name.contains(".tmp-")
        || name.ends_with(".part")
        || name.ends_with(".partial")
        || is_artifact_staging_name(name);
    match root {
        StateBackupRoot::Data => Ok(true),
        StateBackupRoot::State => {
            if name == ACTIVE_STATE_RESTORE_MARKER || temporary {
                return Ok(true);
            }
            if metadata.is_dir() {
                return Ok(false);
            }
            let portable = portable_path(relative)?;
            let parts = portable.split('/').collect::<Vec<_>>();
            match parts.as_slice() {
                ["operations", "plugins", .., "active.json"] => {
                    terminal_json_field(absolute, metadata, "status", &["completed", "rolled-back"])
                        .map(|terminal| !terminal)
                }
                ["operations", "package-graphs", ..] | ["operations", "package-downloads", ..]
                    if !excluded_lock(root, relative) =>
                {
                    Ok(true)
                }
                ["operations", "state-restores", _, "operation.json"] => {
                    terminal_json_field(absolute, metadata, "status", &["completed"])
                        .map(|terminal| !terminal)
                }
                ["operations", "package-resolutions", ..] if name.ends_with(".json") => {
                    terminal_json_field(absolute, metadata, "status", &["resolved", "failed"])
                        .map(|terminal| !terminal)
                }
                ["grants", ".operations", ..] if name.ends_with(".json") => {
                    terminal_json_field(absolute, metadata, "phase", &["completed", "rolled-back"])
                        .map(|terminal| !terminal)
                }
                ["bindings", "runtime", ..] if name.ends_with(".provisioning.json") => Ok(true),
                ["package-enablement", "scopes", .., "state.json"] => {
                    let value = read_bounded_json(absolute, metadata)?;
                    Ok(!value.get("active").is_none_or(serde_json::Value::is_null))
                }
                _ => Ok(false),
            }
        }
    }
}

fn terminal_json_field(
    path: &Path,
    metadata: &std::fs::Metadata,
    field: &str,
    terminal: &[&str],
) -> UseResult<bool> {
    let value = read_bounded_json(path, metadata)?;
    let status = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            state_backup_invalid("A durable operation record omits its terminal-state field.")
        })?;
    Ok(terminal.contains(&status))
}

fn read_bounded_json(path: &Path, metadata: &std::fs::Metadata) -> UseResult<serde_json::Value> {
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_STATE_BACKUP_FILE_BYTES {
        return Err(state_backup_invalid(
            "A durable operation record is not a bounded regular file.",
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        super::state_backup_io(format!(
            "A durable operation record cannot be read: {error}"
        ))
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(state_backup_nonterminal(
            "A durable operation record changed while its state was inspected.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| state_backup_invalid("A durable operation record contains invalid JSON."))
}

pub(super) fn expected_family(root: StateBackupRoot, path: &str) -> UseResult<StateBackupFamily> {
    let mut parts = path.split('/');
    let first = parts.next().unwrap_or_default();
    match root {
        StateBackupRoot::Data => Err(state_backup_layout_unsupported(
            "The backup manifest cannot contain installation data payloads or global artifacts.",
        )),
        StateBackupRoot::State => match first {
            "extensions" | "registry.json" => Ok(StateBackupFamily::Registry),
            "extension-generations" => Ok(StateBackupFamily::RetainedGenerations),
            "grants" => Ok(StateBackupFamily::Grants),
            "bindings" => Ok(StateBackupFamily::Bindings),
            "operations" => match parts.next() {
                Some("plugins") => Ok(StateBackupFamily::LifecycleOperations),
                Some(
                    "package-graphs"
                    | "package-downloads"
                    | "package-resolutions"
                    | "package-diagnostic-history"
                    | "state-restores",
                ) => Ok(StateBackupFamily::PackageOperations),
                _ => Err(state_backup_layout_unsupported(
                    "The backup manifest contains an unknown operation family.",
                )),
            },
            "installation-snapshot.json" => Ok(StateBackupFamily::PackageGraph),
            "knowledge" => Ok(StateBackupFamily::Knowledge),
            "package-enablement" => Ok(StateBackupFamily::Enablement),
            "plugin-host-manager" => Ok(StateBackupFamily::HostManager),
            "generation-leases" => Err(state_backup_invalid(
                "The backup manifest must not contain generation lease files.",
            )),
            _ => Err(state_backup_layout_unsupported(
                "The backup manifest contains an unknown state family.",
            )),
        },
    }
}

fn hash_file(path: &Path, expected: &std::fs::Metadata) -> UseResult<(u64, String)> {
    if expected.len() > MAX_STATE_BACKUP_FILE_BYTES {
        return Err(state_backup_limit(
            "A Use-owned state file exceeds the per-file backup bound.",
        ));
    }
    let mut file = File::open(path).map_err(|error| {
        super::state_backup_io(format!("A Use-owned state file cannot be opened: {error}"))
    })?;
    let opened = file.metadata().map_err(|error| {
        super::state_backup_io(format!(
            "A Use-owned state file cannot be inspected: {error}"
        ))
    })?;
    if !opened.is_file() || opened.len() != expected.len() {
        return Err(state_backup_nonterminal(
            "A Use-owned state file changed while its inventory was read.",
        ));
    }
    let mut hasher = Sha256::new();
    let mut length = 0u64;
    let mut buffer = vec![0u8; READ_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            super::state_backup_io(format!("A Use-owned state file cannot be read: {error}"))
        })?;
        if read == 0 {
            break;
        }
        length = length.checked_add(read as u64).ok_or_else(|| {
            state_backup_limit("A Use-owned state file length overflowed its bound.")
        })?;
        if length > MAX_STATE_BACKUP_FILE_BYTES {
            return Err(state_backup_limit(
                "A Use-owned state file exceeds the per-file backup bound.",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    if length != expected.len() {
        return Err(state_backup_nonterminal(
            "A Use-owned state file changed while its inventory was read.",
        ));
    }
    let after = std::fs::symlink_metadata(path).map_err(|error| {
        super::state_backup_io(format!(
            "A Use-owned state file cannot be reinspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != expected.len()
        || after.permissions().readonly() != expected.permissions().readonly()
        || unix_mode(&after) != unix_mode(expected)
    {
        return Err(state_backup_nonterminal(
            "A Use-owned state file changed while its inventory was read.",
        ));
    }
    Ok((length, format!("sha256:{:x}", hasher.finalize())))
}

fn validate_portable_segment(segment: &str) -> UseResult<()> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.len() > 255
        || segment.ends_with([' ', '.'])
        || segment.bytes().any(|byte| {
            byte < 0x20
                || byte == 0x7f
                || matches!(
                    byte,
                    b'<' | b'>' | b':' | b'"' | b'/' | b'\\' | b'|' | b'?' | b'*'
                )
        })
    {
        return Err(state_backup_path_invalid(
            "Use-owned backup state contains a non-portable path segment.",
        ));
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem.strip_prefix("COM").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
        || stem.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
    {
        return Err(state_backup_path_invalid(
            "Use-owned backup state contains a Windows-reserved path segment.",
        ));
    }
    Ok(())
}

pub(super) fn validate_portable_path(path: &str) -> UseResult<()> {
    if path.is_empty() || path.len() > MAX_STATE_BACKUP_PATH_BYTES || path.starts_with('/') {
        return Err(state_backup_path_invalid(
            "A backup manifest path is empty, absolute, or exceeds its bound.",
        ));
    }
    for segment in path.split('/') {
        validate_portable_segment(segment)?;
    }
    Ok(())
}

fn portable_path(path: &Path) -> UseResult<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        let Component::Normal(segment) = component else {
            return Err(state_backup_path_invalid(
                "Use-owned backup paths must remain relative and normalized.",
            ));
        };
        segments.push(segment.to_str().ok_or_else(|| {
            state_backup_path_invalid("Use-owned backup paths must be valid UTF-8.")
        })?);
    }
    let portable = segments.join("/");
    validate_portable_path(&portable)?;
    Ok(portable)
}

pub(super) fn validate_archived_path(root: StateBackupRoot, path: &str) -> UseResult<()> {
    validate_portable_path(path)?;
    let parts = path.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|segment| is_artifact_staging_name(segment))
    {
        return Err(state_backup_invalid(
            "A backup manifest contains artifact-staging evidence.",
        ));
    }
    if root == StateBackupRoot::State {
        let name = parts.last().copied().unwrap_or_default();
        if name == ACTIVE_STATE_RESTORE_MARKER
            || name.ends_with(".lock")
            || name.ends_with(".tmp")
            || name.contains(".tmp-")
            || name.ends_with(".part")
            || name.ends_with(".partial")
            || name.ends_with(".provisioning.json")
            || matches!(parts.as_slice(), ["operations", "package-graphs", ..])
            || matches!(parts.as_slice(), ["operations", "package-downloads", ..])
        {
            return Err(state_backup_invalid(
                "A backup manifest contains excluded lock or nonterminal evidence.",
            ));
        }
    }
    Ok(())
}

fn is_artifact_staging_name(name: &str) -> bool {
    name.starts_with(".artifact-staging-") || name.starts_with(".lifecycle-staging-")
}

pub(super) fn validate_inventory_bounds<'a>(
    entries: impl IntoIterator<Item = &'a StateBackupEntry>,
) -> UseResult<(u64, u64)> {
    let mut file_count = 0u64;
    let mut byte_count = 0u64;
    for entry in entries {
        file_count = file_count
            .checked_add(1)
            .ok_or_else(|| state_backup_limit("The backup file count overflowed."))?;
        byte_count = byte_count
            .checked_add(entry.length)
            .ok_or_else(|| state_backup_limit("The backup byte count overflowed."))?;
        if file_count > MAX_STATE_BACKUP_FILES || byte_count > MAX_STATE_BACKUP_BYTES {
            return Err(state_backup_limit(
                "The Use state inventory exceeds its file or byte bound.",
            ));
        }
        if entry.length > MAX_STATE_BACKUP_FILE_BYTES {
            return Err(state_backup_limit(
                "A backup manifest entry exceeds the per-file byte bound.",
            ));
        }
    }
    Ok((file_count, byte_count))
}

pub(super) fn summarize_families(
    entries: &[StateBackupEntry],
) -> UseResult<Vec<StateBackupFamilySummary>> {
    let mut grouped = BTreeMap::<StateBackupFamily, Vec<&StateBackupEntry>>::new();
    for entry in entries {
        grouped.entry(entry.family).or_default().push(entry);
    }
    grouped
        .into_iter()
        .map(|(family, family_entries)| {
            let file_count = family_entries.len() as u64;
            let byte_count = family_entries.iter().try_fold(0u64, |total, entry| {
                total.checked_add(entry.length).ok_or_else(|| {
                    state_backup_limit("A backup family byte count overflowed its bound.")
                })
            })?;
            let canonical = canonical_json(&family_entries)?;
            Ok(StateBackupFamilySummary {
                family,
                file_count,
                byte_count,
                inventory_digest: sha256_digest(&canonical),
            })
        })
        .collect()
}

#[cfg(unix)]
fn unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn unix_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

impl StateBackupEntry {
    fn cmp_key(&self) -> (StateBackupRoot, &str) {
        (self.root, &self.path)
    }
}
