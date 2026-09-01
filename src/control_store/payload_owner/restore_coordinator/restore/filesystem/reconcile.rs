use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;

use super::candidate::CanonicalRestoreHistory;
use super::records;
use super::{
    ensure_owned_directory, optional_owned_directory, rename_owned, retired_path, sync_directory,
    validate_staging_entries, ExpectedActiveRestore,
};
use crate::control_store::payload_owner::restore_coordinator::restore::evidence::{
    entries_from_scan, RestoreCoordinatorActivation,
};
use crate::control_store::payload_owner::restore_coordinator::restore::{
    restore_invalid, restore_requires_active,
};
use crate::control_store::payload_owner::restore_coordinator::{
    ControlRestoreCoordinatorEntry, ControlRestoreCoordinatorSnapshot,
    ControlRestoreCoordinatorState,
};
use crate::state_restore::{
    scan_state_restore_history_snapshot, StateRestoreHistorySnapshotActive,
    StateRestoreHistorySnapshotScan,
};

mod marker;
mod publication;

pub(super) async fn activate(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
    expected_active: Option<ExpectedActiveRestore<'_>>,
) -> UseResult<RestoreCoordinatorActivation> {
    validate_staging_entries(staging_directory, snapshot).await?;
    let canonical = match snapshot.manifest.payload {
        ControlRestoreCoordinatorState::Absent => {
            super::require_candidate_absent(staging_directory).await?;
            CanonicalRestoreHistory::absent()
        }
        ControlRestoreCoordinatorState::Archive { .. } => {
            super::inspect_candidate(staging_directory, snapshot).await?
        }
    };
    let first = scan_live(state_root, snapshot).await?;
    let active = require_active(&first)?.clone();
    if expected_active.is_some_and(|expected| {
        active.plan_digest != expected.plan_digest
            || active.marker_length != expected.marker_length
            || active.marker_sha256 != expected.marker_sha256
    }) {
        return Err(restore_invalid(
            "The Restore Coordinator active identity differs from the complete restore marker.",
        ));
    }
    let (target, pruned) = canonical.target(&active.plan_digest, active.reserves_terminal_slot)?;
    let (activation, created) = marker::load_or_create(
        staging_directory,
        snapshot,
        &first,
        &target,
        pruned.as_deref(),
    )
    .await?;
    if !activation.binds_active(&active) {
        return Err(restore_invalid(
            "The active restore identity changed before Restore Coordinator activation.",
        ));
    }
    if created {
        let second = scan_live(state_root, snapshot).await?;
        let second_active = require_active(&second)?;
        if second_active != &active
            || entries_from_scan(&second.terminal) != activation.before_entries
        {
            return Err(restore_invalid(
                "Restore history changed while its activation marker was committed.",
            ));
        }
    }

    let live = entries_from_scan(&scan_live(state_root, snapshot).await?.terminal);
    let retired = inspect_retired(staging_directory, snapshot).await?;
    validate_replay_state(&live, &retired, &activation)?;
    retire_replaced_history(
        state_root,
        staging_directory,
        snapshot,
        &activation,
        live,
        retired,
    )
    .await?;

    let after_retire = scan_live(state_root, snapshot).await?;
    if require_active(&after_retire)? != &active {
        return Err(restore_invalid(
            "The active restore identity changed while terminal history was retired.",
        ));
    }
    let mut live = entry_map(entries_from_scan(&after_retire.terminal))?;
    let retired = inspect_retired(staging_directory, snapshot).await?;
    validate_replay_state(
        &live.values().cloned().collect::<Vec<_>>(),
        &retired,
        &activation,
    )?;
    publication::publish_target_history(
        state_root,
        staging_directory,
        snapshot,
        &canonical,
        &activation,
        &mut live,
    )
    .await?;

    let final_scan = scan_live(state_root, snapshot).await?;
    if require_active(&final_scan)? != &active
        || entries_from_scan(&final_scan.terminal) != activation.target_entries
    {
        return Err(restore_invalid(
            "The activated Restore Coordinator history differs from its exact target.",
        ));
    }
    let retired = inspect_retired(staging_directory, snapshot).await?;
    validate_replay_state(
        &entries_from_scan(&final_scan.terminal),
        &retired,
        &activation,
    )?;
    publication::require_empty_publication(staging_directory).await?;
    validate_staging_entries(staging_directory, snapshot).await?;
    Ok(activation)
}

