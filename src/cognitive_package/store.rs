use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    InstallationId, InstallationRootSelection, InstallationSnapshot, PluginOperationAction,
    PluginPackageId, PluginPackageLock, UseError, UseResult, MAX_INSTALLATION_SNAPSHOT_BYTES,
};
use a3s_use_extension::ExtensionPaths;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::package_manager_error;

mod inventory;
mod pending;

pub(super) use pending::{
    PackageGraphOperationPhase, PendingPackageGraphOperation, PendingPackageGraphStore,
};

const MAX_GRAPH_RECORD_BYTES: u64 = 2 * 1024 * 1024;
const INSTALLATION_SNAPSHOT_FILE: &str = "installation-snapshot.json";
const LEGACY_INSTALLED_GRAPHS_DIRECTORY: &str = "package-graphs";

/// Durable owner of the single resolved package graph for one installation.
///
/// Root-specific locks are derived views. The only stored authority is one
/// monotonically generated `InstallationSnapshot`; a package ID cannot hold
/// conflicting selections beneath different roots.
#[derive(Debug, Clone)]
pub(super) struct InstallationSnapshotStore {
    installation: InstallationId,
    state_root: PathBuf,
    path: PathBuf,
    legacy_root: PathBuf,
}

impl InstallationSnapshotStore {
    #[cfg(test)]
    fn new(state_root: impl Into<PathBuf>, installation: InstallationId) -> UseResult<Self> {
        installation.validate()?;
        Ok(Self::from_parts(state_root.into(), installation))
    }

    fn from_parts(state_root: PathBuf, installation: InstallationId) -> Self {
        Self {
            installation,
            path: state_root.join(INSTALLATION_SNAPSHOT_FILE),
            legacy_root: state_root.join(LEGACY_INSTALLED_GRAPHS_DIRECTORY),
            state_root,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::from_parts(
            paths.installation_state_root(),
            paths.installation().clone(),
        )
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    async fn snapshot(&self) -> UseResult<Option<InstallationSnapshot>> {
        self.read_snapshot().await
    }

    pub async fn put(&self, lock: &PluginPackageLock, installed_at_ms: u64) -> UseResult<bool> {
        lock.validate()?;
        let selection = InstallationRootSelection::new(&lock.root_package_id, installed_at_ms)?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let current = self.read_snapshot_file().await?;
        let (generation, host, mut roots) = match current {
            Some(snapshot) => {
                if let Some(current_lock) = snapshot.package_lock(&lock.root_package_id)? {
                    if current_lock == *lock {
                        return Ok(false);
                    }
                    return Err(package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Cognitive package '{}' already owns a different installed dependency lock.",
                            lock.root_package_id
                        ),
                    ));
                }
                let host = if snapshot.roots.is_empty() {
                    lock.host.clone()
                } else {
                    snapshot.host.clone()
                };
                (
                    next_snapshot_generation(snapshot.generation)?,
                    host,
                    snapshot_root_locks(&snapshot)?,
                )
            }
            None => (1, lock.host.clone(), Vec::new()),
        };
        if host != lock.host {
            return Err(store_error(
                "An installed package lock targets a different installation host.",
            ));
        }
        roots.push((selection, lock.clone()));
        let replacement = InstallationSnapshot::from_root_locks(
            self.installation.clone(),
            generation,
            host,
            roots,
        )?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok(true)
    }

    pub async fn get(&self, root_package_id: &str) -> UseResult<Option<PluginPackageLock>> {
        validate_root_package_id(root_package_id)?;
        let Some(snapshot) = self.read_snapshot().await? else {
            return Ok(None);
        };
        snapshot.package_lock(root_package_id)
    }

    pub async fn replace(
        &self,
        root_package_id: &str,
        expected_digest: &str,
        replacement: &PluginPackageLock,
        installed_at_ms: u64,
    ) -> UseResult<bool> {
        validate_root_package_id(root_package_id)?;
        replacement.validate()?;
        if replacement.root_package_id != root_package_id {
            return Err(store_error(
                "A replacement graph does not own the requested root package.",
            ));
        }
        let selection = InstallationRootSelection::new(root_package_id, installed_at_ms)?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let snapshot = self.read_snapshot_file().await?.ok_or_else(|| {
            store_error("The installation snapshot disappeared before replacement.")
        })?;
        let current = snapshot.package_lock(root_package_id)?.ok_or_else(|| {
            store_error("The installed package graph disappeared before replacement.")
        })?;
        if current == *replacement {
            return Ok(false);
        }
        if current.descriptor_digest()? != expected_digest {
            return Err(store_error(
                "The installed package graph changed before replacement.",
            ));
        }
        let mut roots = snapshot_root_locks(&snapshot)?;
        roots.retain(|(root, _)| root.package_id != root_package_id);
        roots.push((selection, replacement.clone()));
        let replacement = InstallationSnapshot::from_root_locks(
            self.installation.clone(),
            next_snapshot_generation(snapshot.generation)?,
            snapshot.host.clone(),
            roots,
        )?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok(true)
    }

