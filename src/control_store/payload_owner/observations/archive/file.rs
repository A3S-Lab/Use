use std::path::Path;

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::AsyncReadExt;

use super::super::{observation_error, MAX_RECORD_BYTES};
use super::archive_io;

pub(super) async fn read_record(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    if expected_length == 0 || expected_length > MAX_RECORD_BYTES {
        return Err(observation_error(
            "An observation record exceeds its per-record byte bound.",
        ));
    }
    let (mut file, opened) = open_owned_regular_file(path, "observation record").await?;
    if opened.len() != expected_length {
        return Err(observation_error(
            "An observation record changed before it was opened.",
        ));
    }
    let capacity = usize::try_from(expected_length)
        .map_err(|_| observation_error("An observation record length is invalid."))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| archive_io("read observation record", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(observation_error(
            "An observation record changed while it was read.",
        ));
    }
    let opened_after = file
        .metadata()
        .await
        .map_err(|error| archive_io("reinspect opened observation record", error))?;
    if !owned_regular_metadata(&opened_after) || opened_after.len() != expected_length {
        return Err(observation_error(
            "An observation record changed while it was read.",
        ));
    }
    let path_after = inspect_owned_regular_file(path, "observation record").await?;
    if path_after.len() != expected_length {
        return Err(observation_error(
            "An observation record changed while it was read.",
        ));
    }
    Ok(bytes)
}

pub(in crate::control_store::payload_owner::observations) async fn open_owned_regular_file(
    path: &Path,
    label: &str,
) -> UseResult<(fs::File, std::fs::Metadata)> {
    inspect_owned_regular_file(path, label).await?;
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
    let file = options
        .open(path)
        .await
        .map_err(|error| archive_io(&format!("open {label}"), error))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| archive_io(&format!("inspect opened {label}"), error))?;
    if !owned_regular_metadata(&metadata) {
        return Err(observation_error(format!(
            "The {label} is not an owned regular file."
        )));
    }
    Ok((file, metadata))
}

pub(in crate::control_store::payload_owner::observations) async fn inspect_owned_regular_file(
    path: &Path,
    label: &str,
) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| archive_io(&format!("inspect {label}"), error))?;
    if !owned_regular_metadata(&metadata) {
        return Err(observation_error(format!(
            "The {label} is not an owned regular file."
        )));
    }
    Ok(metadata)
}

fn owned_regular_metadata(metadata: &std::fs::Metadata) -> bool {
    !a3s_use_core::metadata_is_link_or_reparse_point(metadata) && metadata.is_file()
}
