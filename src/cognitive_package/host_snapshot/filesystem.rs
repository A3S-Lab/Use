use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncReadExt;

use super::{
    host_snapshot_invalid, scope_digest, valid_hex_digest,
    validate_host_projection_snapshot_record, validate_host_projection_snapshot_set,
    HostProjectionSnapshotInventory, HostProjectionSnapshotRecord,
    HostProjectionSnapshotRecordKind, HostProjectionSnapshotSource,
};
use crate::cognitive_package::host_store::{
    operation_binding_digest, sha256_hex, StoredPluginHostCancellation,
    StoredPluginHostEnablementDiagnosticIndex, StoredPluginHostOperationIndex,
    StoredPluginHostRequest, MAX_HOST_RECORD_BYTES,
};

const MAX_LOCK_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
struct ScannedRequest {
    stored: StoredPluginHostRequest,
    source: HostProjectionSnapshotSource,
}

#[derive(Debug)]
struct ScannedCancellation {
    stored: StoredPluginHostCancellation,
    source: HostProjectionSnapshotSource,
    exact_alias: bool,
}

#[derive(Debug)]
struct ScannedOperationIndex {
    scope_digest: String,
    file_name: String,
    index: StoredPluginHostOperationIndex,
}

#[derive(Debug)]
struct ScannedDiagnosticIndex {
    kind: String,
    scope_storage_key: String,
    file_name: String,
    index: StoredPluginHostEnablementDiagnosticIndex,
}

#[derive(Debug)]
struct ScanBounds {
    max_files: u64,
    max_bytes: u64,
    max_entries: u64,
    files: u64,
    entries: u64,
    bytes: u64,
}

impl ScanBounds {
    fn new(max_files: u64, max_bytes: u64) -> UseResult<Self> {
        if max_files == 0 || max_bytes == 0 {
            return Err(host_snapshot_invalid(
                "The Host projection scan requires nonzero safety bounds.",
            ));
        }
        Ok(Self {
            max_files,
            max_bytes,
            max_entries: max_files.saturating_mul(8).min(800_000),
            files: 0,
            entries: 0,
            bytes: 0,
        })
    }

    fn visit_entry(&mut self) -> UseResult<()> {
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| host_snapshot_invalid("Host filesystem accounting overflowed."))?;
        if self.entries > self.max_entries {
            return Err(host_snapshot_invalid(
                "The Host projection exceeds its filesystem entry bound.",
            ));
        }
        Ok(())
    }

    fn visit_file(&mut self, length: u64, payload: bool) -> UseResult<()> {
        self.files = self
            .files
            .checked_add(1)
            .ok_or_else(|| host_snapshot_invalid("Host file accounting overflowed."))?;
        if self.files > self.max_files {
            return Err(host_snapshot_invalid(
                "The Host projection exceeds its registered file bound.",
            ));
        }
        if payload {
            self.bytes = self
                .bytes
                .checked_add(length)
                .ok_or_else(|| host_snapshot_invalid("Host byte accounting overflowed."))?;
            if self.bytes > self.max_bytes {
                return Err(host_snapshot_invalid(
                    "The Host projection exceeds its registered byte bound.",
                ));
            }
        }
        Ok(())
    }
}

