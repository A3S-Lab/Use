use std::collections::BTreeMap;

use a3s_use_core::{InstallationId, UseResult};

use super::{
    host_snapshot_invalid, scope_digest, validate_host_projection_snapshot_record,
    HostProjectionSnapshotRecord,
};
use crate::cognitive_package::host_store::{
    operation_binding_digest, sha256_hex, StoredPluginHostEnablementDiagnosticIndex,
    StoredPluginHostOperationIndex, StoredPluginHostRequest, MAX_HOST_RECORD_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostProjectionRestoreDerivedFile {
    pub(crate) logical_path: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug)]
struct DiagnosticCandidate {
    planned_at_ms: u64,
    request_id: String,
    file: HostProjectionRestoreDerivedFile,
}

/// Rebuild only the canonical indexes that the Host owner can derive from its
/// immutable request records. Legacy aliases and operational lock files are
/// deliberately absent from the result.
#[derive(Debug)]
pub(crate) struct HostProjectionRestoreIndexBuilder {
    installation: InstallationId,
    operations: BTreeMap<String, HostProjectionRestoreDerivedFile>,
    diagnostics: BTreeMap<String, DiagnosticCandidate>,
}

impl HostProjectionRestoreIndexBuilder {
    pub(crate) fn new(installation: InstallationId) -> UseResult<Self> {
        installation.validate().map_err(|_| {
            host_snapshot_invalid("The Host restore installation identity is invalid.")
        })?;
        Ok(Self {
            installation,
            operations: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
        })
    }

    pub(crate) fn observe(
        &mut self,
        logical_path: &str,
        bytes: &[u8],
    ) -> UseResult<HostProjectionSnapshotRecord> {
        let record =
            validate_host_projection_snapshot_record(logical_path, bytes, &self.installation)?;
        let HostProjectionSnapshotRecord::Request(_) = &record else {
            return Ok(record);
        };
        let stored: StoredPluginHostRequest = serde_json::from_slice(bytes).map_err(|_| {
            host_snapshot_invalid("A Host restore request is not valid owner-schema JSON.")
        })?;
        stored.validate().map_err(|_| {
            host_snapshot_invalid("A Host restore request failed owner-native validation.")
        })?;
        let managed_scope_digest = scope_digest(stored.plan.scope())?;

        if let Some(index) =
            StoredPluginHostOperationIndex::from_request(&stored).map_err(|_| {
                host_snapshot_invalid("A Host restore request cannot derive its operation index.")
            })?
        {
            let path = format!(
                "{managed_scope_digest}/operations/{}.json",
                operation_binding_digest(&index.operation_id, &index.plan_digest)
            );
            let file = derived_file(path.clone(), &index)?;
            if self.operations.insert(path, file).is_some() {
                return Err(host_snapshot_invalid(
                    "Host restore requests derive duplicate operation bindings.",
                ));
            }
        }

        if let Some(index) = StoredPluginHostEnablementDiagnosticIndex::from_request(&stored)
            .map_err(|_| {
                host_snapshot_invalid("A Host restore request cannot derive its diagnostic index.")
            })?
        {
            let scope_storage_key = index.scope.storage_key().map_err(|_| {
                host_snapshot_invalid("A Host restore diagnostic scope identity is invalid.")
            })?;
            let path = format!(
                "diagnostics/enablement/{}/{}/{}.json",
                index.scope.kind.as_str(),
                scope_storage_key,
                sha256_hex(index.package_id.as_bytes())
            );
            let candidate = DiagnosticCandidate {
                planned_at_ms: index.planned_at_ms,
                request_id: index.request_id.clone(),
                file: derived_file(path.clone(), &index)?,
            };
            let replace = self.diagnostics.get(&path).is_none_or(|current| {
                (candidate.planned_at_ms, candidate.request_id.as_str())
                    > (current.planned_at_ms, current.request_id.as_str())
            });
            if replace {
                self.diagnostics.insert(path, candidate);
            }
        }
        Ok(record)
    }

    pub(crate) fn finish(self) -> UseResult<Vec<HostProjectionRestoreDerivedFile>> {
        let mut files = self.operations.into_values().collect::<Vec<_>>();
        files.extend(
            self.diagnostics
                .into_values()
                .map(|candidate| candidate.file),
        );
        files.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
        if files
            .windows(2)
            .any(|pair| pair[0].logical_path >= pair[1].logical_path)
        {
            return Err(host_snapshot_invalid(
                "The canonical Host restore indexes are not uniquely ordered.",
            ));
        }
        Ok(files)
    }
}

fn derived_file<T: serde::Serialize>(
    logical_path: String,
    value: &T,
) -> UseResult<HostProjectionRestoreDerivedFile> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        host_snapshot_invalid(format!("Failed to encode a Host restore index: {error}"))
    })?;
    bytes.push(b'\n');
    if bytes.is_empty() || bytes.len() as u64 > MAX_HOST_RECORD_BYTES {
        return Err(host_snapshot_invalid(
            "A canonical Host restore index exceeds its owner byte bound.",
        ));
    }
    Ok(HostProjectionRestoreDerivedFile {
        logical_path,
        bytes,
    })
}