async fn retire_replaced_history(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
    activation: &RestoreCoordinatorActivation,
    live: Vec<ControlRestoreCoordinatorEntry>,
    retired: Vec<ControlRestoreCoordinatorEntry>,
) -> UseResult<()> {
    let mut live = entry_map(live)?;
    let mut retired = entry_map(retired)?;
    let target = entry_map(activation.target_entries.clone())?;
    let live_root = state_root.join("operations").join("state-restores");
    let retired_root = retired_path(staging_directory);
    for before in &activation.before_entries {
        if target.get(&before.plan_digest) == Some(before) {
            continue;
        }
        if retired.get(&before.plan_digest) == Some(before) {
            continue;
        }
        if live.get(&before.plan_digest) != Some(before) {
            return Err(restore_invalid(
                "A Restore Coordinator history record vanished before retirement.",
            ));
        }
        if !optional_owned_directory(&retired_root).await? {
            ensure_owned_directory(staging_directory, &retired_root).await?;
        }
        let source = records::record_directory(&live_root, &before.plan_digest)?;
        let target_path = records::record_directory(&retired_root, &before.plan_digest)?;
        rename_owned(&source, &target_path, "retire Restore Coordinator history").await?;
        sync_directory(&live_root).await?;
        live.remove(&before.plan_digest);
        retired.insert(before.plan_digest.clone(), before.clone());
    }
    let observed = inspect_retired(staging_directory, snapshot).await?;
    if entry_map(observed)? != retired {
        return Err(restore_invalid(
            "Retired Restore Coordinator evidence changed during activation.",
        ));
    }
    Ok(())
}

async fn inspect_retired(
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<Vec<ControlRestoreCoordinatorEntry>> {
    let root = retired_path(staging_directory);
    if !optional_owned_directory(&root).await? {
        return Ok(Vec::new());
    }
    Ok(
        records::inspect_exact_tree(&root, &snapshot.manifest.binding.installation)
            .await?
            .into_iter()
            .map(|record| record.evidence)
            .collect(),
    )
}

fn validate_replay_state(
    live: &[ControlRestoreCoordinatorEntry],
    retired: &[ControlRestoreCoordinatorEntry],
    activation: &RestoreCoordinatorActivation,
) -> UseResult<()> {
    let live = entry_map(live.to_vec())?;
    let retired = entry_map(retired.to_vec())?;
    let before = entry_map(activation.before_entries.clone())?;
    let target = entry_map(activation.target_entries.clone())?;
    for (digest, evidence) in &live {
        if before.get(digest) != Some(evidence) && target.get(digest) != Some(evidence) {
            return Err(restore_invalid(
                "Live restore history contains evidence outside the activation boundary.",
            ));
        }
    }
    for (digest, evidence) in &retired {
        if before.get(digest) != Some(evidence) || target.get(digest) == Some(evidence) {
            return Err(restore_invalid(
                "Retired restore history differs from the activation's before inventory.",
            ));
        }
    }
    for (digest, evidence) in &before {
        if target.get(digest) == Some(evidence) {
            if live.get(digest) != Some(evidence) || retired.contains_key(digest) {
                return Err(restore_invalid(
                    "A retained restore history record is not exact.",
                ));
            }
            continue;
        }
        match retired.get(digest) {
            Some(retired_evidence) if retired_evidence == evidence => {
                if live
                    .get(digest)
                    .is_some_and(|current| target.get(digest) != Some(current))
                {
                    return Err(restore_invalid(
                        "Retired and live Restore Coordinator evidence conflict.",
                    ));
                }
            }
            None if live.get(digest) == Some(evidence) => {}
            _ => {
                return Err(restore_invalid(
                    "A Restore Coordinator before record is neither live nor retired.",
                ))
            }
        }
    }
    Ok(())
}

pub(super) fn entry_map(
    entries: Vec<ControlRestoreCoordinatorEntry>,
) -> UseResult<BTreeMap<String, ControlRestoreCoordinatorEntry>> {
    let mut map = BTreeMap::new();
    for entry in entries {
        if map.insert(entry.plan_digest.clone(), entry).is_some() {
            return Err(restore_invalid(
                "Restore Coordinator evidence has a duplicate plan identity.",
            ));
        }
    }
    Ok(map)
}

pub(super) async fn scan_live(
    state_root: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<StateRestoreHistorySnapshotScan> {
    scan_state_restore_history_snapshot(state_root, &snapshot.manifest.binding.installation)
        .await
        .map_err(|error| {
            restore_invalid(format!(
                "Live Restore Coordinator validation failed: {}",
                error.message
            ))
        })
}

pub(super) fn require_active(
    scan: &StateRestoreHistorySnapshotScan,
) -> UseResult<&StateRestoreHistorySnapshotActive> {
    scan.active.as_ref().ok_or_else(restore_requires_active)
}

pub(super) fn live_root(state_root: &Path) -> PathBuf {
    state_root.join("operations").join("state-restores")
}
