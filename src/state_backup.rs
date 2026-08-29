//! Coordinated, path-free inventory backups for all A3S Use-owned state.
//!
//! A state backup is integrity evidence for one quiescent Use installation. It
//! is deliberately not a restore authority: clean-machine recovery still
//! requires an independently reviewed restore plan and retained trust/Grant
//! authority.

use std::path::{Component, Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, ExtensionRegistry, StateMaintenanceLock};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod archive;
mod inventory;
mod retention;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod tests;

pub use retention::{
    StateBackupRetentionEntry, StateBackupRetentionPlan, StateBackupRetentionPolicy,
    StateBackupRetentionResult, A3S_USE_STATE_BACKUP_RETENTION_PLAN_SCHEMA,
    A3S_USE_STATE_BACKUP_RETENTION_RESULT_SCHEMA, DEFAULT_STATE_BACKUP_RETENTION_MAX_BACKUPS,
    DEFAULT_STATE_BACKUP_RETENTION_MAX_BYTES, MAX_STATE_BACKUP_RETENTION_BACKUPS,
    MAX_STATE_BACKUP_RETENTION_BYTES, MIN_STATE_BACKUP_RETENTION_BACKUPS,
};

pub const A3S_USE_STATE_BACKUP_SCHEMA: &str = "a3s.use.state-backup.v2";
pub const MAX_STATE_BACKUP_FILES: u64 = 100_000;
pub const MAX_STATE_BACKUP_ENTRIES: u64 = 200_000;
pub const MAX_STATE_BACKUP_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_STATE_BACKUP_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_STATE_BACKUP_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_STATE_BACKUP_PATH_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateBackupRoot {
    Data,
    State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateBackupFamily {
    Registry,
    RetainedGenerations,
    Grants,
    Bindings,
    LifecycleOperations,
    PackageOperations,
    Knowledge,
    PackageGraph,
    Enablement,
    HostManager,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupEntry {
    pub root: StateBackupRoot,
    pub path: String,
    pub family: StateBackupFamily,
    pub length: u64,
    pub sha256: String,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupFamilySummary {
    pub family: StateBackupFamily,
    pub file_count: u64,
    pub byte_count: u64,
    pub inventory_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupPackageAuthority {
    pub package_id: String,
    pub receipt_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupAuthority {
    pub registry_generation: u64,
    pub registry_digest: String,
    pub packages: Vec<StateBackupPackageAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackupManifest {
    pub schema: String,
    pub installation: a3s_use_core::InstallationId,
    pub use_version: String,
    pub os: String,
    pub architecture: String,
    pub file_count: u64,
    pub byte_count: u64,
    pub inventory_digest: String,
    pub authority: StateBackupAuthority,
    pub families: Vec<StateBackupFamilySummary>,
    pub entries: Vec<StateBackupEntry>,
}

impl StateBackupManifest {
    pub fn validate(&self) -> UseResult<()> {
        archive::validate_manifest(self)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(sha256_digest(&canonical_json(self)?))
    }
}

pub(crate) fn scan_state_for_restore(
    paths: &ExtensionPaths,
    active_plan_digest: Option<&str>,
) -> UseResult<Vec<StateBackupEntry>> {
    inventory::scan_for_state_restore(paths, active_plan_digest)
}

pub(crate) fn validate_state_backup_entry_path(
    root: StateBackupRoot,
    path: &str,
) -> UseResult<StateBackupFamily> {
    inventory::validate_archived_path(root, path)?;
    inventory::expected_family(root, path)
}

pub(crate) async fn stage_state_restore_entries(
    backup_path: impl AsRef<Path>,
    expected_manifest: StateBackupManifest,
    selected_entries: Vec<StateBackupEntry>,
    data_candidate_root: PathBuf,
    state_candidate_root: PathBuf,
) -> UseResult<()> {
    let backup_path = backup_path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || {
        archive::stage_restore_entries(
            &backup_path,
            &expected_manifest,
            &selected_entries,
            &data_candidate_root,
            &state_candidate_root,
        )
    })
    .await
    .map_err(|error| {
        state_backup_io(format!(
            "The state restore staging worker did not complete: {error}"
        ))
    })?
}

pub(crate) async fn validate_state_restore_entries(
    expected_manifest: StateBackupManifest,
    selected_entries: Vec<StateBackupEntry>,
    data_candidate_root: PathBuf,
    state_candidate_root: PathBuf,
) -> UseResult<()> {
    tokio::task::spawn_blocking(move || {
        archive::validate_restore_entries(
            &expected_manifest,
            &selected_entries,
            &data_candidate_root,
            &state_candidate_root,
        )
    })
    .await
    .map_err(|error| {
        state_backup_io(format!(
            "The state restore candidate validation worker did not complete: {error}"
        ))
    })?
}

#[derive(Debug, Clone)]
pub struct StateBackupManager {
    paths: ExtensionPaths,
}

impl StateBackupManager {
    pub fn new(paths: ExtensionPaths) -> Self {
        Self { paths }
    }

    /// Create one deterministic backup without overwriting an existing path.
    pub async fn backup(&self, destination: impl AsRef<Path>) -> UseResult<StateBackupManifest> {
        validate_owned_roots(&self.paths)?;
        let _maintenance = StateMaintenanceLock::new(self.paths.state_root())
            .acquire_exclusive()
            .await?;
        self.backup_under_exclusive(destination).await
    }

    /// Create a coordinated backup while the caller retains the exclusive
    /// maintenance guard. This is used to bind rollback evidence before a
    /// whole-installation restore publishes its active marker.
    pub(crate) async fn backup_under_exclusive(
        &self,
        destination: impl AsRef<Path>,
    ) -> UseResult<StateBackupManifest> {
        validate_owned_roots(&self.paths)?;
        let destination = resolve_destination(destination.as_ref(), &self.paths)?;
        inventory::reject_active_restore(self.paths.state_root())?;
        let authority = read_authority(&self.paths).await?;
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || archive::create_backup(&paths, &destination, authority))
            .await
            .map_err(|error| {
                state_backup_io(format!(
                    "The coordinated backup worker did not complete: {error}"
                ))
            })?
    }

    /// Verify the manifest, complete archive length, and every payload digest
    /// without extracting or consulting any local Use state.
    pub async fn verify_backup(path: impl AsRef<Path>) -> UseResult<StateBackupManifest> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || archive::verify_backup(&path))
            .await
            .map_err(|error| {
                state_backup_io(format!(
                    "The backup verification worker did not complete: {error}"
                ))
            })?
    }

    /// Build a path-free oldest-first retention plan for one external
    /// directory of fully verified coordinated backups. No archive is removed.
    pub async fn plan_backup_retention(
        &self,
        directory: impl AsRef<Path>,
        policy: StateBackupRetentionPolicy,
    ) -> UseResult<StateBackupRetentionPlan> {
        validate_owned_roots(&self.paths)?;
        let directory = retention::resolve_directory(directory.as_ref(), &self.paths)?;
        let installation = self.paths.installation().clone();
        tokio::task::spawn_blocking(move || retention::plan(&directory, &installation, policy))
            .await
            .map_err(|error| {
                retention::retention_io(format!(
                    "The state backup retention planning worker did not complete: {error}"
                ))
            })?
    }

    /// Remove only the archives selected by the exact canonical plan digest
    /// after re-verifying the unchanged external directory inventory.
    pub async fn apply_backup_retention(
        &self,
        directory: impl AsRef<Path>,
        policy: StateBackupRetentionPolicy,
        expected_plan_digest: impl Into<String>,
    ) -> UseResult<StateBackupRetentionResult> {
        validate_owned_roots(&self.paths)?;
        let directory = retention::resolve_directory(directory.as_ref(), &self.paths)?;
        let expected_plan_digest = expected_plan_digest.into();
        let installation = self.paths.installation().clone();
        tokio::task::spawn_blocking(move || {
            retention::apply(&directory, &installation, policy, &expected_plan_digest)
        })
        .await
        .map_err(|error| {
            retention::retention_io(format!(
                "The state backup retention apply worker did not complete: {error}"
            ))
        })?
    }
}

pub(crate) fn validate_owned_roots(paths: &ExtensionPaths) -> UseResult<()> {
    let data_root = canonical_or_absolute(paths.use_paths().data_root())?;
    let state_root = canonical_or_absolute(paths.use_paths().state_root())?;
    if data_root == state_root
        || data_root.starts_with(&state_root)
        || state_root.starts_with(&data_root)
    {
        return Err(state_backup_path_invalid(
            "The Use data and state roots must be distinct, non-overlapping directories.",
        ));
    }
    Ok(())
}

async fn read_authority(paths: &ExtensionPaths) -> UseResult<StateBackupAuthority> {
    let registry = ExtensionRegistry::new(paths.clone());
    let snapshot = registry.published_snapshot().await?;
    if !snapshot.pending_cutovers.is_empty() {
        return Err(state_backup_nonterminal(
            "The Registry contains an unacknowledged capability cutover.",
        ));
    }
    for binding in &snapshot.routes {
        if registry.get_snapshot_binding(binding).await?.is_none() {
            return Err(state_backup_invalid(
                "The published Registry projection is missing its exact retained package receipt.",
            ));
        }
    }
    let installed = registry.list().await?;
    let expected_routes = installed
        .iter()
        .map(|extension| a3s_use_extension::ExtensionRouteBinding {
            package_id: extension.receipt.package_id.clone(),
            component_id: extension.receipt.component_id.clone(),
            route: extension.receipt.route.clone(),
            version: extension.receipt.version.clone(),
            package_root: extension.receipt.package_root.clone(),
            manifest_sha256: extension.receipt.manifest_sha256.clone(),
            package_sha256: extension.receipt.package_sha256.clone(),
            lifecycle_generation: extension.receipt.lifecycle_generation,
            enabled: extension.receipt.enabled,
            surfaces: extension
                .surfaces()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        })
        .collect::<Vec<_>>();
    if snapshot.routes != expected_routes {
        return Err(state_backup_nonterminal(
            "The installed receipts and published Registry projection have not converged.",
        ));
    }
    let mut packages = installed
        .into_iter()
        .map(|installed| {
            Ok(StateBackupPackageAuthority {
                package_id: installed.receipt.package_id.clone(),
                receipt_digest: installed.receipt.descriptor_digest()?,
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    Ok(StateBackupAuthority {
        registry_generation: snapshot.generation,
        registry_digest: snapshot.descriptor_digest()?,
        packages,
    })
}

fn resolve_destination(destination: &Path, paths: &ExtensionPaths) -> UseResult<PathBuf> {
    let file_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| state_backup_path_invalid("The backup destination has no file name."))?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent).map_err(|error| {
        state_backup_io(format!(
            "The backup destination directory cannot be resolved: {error}"
        ))
    })?;
    let metadata = std::fs::symlink_metadata(&parent).map_err(|error| {
        state_backup_io(format!(
            "The backup destination directory cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(state_backup_path_invalid(
            "The backup destination parent is not an owned directory.",
        ));
    }
    let resolved = parent.join(file_name);
    for owned_root in [
        paths.use_paths().data_root(),
        paths.use_paths().state_root(),
    ] {
        let owned_root = canonical_or_absolute(owned_root)?;
        if resolved.starts_with(&owned_root) {
            return Err(state_backup_path_invalid(
                "A coordinated backup must be written outside the Use data and state roots.",
            ));
        }
    }
    Ok(resolved)
}

fn canonical_or_absolute(path: &Path) -> UseResult<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|error| {
                        state_backup_io(format!("The current directory is unavailable: {error}"))
                    })?
                    .join(path)
            };
            normalize_lexical(&absolute)
        }
        Err(error) => Err(state_backup_io(format!(
            "A Use-owned root cannot be resolved: {error}"
        ))),
    }
}