    pub async fn list(&self) -> UseResult<Vec<PluginPackageLock>> {
        let Some(snapshot) = self.read_snapshot().await? else {
            return Ok(Vec::new());
        };
        snapshot.package_locks()
    }

    pub async fn remove(&self, root_package_id: &str, expected_digest: &str) -> UseResult<bool> {
        validate_root_package_id(root_package_id)?;
        let _guard = acquire_lock(&self.state_root).await?;
        reject_legacy_graph_layout(&self.legacy_root).await?;
        let Some(snapshot) = self.read_snapshot_file().await? else {
            return Ok(false);
        };
        let Some(current) = snapshot.package_lock(root_package_id)? else {
            return Ok(false);
        };
        if current.descriptor_digest()? != expected_digest {
            return Err(store_error(
                "The installed package graph changed before removal.",
            ));
        }
        let mut roots = snapshot_root_locks(&snapshot)?;
        roots.retain(|(root, _)| root.package_id != root_package_id);
        let replacement = InstallationSnapshot::from_root_locks(
            self.installation.clone(),
            next_snapshot_generation(snapshot.generation)?,
            snapshot.host.clone(),
            roots,
        )?;
        write_new_bounded(
            &self.state_root,
            &self.path,
            &replacement,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        Ok(true)
    }

    async fn read_snapshot(&self) -> UseResult<Option<InstallationSnapshot>> {
        if !validate_existing_directory_chain(&self.state_root, &self.state_root).await? {
            return Ok(None);
        }
        reject_legacy_graph_layout(&self.legacy_root).await?;
        self.read_snapshot_file().await
    }

    async fn read_snapshot_file(&self) -> UseResult<Option<InstallationSnapshot>> {
        let snapshot = read_optional_bounded::<InstallationSnapshot>(
            &self.path,
            MAX_INSTALLATION_SNAPSHOT_BYTES as u64,
        )
        .await?;
        if let Some(snapshot) = &snapshot {
            snapshot.validate()?;
            self.installation.ensure_same(&snapshot.installation)?;
        }
        Ok(snapshot)
    }
}

fn snapshot_root_locks(
    snapshot: &InstallationSnapshot,
) -> UseResult<Vec<(InstallationRootSelection, PluginPackageLock)>> {
    let locks = snapshot.package_locks()?;
    if locks.len() != snapshot.roots.len() {
        return Err(store_error(
            "The installation snapshot root and lock counts differ.",
        ));
    }
    Ok(snapshot.roots.iter().cloned().zip(locks).collect())
}

fn next_snapshot_generation(generation: u64) -> UseResult<u64> {
    generation.checked_add(1).ok_or_else(|| {
        package_manager_error(
            "use.installation.snapshot_generation_exhausted",
            "The installation snapshot generation is exhausted.",
        )
    })
}

fn validate_root_package_id(root_package_id: &str) -> UseResult<()> {
    PluginPackageId::parse(root_package_id.to_owned())
        .map(drop)
        .map_err(|_| store_error("An installation root package identity is invalid."))
}

async fn reject_legacy_graph_layout(path: &Path) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Err(package_manager_error(
            "use.installation.snapshot_legacy_state_unsupported",
            "Per-root installed package graph files are unsupported; preserve the old state for review and reinstall into a clean installation root.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error("inspect legacy installed graph state", path, error)),
    }
}

fn package_record_path(root: &Path, package_id: &str) -> UseResult<PathBuf> {
    PluginPackageId::parse(package_id.to_string())
        .map_err(|_| store_error("A package graph path contains an invalid package identity."))?;
    let (publisher, package) = package_id
        .split_once('/')
        .ok_or_else(|| store_error("A package graph path is incomplete."))?;
    Ok(root.join(publisher).join(format!("{package}.json")))
}

fn pending_record_path(
    root: &Path,
    action: PluginOperationAction,
    package_id: &str,
) -> UseResult<PathBuf> {
    Ok(root
        .join(action_name(action))
        .join(package_record_path(Path::new(""), package_id)?))
}

fn action_name(action: PluginOperationAction) -> &'static str {
    match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Uninstall => "uninstall",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Enable => "enable",
        PluginOperationAction::Disable => "disable",
    }
}

async fn read_optional<T>(path: &Path) -> UseResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    read_optional_bounded(path, MAX_GRAPH_RECORD_BYTES).await
}

