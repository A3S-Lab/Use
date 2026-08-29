use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::digest::package_fingerprint;
use super::package::{io_error, read_manifest, validate_surface_files};

pub const RELEASE_BUNDLE_SCHEMA_VERSION: u32 = 2;

/// One optional extension package shipped inside a verified A3S Use release.
///
/// The release bundle remains uninstallable and disabled until the user asks
/// to install it. Its expanded-package digest is included in the outer A3S
/// review plan and rechecked by A3S Use immediately before activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseBundlePackage {
    pub schema_version: u32,
    pub package_id: String,
    pub component_id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_alias: Option<String>,
    pub package_sha256: String,
    pub file_count: u64,
    pub byte_count: u64,
    pub surfaces: Vec<String>,
    pub activity_count: u64,
}

impl ReleaseBundlePackage {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema_version != RELEASE_BUNDLE_SCHEMA_VERSION {
            return Err(UseError::new(
                "use.extension.release_bundle_incompatible",
                format!(
                    "Release bundle schema {} is not supported.",
                    self.schema_version
                ),
            ));
        }
        if self.component_id != format!("use/{}", self.package_id)
            || self
                .route_alias
                .as_deref()
                .is_some_and(|alias| !super::valid_route_alias(alias))
            || self.package_sha256.len() != 64
            || !self
                .package_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(UseError::new(
                "use.extension.release_bundle_invalid",
                format!(
                    "Release bundle metadata for '{}' is inconsistent.",
                    self.package_id
                ),
            ));
        }
        Ok(())
    }
}

pub async fn inspect_release_bundle(package_root: &Path) -> UseResult<ReleaseBundlePackage> {
    let metadata = fs::symlink_metadata(package_root)
        .await
        .map_err(|error| io_error("inspect release bundle", package_root, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.release_bundle_invalid",
            format!(
                "Release bundle '{}' must be a real package directory.",
                package_root.display()
            ),
        ));
    }
    let package_root = fs::canonicalize(package_root)
        .await
        .map_err(|error| io_error("resolve release bundle", package_root, error))?;
    let (manifest, _) = read_manifest(&package_root).await?;
    validate_surface_files(&manifest, &package_root).await?;
    let fingerprint = package_fingerprint(&package_root).await?;
    let surfaces = manifest
        .surface_kinds()
        .into_iter()
        .map(str::to_string)
        .collect();
    let activity_count = manifest.ui_count() as u64;
    let package = ReleaseBundlePackage {
        schema_version: RELEASE_BUNDLE_SCHEMA_VERSION,
        component_id: format!("use/{}", manifest.package_id),
        package_id: manifest.package_id,
        version: manifest.version,
        route_alias: manifest.route_alias,
        package_sha256: fingerprint.sha256,
        file_count: fingerprint.file_count,
        byte_count: fingerprint.byte_count,
        surfaces,
        activity_count,
    };
    package.validate()?;
    Ok(package)
}
