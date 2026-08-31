use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{metadata_is_link_or_reparse_point, PlanScope, UseResult};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{
    restore_invalid, restore_io, restore_target_not_empty, wrap_restore_error, CANDIDATE_FILE,
    PARTIAL_FILE,
};
use crate::okf_knowledge::{OkfKnowledgeBackupManifest, SqliteOkfKnowledgeAdapter};

const KNOWLEDGE_LOCK_FILE: &str = ".knowledge.lock";

pub(super) enum LiveKnowledgePayloadLayout {
    Absent,
    Empty,
    Database(PathBuf),
}

pub(super) async fn stage_database(
    adapter: &SqliteOkfKnowledgeAdapter,
    source: &Path,
    candidate: &Path,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<()> {
    let partial = candidate.with_file_name(PARTIAL_FILE);
    if optional_regular_file(candidate).await? {
        if optional_regular_file(&partial).await? {
            return Err(restore_invalid(
                "The Knowledge restore staging directory contains both complete and partial candidates.",
            ));
        }
        adapter
            .inspect_staged_restore_database(candidate, manifest)
            .await
            .map_err(wrap_restore_error)?;
        return Ok(());
    }
    if optional_regular_file(&partial).await? {
        let bytes = fs::metadata(&partial)
            .await
            .map_err(|error| restore_io("inspect partial Knowledge restore candidate", error))?
            .len();
        if bytes == manifest.database_bytes
            && adapter
                .inspect_staged_restore_database(&partial, manifest)
                .await
                .is_ok()
        {
            fs::rename(&partial, candidate)
                .await
                .map_err(|error| restore_io("publish staged Knowledge candidate", error))?;
            sync_parent(candidate).await?;
            return Ok(());
        }
        if bytes >= manifest.database_bytes {
            return Err(restore_invalid(
                "The partial Knowledge restore candidate has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete Knowledge restore candidate", error))?;
    }
    let mut input = fs::File::open(source)
        .await
        .map_err(|error| restore_io("open verified Knowledge restore source", error))?;
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create partial Knowledge restore candidate", error))?;
    let copied = tokio::io::copy(&mut input, &mut output)
        .await
        .map_err(|error| restore_io("copy Knowledge restore candidate", error))?;
    if copied != manifest.database_bytes {
        return Err(restore_invalid(
            "The verified Knowledge restore source changed while it was staged.",
        ));
    }
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush Knowledge restore candidate", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Knowledge restore candidate", error))?;
    drop(output);
    adapter
        .inspect_staged_restore_database(&partial, manifest)
        .await
        .map_err(wrap_restore_error)?;
    fs::rename(&partial, candidate)
        .await
        .map_err(|error| restore_io("publish staged Knowledge candidate", error))?;
    sync_parent(candidate).await
}

pub(super) async fn activate_candidate(
    candidate: &Path,
    target: &Path,
    staging_directory: &Path,
) -> UseResult<()> {
    fs::rename(candidate, target)
        .await
        .map_err(|error| restore_io("activate Knowledge restore candidate", error))?;
    sync_parent(target).await?;
    sync_directory(staging_directory).await
}

pub(super) async fn inspect_live_payload_layout(
    adapter: &SqliteOkfKnowledgeAdapter,
    scope: &PlanScope,
) -> UseResult<LiveKnowledgePayloadLayout> {
    let sqlite_root = adapter.root();
    let payload_root = sqlite_root.parent().ok_or_else(|| {
        restore_invalid("The live Knowledge payload root has no state-owned parent.")
    })?;
    if !optional_owned_directory(payload_root).await? {
        return Ok(LiveKnowledgePayloadLayout::Absent);
    }
    if !only_optional_directory(payload_root, sqlite_root).await? {
        return Ok(LiveKnowledgePayloadLayout::Empty);
    }

    let scope_directory = adapter.scope_directory(scope).map_err(wrap_restore_error)?;
    let kind_directory = scope_directory.parent().ok_or_else(|| {
        restore_invalid("The live Knowledge scope has no state-owned kind directory.")
    })?;
    if !only_optional_directory(sqlite_root, kind_directory).await?
        || !only_optional_directory(kind_directory, &scope_directory).await?
    {
        return Ok(LiveKnowledgePayloadLayout::Empty);
    }

    let mut database = None;
    let mut entries = fs::read_dir(&scope_directory)
        .await
        .map_err(|error| restore_io("read live Knowledge scope directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read live Knowledge scope entry", error))?
    {
        let name = entry.file_name();
        if name == KNOWLEDGE_LOCK_FILE {
            optional_regular_file(&entry.path()).await?;
        } else if name == CANDIDATE_FILE {
            optional_regular_file(&entry.path()).await?;
            database = Some(entry.path());
        } else {
            return Err(restore_target_not_empty());
        }
    }
    Ok(database.map_or(
        LiveKnowledgePayloadLayout::Empty,
        LiveKnowledgePayloadLayout::Database,
    ))
}

pub(super) async fn ensure_owned_directory(state_root: &Path, target: &Path) -> UseResult<()> {
    if target == state_root || !target.starts_with(state_root) {
        return Err(restore_invalid(
            "The Knowledge restore staging directory escapes the target state root.",
        ));
    }
    validate_directory(state_root).await?;
    let relative = target
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("The restore staging directory is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "The restore staging directory is not a normalized owned path.",
        ));
    }
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(restore_io("create Knowledge restore directory", error)),
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_staging_entries(directory: &Path) -> UseResult<()> {
    validate_directory(directory).await?;
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| restore_io("read Knowledge restore staging directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read Knowledge restore staging entry", error))?
    {
        let name = entry.file_name();
        if name != CANDIDATE_FILE && name != PARTIAL_FILE {
            return Err(restore_invalid(
                "The Knowledge restore staging directory contains an unowned entry.",
            ));
        }
        optional_regular_file(&entry.path()).await?;
    }
    Ok(())
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect Knowledge restore directory", error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Knowledge restore directory is not an owned directory.",
        ));
    }
    Ok(())
}

async fn optional_owned_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {
            Ok(true)
        }
        Ok(_) => Err(restore_invalid(
            "A live Knowledge payload directory is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(restore_io(
            "inspect live Knowledge payload directory",
            error,
        )),
    }
}

async fn only_optional_directory(parent: &Path, expected: &Path) -> UseResult<bool> {
    if expected.parent() != Some(parent) {
        return Err(restore_invalid(
            "The live Knowledge payload layout escapes its expected parent.",
        ));
    }
    let expected_name = expected.file_name().ok_or_else(|| {
        restore_invalid("The live Knowledge payload directory has no owned name.")
    })?;
    let mut found = false;
    let mut entries = fs::read_dir(parent)
        .await
        .map_err(|error| restore_io("read live Knowledge payload directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read live Knowledge payload entry", error))?
    {
        if entry.file_name() != expected_name || found {
            return Err(restore_target_not_empty());
        }
        validate_directory(&entry.path()).await?;
        found = true;
    }
    Ok(found)
}

pub(super) async fn optional_regular_file(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_file() => {
            Ok(true)
        }
        Ok(_) => Err(restore_invalid(
            "A Knowledge restore file is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(restore_io("inspect Knowledge restore file", error)),
    }
}

async fn sync_parent(path: &Path) -> UseResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| restore_invalid("A Knowledge restore file has no parent directory."))?;
    sync_directory(parent).await
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_io("open Knowledge restore directory", error))?
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Knowledge restore directory", error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
