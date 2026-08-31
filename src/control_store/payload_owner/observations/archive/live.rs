use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, PluginPackageId, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs;

use super::{archive_io, file::read_record, sha256};
use crate::cognitive_package::{
    validate_planning_observation_snapshot_record, PlanningObservationSnapshotRecordKind,
};
use crate::control_store::payload_owner::canonical_json;
use crate::control_store::payload_owner::observations::{
    observation_error, ControlObservationPayloadEntry, ControlObservationPayloadEntryKind,
    ControlPayloadOwnerLimits,
};

const EXCLUDED_INVENTORY_DOMAIN: &[u8] =
    b"a3s.use.control-observation-payload-excluded-inventory.v1\0";
const MAX_LOCK_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScannedTerminalEntry {
    pub(super) evidence: ControlObservationPayloadEntry,
    pub(super) source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExcludedRecordEvidence {
    path: String,
    length: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiveObservationScan {
    pub(super) terminal: Vec<ScannedTerminalEntry>,
    pub(super) active_count: u64,
    pub(super) excluded_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservationRoot {
    DiagnosticHistory,
    Resolution,
    Download,
}

impl ObservationRoot {
    const ALL: [Self; 3] = [Self::DiagnosticHistory, Self::Resolution, Self::Download];

    const fn directory(self) -> &'static str {
        match self {
            Self::DiagnosticHistory => "package-diagnostic-history",
            Self::Resolution => "package-resolutions",
            Self::Download => "package-downloads",
        }
    }
}

pub(super) async fn scan_live(
    state_root: &Path,
    installation: &InstallationId,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<LiveObservationScan> {
    installation.validate().map_err(|_| {
        observation_error("The observation snapshot installation identity is invalid.")
    })?;
    owned_directory(state_root).await?;
    let operations = state_root.join("operations");
    if !optional_owned_directory(&operations).await? {
        return empty_scan(installation);
    }

    let mut terminal = Vec::new();
    let mut active = Vec::new();
    let mut package_records = BTreeSet::new();
    let mut visited_files = 0_u64;
    let mut visited_entries = 0_u64;
    let max_entries = limits.max_files.saturating_mul(4).min(100_000);
    let mut inspected_bytes = 0_u64;
    for root in ObservationRoot::ALL {
        let owner_root = operations.join(root.directory());
        if !optional_owned_directory(&owner_root).await? {
            continue;
        }
        let mut stack = vec![(owner_root, Vec::<String>::new())];
        while let Some((directory, relative)) = stack.pop() {
            let mut reader = fs::read_dir(&directory)
                .await
                .map_err(|error| archive_io("read observation directory", error))?;
            let mut children = Vec::new();
            while let Some(child) = reader
                .next_entry()
                .await
                .map_err(|error| archive_io("read observation entry", error))?
            {
                visited_entries = visited_entries.checked_add(1).ok_or_else(|| {
                    observation_error("Observation filesystem accounting overflowed.")
                })?;
                if visited_entries > max_entries {
                    return Err(observation_error(
                        "The observation filesystem inventory exceeds its entry bound.",
                    ));
                }
                let name = child.file_name().into_string().map_err(|_| {
                    observation_error("Observation payload paths must be valid UTF-8.")
                })?;
                children.push((name, child.path()));
            }
            children.sort_by(|left, right| left.0.cmp(&right.0));
            for (name, path) in children.into_iter().rev() {
                let mut child_relative = relative.clone();
                child_relative.push(name);
                let metadata = fs::symlink_metadata(&path)
                    .await
                    .map_err(|error| archive_io("inspect observation entry", error))?;
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                    return Err(observation_error(
                        "The observation payload contains a link or reparse point.",
                    ));
                }
                if metadata.is_dir() {
                    validate_directory_layout(root, &child_relative, installation)?;
                    stack.push((path, child_relative));
                    continue;
                }
                if !metadata.is_file() {
                    return Err(observation_error(
                        "The observation payload contains a special filesystem entry.",
                    ));
                }
                visited_files = visited_files
                    .checked_add(1)
                    .ok_or_else(|| observation_error("Observation file accounting overflowed."))?;
                if visited_files > limits.max_files {
                    return Err(observation_error(
                        "The observation payload exceeds its registered file bound.",
                    ));
                }
                if is_valid_lock_path(root, &child_relative, installation)? {
                    if metadata.len() > MAX_LOCK_FILE_BYTES {
                        return Err(observation_error(
                            "An excluded observation lock exceeds its safety bound.",
                        ));
                    }
                    continue;
                }

                inspected_bytes = inspected_bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| observation_error("Observation byte accounting overflowed."))?;
                if inspected_bytes > limits.max_payload_bytes {
                    return Err(observation_error(
                        "The observation records exceed their registered byte bound.",
                    ));
                }
                let portable = format!("{}/{}", root.directory(), child_relative.join("/"));
                let bytes = read_record(&path, metadata.len()).await?;
                let record =
                    validate_planning_observation_snapshot_record(&portable, &bytes, installation)
                        .map_err(|_| {
                            observation_error(
                                "A planning or diagnostic record failed owner-native validation.",
                            )
                        })?;
                let digest = sha256(&bytes);
                match record.kind {
                    PlanningObservationSnapshotRecordKind::DiagnosticHistory => {
                        insert_package(&mut package_records, "history", record.package_id)?;
                        terminal.push(ScannedTerminalEntry {
                            evidence: ControlObservationPayloadEntry {
                                kind: ControlObservationPayloadEntryKind::DiagnosticHistory,
                                path: portable,
                                length: metadata.len(),
                                sha256: digest,
                            },
                            source: path,
                        });
                    }
                    PlanningObservationSnapshotRecordKind::TerminalResolution => {
                        insert_package(&mut package_records, "resolution", record.package_id)?;
                        terminal.push(ScannedTerminalEntry {
                            evidence: ControlObservationPayloadEntry {
                                kind: ControlObservationPayloadEntryKind::TerminalResolution,
                                path: portable,
                                length: metadata.len(),
                                sha256: digest,
                            },
                            source: path,
                        });
                    }
                    PlanningObservationSnapshotRecordKind::ActiveResolution => {
                        insert_package(&mut package_records, "resolution", record.package_id)?;
                        active.push(ExcludedRecordEvidence {
                            path: portable,
                            length: metadata.len(),
                            sha256: digest,
                        });
                    }
                    PlanningObservationSnapshotRecordKind::ActiveDownload => {
                        insert_package(&mut package_records, "download", record.package_id)?;
                        active.push(ExcludedRecordEvidence {
                            path: portable,
                            length: metadata.len(),
                            sha256: digest,
                        });
                    }
                }
            }
        }
    }
    terminal.sort_by(|left, right| left.evidence.path.cmp(&right.evidence.path));
    active.sort_by(|left, right| left.path.cmp(&right.path));
    let excluded_digest = excluded_inventory_digest(installation, &active)?;
    Ok(LiveObservationScan {
        terminal,
        active_count: active.len() as u64,
        excluded_digest,
    })
}

fn insert_package(
    inventory: &mut BTreeSet<(&'static str, String)>,
    family: &'static str,
    package_id: String,
) -> UseResult<()> {
    if !inventory.insert((family, package_id)) {
        return Err(observation_error(
            "The observation payload contains duplicate package records.",
        ));
    }
    Ok(())
}

fn empty_scan(installation: &InstallationId) -> UseResult<LiveObservationScan> {
    Ok(LiveObservationScan {
        terminal: Vec::new(),
        active_count: 0,
        excluded_digest: excluded_inventory_digest(installation, &[])?,
    })
}

fn validate_directory_layout(
    root: ObservationRoot,
    relative: &[String],
    installation: &InstallationId,
) -> UseResult<()> {
    let valid = match (root, relative) {
        (ObservationRoot::DiagnosticHistory, [category]) => {
            matches!(category.as_str(), "scopes" | "locks")
        }
        (ObservationRoot::DiagnosticHistory, [category, scope]) => {
            matches!(category.as_str(), "scopes" | "locks")
                && scope
                    == &installation.storage_key().map_err(|_| {
                        observation_error("The observation scope identity is invalid.")
                    })?
        }
        (ObservationRoot::DiagnosticHistory, [category, scope, publisher]) => {
            matches!(category.as_str(), "scopes" | "locks")
                && scope
                    == &installation.storage_key().map_err(|_| {
                        observation_error("The observation scope identity is invalid.")
                    })?
                && valid_publisher(publisher)
        }
        (ObservationRoot::Resolution, [action]) => {
            matches!(action.as_str(), "install" | "upgrade")
        }
        (ObservationRoot::Resolution, [action, publisher]) => {
            matches!(action.as_str(), "install" | "upgrade") && valid_publisher(publisher)
        }
        (ObservationRoot::Download, [category]) => {
            matches!(category.as_str(), "install" | "upgrade" | "locks")
        }
        (ObservationRoot::Download, [category, publisher]) => {
            matches!(category.as_str(), "install" | "upgrade" | "locks")
                && valid_publisher(publisher)
        }
        _ => false,
    };
    if !valid {
        return Err(observation_error(
            "The observation payload contains an unknown directory layout.",
        ));
    }
    Ok(())
}

fn is_valid_lock_path(
    root: ObservationRoot,
    relative: &[String],
    installation: &InstallationId,
) -> UseResult<bool> {
    let (publisher, file) = match (root, relative) {
        (ObservationRoot::DiagnosticHistory, [category, scope, publisher, file])
            if category == "locks"
                && scope
                    == &installation.storage_key().map_err(|_| {
                        observation_error("The observation scope identity is invalid.")
                    })? =>
        {
            (publisher, file)
        }
        (ObservationRoot::Download, [category, publisher, file]) if category == "locks" => {
            (publisher, file)
        }
        _ => return Ok(false),
    };
    let Some(package) = file.strip_suffix(".lock") else {
        return Err(observation_error(
            "An observation lock path has an unsupported file name.",
        ));
    };
    PluginPackageId::parse(format!("{publisher}/{package}"))
        .map_err(|_| observation_error("An observation lock path has an invalid package ID."))?;
    Ok(true)
}

fn valid_publisher(publisher: &str) -> bool {
    PluginPackageId::parse(format!("{publisher}/placeholder")).is_ok()
}

pub(super) async fn validate_destination(state_root: &Path, destination: &Path) -> UseResult<()> {
    let file_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| observation_error("The observation archive must name a file."))?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    owned_directory(parent).await?;
    let physical_parent = fs::canonicalize(parent)
        .await
        .map_err(|error| archive_io("resolve observation archive parent", error))?;
    let physical_state = fs::canonicalize(state_root)
        .await
        .map_err(|error| archive_io("resolve observation state root", error))?;
    if physical_parent.join(file_name).starts_with(&physical_state) {
        return Err(observation_error(
            "The observation archive destination must remain outside Use-owned state.",
        ));
    }
    match fs::symlink_metadata(destination).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(observation_error(
            "The observation snapshot destination already exists.",
        )),
        Err(error) => Err(archive_io("inspect observation archive destination", error)),
    }
}

async fn owned_directory(path: &Path) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| archive_io("inspect observation directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(observation_error(
            "An observation payload directory is not an owned directory.",
        ));
    }
    Ok(metadata)
}

async fn optional_owned_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(observation_error(
            "An observation payload root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(archive_io("inspect observation payload root", error)),
    }
}

fn excluded_inventory_digest(
    installation: &InstallationId,
    entries: &[ExcludedRecordEvidence],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ExcludedRecordEvidence],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| {
        observation_error(format!(
            "Failed to encode excluded observation inventory: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    digest.update(EXCLUDED_INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| archive_io("open observation archive directory", error))?
        .sync_all()
        .await
        .map_err(|error| archive_io("sync observation archive directory", error))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
