use std::collections::HashMap;
use std::path::Path;

use a3s_acl::{Block, Document, Value};
use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    quota_config_invalid, ArtifactStorageQuotaPolicy, ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION,
};
use crate::package::{
    activate_temporary_file, io_error, remove_file_with_windows_retry, sync_parent_directory,
};

pub(in crate::artifact_store) const STORAGE_QUOTA_POLICY_FILE: &str = "storage-quota.acl";
pub(in crate::artifact_store) const STORAGE_QUOTA_TEMPORARY_FILE: &str = ".storage-quota.tmp";
const MAX_STORAGE_QUOTA_POLICY_BYTES: u64 = 4 * 1024;

const ROOT_BLOCK: &str = "artifact_storage_quota";
const DISABLED_REVISION_INPUT: &[u8] = b"a3s-use-artifact-storage-quota-disabled-v1";

pub(super) fn revision(policy: Option<ArtifactStorageQuotaPolicy>) -> String {
    let digest = match policy {
        Some(policy) => Sha256::digest(encode(policy).as_bytes()),
        None => Sha256::digest(DISABLED_REVISION_INPUT),
    };
    format!("{digest:x}")
}

pub(super) async fn load_policy(root: &Path) -> UseResult<Option<ArtifactStorageQuotaPolicy>> {
    let path = root.join(STORAGE_QUOTA_POLICY_FILE);
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error(
                "inspect Artifact Store quota policy",
                &path,
                error,
            ))
        }
    };
    validate_policy_metadata(&path, &metadata, false)?;

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
        .open(&path)
        .await
        .map_err(|error| io_error("open Artifact Store quota policy", &path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened Artifact Store quota policy", &path, error))?;
    validate_policy_metadata(&path, &opened, false)?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    (&mut file)
        .take(MAX_STORAGE_QUOTA_POLICY_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read Artifact Store quota policy", &path, error))?;
    if bytes.len() as u64 != opened.len() {
        return Err(quota_config_invalid(
            "Artifact Store quota policy changed while it was read.",
        ));
    }
    let input = std::str::from_utf8(&bytes).map_err(|error| {
        quota_config_invalid(format!(
            "Artifact Store quota policy must be UTF-8 A3S ACL: {error}"
        ))
    })?;
    decode(input).map(Some)
}

pub(super) async fn write_policy(root: &Path, policy: ArtifactStorageQuotaPolicy) -> UseResult<()> {
    let path = root.join(STORAGE_QUOTA_POLICY_FILE);
    let temporary = root.join(STORAGE_QUOTA_TEMPORARY_FILE);
    remove_optional_temporary(&temporary).await?;
    let bytes = encode(policy).into_bytes();
    if bytes.is_empty() || bytes.len() as u64 > MAX_STORAGE_QUOTA_POLICY_BYTES {
        return Err(quota_config_invalid(
            "Generated Artifact Store quota policy exceeds its storage bound.",
        ));
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| io_error("create Artifact Store quota policy", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        drop(file);
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write Artifact Store quota policy",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        drop(file);
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "sync Artifact Store quota policy",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path,
        "activate Artifact Store quota policy",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(root, "Artifact Store quota policy").await
}

pub(super) async fn remove_policy(root: &Path) -> UseResult<()> {
    let path = root.join(STORAGE_QUOTA_POLICY_FILE);
    remove_file_with_windows_retry(path, "remove Artifact Store quota policy").await?;
    sync_parent_directory(root, "Artifact Store quota policy").await
}

pub(super) async fn remove_optional_temporary(path: &Path) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            validate_policy_metadata(path, &metadata, true)?;
            remove_file_with_windows_retry(
                path.to_path_buf(),
                "remove stale Artifact Store quota policy staging",
            )
            .await
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(
            "inspect Artifact Store quota policy staging",
            path,
            error,
        )),
    }
}

pub(in crate::artifact_store) fn validate_policy_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    allow_empty: bool,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || (!allow_empty && metadata.len() == 0)
        || metadata.len() > MAX_STORAGE_QUOTA_POLICY_BYTES
    {
        return Err(quota_config_invalid(format!(
            "Artifact Store quota state '{}' is not a bounded regular owned file.",
            path.display()
        )));
    }
    Ok(())
}

fn encode(policy: ArtifactStorageQuotaPolicy) -> String {
    let mut attributes = HashMap::new();
    attributes.insert(
        "schema_version".to_owned(),
        Value::Number(ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION as f64),
    );
    attributes.insert(
        "max_physical_bytes".to_owned(),
        Value::String(policy.max_physical_bytes().to_string()),
    );
    attributes.insert(
        "max_physical_artifacts".to_owned(),
        Value::String(policy.max_physical_artifacts().to_string()),
    );
    let document = Document {
        blocks: vec![Block {
            name: ROOT_BLOCK.to_owned(),
            labels: Vec::new(),
            blocks: Vec::new(),
            attributes,
        }],
    };
    let mut encoded = a3s_acl::generate_acl(&document);
    encoded.push('\n');
    encoded
}

fn decode(input: &str) -> UseResult<ArtifactStorageQuotaPolicy> {
    let parsed = a3s_acl::parse_acl(input).map_err(|error| {
        quota_config_invalid(format!("Failed to parse Artifact Store quota ACL: {error}"))
    })?;
    let [root] = parsed.blocks.as_slice() else {
        return Err(quota_config_invalid(
            "Artifact Store quota policy must contain exactly one artifact_storage_quota block.",
        ));
    };
    if root.name != ROOT_BLOCK || !root.labels.is_empty() || !root.blocks.is_empty() {
        return Err(quota_config_invalid(
            "Artifact Store quota policy must contain one unlabeled block without nested blocks.",
        ));
    }
    let allowed = [
        "schema_version",
        "max_physical_bytes",
        "max_physical_artifacts",
    ];
    if let Some(name) = root
        .attributes
        .keys()
        .find(|name| !allowed.contains(&name.as_str()))
    {
        return Err(quota_config_invalid(format!(
            "Artifact Store quota policy contains unknown attribute '{name}'."
        )));
    }
    let schema = root
        .attributes
        .get("schema_version")
        .and_then(Value::as_number)
        .ok_or_else(|| {
            quota_config_invalid("Artifact Store quota policy requires numeric schema_version.")
        })?;
    if schema != ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION as f64 {
        return Err(quota_config_invalid(format!(
            "Artifact Store quota schema version must be {ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION}."
        )));
    }
    let policy = ArtifactStorageQuotaPolicy::new(
        decimal_attribute(root, "max_physical_bytes")?,
        decimal_attribute(root, "max_physical_artifacts")?,
    )
    .map_err(|error| quota_config_invalid(error.message))?;
    if encode(policy) != input {
        return Err(quota_config_invalid(
            "Artifact Store quota policy is not in canonical A3S ACL form.",
        ));
    }
    Ok(policy)
}

fn decimal_attribute(block: &Block, name: &str) -> UseResult<u64> {
    let value = block
        .attributes
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            quota_config_invalid(format!(
                "Artifact Store quota attribute '{name}' must be an unsigned decimal string."
            ))
        })?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(quota_config_invalid(format!(
            "Artifact Store quota attribute '{name}' must be an unsigned decimal string."
        )));
    }
    value.parse::<u64>().map_err(|error| {
        quota_config_invalid(format!(
            "Artifact Store quota attribute '{name}' is outside the supported range: {error}"
        ))
    })
}