async fn read_optional_bounded<T>(path: &Path, max_bytes: u64) -> UseResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() <= max_bytes => {}
        Ok(_) => return Err(store_error("A package graph record path is invalid.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect package graph record", path, error)),
    }
    read_required_bounded(path, max_bytes).await.map(Some)
}

async fn read_required_bounded<T>(path: &Path, max_bytes: u64) -> UseResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect package graph record", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(store_error("A package graph record path is invalid."));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read package graph record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        return Err(store_error(
            "A package graph record exceeds its size bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| store_error("A package graph record contains invalid JSON."))
}

async fn write_new<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    write_new_bounded(state_root, path, value, MAX_GRAPH_RECORD_BYTES).await
}

async fn write_new_bounded<T: Serialize>(
    state_root: &Path,
    path: &Path,
    value: &T,
    max_bytes: u64,
) -> UseResult<()> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(path_identity_error());
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| store_error("Failed to encode a package graph record."))?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        return Err(store_error(
            "A package graph record exceeds its size bound.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| store_error("A package graph record has no owned parent."))?;
    ensure_owned_directory(state_root, parent).await?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{suffix}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary package graph record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        Ok::<_, std::io::Error>(())
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error("commit package graph record", path, error));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(parent).await
}

async fn activate_temporary_file(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_replace_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        package_manager_error(
            "use.plugin.package_graph_io",
            format!(
                "Failed to commit package graph record '{}': atomic replacement task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("commit package graph record", &error_target, error))
}

async fn acquire_lock(state_root: &Path) -> UseResult<StdFile> {
    ensure_owned_directory(state_root, state_root).await?;
    let path = state_root.join(".package-graph.lock");
    match fs::symlink_metadata(&path).await {
        Ok(metadata)
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file() =>
        {
            return Err(path_identity_error())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect package graph lock", &path, error)),
    }
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| path_error("open package graph lock", &path, error))?;
        file.lock_exclusive()
            .map_err(|error| path_error("lock package graph store", &path, error))?;
        Ok(file)
    })
    .await
    .map_err(|error| {
        store_error(format!(
            "Failed to join the package graph lock task: {error}"
        ))
    })?
}