fn normalize_lexical(path: &Path) -> UseResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str())
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(state_backup_path_invalid(
                        "A Use-owned root cannot be normalized safely.",
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        state_backup_invalid(format!("Canonical backup JSON encoding failed: {error}"))
    })?;
    Ok(bytes)
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn state_backup_exists() -> UseError {
    UseError::new(
        "use.state_backup_exists",
        "The state backup destination already exists; coordinated backups never overwrite files.",
    )
}

fn state_backup_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.state_backup_invalid", message)
}

fn state_backup_nonterminal(message: impl Into<String>) -> UseError {
    UseError::new("use.state_backup_nonterminal", message).with_suggestion(
        "Finish or recover the exact pending Use operation before creating a backup.",
    )
}

fn state_backup_layout_unsupported(message: impl Into<String>) -> UseError {
    UseError::new("use.state_backup_layout_unsupported", message).with_suggestion(
        "Upgrade A3S Use or remove only state that is independently proven to be unowned.",
    )
}

fn state_backup_path_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.state_backup_path_invalid", message)
}

fn state_backup_limit(message: impl Into<String>) -> UseError {
    UseError::new("use.state_backup_limit", message)
}

fn state_backup_io(message: impl Into<String>) -> UseError {
    UseError::new("use.state_backup_io", message)
}
