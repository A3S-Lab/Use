use std::path::{Path, PathBuf};

use a3s_use_core::{LockedPluginPackage, UseError, UseResult, VerifiedPluginCatalogRecord};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs;

use super::super::{
    normalize_package_id, ExtensionReceipt, ExtensionTrust, InstalledExtension,
    RECEIPT_SCHEMA_VERSION_V3,
};
use crate::package::io_error;
use crate::remote::ResolvedRemotePackage;
use crate::source::PreparedPackageSource;
use crate::{ExtensionManifest, ExtensionPaths};

/// Exact package identity owned by one schema-v3 lifecycle operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecycleIdentity {
    pub(super) package_id: String,
    pub(super) package_digest: String,
    pub(super) manifest_digest: String,
    pub(super) generation: u64,
    #[serde(skip)]
    package_sha256: String,
    #[serde(skip)]
    manifest_sha256: String,
}

impl ExtensionLifecycleIdentity {
    pub fn new(
        package_id: impl AsRef<str>,
        package_digest: impl Into<String>,
        manifest_digest: impl Into<String>,
        generation: u64,
    ) -> UseResult<Self> {
        let package_id = normalize_package_id(package_id.as_ref())?;
        let package_digest = canonical_sha256(package_digest.into(), "package")?;
        let manifest_digest = canonical_sha256(manifest_digest.into(), "manifest")?;
        let package_sha256 = package_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| lifecycle_identity_error("The package digest prefix is invalid."))?
            .to_string();
        let manifest_sha256 = manifest_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| lifecycle_identity_error("The manifest digest prefix is invalid."))?
            .to_string();
        if generation == 0 {
            return Err(lifecycle_identity_error(
                "A lifecycle package generation must be positive.",
            ));
        }
        Ok(Self {
            package_id,
            package_digest,
            manifest_digest,
            generation,
            package_sha256,
            manifest_sha256,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            lifecycle_identity_error(format!(
                "Failed to encode the lifecycle package identity: {error}"
            ))
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    pub(super) fn package_sha256(&self) -> &str {
        &self.package_sha256
    }

    pub(super) fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
}

/// Validated schema-v3 package bytes retained until immutable commit.
///
/// Constructors preserve the same trust boundaries as the legacy installer:
/// local packages require explicit approval, release bundles recheck their
/// reviewed digest, and remote packages can only originate from a verified
/// TUF download object.
#[derive(Debug)]
pub struct ExtensionLifecyclePackage {
    pub(super) source: PreparedPackageSource,
    pub(super) manifest: ExtensionManifest,
    pub(super) package_digest: String,
    pub(super) manifest_digest: String,
    pub(super) trust: ExtensionTrust,
    pub(super) registry: Option<ResolvedRemotePackage>,
    pub(super) verified_catalog: Option<VerifiedPluginCatalogRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecycleResult {
    pub changed: bool,
    pub extension: InstalledExtension,
    pub registry_generation: u64,
}

/// Exact non-secret evidence for one dependency-graph Registry cutover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecycleGraphPublication {
    pub packages: Vec<ExtensionLifecycleResult>,
    pub registry_generation: u64,
    pub registry_snapshot_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionLifecycleRollbackResult {
    pub package_id: String,
    pub changed: bool,
    pub registry_generation: u64,
}

#[derive(Debug, Clone)]
pub(super) struct RemovedLifecyclePackage {
    pub(super) identity: ExtensionLifecycleIdentity,
    pub(super) extension: InstalledExtension,
    pub(super) selected: bool,
}

pub(super) fn lifecycle_root(
    paths: &ExtensionPaths,
    identity: &ExtensionLifecycleIdentity,
) -> PathBuf {
    paths.lifecycle_package_root(
        identity.package_id(),
        identity.generation(),
        identity.package_sha256(),
    )
}

pub(super) fn exact_receipt(
    identity: &ExtensionLifecycleIdentity,
    receipt: &ExtensionReceipt,
) -> UseResult<()> {
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION_V3
        || receipt.package_id != identity.package_id
        || receipt.lifecycle_generation != Some(identity.generation)
        || receipt.package_sha256.as_deref() != Some(identity.package_sha256())
        || receipt.manifest_sha256 != identity.manifest_sha256()
    {
        return Err(lifecycle_identity_error(
            "The installed cognitive package does not match the exact lifecycle generation.",
        ));
    }
    Ok(())
}

pub(super) async fn remove_exact_root(path: &Path) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error("inspect lifecycle package", path, error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.ownership_invalid",
            format!(
                "Refusing to remove invalid lifecycle package root '{}'.",
                path.display()
            ),
        ));
    }
    fs::remove_dir_all(path)
        .await
        .map_err(|error| io_error("remove lifecycle package generation", path, error))?;
    Ok(true)
}

pub(super) fn validate_locked_extension(
    locked: &LockedPluginPackage,
    extension: &InstalledExtension,
    host_version: &str,
) -> UseResult<()> {
    let record = &locked.catalog.record;
    let package_sha256 = record
        .package
        .sha256
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    let manifest_sha256 = record
        .package
        .manifest_sha256
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    if extension.receipt.schema_version != RECEIPT_SCHEMA_VERSION_V3
        || extension.receipt.trust != ExtensionTrust::RegistryTuf
        || extension.receipt.package_id != record.package_id
        || extension.receipt.version != record.version
        || extension.receipt.package_sha256.as_deref() != package_sha256
        || Some(extension.receipt.manifest_sha256.as_str()) != manifest_sha256
        || extension.plan_ready_catalog()? != &locked.catalog
        || !extension.supports_use_version(host_version)
    {
        return Err(lifecycle_graph_error(
            "An installed cognitive package does not match its reviewed dependency-lock node.",
        ));
    }
    Ok(())
}

fn canonical_sha256(value: String, label: &str) -> UseResult<String> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    if !valid {
        return Err(lifecycle_identity_error(format!(
            "The lifecycle {label} digest must be canonical SHA-256."
        )));
    }
    Ok(value)
}

pub(super) fn lifecycle_identity_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.lifecycle_identity_mismatch", message)
}

pub(super) fn lifecycle_state_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.lifecycle_state_invalid", message)
}

pub(super) fn lifecycle_graph_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.lifecycle_package_graph_invalid", message)
}
