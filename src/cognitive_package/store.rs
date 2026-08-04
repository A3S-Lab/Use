use std::collections::BTreeMap;
use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPackageLock, UseError, UseResult, MAX_PLUGIN_PLAN_ITEMS,
};
use a3s_use_extension::ExtensionManifest;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::package_manager_error;

const INSTALLED_GRAPH_SCHEMA: &str = "a3s.use.installed-package-graph.v1";
const PENDING_GRAPH_SCHEMA: &str = "a3s.use.pending-package-graph-operation.v1";
const MAX_GRAPH_RECORD_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct InstalledPackageGraph {
    pub schema: String,
    pub package_lock_digest: String,
    pub package_lock: PluginPackageLock,
    pub installed_at_ms: u64,
}

impl InstalledPackageGraph {
    fn new(package_lock: PluginPackageLock, installed_at_ms: u64) -> UseResult<Self> {
        let graph = Self {
            schema: INSTALLED_GRAPH_SCHEMA.to_string(),
            package_lock_digest: package_lock.descriptor_digest()?,
            package_lock,
            installed_at_ms,
        };
        graph.validate()?;
        Ok(graph)
    }

    fn validate(&self) -> UseResult<()> {
        self.package_lock.validate()?;
        if self.schema != INSTALLED_GRAPH_SCHEMA
            || self.package_lock_digest != self.package_lock.descriptor_digest()?
            || self.installed_at_ms == 0
        {
            return Err(store_error(
                "An installed cognitive-package graph record is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingPackageGraphOperation {
    pub schema: String,
    pub envelope: PluginOperationPlanEnvelope,
    pub admitted_at_ms: u64,
    pub generations: BTreeMap<String, u64>,
    pub manifests: BTreeMap<String, ExtensionManifest>,
}

impl PendingPackageGraphOperation {
    pub fn new(
        envelope: PluginOperationPlanEnvelope,
        admitted_at_ms: u64,
        generations: BTreeMap<String, u64>,
        manifests: BTreeMap<String, ExtensionManifest>,
    ) -> UseResult<Self> {
        let operation = Self {
            schema: PENDING_GRAPH_SCHEMA.to_string(),
            envelope,
            admitted_at_ms,
            generations,
            manifests,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.envelope.validate()?;
        self.envelope.verify_confirmed_apply(
            &self.envelope.plan.operation_id,
            &self.envelope.plan_digest,
            None,
            self.admitted_at_ms,
        )?;
        let changed = self
            .envelope
            .plan
            .packages
            .iter()
            .filter(|package| package.change != PlanPackageChangeKind::Retain)
            .map(|package| package.package_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let generations = self
            .generations
            .iter()
            .filter_map(|(package_id, generation)| (*generation > 0).then_some(package_id.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let manifests = self
            .manifests
            .iter()
            .filter_map(|(package_id, manifest)| {
                (manifest.schema_version == 3 && manifest.package_id == *package_id)
                    .then_some(package_id.as_str())
            })
            .collect::<std::collections::BTreeSet<_>>();
        if self.schema != PENDING_GRAPH_SCHEMA
            || changed != generations
            || changed != manifests
            || self.generations.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(store_error(
                "A pending cognitive-package graph operation is invalid.",
            ));
        }
        Ok(())
    }

    pub fn action(&self) -> PluginOperationAction {
        self.envelope.plan.action
    }

    pub fn root_package_id(&self) -> &str {
        &self.envelope.plan.package_id
    }
}

#[derive(Debug, Clone)]
pub(super) struct InstalledPackageGraphStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl InstalledPackageGraphStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("package-graphs"),
            state_root,
        }
    }

    pub async fn put(&self, lock: &PluginPackageLock, installed_at_ms: u64) -> UseResult<bool> {
        let record = InstalledPackageGraph::new(lock.clone(), installed_at_ms)?;
        let _guard = acquire_lock(&self.state_root).await?;
        let path = package_record_path(&self.root, &lock.root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        let current = if validate_existing_directory_chain(&self.state_root, parent).await? {
            read_optional::<InstalledPackageGraph>(&path).await?
        } else {
            None
        };
        if let Some(current) = current {
            current.validate()?;
            if current.package_lock == record.package_lock {
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
        write_new(&self.state_root, &path, &record).await?;
        Ok(true)
    }

    pub async fn get(&self, root_package_id: &str) -> UseResult<Option<InstalledPackageGraph>> {
        let path = package_record_path(&self.root, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(None);
        }
        let value: Option<InstalledPackageGraph> = read_optional(&path).await?;
        if let Some(value) = &value {
            value.validate()?;
            if value.package_lock.root_package_id != root_package_id {
                return Err(store_error(
                    "An installed graph record does not match its root package path.",
                ));
            }
        }
        Ok(value)
    }

    pub async fn list(&self) -> UseResult<Vec<InstalledPackageGraph>> {
        let mut records = Vec::new();
        if !validate_existing_directory_chain(&self.state_root, &self.root).await? {
            return Ok(records);
        }
        let mut publishers = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(error) => return Err(path_error("read installed graph store", &self.root, error)),
        };
        while let Some(publisher) = publishers
            .next_entry()
            .await
            .map_err(|error| path_error("read installed graph publisher", &self.root, error))?
        {
            if !publisher
                .file_type()
                .await
                .map_err(|error| path_error("inspect graph publisher", &publisher.path(), error))?
                .is_dir()
            {
                return Err(store_error(
                    "The installed graph store contains an invalid publisher entry.",
                ));
            }
            let mut packages = fs::read_dir(publisher.path()).await.map_err(|error| {
                path_error("read installed graph packages", &publisher.path(), error)
            })?;
            while let Some(package) = packages.next_entry().await.map_err(|error| {
                path_error("read installed graph package", &publisher.path(), error)
            })? {
                if records.len() >= MAX_PLUGIN_PLAN_ITEMS
                    || !package
                        .file_type()
                        .await
                        .map_err(|error| {
                            path_error("inspect installed graph record", &package.path(), error)
                        })?
                        .is_file()
                    || package.path().extension().and_then(|value| value.to_str()) != Some("json")
                {
                    return Err(store_error(
                        "The installed graph store contains an invalid or oversized record set.",
                    ));
                }
                let record = read_required::<InstalledPackageGraph>(&package.path()).await?;
                record.validate()?;
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            left.package_lock
                .root_package_id
                .cmp(&right.package_lock.root_package_id)
        });
        Ok(records)
    }

    pub async fn remove(&self, root_package_id: &str, expected_digest: &str) -> UseResult<bool> {
        let _guard = acquire_lock(&self.state_root).await?;
        let path = package_record_path(&self.root, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(false);
        }
        let Some(current) = read_optional::<InstalledPackageGraph>(&path).await? else {
            return Ok(false);
        };
        current.validate()?;
        if current.package_lock_digest != expected_digest {
            return Err(store_error(
                "The installed package graph changed before removal.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove installed package graph", &path, error))?;
        sync_parent(path.parent().ok_or_else(path_identity_error)?).await?;
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingPackageGraphStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl PendingPackageGraphStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("operations").join("package-graphs"),
            state_root,
        }
    }

    pub async fn get(
        &self,
        action: PluginOperationAction,
        root_package_id: &str,
    ) -> UseResult<Option<PendingPackageGraphOperation>> {
        let path = pending_record_path(&self.root, action, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(None);
        }
        let value: Option<PendingPackageGraphOperation> = read_optional(&path).await?;
        if let Some(value) = &value {
            value.validate()?;
            if value.action() != action || value.root_package_id() != root_package_id {
                return Err(store_error(
                    "A pending graph operation does not match its owned path.",
                ));
            }
        }
        Ok(value)
    }

    pub async fn put(&self, value: &PendingPackageGraphOperation) -> UseResult<bool> {
        value.validate()?;
        let _guard = acquire_lock(&self.state_root).await?;
        let path = pending_record_path(&self.root, value.action(), value.root_package_id())?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        let current = if validate_existing_directory_chain(&self.state_root, parent).await? {
            read_optional::<PendingPackageGraphOperation>(&path).await?
        } else {
            None
        };
        if let Some(current) = current {
            current.validate()?;
            if current == *value {
                return Ok(false);
            }
            return Err(package_manager_error(
                "use.plugin.package_graph_busy",
                format!(
                    "Another '{}' graph operation is pending for cognitive package '{}'.",
                    action_name(value.action()),
                    value.root_package_id()
                ),
            ));
        }
        write_new(&self.state_root, &path, value).await?;
        Ok(true)
    }

    pub async fn remove(&self, expected: &PendingPackageGraphOperation) -> UseResult<bool> {
        expected.validate()?;
        let _guard = acquire_lock(&self.state_root).await?;
        let path = pending_record_path(&self.root, expected.action(), expected.root_package_id())?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(false);
        }
        let Some(current) = read_optional::<PendingPackageGraphOperation>(&path).await? else {
            return Ok(false);
        };
        if current != *expected {
            return Err(store_error(
                "The pending package graph changed before completion.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove pending package graph", &path, error))?;
        sync_parent(path.parent().ok_or_else(path_identity_error)?).await?;
        Ok(true)
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
    }
}

async fn read_optional<T>(path: &Path) -> UseResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && metadata.is_file()
                && metadata.len() <= MAX_GRAPH_RECORD_BYTES => {}
        Ok(_) => return Err(store_error("A package graph record path is invalid.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect package graph record", path, error)),
    }
    read_required(path).await.map(Some)
}

async fn read_required<T>(path: &Path) -> UseResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect package graph record", path, error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_GRAPH_RECORD_BYTES
    {
        return Err(store_error("A package graph record path is invalid."));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read package graph record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_GRAPH_RECORD_BYTES {
        return Err(store_error(
            "A package graph record exceeds its size bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| store_error("A package graph record contains invalid JSON."))
}

async fn write_new<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(path_identity_error());
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| store_error("Failed to encode a package graph record."))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_GRAPH_RECORD_BYTES {
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
        fs::rename(&temporary, path).await
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error("commit package graph record", path, error));
    }
    sync_parent(parent).await
}

async fn acquire_lock(state_root: &Path) -> UseResult<StdFile> {
    ensure_owned_directory(state_root, state_root).await?;
    let path = state_root.join(".package-graph.lock");
    match fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
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
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
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
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[tokio::test]
    async fn installed_graph_reads_reject_a_symlinked_publisher_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("state");
        let external = temp.path().join("external");
        fs::create_dir_all(&external).await.unwrap();
        fs::write(external.join("root.json"), b"{}").await.unwrap();
        let graph_root = state_root.join("package-graphs");
        fs::create_dir_all(&graph_root).await.unwrap();
        symlink(&external, graph_root.join("acme")).unwrap();

        let error = InstalledPackageGraphStore::new(&state_root)
            .get("acme/root")
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
    }

    #[tokio::test]
    async fn pending_graph_reads_reject_a_symlinked_publisher_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("state");
        let external = temp.path().join("external");
        fs::create_dir_all(&external).await.unwrap();
        fs::write(external.join("root.json"), b"{}").await.unwrap();
        let operation_root = state_root
            .join("operations")
            .join("package-graphs")
            .join("uninstall");
        fs::create_dir_all(&operation_root).await.unwrap();
        symlink(&external, operation_root.join("acme")).unwrap();

        let error = PendingPackageGraphStore::new(&state_root)
            .get(PluginOperationAction::Uninstall, "acme/root")
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.plugin.package_graph_store_invalid");
    }
}
