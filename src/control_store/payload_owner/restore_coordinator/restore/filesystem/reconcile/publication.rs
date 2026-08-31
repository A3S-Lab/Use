use std::collections::BTreeMap;
use std::path::Path;

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::super::candidate::CanonicalRestoreHistory;
use super::super::records;
use super::super::{
    ensure_owned_directory, optional_owned_directory, optional_regular_file_length,
    publish_noclobber, publishing_path, read_owned_file, rename_owned, restore_io, segment,
    sync_directory, valid_segment, validate_directory, OPERATION_FILE, OPERATION_PARTIAL_FILE,
};
use crate::control_store::payload_owner::restore_coordinator::restore::evidence::RestoreCoordinatorActivation;
use crate::control_store::payload_owner::restore_coordinator::restore::restore_invalid;
use crate::control_store::payload_owner::restore_coordinator::{
    ControlRestoreCoordinatorEntry, ControlRestoreCoordinatorSnapshot,
};
use crate::state_restore::STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES;

pub(super) async fn publish_target_history(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
    canonical: &CanonicalRestoreHistory,
    activation: &RestoreCoordinatorActivation,
    live: &mut BTreeMap<String, ControlRestoreCoordinatorEntry>,
) -> UseResult<()> {
    if let Some(digest) = publication_identity(staging_directory).await? {
        let expected = activation
            .target_entries
            .iter()
            .find(|entry| entry.plan_digest == digest)
            .ok_or_else(|| {
                restore_invalid("A staged publication is outside the exact target inventory.")
            })?;
        prepare_publication(staging_directory, canonical, expected, Some(&digest)).await?;
        publish_or_reconcile(state_root, staging_directory, snapshot, expected, live).await?;
    }

    for expected in &activation.target_entries {
        match live.get(&expected.plan_digest) {
            Some(current) if current == expected => continue,
            Some(_) => {
                return Err(restore_invalid(
                    "A live restore history identity has unexpected target bytes.",
                ))
            }
            None => {}
        }
        prepare_publication(staging_directory, canonical, expected, None).await?;
        publish_or_reconcile(state_root, staging_directory, snapshot, expected, live).await?;
    }
    let target = super::entry_map(activation.target_entries.clone())?;
    if *live != target {
        return Err(restore_invalid(
            "Restore Coordinator publication did not reach its exact target inventory.",
        ));
    }
    require_empty_publication(staging_directory).await
}

pub(super) async fn require_empty_publication(staging_directory: &Path) -> UseResult<()> {
    let root = publishing_path(staging_directory);
    if !optional_owned_directory(&root).await? {
        return Ok(());
    }
    let mut reader = fs::read_dir(&root)
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication root", error))?;
    if reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication entry", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "Restore Coordinator publication left staged record evidence.",
        ));
    }
    Ok(())
}

async fn publication_identity(staging_directory: &Path) -> UseResult<Option<String>> {
    let root = publishing_path(staging_directory);
    if !optional_owned_directory(&root).await? {
        return Ok(None);
    }
    let mut reader = fs::read_dir(&root)
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication root", error))?;
    let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication entry", error))?
    else {
        return Ok(None);
    };
    if reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication entry", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "Multiple Restore Coordinator records are being published.",
        ));
    }
    let segment = entry.file_name().into_string().map_err(|_| {
        restore_invalid("Restore Coordinator publication names must be valid UTF-8.")
    })?;
    if !valid_segment(&segment) {
        return Err(restore_invalid(
            "A Restore Coordinator publication has an invalid identity.",
        ));
    }
    validate_publication_record_state(&entry.path()).await?;
    Ok(Some(format!("sha256:{segment}")))
}

async fn prepare_publication(
    staging_directory: &Path,
    canonical: &CanonicalRestoreHistory,
    expected: &ControlRestoreCoordinatorEntry,
    existing: Option<&str>,
) -> UseResult<()> {
    if existing.is_some_and(|digest| digest != expected.plan_digest) {
        return Err(restore_invalid(
            "The residual Restore Coordinator publication binds another record.",
        ));
    }
    let canonical_record = canonical.record(&expected.plan_digest).ok_or_else(|| {
        restore_invalid("The Restore Coordinator target has no canonical source record.")
    })?;
    if canonical_record.evidence != *expected {
        return Err(restore_invalid(
            "The Restore Coordinator target differs from its candidate evidence.",
        ));
    }
    let candidate_root = super::super::candidate_path(staging_directory);
    let source = records::operation_path(&candidate_root, &expected.plan_digest)?;
    let bytes = read_owned_file(
        &source,
        STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
        "Restore Coordinator candidate publication source",
    )
    .await?;
    if bytes.len() as u64 != expected.length || digest(&bytes) != expected.sha256 {
        return Err(restore_invalid(
            "The Restore Coordinator candidate changed before publication.",
        ));
    }

    let root = publishing_path(staging_directory);
    if !optional_owned_directory(&root).await? {
        ensure_owned_directory(staging_directory, &root).await?;
    }
    let directory = records::record_directory(&root, &expected.plan_digest)?;
    if !optional_owned_directory(&directory).await? {
        if publication_identity(staging_directory).await?.is_some() {
            return Err(restore_invalid(
                "Another Restore Coordinator record is already staged for publication.",
            ));
        }
        ensure_owned_directory(&root, &directory).await?;
    }
    write_publication_record(&directory, expected, &bytes).await?;
    let published = read_owned_file(
        &directory.join(OPERATION_FILE),
        STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
        "staged Restore Coordinator publication",
    )
    .await?;
    if published != bytes {
        return Err(restore_invalid(
            "The staged Restore Coordinator publication differs from its candidate.",
        ));
    }
    Ok(())
}

