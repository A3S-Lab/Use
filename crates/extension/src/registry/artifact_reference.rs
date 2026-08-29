use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncReadExt;

use super::{
    normalize_package_id, ExtensionReceipt, ExtensionTrust, EXTENSION_RECEIPT_SCHEMA_VERSION,
    MAX_EXTENSION_RECEIPT_BYTES,
};
use crate::package::io_error;
use crate::remote::ResolvedRemotePackage;
use crate::ArtifactStore;

/// Physical expectations carried by one validated extension receipt without
/// requiring the referenced package tree to be present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionArtifactReference {
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_files: Option<u64>,
}

pub(super) fn validate_receipt_artifact_reference(
    receipt: &ExtensionReceipt,
    artifact_store: &ArtifactStore,
) -> UseResult<ExtensionArtifactReference> {
    if receipt.schema_version != EXTENSION_RECEIPT_SCHEMA_VERSION {
        return Err(UseError::new(
            "use.extension.receipt_incompatible",
            format!(
                "Extension receipt schema {} is obsolete; remove the pre-release state and reinstall the package.",
                receipt.schema_version
            ),
        ));
    }
    receipt.installation.validate().map_err(|_| {
        UseError::new(
            "use.extension.receipt_invalid",
            "The extension receipt installation identity is invalid.",
        )
    })?;
    let generation = receipt.lifecycle_generation.ok_or_else(|| {
        UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "A cognitive-package receipt omitted its generation.",
        )
    })?;
    let package_sha256 = receipt.package_sha256.as_deref().ok_or_else(|| {
        UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "A cognitive-package receipt omitted its package digest.",
        )
    })?;
    if generation == 0
        || !valid_raw_sha256(package_sha256)
        || !valid_raw_sha256(&receipt.manifest_sha256)
    {
        return Err(UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            format!(
                "Extension receipt for '{}' has an invalid generation or package digest.",
                receipt.package_id
            ),
        ));
    }

    let package_id = normalize_package_id(&receipt.package_id)?;
    let digest = format!("sha256:{package_sha256}");
    let expected_path = artifact_store.expanded_package_path(&digest)?;
    if receipt.component_id != format!("use/{package_id}")
        || !receipt.package_root.is_absolute()
        || receipt.package_root != expected_path
    {
        return Err(UseError::new(
            "use.extension.ownership_invalid",
            format!("Receipt for '{package_id}' has invalid ownership metadata."),
        ));
    }

    let (expected_bytes, expected_files) = match (
        receipt.trust,
        receipt.registry.as_ref(),
        receipt.verified_catalog.as_ref(),
        receipt.planning_bundle.as_ref(),
    ) {
        (ExtensionTrust::LocalExplicit | ExtensionTrust::ReleaseBundle, None, None, None) => {
            (None, None)
        }
        (ExtensionTrust::RegistryTuf, Some(registry), Some(catalog), planning_bundle) => {
            registry.validate_provenance()?;
            catalog.validate().map_err(|error| {
                UseError::new(
                    "use.extension.receipt_invalid",
                    format!(
                        "Extension receipt for '{}' has invalid catalog evidence: {}",
                        receipt.package_id, error.message
                    ),
                )
            })?;
            let resolved = ResolvedRemotePackage::from_verified_catalog(catalog)?;
            let record = &catalog.record;
            let expected_manifest_digest = format!("sha256:{}", receipt.manifest_sha256);
            if registry != &resolved
                || registry.package_id != receipt.package_id
                || registry.version != receipt.version
                || !record.is_package_plan_ready()
                || record.package.sha256.as_deref() != Some(digest.as_str())
                || record.package.manifest_sha256.as_deref()
                    != Some(expected_manifest_digest.as_str())
            {
                return Err(UseError::new(
                    "use.extension.receipt_invalid",
                    format!(
                        "Registry provenance for '{}' does not match its receipt.",
                        receipt.package_id
                    ),
                ));
            }
            match (record.planning.as_ref(), planning_bundle) {
                (None, None) => {}
                (Some(_), Some(bundle)) => bundle.validate_catalog_binding(catalog)?,
                _ => {
                    return Err(UseError::new(
                        "use.extension.receipt_invalid",
                        format!(
                        "Extension receipt for '{}' does not retain its signed planning target.",
                        receipt.package_id
                    ),
                    )
                    .with_suggestion("Reinstall the cognitive package from its trusted Registry."))
                }
            }
            (
                Some(record.package.expanded_bytes),
                Some(record.package.file_count),
            )
        }
        _ => {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                format!(
                    "Extension receipt for '{}' has inconsistent trust provenance.",
                    receipt.package_id
                ),
            ))
        }
    };
    Ok(ExtensionArtifactReference {
        digest,
        expected_bytes,
        expected_files,
    })
}

pub(super) async fn read_extension_receipt_bytes(path: &Path) -> UseResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect extension receipt", path, error))?;
    validate_extension_receipt_metadata(path, &metadata)?;

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
        .map_err(|error| io_error("open extension receipt", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened extension receipt", path, error))?;
    validate_extension_receipt_metadata(path, &opened)?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    (&mut file)
        .take(MAX_EXTENSION_RECEIPT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read extension receipt", path, error))?;
    if bytes.len() as u64 != opened.len() {
        return Err(UseError::new(
            "use.extension.receipt_invalid",
            format!(
                "Extension receipt '{}' changed while it was read.",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

fn validate_extension_receipt_metadata(path: &Path, metadata: &std::fs::Metadata) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_EXTENSION_RECEIPT_BYTES
    {
        return Err(UseError::new(
            "use.extension.receipt_invalid",
            format!(
                "Extension receipt '{}' is not a bounded owned regular file.",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn valid_raw_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