pub(crate) async fn scan_host_projection_snapshot(
    state_root: &Path,
    installation: &InstallationId,
    max_files: u64,
    max_bytes: u64,
) -> UseResult<HostProjectionSnapshotInventory> {
    installation
        .validate()
        .map_err(|_| host_snapshot_invalid("The Host projection installation is invalid."))?;
    owned_directory(state_root, "state root").await?;
    let host_root = state_root.join("plugin-host-manager");
    if !optional_owned_directory(&host_root, "Host projection root").await? {
        return Ok(HostProjectionSnapshotInventory {
            sources: Vec::new(),
            validated_index_records: 0,
        });
    }

    let mut bounds = ScanBounds::new(max_files, max_bytes)?;
    let mut requests = BTreeMap::new();
    let mut cancellations = BTreeMap::<(String, String, String), Vec<ScannedCancellation>>::new();
    let mut operation_indexes = Vec::new();
    let mut diagnostics = Vec::new();

    for (name, path, metadata) in children(&host_root, &mut bounds).await? {
        if metadata.is_dir() {
            if name == "diagnostics" {
                scan_diagnostics(&path, &mut bounds, &mut diagnostics).await?;
            } else if valid_hex_digest(&name) {
                scan_scope(
                    &path,
                    &name,
                    installation,
                    &mut bounds,
                    &mut requests,
                    &mut cancellations,
                    &mut operation_indexes,
                )
                .await?;
            } else {
                return Err(host_snapshot_invalid(
                    "The Host projection contains an unknown root directory.",
                ));
            }
        } else {
            return Err(host_snapshot_invalid(
                "The Host projection root contains an unsupported file.",
            ));
        }
    }

    let validated_operation_indexes = validate_operation_indexes(&requests, &operation_indexes)?;
    let validated_diagnostics = validate_diagnostic_indexes(&requests, &diagnostics)?;
    let mut sources = requests
        .into_values()
        .map(|request| request.source)
        .collect::<Vec<_>>();
    for aliases in cancellations.into_values() {
        let selected = select_cancellation_alias(aliases)?;
        sources.push(selected.source);
    }
    sources.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    if sources
        .windows(2)
        .any(|pair| pair[0].logical_path >= pair[1].logical_path)
    {
        return Err(host_snapshot_invalid(
            "The Host semantic inventory is not uniquely ordered.",
        ));
    }
    let records = sources
        .iter()
        .map(|source| source.record.clone())
        .collect::<Vec<_>>();
    validate_host_projection_snapshot_set(&records, installation)?;
    Ok(HostProjectionSnapshotInventory {
        sources,
        validated_index_records: validated_operation_indexes
            .checked_add(validated_diagnostics)
            .ok_or_else(|| host_snapshot_invalid("Host index accounting overflowed."))?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn scan_scope(
    scope_root: &Path,
    scope_path_digest: &str,
    installation: &InstallationId,
    bounds: &mut ScanBounds,
    requests: &mut BTreeMap<(String, String), ScannedRequest>,
    cancellations: &mut BTreeMap<(String, String, String), Vec<ScannedCancellation>>,
    operation_indexes: &mut Vec<ScannedOperationIndex>,
) -> UseResult<()> {
    for (name, path, metadata) in children(scope_root, bounds).await? {
        if metadata.is_file() {
            bounds.visit_file(metadata.len(), false)?;
            if name != ".store.lock" || metadata.len() > MAX_LOCK_FILE_BYTES {
                return Err(host_snapshot_invalid(
                    "A Host scope contains an unsupported or oversized root file.",
                ));
            }
            continue;
        }
        if !metadata.is_dir() {
            return Err(host_snapshot_invalid(
                "A Host scope contains a special filesystem entry.",
            ));
        }
        match name.as_str() {
            "requests" => {
                for (file_name, source, metadata) in children(&path, bounds).await? {
                    require_regular_record(&metadata)?;
                    bounds.visit_file(metadata.len(), true)?;
                    let bytes = read_owned_record(&source, metadata.len()).await?;
                    let logical_path = format!("{scope_path_digest}/requests/{file_name}");
                    let record = validate_host_projection_snapshot_record(
                        &logical_path,
                        &bytes,
                        installation,
                    )?;
                    let HostProjectionSnapshotRecord::Request(decoded) = &record else {
                        return Err(host_snapshot_invalid(
                            "A Host request directory contained another record kind.",
                        ));
                    };
                    let stored: StoredPluginHostRequest = serde_json::from_slice(&bytes)
                        .map_err(|_| host_snapshot_invalid("A Host request record is invalid."))?;
                    let key = (scope_path_digest.to_owned(), decoded.request_id.clone());
                    let scanned = ScannedRequest {
                        stored,
                        source: snapshot_source(
                            source,
                            logical_path,
                            HostProjectionSnapshotRecordKind::Request,
                            metadata.len(),
                            &bytes,
                            record,
                        ),
                    };
                    if requests.insert(key, scanned).is_some() {
                        return Err(host_snapshot_invalid(
                            "A Host scope contains duplicate request records.",
                        ));
                    }
                }
            }
            "operations" => {
                for (file_name, source, metadata) in children(&path, bounds).await? {
                    require_regular_record(&metadata)?;
                    bounds.visit_file(metadata.len(), true)?;
                    let bytes = read_owned_record(&source, metadata.len()).await?;
                    let index: StoredPluginHostOperationIndex = serde_json::from_slice(&bytes)
                        .map_err(|_| host_snapshot_invalid("A Host operation index is invalid."))?;
                    index.validate().map_err(|_| {
                        host_snapshot_invalid("A Host operation index failed owner validation.")
                    })?;
                    operation_indexes.push(ScannedOperationIndex {
                        scope_digest: scope_path_digest.to_owned(),
                        file_name,
                        index,
                    });
                }
            }
            "cancellations" => {
                for (file_name, source, metadata) in children(&path, bounds).await? {
                    require_regular_record(&metadata)?;
                    bounds.visit_file(metadata.len(), true)?;
                    let bytes = read_owned_record(&source, metadata.len()).await?;
                    let stored: StoredPluginHostCancellation = serde_json::from_slice(&bytes)
                        .map_err(|_| host_snapshot_invalid("A Host cancellation is invalid."))?;
                    stored.validate().map_err(|_| {
                        host_snapshot_invalid("A Host cancellation failed owner validation.")
                    })?;
                    let exact_name = format!(
                        "{}.json",
                        operation_binding_digest(&stored.operation_id, &stored.plan_digest)
                    );
                    let legacy_name =
                        format!("{}.json", sha256_hex(stored.operation_id.as_bytes()));
                    let exact_alias = file_name == exact_name;
                    if !exact_alias && file_name != legacy_name {
                        return Err(host_snapshot_invalid(
                            "A Host cancellation does not match an exact or legacy owned path.",
                        ));
                    }
                    let logical_path = format!("{scope_path_digest}/cancellations/{exact_name}");
                    let record = validate_host_projection_snapshot_record(
                        &logical_path,
                        &bytes,
                        installation,
                    )?;
                    let key = (
                        scope_path_digest.to_owned(),
                        stored.operation_id.clone(),
                        stored.plan_digest.clone(),
                    );
                    cancellations
                        .entry(key)
                        .or_default()
                        .push(ScannedCancellation {
                            stored,
                            source: snapshot_source(
                                source,
                                logical_path,
                                HostProjectionSnapshotRecordKind::Cancellation,
                                metadata.len(),
                                &bytes,
                                record,
                            ),
                            exact_alias,
                        });
                }
            }
            "request-locks" | "operation-locks" => {
                scan_lock_directory(&path, bounds).await?;
            }
            _ => {
                return Err(host_snapshot_invalid(
                    "A Host scope contains an unknown directory layout.",
                ))
            }
        }
    }
    Ok(())
}

async fn scan_lock_directory(path: &Path, bounds: &mut ScanBounds) -> UseResult<()> {
    for (name, _, metadata) in children(path, bounds).await? {
        bounds.visit_file(metadata.len(), false)?;
        let digest = name.strip_suffix(".lock");
        if !metadata.is_file()
            || metadata.len() > MAX_LOCK_FILE_BYTES
            || digest.is_none_or(|digest| !valid_hex_digest(digest))
        {
            return Err(host_snapshot_invalid(
                "A Host lock directory contains an invalid entry.",
            ));
        }
    }
    Ok(())
}

async fn scan_diagnostics(
    diagnostics_root: &Path,
    bounds: &mut ScanBounds,
    diagnostics: &mut Vec<ScannedDiagnosticIndex>,
) -> UseResult<()> {
    let roots = children(diagnostics_root, bounds).await?;
    if roots
        .iter()
        .any(|(name, _, metadata)| name != "enablement" || !metadata.is_dir())
    {
        return Err(host_snapshot_invalid(
            "The Host diagnostics root contains an unknown layout.",
        ));
    }
    for (_, enablement_root, _) in roots {
        for (kind, kind_root, metadata) in children(&enablement_root, bounds).await? {
            if !metadata.is_dir() || !matches!(kind.as_str(), "user" | "workspace") {
                return Err(host_snapshot_invalid(
                    "A Host enablement diagnostic has an invalid scope-kind directory.",
                ));
            }
            for (scope_storage_key, scope_root, metadata) in children(&kind_root, bounds).await? {
                if !metadata.is_dir() || !valid_hex_digest(&scope_storage_key) {
                    return Err(host_snapshot_invalid(
                        "A Host enablement diagnostic has an invalid scope directory.",
                    ));
                }
                for (file_name, source, metadata) in children(&scope_root, bounds).await? {
                    bounds.visit_file(metadata.len(), file_name != ".store.lock")?;
                    if file_name == ".store.lock" {
                        if !metadata.is_file() || metadata.len() > MAX_LOCK_FILE_BYTES {
                            return Err(host_snapshot_invalid(
                                "A Host diagnostic lock is invalid or oversized.",
                            ));
                        }
                        continue;
                    }
                    require_regular_record(&metadata)?;
                    let bytes = read_owned_record(&source, metadata.len()).await?;
                    let index: StoredPluginHostEnablementDiagnosticIndex =
                        serde_json::from_slice(&bytes).map_err(|_| {
                            host_snapshot_invalid("A Host diagnostic index is invalid.")
                        })?;
                    index.validate().map_err(|_| {
                        host_snapshot_invalid("A Host diagnostic failed owner validation.")
                    })?;
                    diagnostics.push(ScannedDiagnosticIndex {
                        kind: kind.clone(),
                        scope_storage_key: scope_storage_key.clone(),
                        file_name,
                        index,
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_operation_indexes(
    requests: &BTreeMap<(String, String), ScannedRequest>,
    indexes: &[ScannedOperationIndex],
) -> UseResult<u64> {
    let mut expected = BTreeMap::new();
    for ((scope_path_digest, _), request) in requests {
        let Some(index) =
            StoredPluginHostOperationIndex::from_request(&request.stored).map_err(|_| {
                host_snapshot_invalid("A Host request cannot derive its operation index.")
            })?
        else {
            continue;
        };
        let key = (
            scope_path_digest.clone(),
            index.operation_id.clone(),
            index.plan_digest.clone(),
        );
        if expected.insert(key, index).is_some() {
            return Err(host_snapshot_invalid(
                "Host requests derive duplicate operation bindings.",
            ));
        }
    }

    let mut aliases = BTreeMap::<(String, String, String), BTreeSet<String>>::new();
    for scanned in indexes {
        let exact_name = format!(
            "{}.json",
            operation_binding_digest(&scanned.index.operation_id, &scanned.index.plan_digest)
        );
        let legacy_name = format!("{}.json", sha256_hex(scanned.index.operation_id.as_bytes()));
        if scanned.file_name != exact_name && scanned.file_name != legacy_name {
            return Err(host_snapshot_invalid(
                "A Host operation index moved outside its exact or legacy path.",
            ));
        }
        let key = (
            scanned.scope_digest.clone(),
            scanned.index.operation_id.clone(),
            scanned.index.plan_digest.clone(),
        );
        if expected.get(&key) != Some(&scanned.index) {
            return Err(host_snapshot_invalid(
                "A Host operation index is stale, orphaned, or disagrees with its request.",
            ));
        }
        if !aliases
            .entry(key)
            .or_default()
            .insert(scanned.file_name.clone())
        {
            return Err(host_snapshot_invalid(
                "A Host operation index alias appears more than once.",
            ));
        }
    }
    if expected.keys().any(|key| !aliases.contains_key(key)) {
        return Err(host_snapshot_invalid(
            "A Host request is missing its derived operation index.",
        ));
    }
    u64::try_from(indexes.len())
        .map_err(|_| host_snapshot_invalid("Host operation index accounting overflowed."))
}

fn validate_diagnostic_indexes(
    requests: &BTreeMap<(String, String), ScannedRequest>,
    diagnostics: &[ScannedDiagnosticIndex],
) -> UseResult<u64> {
    let mut expected =
        BTreeMap::<(String, String, String), StoredPluginHostEnablementDiagnosticIndex>::new();
    for request in requests.values() {
        let Some(index) = StoredPluginHostEnablementDiagnosticIndex::from_request(&request.stored)
            .map_err(|_| host_snapshot_invalid("A Host request cannot derive its diagnostic."))?
        else {
            continue;
        };
        let key = (
            index.scope.kind.as_str().to_owned(),
            index.scope.storage_key().map_err(|_| {
                host_snapshot_invalid("A Host diagnostic scope identity is invalid.")
            })?,
            index.package_id.clone(),
        );
        let replace = expected.get(&key).is_none_or(|current| {
            (index.planned_at_ms, index.request_id.as_str())
                > (current.planned_at_ms, current.request_id.as_str())
        });
        if replace {
            expected.insert(key, index);
        }
    }

    let mut observed = BTreeSet::new();
    for scanned in diagnostics {
        let expected_file = format!("{}.json", sha256_hex(scanned.index.package_id.as_bytes()));
        let key = (
            scanned.kind.clone(),
            scanned.scope_storage_key.clone(),
            scanned.index.package_id.clone(),
        );
        if scanned.file_name != expected_file
            || scanned.index.scope.kind.as_str() != scanned.kind
            || scanned.index.scope.storage_key().ok().as_deref()
                != Some(scanned.scope_storage_key.as_str())
            || expected.get(&key) != Some(&scanned.index)
            || !observed.insert(key)
        {
            return Err(host_snapshot_invalid(
                "A Host diagnostic index is stale, orphaned, duplicated, or moved.",
            ));
        }
        let managed_digest = scope_digest(&scanned.index.managed_scope)?;
        let request = requests.get(&(managed_digest, scanned.index.request_id.clone()));
        if request.is_none_or(|request| !scanned.index.matches(&request.stored)) {
            return Err(host_snapshot_invalid(
                "A Host diagnostic index disagrees with its reviewed request.",
            ));
        }
    }
    if expected.keys().any(|key| !observed.contains(key)) {
        return Err(host_snapshot_invalid(
            "A Host request is missing its latest enablement diagnostic index.",
        ));
    }
    u64::try_from(diagnostics.len())
        .map_err(|_| host_snapshot_invalid("Host diagnostic accounting overflowed."))
}

fn select_cancellation_alias(
    mut aliases: Vec<ScannedCancellation>,
) -> UseResult<ScannedCancellation> {
    if aliases.is_empty() || aliases.len() > 2 {
        return Err(host_snapshot_invalid(
            "A Host cancellation has an invalid alias inventory.",
        ));
    }
    let first = &aliases[0].stored;
    if aliases.iter().any(|alias| alias.stored != *first) {
        return Err(host_snapshot_invalid(
            "Exact and legacy Host cancellation aliases disagree.",
        ));
    }
    aliases.sort_by_key(|alias| !alias.exact_alias);
    Ok(aliases.remove(0))
}

fn snapshot_source(
    source: PathBuf,
    logical_path: String,
    kind: HostProjectionSnapshotRecordKind,
    length: u64,
    bytes: &[u8],
    record: HostProjectionSnapshotRecord,
) -> HostProjectionSnapshotSource {
    HostProjectionSnapshotSource {
        source,
        logical_path,
        kind,
        length,
        sha256: sha256(bytes),
        record,
    }
}

async fn children(
    directory: &Path,
    bounds: &mut ScanBounds,
) -> UseResult<Vec<(String, PathBuf, std::fs::Metadata)>> {
    let mut reader = fs::read_dir(directory)
        .await
        .map_err(|error| host_io("read Host projection directory", error))?;
    let mut result = Vec::new();
    while let Some(child) = reader
        .next_entry()
        .await
        .map_err(|error| host_io("read Host projection entry", error))?
    {
        bounds.visit_entry()?;
        let name = child
            .file_name()
            .into_string()
            .map_err(|_| host_snapshot_invalid("Host projection paths must be valid UTF-8."))?;
        let path = child.path();
        let metadata = fs::symlink_metadata(&path)
            .await
            .map_err(|error| host_io("inspect Host projection entry", error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
            return Err(host_snapshot_invalid(
                "The Host projection contains a link or reparse point.",
            ));
        }
        result.push((name, path, metadata));
    }
    result.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(result)
}

fn require_regular_record(metadata: &std::fs::Metadata) -> UseResult<()> {
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_HOST_RECORD_BYTES {
        return Err(host_snapshot_invalid(
            "A Host projection record is not a bounded regular file.",
        ));
    }
    Ok(())
}

async fn read_owned_record(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    require_regular_record(&inspect_owned_file(path).await?)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let mut file = options
        .open(path)
        .await
        .map_err(|error| host_io("open Host projection record", error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| host_io("inspect opened Host projection record", error))?;
    require_regular_record(&opened)?;
    if opened.len() != expected_length {
        return Err(host_snapshot_invalid(
            "A Host projection record changed before it was read.",
        ));
    }
    let capacity = usize::try_from(expected_length)
        .map_err(|_| host_snapshot_invalid("A Host projection record length is invalid."))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| host_io("read Host projection record", error))?;
    let opened_after = file
        .metadata()
        .await
        .map_err(|error| host_io("reinspect opened Host projection record", error))?;
    let path_after = inspect_owned_file(path).await?;
    if bytes.len() as u64 != expected_length
        || opened_after.len() != expected_length
        || path_after.len() != expected_length
    {
        return Err(host_snapshot_invalid(
            "A Host projection record changed while it was read.",
        ));
    }
    Ok(bytes)
}

async fn inspect_owned_file(path: &Path) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| host_io("inspect Host projection file", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(host_snapshot_invalid(
            "A Host projection record is not an owned regular file.",
        ));
    }
    Ok(metadata)
}

async fn owned_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| host_io(&format!("inspect {label}"), error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(host_snapshot_invalid(format!(
            "The {label} is not an owned directory."
        )));
    }
    Ok(())
}

async fn optional_owned_directory(path: &Path, label: &str) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(host_snapshot_invalid(format!(
            "The {label} is not an owned directory."
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(host_io(&format!("inspect {label}"), error)),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn host_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    host_snapshot_invalid(format!("Failed to {action}: {error}"))
}