async fn write_publication_record(
    directory: &Path,
    expected: &ControlRestoreCoordinatorEntry,
    bytes: &[u8],
) -> UseResult<()> {
    validate_publication_record_state(directory).await?;
    let target = directory.join(OPERATION_FILE);
    let partial = directory.join(OPERATION_PARTIAL_FILE);
    let target_length = optional_regular_file_length(&target).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if target_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "A Restore Coordinator publication has ambiguous record bytes.",
        ));
    }
    if let Some(length) = target_length {
        if length != expected.length
            || read_owned_file(
                &target,
                STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
                "staged Restore Coordinator publication",
            )
            .await?
                != bytes
        {
            return Err(restore_invalid(
                "A staged Restore Coordinator publication was modified.",
            ));
        }
        return Ok(());
    }
    if let Some(length) = partial_length {
        if length == expected.length
            && read_owned_file(
                &partial,
                STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
                "partial Restore Coordinator publication",
            )
            .await?
                == bytes
        {
            publish_noclobber(partial, target, "publish staged Restore Coordinator record").await?;
            sync_directory(directory).await?;
            return Ok(());
        }
        if length >= expected.length {
            return Err(restore_invalid(
                "A partial Restore Coordinator publication has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial).await.map_err(|error| {
            restore_io("remove incomplete Restore Coordinator publication", error)
        })?;
        sync_directory(directory).await?;
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create Restore Coordinator publication partial", error))?;
    output
        .write_all(bytes)
        .await
        .map_err(|error| restore_io("write Restore Coordinator publication", error))?;
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush Restore Coordinator publication", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Restore Coordinator publication", error))?;
    drop(output);
    sync_directory(directory).await?;
    if read_owned_file(
        &partial,
        STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
        "partial Restore Coordinator publication",
    )
    .await?
        != bytes
    {
        return Err(restore_invalid(
            "A Restore Coordinator publication changed before publication.",
        ));
    }
    publish_noclobber(partial, target, "publish staged Restore Coordinator record").await?;
    sync_directory(directory).await
}

async fn publish_or_reconcile(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
    expected: &ControlRestoreCoordinatorEntry,
    live: &mut BTreeMap<String, ControlRestoreCoordinatorEntry>,
) -> UseResult<()> {
    let publication_root = publishing_path(staging_directory);
    let directory = records::record_directory(&publication_root, &expected.plan_digest)?;
    let observed = records::inspect_record_directory(
        &directory,
        segment(&expected.plan_digest)?,
        &snapshot.manifest.binding.installation,
    )
    .await?;
    if observed.evidence != *expected {
        return Err(restore_invalid(
            "The staged Restore Coordinator record differs from its exact target.",
        ));
    }
    match live.get(&expected.plan_digest) {
        Some(current) if current == expected => {
            cleanup_duplicate_publication(&publication_root, &directory).await?;
        }
        Some(_) => {
            return Err(restore_invalid(
                "A staged Restore Coordinator record conflicts with live history.",
            ))
        }
        None => {
            let live_root = super::live_root(state_root);
            if !optional_owned_directory(&live_root).await? {
                ensure_owned_directory(state_root, &live_root).await?;
            }
            let target = records::record_directory(&live_root, &expected.plan_digest)?;
            rename_owned(
                &directory,
                &target,
                "publish Restore Coordinator history record",
            )
            .await?;
            sync_directory(&publication_root).await?;
            live.insert(expected.plan_digest.clone(), expected.clone());
        }
    }
    Ok(())
}

async fn cleanup_duplicate_publication(root: &Path, directory: &Path) -> UseResult<()> {
    let operation = directory.join(OPERATION_FILE);
    fs::remove_file(&operation)
        .await
        .map_err(|error| restore_io("remove duplicate Restore Coordinator publication", error))?;
    sync_directory(directory).await?;
    fs::remove_dir(directory)
        .await
        .map_err(|error| restore_io("remove empty Restore Coordinator publication", error))?;
    sync_directory(root).await
}

async fn validate_publication_record_state(directory: &Path) -> UseResult<()> {
    validate_directory(directory).await?;
    let mut reader = fs::read_dir(directory)
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication", error))?;
    let mut count = 0_usize;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator publication evidence", error))?
    {
        count += 1;
        if count > 1
            || (entry.file_name() != OPERATION_FILE && entry.file_name() != OPERATION_PARTIAL_FILE)
        {
            return Err(restore_invalid(
                "A Restore Coordinator publication contains unknown evidence.",
            ));
        }
        optional_regular_file_length(&entry.path()).await?;
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
