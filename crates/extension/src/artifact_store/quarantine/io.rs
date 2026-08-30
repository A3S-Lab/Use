use std::path::Path;

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{canonical_json, quarantine_state_invalid, ArtifactQuarantineRecord};
use crate::artifact_store::ArtifactKind;
use crate::package::{io_error, sync_parent_directory};

pub(in crate::artifact_store) const QUARANTINE_RECORD: &str = "quarantine.json";
pub(in crate::artifact_store) const QUARANTINE_TEMPORARY: &str = ".quarantine.tmp";
const MAX_QUARANTINE_RECORD_BYTES: u64 = 8 * 1024;

#[derive(Debug)]
pub(in crate::artifact_store) enum ContainerQuarantineState {
    None,
    Interrupted,
    Quarantined(ArtifactQuarantineRecord),
}

pub(in crate::artifact_store) async fn inspect_container_state(
    container: &Path,
    kind: ArtifactKind,
    digest: &str,
) -> UseResult<ContainerQuarantineState> {
    let record_path = container.join(QUARANTINE_RECORD);
    let temporary_path = container.join(QUARANTINE_TEMPORARY);
    let record = optional_record(&record_path).await?;
    let temporary = optional_temporary(&temporary_path).await?;
    if record.is_some() && temporary {
        return Err(quarantine_state_invalid(
            "An Artifact Store quarantine container has both active and temporary records.",
        ));
    }
    match record {
        Some(record) => {
            if record.plan.kind != kind || record.plan.digest != digest {
                return Err(quarantine_state_invalid(
                    "An Artifact Store quarantine record does not match its digest container.",
                ));
            }
            Ok(ContainerQuarantineState::Quarantined(record))
        }
        None if temporary => Ok(ContainerQuarantineState::Interrupted),
        None => Ok(ContainerQuarantineState::None),
    }
}

pub(in crate::artifact_store) fn validate_quarantine_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    allow_empty: bool,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || (!allow_empty && metadata.len() == 0)
        || metadata.len() > MAX_QUARANTINE_RECORD_BYTES
    {
        return Err(quarantine_state_invalid(format!(
            "Artifact Store quarantine state '{}' is not a bounded owned regular file.",
            path.display()
        )));
    }
    Ok(())
}

async fn optional_record(path: &Path) -> UseResult<Option<ArtifactQuarantineRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error(
                "inspect Artifact Store quarantine record",
                path,
                error,
            ))
        }
    };
    validate_quarantine_metadata(path, &metadata, false)?;
    load_record(path).await.map(Some)
}

async fn optional_temporary(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            validate_quarantine_metadata(path, &metadata, true)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(
            "inspect Artifact Store quarantine temporary",
            path,
            error,
        )),
    }
}

async fn load_record(path: &Path) -> UseResult<ArtifactQuarantineRecord> {
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
        .map_err(|error| io_error("open Artifact Store quarantine record", path, error))?;
    let metadata = file.metadata().await.map_err(|error| {
        io_error(
            "inspect opened Artifact Store quarantine record",
            path,
            error,
        )
    })?;
    validate_quarantine_metadata(path, &metadata, false)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_QUARANTINE_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read Artifact Store quarantine record", path, error))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(quarantine_state_invalid(
            "The Artifact Store quarantine record changed while it was read.",
        ));
    }
    let record: ArtifactQuarantineRecord = serde_json::from_slice(&bytes).map_err(|error| {
        quarantine_state_invalid(format!(
            "The Artifact Store quarantine record is invalid JSON: {error}"
        ))
    })?;
    record.validate()?;
    if canonical_json(&record)? != bytes {
        return Err(quarantine_state_invalid(
            "The Artifact Store quarantine record is not canonical JSON.",
        ));
    }
    Ok(record)
}

pub(super) async fn write_record(
    container: &Path,
    record: &ArtifactQuarantineRecord,
    recover_interrupted: bool,
) -> UseResult<()> {
    let path = container.join(QUARANTINE_RECORD);
    let temporary = container.join(QUARANTINE_TEMPORARY);
    let bytes = canonical_json(record)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_QUARANTINE_RECORD_BYTES {
        return Err(quarantine_state_invalid(
            "The generated Artifact Store quarantine record exceeds its storage bound.",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if recover_interrupted {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
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
        .open(&temporary)
        .await
        .map_err(|error| io_error("open Artifact Store quarantine record", &temporary, error))?;
    let metadata = file.metadata().await.map_err(|error| {
        io_error(
            "inspect opened Artifact Store quarantine temporary",
            &temporary,
            error,
        )
    })?;
    validate_quarantine_metadata(&temporary, &metadata, true)?;
    if let Err(error) = file.write_all(&bytes).await {
        drop(file);
        return Err(io_error(
            "write Artifact Store quarantine record",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        drop(file);
        return Err(io_error(
            "sync Artifact Store quarantine record",
            &temporary,
            error,
        ));
    }
    drop(file);

    let path_for_worker = path.clone();
    let published = tokio::task::spawn_blocking(move || {
        crate::atomic_file::persist_temporary_noclobber_retain_blocking(temporary, &path_for_worker)
    })
    .await
    .map_err(|error| {
        quarantine_state_invalid(format!(
            "Artifact Store quarantine publication worker did not complete: {error}"
        ))
    })?;
    if let Err(error) = published {
        return Err(io_error(
            "publish Artifact Store quarantine record",
            &path,
            error,
        ));
    }
    sync_parent_directory(container, "Artifact Store quarantine record").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_store::{
        ArtifactKind, ArtifactQuarantinePlan, ARTIFACT_QUARANTINE_PLAN_SCHEMA,
        ARTIFACT_QUARANTINE_RECORD_SCHEMA,
    };

    #[tokio::test]
    async fn interrupted_publication_failure_retains_the_fail_closed_sentinel() {
        let directory = tempfile::tempdir().unwrap();
        let container = directory.path();
        let temporary = container.join(QUARANTINE_TEMPORARY);
        let target = container.join(QUARANTINE_RECORD);
        std::fs::write(&temporary, b"partial").unwrap();
        std::fs::write(&target, b"conflict").unwrap();
        let plan = ArtifactQuarantinePlan {
            schema: ARTIFACT_QUARANTINE_PLAN_SCHEMA.to_owned(),
            kind: ArtifactKind::Blob,
            digest: format!("sha256:{}", "a".repeat(64)),
            observed_digest: format!("sha256:{}", "b".repeat(64)),
            content_bytes: 4,
            content_files: 1,
        };
        let record = ArtifactQuarantineRecord {
            schema: ARTIFACT_QUARANTINE_RECORD_SCHEMA.to_owned(),
            plan_digest: plan.descriptor_digest().unwrap(),
            plan,
        };

        write_record(container, &record, true).await.unwrap_err();

        assert_eq!(std::fs::read(&target).unwrap(), b"conflict");
        assert_eq!(
            std::fs::read(&temporary).unwrap(),
            canonical_json(&record).unwrap()
        );
    }
}
