use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::package::{
    copy_package, io_error, owned_package_path, read_manifest, sha256, unique_suffix,
    unix_timestamp, validate_surface_files, write_receipt, RegistryLock,
};
use super::{ExtensionManifest, ExtensionPaths};

const RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionTrust {
    LocalExplicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionReceipt {
    pub schema_version: u32,
    pub package_id: String,
    pub component_id: String,
    pub route: String,
    pub version: String,
    pub package_root: PathBuf,
    pub manifest_sha256: String,
    pub trust: ExtensionTrust,
    pub installed_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtension {
    pub receipt: ExtensionReceipt,
    pub manifest: ExtensionManifest,
}

impl InstalledExtension {
    pub fn surfaces(&self) -> Vec<&'static str> {
        let mut surfaces = Vec::with_capacity(3);
        if self.manifest.cli.is_some() {
            surfaces.push("cli");
        }
        if self.manifest.mcp.is_some() {
            surfaces.push("mcp");
        }
        if self.manifest.skill.is_some() {
            surfaces.push("skill");
        }
        surfaces
    }

    pub fn cli_executable(&self) -> Option<PathBuf> {
        self.manifest
            .cli
            .as_ref()
            .map(|surface| self.receipt.package_root.join(&surface.executable))
    }

    pub fn skill_path(&self) -> Option<PathBuf> {
        self.manifest
            .skill
            .as_ref()
            .map(|surface| self.receipt.package_root.join(&surface.path))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub force: bool,
    pub allow_unsigned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub changed: bool,
    pub extension: InstalledExtension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub package_id: String,
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct ExtensionRegistry {
    paths: ExtensionPaths,
}

impl ExtensionRegistry {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(ExtensionPaths::from_env()?))
    }

    pub fn new(paths: ExtensionPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &ExtensionPaths {
        &self.paths
    }

    pub async fn list(&self) -> UseResult<Vec<InstalledExtension>> {
        let root = self.paths.receipts_root();
        let mut publishers = match fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error("read extension receipts", &root, error)),
        };
        let mut receipt_paths = Vec::new();
        while let Some(publisher) = publishers
            .next_entry()
            .await
            .map_err(|error| io_error("read extension receipt directory", &root, error))?
        {
            let metadata = publisher
                .file_type()
                .await
                .map_err(|error| io_error("inspect receipt publisher", &publisher.path(), error))?;
            if !metadata.is_dir() || metadata.is_symlink() {
                continue;
            }
            let mut entries = fs::read_dir(publisher.path())
                .await
                .map_err(|error| io_error("read publisher receipts", &publisher.path(), error))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| io_error("read publisher receipt", &publisher.path(), error))?
            {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) == Some("json") {
                    receipt_paths.push(path);
                }
            }
        }
        receipt_paths.sort();
        let mut installed = Vec::with_capacity(receipt_paths.len());
        for path in receipt_paths {
            installed.push(self.load_receipt(&path).await?);
        }
        installed.sort_by(|left, right| left.receipt.package_id.cmp(&right.receipt.package_id));
        ensure_unique_routes(&installed)?;
        Ok(installed)
    }

    pub async fn get(&self, package_id: &str) -> UseResult<Option<InstalledExtension>> {
        let package_id = normalize_package_id(package_id)?;
        let path = self.paths.receipt_path(&package_id);
        match fs::metadata(&path).await {
            Ok(_) => self.load_receipt(&path).await.map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("inspect extension receipt", &path, error)),
        }
    }

    pub async fn find_route(&self, route: &str) -> UseResult<Option<InstalledExtension>> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .find(|extension| extension.receipt.route == route))
    }

    pub async fn install_local(
        &self,
        expected_package_id: &str,
        source: &Path,
        options: InstallOptions,
    ) -> UseResult<InstallResult> {
        let expected_package_id = normalize_package_id(expected_package_id)?;
        if !options.allow_unsigned {
            return Err(UseError::new(
                "use.extension.trust_required",
                "Unsigned local extensions require explicit trust approval.",
            )
            .with_suggestion("Rerun the explicit install with --allow-unsigned."));
        }

        let source = fs::canonicalize(source)
            .await
            .map_err(|error| io_error("resolve extension package", source, error))?;
        let source_metadata = fs::metadata(&source)
            .await
            .map_err(|error| io_error("inspect extension package", &source, error))?;
        if !source_metadata.is_dir() {
            return Err(UseError::new(
                "use.extension.package_unsupported",
                "The initial local installer accepts a package directory.",
            )
            .with_suggestion("Extract the package archive and pass its directory with --from."));
        }

        let (manifest, manifest_bytes) = read_manifest(&source).await?;
        if manifest.package_id != expected_package_id {
            return Err(UseError::new(
                "use.extension.identity_mismatch",
                format!(
                    "Requested extension '{}' but the package declares '{}'.",
                    expected_package_id, manifest.package_id
                ),
            ));
        }
        validate_surface_files(&manifest, &source).await?;

        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let installed = self.list().await?;
        if let Some(conflict) = installed.iter().find(|extension| {
            extension.receipt.package_id != expected_package_id
                && extension.receipt.route == manifest.route
        }) {
            return Err(UseError::new(
                "use.extension.route_conflict",
                format!(
                    "Route '{}' is already owned by extension '{}'.",
                    manifest.route, conflict.receipt.package_id
                ),
            ));
        }

        let digest = sha256(&manifest_bytes);
        if let Some(current) = installed
            .iter()
            .find(|extension| extension.receipt.package_id == expected_package_id)
        {
            if !options.force
                && current.receipt.version == manifest.version
                && current.receipt.manifest_sha256 == digest
            {
                return Ok(InstallResult {
                    changed: false,
                    extension: current.clone(),
                });
            }
            if !options.force && current.receipt.version == manifest.version {
                return Err(UseError::new(
                    "use.extension.version_conflict",
                    format!(
                        "Extension '{}' version {} is already active with different content.",
                        expected_package_id, manifest.version
                    ),
                )
                .with_suggestion("Use a new version or rerun the explicit install with --force."));
            }
        }

        let package_parent = self.paths.package_parent(&expected_package_id);
        fs::create_dir_all(&package_parent).await.map_err(|error| {
            io_error("create extension package directory", &package_parent, error)
        })?;
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&package_parent)
            .map_err(|error| {
                io_error("create extension staging directory", &package_parent, error)
            })?;
        copy_package(&source, staging.path()).await?;
        let (staged_manifest, staged_bytes) = read_manifest(staging.path()).await?;
        if staged_manifest != manifest || sha256(&staged_bytes) != digest {
            return Err(UseError::new(
                "use.extension.package_changed",
                "The extension manifest changed while the package was staged.",
            ));
        }
        validate_surface_files(&staged_manifest, staging.path()).await?;

        let target = self
            .paths
            .package_root(&expected_package_id, &manifest.version);
        let backup =
            package_parent.join(format!(".backup-{}-{}", manifest.version, unique_suffix()));
        let staging = staging.keep();
        let had_target = fs::metadata(&target).await.is_ok();
        if had_target {
            fs::rename(&target, &backup)
                .await
                .map_err(|error| io_error("back up active extension package", &target, error))?;
        }
        if let Err(error) = fs::rename(&staging, &target).await {
            if had_target {
                let _ = fs::rename(&backup, &target).await;
            }
            let _ = fs::remove_dir_all(&staging).await;
            return Err(io_error("activate extension package", &target, error));
        }

        let receipt = ExtensionReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            package_id: expected_package_id.clone(),
            component_id: format!("use/{expected_package_id}"),
            route: manifest.route.clone(),
            version: manifest.version.clone(),
            package_root: target.clone(),
            manifest_sha256: digest,
            trust: ExtensionTrust::LocalExplicit,
            installed_at_unix: unix_timestamp(),
        };
        let receipt_path = self.paths.receipt_path(&expected_package_id);
        if let Err(error) = write_receipt(&receipt_path, &receipt).await {
            let _ = fs::remove_dir_all(&target).await;
            if had_target {
                let _ = fs::rename(&backup, &target).await;
            }
            return Err(error);
        }

        if had_target {
            let _ = fs::remove_dir_all(&backup).await;
        }
        if let Some(previous) = installed
            .iter()
            .find(|extension| extension.receipt.package_id == expected_package_id)
        {
            if previous.receipt.package_root != target
                && owned_package_path(
                    &self.paths,
                    &expected_package_id,
                    &previous.receipt.package_root,
                )
            {
                let _ = fs::remove_dir_all(&previous.receipt.package_root).await;
            }
        }

        Ok(InstallResult {
            changed: true,
            extension: InstalledExtension { receipt, manifest },
        })
    }

    pub async fn uninstall(&self, package_id: &str) -> UseResult<UninstallResult> {
        let package_id = normalize_package_id(package_id)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let Some(extension) = self.get(&package_id).await? else {
            return Ok(UninstallResult {
                package_id,
                changed: false,
            });
        };
        let receipt_path = self.paths.receipt_path(&package_id);
        fs::remove_file(&receipt_path)
            .await
            .map_err(|error| io_error("disable extension route", &receipt_path, error))?;
        if !owned_package_path(&self.paths, &package_id, &extension.receipt.package_root) {
            return Err(UseError::new(
                "use.extension.ownership_invalid",
                "The extension receipt does not own its package directory.",
            )
            .with_detail("routeDisabled", true));
        }
        if let Err(error) = fs::remove_dir_all(&extension.receipt.package_root).await {
            return Err(io_error(
                "remove extension package",
                &extension.receipt.package_root,
                error,
            )
            .with_detail("routeDisabled", true));
        }
        Ok(UninstallResult {
            package_id,
            changed: true,
        })
    }

    async fn load_receipt(&self, receipt_path: &Path) -> UseResult<InstalledExtension> {
        let bytes = fs::read(receipt_path)
            .await
            .map_err(|error| io_error("read extension receipt", receipt_path, error))?;
        let receipt: ExtensionReceipt = serde_json::from_slice(&bytes).map_err(|error| {
            UseError::new(
                "use.extension.receipt_invalid",
                format!(
                    "Invalid extension receipt '{}': {error}",
                    receipt_path.display()
                ),
            )
        })?;
        if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(UseError::new(
                "use.extension.receipt_incompatible",
                format!(
                    "Extension receipt schema {} is not supported.",
                    receipt.schema_version
                ),
            ));
        }
        let package_id = normalize_package_id(&receipt.package_id)?;
        if receipt.component_id != format!("use/{package_id}")
            || !owned_package_path(&self.paths, &package_id, &receipt.package_root)
        {
            return Err(UseError::new(
                "use.extension.ownership_invalid",
                format!(
                    "Receipt for '{}' has invalid ownership metadata.",
                    package_id
                ),
            ));
        }
        let (manifest, manifest_bytes) = read_manifest(&receipt.package_root).await?;
        if manifest.package_id != receipt.package_id
            || manifest.version != receipt.version
            || manifest.route != receipt.route
            || sha256(&manifest_bytes) != receipt.manifest_sha256
        {
            return Err(UseError::new(
                "use.extension.receipt_mismatch",
                format!(
                    "Installed package '{}' does not match its receipt.",
                    package_id
                ),
            ));
        }
        validate_surface_files(&manifest, &receipt.package_root).await?;
        Ok(InstalledExtension { receipt, manifest })
    }
}

fn normalize_package_id(value: &str) -> UseResult<String> {
    let value = value.strip_prefix("use/").unwrap_or(value);
    if !super::valid_package_id(value) {
        return Err(UseError::new(
            "use.extension.id_invalid",
            "Extension IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    Ok(value.to_string())
}

fn ensure_unique_routes(installed: &[InstalledExtension]) -> UseResult<()> {
    for (index, extension) in installed.iter().enumerate() {
        if let Some(conflict) = installed[index + 1..]
            .iter()
            .find(|candidate| candidate.receipt.route == extension.receipt.route)
        {
            return Err(UseError::new(
                "use.extension.route_conflict",
                format!(
                    "Route '{}' is claimed by '{}' and '{}'.",
                    extension.receipt.route,
                    extension.receipt.package_id,
                    conflict.receipt.package_id
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "registry_tests.rs"]
mod tests;
