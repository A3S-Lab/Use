use a3s_use_core::{InstallationId, UseError, UseResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanningObservationSnapshotRecordKind {
    DiagnosticHistory,
    TerminalResolution,
    ActiveResolution,
    ActiveDownload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanningObservationSnapshotRecord {
    pub(crate) kind: PlanningObservationSnapshotRecordKind,
    pub(crate) package_id: String,
}

/// Reuse each owning store's decoder and invariants when an external payload
/// snapshot inspects a machine-owned record. The Control Store adapter owns
/// archive mechanics; it does not duplicate cognitive-package schemas.
pub(crate) fn validate_planning_observation_snapshot_record(
    path: &str,
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<PlanningObservationSnapshotRecord> {
    installation.validate().map_err(|_| snapshot_invalid())?;
    let (owner, relative) = path.split_once('/').ok_or_else(snapshot_invalid)?;
    let (kind, package_id) = match owner {
        "package-diagnostic-history" => (
            PlanningObservationSnapshotRecordKind::DiagnosticHistory,
            super::diagnostic_history::validate_snapshot_record(relative, bytes, installation)?,
        ),
        "package-resolutions" => {
            let (package_id, terminal) =
                super::resolution_attempt::validate_snapshot_record(relative, bytes, installation)?;
            (
                if terminal {
                    PlanningObservationSnapshotRecordKind::TerminalResolution
                } else {
                    PlanningObservationSnapshotRecordKind::ActiveResolution
                },
                package_id,
            )
        }
        "package-downloads" => (
            PlanningObservationSnapshotRecordKind::ActiveDownload,
            super::download_attempt::validate_snapshot_record(relative, bytes, installation)?,
        ),
        _ => return Err(snapshot_invalid()),
    };
    Ok(PlanningObservationSnapshotRecord { kind, package_id })
}

fn snapshot_invalid() -> UseError {
    UseError::new(
        "use.control_store.observation_payload_snapshot_invalid",
        "A planning or diagnostic observation record is invalid or moved across its owned path.",
    )
}

#[cfg(test)]
pub(crate) fn planning_observation_snapshot_fixtures(
    installation: &InstallationId,
) -> Vec<(String, Vec<u8>)> {
    let mut fixtures = super::resolution_attempt::snapshot_fixtures(installation);
    fixtures.push(super::diagnostic_history::snapshot_fixture(installation));
    fixtures
}