async fn ensure_owned_directory(state_root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(state_root) {
        return Err(path_identity_error());
    }
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_error("create package graph state root", state_root, error))?;
    validate_directory(state_root).await?;
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| path_identity_error())?;
    let mut current = state_root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create package graph directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(state_root: &Path, directory: &Path) -> UseResult<bool> {
    if !directory.starts_with(state_root) {
        return Err(path_identity_error());
    }
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| path_identity_error())?;
    let mut current = state_root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(path_identity_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect package graph directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect package graph directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(path_identity_error());
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open package graph directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync package graph directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn path_identity_error() -> UseError {
    store_error("A package graph record escaped or traversed its configured state root.")
}

fn path_error(operation: &str, path: &Path, error: std::io::Error) -> UseError {
    package_manager_error(
        "use.plugin.package_graph_io",
        format!("Failed to {operation} '{}': {error}", path.display()),
    )
}

fn store_error(message: impl Into<String>) -> UseError {
    package_manager_error("use.plugin.package_graph_store_invalid", message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::cognitive_package::{
        grant::PackageGraphAuthorization, InstallDisposition, UninstallDisposition,
        UpgradeDisposition,
    };
    use a3s_use_core::{
        CatalogAvailability, PlanScope, PlanScopeKind, PluginCatalogRecord, PluginPackageLockHost,
        PluginPackageResolver, PluginWorkspaceGrantSnapshot, SurfaceChangeKind,
        VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V3,
        PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
    };
    use a3s_use_extension::ExtensionManifest;

    const CATALOG: &[u8] =
        include_bytes!("../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");
    const MANIFEST: &str =
        include_str!("../../crates/extension/fixtures/manifests/plugin-v3-okf.acl");

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    fn manifest(version: &str) -> ExtensionManifest {
        manifest_for("acme/knowledge", version)
    }

    fn manifest_for(package_id: &str, version: &str) -> ExtensionManifest {
        let mut manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
        let (_, name) = package_id.split_once('/').unwrap();
        manifest.package_id = package_id.to_string();
        manifest.version = version.to_string();
        manifest.route = name.to_string();
        if let Some(repository) = &mut manifest.repository {
            repository.url = format!("https://github.com/acme/{name}");
        }
        manifest
    }

    fn package_lock(version: &str, seed: char) -> PluginPackageLock {
        package_lock_for("acme/knowledge", version, seed)
    }

    fn package_lock_for(package_id: &str, version: &str, seed: char) -> PluginPackageLock {
        let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
        let (publisher, name) = package_id.split_once('/').unwrap();
        record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
        record.package_id = package_id.to_string();
        record.publisher = publisher.to_string();
        record.display_name = format!("{name} fixture");
        record.description = format!("Package graph fixture for {package_id}.");
        record.version = version.to_string();
        record.archive.target_name = format!(
            "extensions/{package_id}/{version}/stable/linux-x86_64/{publisher}-{name}-{version}.tar.gz"
        );
        record.archive.sha256 = digest(seed);
        record.package.sha256 = Some(digest(seed));
        record.package.manifest_sha256 = Some(digest(seed));
        record.repository = format!("https://github.com/{publisher}/{name}");
        record.availability = CatalogAvailability::Available;
        record.validate().unwrap();
        let provenance = VerifiedCatalogProvenance {
            registry_name: "packages".to_string(),
            registry_url: "https://packages.example.test/a3s/".to_string(),
            root_sha256: digest('f'),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest: record.descriptor_digest().unwrap(),
        };
        let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
        PluginPackageResolver::new(
            PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
        )
        .resolve(verified, Vec::new())
        .unwrap()
    }

    fn grant_snapshot(state_revision: u64) -> PluginWorkspaceGrantSnapshot {
        PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
            scope_id: "current".to_string(),
            state_revision,
            grants: Vec::new(),
        }
    }

    fn scope() -> PlanScope {
        PlanScope {
            kind: PlanScopeKind::User,
            id: "current".to_string(),
        }
    }

    fn install_pending(lock: &PluginPackageLock) -> PendingPackageGraphOperation {
        let package_id = lock.root_package_id.clone();
        let manifests = BTreeMap::from([(
            package_id.clone(),
            manifest_for(&package_id, lock.packages[0].version()),
        )]);
        let dispositions = BTreeMap::from([(package_id, InstallDisposition::Add)]);
        let generated = crate::cognitive_package::plan::install_operation(
            lock,
            &dispositions,
            &crate::cognitive_package::plan::all_surface_selections(lock),
            &manifests,
            1,
            &scope(),
            10,
            &grant_snapshot(2),
            &crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        )
        .unwrap();
        PendingPackageGraphOperation::planned(
            generated.envelope,
            10,
            generated.generations,
            manifests,
        )
        .unwrap()
        .admit(10, PackageGraphAuthorization::default())
        .unwrap()
    }

    fn uninstall_pending(lock: &PluginPackageLock) -> PendingPackageGraphOperation {
        let package_id = lock.root_package_id.clone();
        let manifests =
            BTreeMap::from([(package_id.clone(), manifest(lock.packages[0].version()))]);
        let dispositions = BTreeMap::from([(package_id.clone(), UninstallDisposition::Remove)]);
        let generations = BTreeMap::from([(package_id, 7)]);
        let generated = crate::cognitive_package::plan::uninstall_operation(
            lock,
            &dispositions,
            &crate::cognitive_package::plan::all_surface_selections(lock),
            generations,
            digest('9'),
            1,
            &scope(),
            10,
            &grant_snapshot(2),
            &crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        )
        .unwrap();
        PendingPackageGraphOperation::planned(
            generated.envelope,
            10,
            generated.generations,
            manifests,
        )
        .unwrap()
        .admit(10, PackageGraphAuthorization::default())
        .unwrap()
    }

    fn upgrade_pending(
        prior: &PluginPackageLock,
        candidate: &PluginPackageLock,
    ) -> PendingPackageGraphOperation {
        let package_id = candidate.root_package_id.clone();
        let manifests = BTreeMap::from([(
            package_id.clone(),
            manifest(candidate.packages[0].version()),
        )]);
        let prior_manifests =
            BTreeMap::from([(package_id.clone(), manifest(prior.packages[0].version()))]);
        let dispositions = BTreeMap::from([(package_id.clone(), UpgradeDisposition::Replace)]);
        let prior_generations = BTreeMap::from([(package_id, 7)]);
        let generated = crate::cognitive_package::plan::upgrade_operation(
            candidate,
            prior,
            &dispositions,
            &crate::cognitive_package::plan::all_surface_selections(prior),
            &crate::cognitive_package::plan::all_surface_selections(candidate),
            &manifests,
            &prior_generations,
            digest('9'),
            8,
            &scope(),
            10,
            &grant_snapshot(9),
            &crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        )
        .unwrap();
        PendingPackageGraphOperation::planned_upgrade(
            generated.envelope,
            10,
            generated.generations,
            manifests,
            prior.clone(),
            prior_generations,
            prior_manifests,
        )
        .unwrap()
        .admit(10, PackageGraphAuthorization::default())
        .unwrap()
    }

    mod records;
    #[cfg(any(unix, windows))]
    mod symlinks;
}
