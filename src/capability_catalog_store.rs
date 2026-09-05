//! Durable host-owned storage for immutable Agent-facing Gateway catalogs.
//!
//! A catalog is a projection payload, not lifecycle authority. This store
//! therefore addresses the canonical catalog bytes by their own digest and
//! never maintains a mutable "current" pointer. Control/lifecycle code must
//! still bind the returned digest to its committed generation before exposing
//! a session.

use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use a3s_use_core::{
    metadata_is_link_or_reparse_point, CapabilityGatewayCatalog, InstallationId, UseError,
    UseResult,
};
use fs2::FileExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(feature = "extensions")]
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};

mod layout;
#[cfg(feature = "extensions")]
mod restore;
mod retention;
use layout::validate_store_layout;

#[cfg(feature = "extensions")]
pub use restore::{
    CapabilityGatewayCatalogRestoreEntry, CapabilityGatewayCatalogRestorePlan,
    CapabilityGatewayCatalogRestoreResult, CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA,
    CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA,
};
pub use retention::{
    CapabilityGatewayCatalogRetentionEntry, CapabilityGatewayCatalogRetentionPlan,
    CapabilityGatewayCatalogRetentionResult, CAPABILITY_GATEWAY_CATALOG_RETENTION_JOURNAL_SCHEMA,
    CAPABILITY_GATEWAY_CATALOG_RETENTION_PLAN_SCHEMA,
    CAPABILITY_GATEWAY_CATALOG_RETENTION_RESULT_SCHEMA,
};

/// Stable identifier for this payload-owner layout.
///
/// The identifier documents the directory contract. Individual records carry
/// their own canonical Capability Gateway schema and are validated before
/// publication or read; no mutable store-wide marker is trusted as authority.
pub const CAPABILITY_GATEWAY_CATALOG_STORE_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-store.v1";
/// Maximum canonical bytes retained for one Agent-facing catalog.
pub const MAX_CAPABILITY_GATEWAY_CATALOG_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum immutable catalog records retained in one installation store.
pub const MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS: usize = 4_096;

const CATALOG_DIRECTORY: &str = "capability-gateway/catalogs";
const CATALOG_LOCK: &str = ".mutation.lock";
const CATALOG_STAGING: &str = ".staging";
/// Durable intent/progress marker for an interrupted retention operation.
const CATALOG_RETENTION_JOURNAL: &str = ".retention.journal";
const MAX_DIRECTORY_ENTRIES: usize = MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS * 4;
const MAX_STAGING_BYTES: u64 = 64 * 1024 * 1024;
/// Bound for the append-only retention journal, including its reviewed plan.
const MAX_RETENTION_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(25);
const MAX_REVISION_BYTES: usize = 128;
const ERROR_INVALID: &str = "use.plugin.capability_gateway_catalog_store_invalid";
const ERROR_IO: &str = "use.plugin.capability_gateway_catalog_store_io";
const ERROR_CONFLICT: &str = "use.plugin.capability_gateway_catalog_store_conflict";

/// Evidence returned after one catalog's canonical bytes are durably
/// published. The digest is the only address used by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewayCatalogPublication {
    pub digest: String,
    pub installation: InstallationId,
    pub generation: u64,
    pub revision: String,
}

impl CapabilityGatewayCatalogPublication {
    /// Validate the portable identity returned by a successful publication.
    ///
    /// The identity is intentionally not treated as proof of durable bytes on
    /// its own: a composition boundary must still ask the same store for an
    /// exact read before exposing a live session. Keeping this structural
    /// check public lets hosts reject forged or deserialized evidence before
    /// crossing that boundary.
    pub fn validate(&self) -> UseResult<()> {
        self.installation.validate()?;
        validate_digest(&self.digest)?;
        validate_revision(&self.revision)?;
        Ok(())
    }
}

/// Installation-scoped immutable catalog payload owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewayCatalogStore {
    installation: InstallationId,
    state_root: PathBuf,
    root: PathBuf,
}

impl CapabilityGatewayCatalogStore {
    /// Construct a store below an installation-owned state root.
    pub fn new(state_root: impl Into<PathBuf>, installation: InstallationId) -> UseResult<Self> {
        installation.validate()?;
        let state_root = state_root.into();
        if !state_root.is_absolute()
            || state_root
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(store_invalid(
                "The catalog store state root must be an absolute normalized path.",
            ));
        }
        Ok(Self {
            root: state_root.join(CATALOG_DIRECTORY),
            state_root,
            installation,
        })
    }

    /// Construct the store below the exact installation state root selected
    /// by `ExtensionPaths`.
    #[cfg(feature = "extensions")]
    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        let state_root = paths.installation_state_root();
        Self {
            root: state_root.join(CATALOG_DIRECTORY),
            state_root,
            installation: paths.installation().clone(),
        }
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    /// Return the installation-owned logical state root used by this store.
    ///
    /// Composition boundaries use this identity to reject a catalog payload
    /// owner assembled from another installation root before any publication
    /// is attempted.
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Return the implementation path for diagnostics and maintenance. The
    /// path is never serialized into a Gateway contract.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve the configured logical state root to the physical directory
    /// used by no-follow file operations.
    ///
    /// Operating systems are allowed to expose a configured root through an
    /// ancestor alias (macOS commonly exposes temporary directories through
    /// `/var`).  The final state-root component itself must still be a regular
    /// directory; resolving it once lets the catalog store retain that alias
    /// compatibility without asking `O_NOFOLLOW` to traverse it repeatedly.
    async fn physical_paths(&self) -> UseResult<(PathBuf, PathBuf)> {
        let metadata = fs::symlink_metadata(&self.state_root)
            .await
            .map_err(|error| path_error("inspect catalog state root", &self.state_root, error))?;
        if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(path_invalid());
        }
        let physical_state_root = fs::canonicalize(&self.state_root).await.map_err(|error| {
            path_error(
                "resolve physical catalog state root",
                &self.state_root,
                error,
            )
        })?;
        let physical_metadata =
            fs::symlink_metadata(&physical_state_root)
                .await
                .map_err(|error| {
                    path_error(
                        "inspect physical catalog state root",
                        &physical_state_root,
                        error,
                    )
                })?;
        if metadata_is_link_or_reparse_point(&physical_metadata) || !physical_metadata.is_dir() {
            return Err(path_invalid());
        }
        let root = physical_state_root.join(CATALOG_DIRECTORY);
        Ok((physical_state_root, root))
    }

    async fn existing_physical_paths(&self) -> UseResult<Option<(PathBuf, PathBuf)>> {
        match fs::symlink_metadata(&self.state_root).await {
            Ok(_) => self.physical_paths().await.map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(path_error(
                "inspect catalog state root",
                &self.state_root,
                error,
            )),
        }
    }

    /// Publish one immutable catalog. Equal bytes at the addressed digest are
    /// idempotent; a different payload at that address fails closed. No
    /// mutable latest pointer is written.
    pub async fn publish(
        &self,
        catalog: &CapabilityGatewayCatalog,
    ) -> UseResult<CapabilityGatewayCatalogPublication> {
        self.validate_catalog(catalog)?;
        let bytes = canonical_catalog_bytes(catalog)?;
        let digest = catalog.descriptor_digest()?;
        if digest_for_bytes(&bytes)? != digest {
            return Err(catalog_conflict());
        }
        // Establish the configured state root before the optional shared
        // maintenance lock runs. The lock implementation is shared with
        // legacy stores and may create its lock file when the root is absent.
        ensure_directory_exists(&self.state_root).await?;
        #[cfg(feature = "extensions")]
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        let (state_root, root) = self.physical_paths().await?;
        let target = path_for_digest(&root, &digest)?;
        let _mutation = self.acquire_mutation(&state_root, &root).await?;

        ensure_owned_directory_chain(&state_root, &root).await?;
        validate_store_layout(&root).await?;
        retention::ensure_no_pending_journal(&root).await?;
        let parent = target.parent().ok_or_else(path_invalid)?;
        ensure_owned_directory_chain(&state_root, parent).await?;
        let existing = read_catalog_at(&target, &digest).await?;
        if let Some((current, current_bytes)) = existing {
            if current != *catalog || current_bytes != bytes {
                return Err(catalog_conflict());
            }
            // A replayed target must be durable before its recovery link is
            // retired. Otherwise a crash can lose the only stable directory
            // entry for an otherwise valid publication.
            sync_directory(parent).await?;
            retire_staging(&root, &digest).await?;
            return Ok(publication(catalog, digest));
        }

        let records = self.scan_records(&root).await?;
        if records.len() >= MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS {
            return Err(store_invalid(
                "The Capability Gateway catalog store reached its retained-record limit.",
            ));
        }
        write_new_record(&root, &target, &bytes).await?;
        Ok(publication(catalog, digest))
    }

    /// Read one exact content-addressed catalog. `None` means only that the
    /// addressed record has not been published; malformed existing state is an
    /// error.
    pub async fn get(&self, digest: &str) -> UseResult<Option<CapabilityGatewayCatalog>> {
        let digest = validate_digest(digest)?;
        if !validate_existing_path_ancestors(&self.state_root).await? {
            return Ok(None);
        }
        #[cfg(feature = "extensions")]
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        let Some((state_root, root)) = self.existing_physical_paths().await? else {
            return Ok(None);
        };
        if !validate_existing_directory_chain(&state_root, &root).await? {
            return Ok(None);
        }
        validate_store_layout(&root).await?;
        let _mutation = self.acquire_shared_mutation(&state_root, &root).await?;
        retention::ensure_no_pending_journal(&root).await?;
        let target = path_for_digest(&root, &digest)?;
        let Some(parent) = target.parent() else {
            return Err(path_invalid());
        };
        if !validate_existing_directory_chain(&state_root, parent).await? {
            return Ok(None);
        }
        let Some((catalog, _bytes)) = read_catalog_at(&target, &digest).await? else {
            return Ok(None);
        };
        self.validate_catalog(&catalog)?;
        Ok(Some(catalog))
    }

    /// Read and bind one catalog to an exact installation/generation/revision
    /// tuple. This is the intended hand-off for a lifecycle cursor reader.
    pub async fn get_exact(
        &self,
        digest: &str,
        generation: u64,
        revision: &str,
    ) -> UseResult<Option<CapabilityGatewayCatalog>> {
        validate_revision(revision)?;
        let Some(catalog) = self.get(digest).await? else {
            return Ok(None);
        };
        if catalog.generation() != generation || catalog.revision() != revision {
            return Err(catalog_conflict());
        }
        Ok(Some(catalog))
    }

    /// Verify and list the bounded immutable inventory. Operational lock and
    /// staging entries are not returned.
    pub async fn list(&self) -> UseResult<Vec<CapabilityGatewayCatalogPublication>> {
        if !validate_existing_path_ancestors(&self.state_root).await? {
            return Ok(Vec::new());
        }
        #[cfg(feature = "extensions")]
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        let Some((state_root, root)) = self.existing_physical_paths().await? else {
            return Ok(Vec::new());
        };
        if !validate_existing_directory_chain(&state_root, &root).await? {
            return Ok(Vec::new());
        }
        validate_store_layout(&root).await?;
        let _mutation = self.acquire_shared_mutation(&state_root, &root).await?;
        retention::ensure_no_pending_journal(&root).await?;
        self.scan_records(&root)
            .await?
            .into_iter()
            .map(|(digest, catalog)| Ok(publication(&catalog, digest)))
            .collect()
    }

    fn validate_catalog(&self, catalog: &CapabilityGatewayCatalog) -> UseResult<()> {
        catalog.validate()?;
        if catalog.installation() != &self.installation {
            return Err(store_invalid(
                "The catalog belongs to another installation.",
            ));
        }
        validate_revision(catalog.revision())?;
        let bytes = canonical_catalog_bytes(catalog)?;
        if bytes.len() as u64 > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES {
            return Err(store_invalid(
                "The canonical Capability Gateway catalog exceeds its byte bound.",
            ));
        }
        Ok(())
    }

    async fn acquire_mutation(&self, state_root: &Path, root: &Path) -> UseResult<MutationGuard> {
        self.acquire_lock(state_root, root, MutationMode::Exclusive)
            .await
    }

    async fn acquire_shared_mutation(
        &self,
        state_root: &Path,
        root: &Path,
    ) -> UseResult<MutationGuard> {
        self.acquire_lock(state_root, root, MutationMode::Shared)
            .await
    }

    async fn acquire_lock(
        &self,
        state_root: &Path,
        root: &Path,
        mode: MutationMode,
    ) -> UseResult<MutationGuard> {
        ensure_owned_directory_chain(state_root, root).await?;
        let path = root.join(CATALOG_LOCK);
        let error_path = path.clone();
        let file = tokio::task::spawn_blocking(move || acquire_lock_blocking(&path, mode))
            .await
            .map_err(|error| store_io(format!("Catalog mutation lock task failed: {error}")))?
            .map_err(|error| path_error("acquire catalog mutation lock", &error_path, error))?;
        validate_regular_file(&error_path).await?;
        Ok(MutationGuard(file))
    }

    async fn scan_records(
        &self,
        root: &Path,
    ) -> UseResult<Vec<(String, CapabilityGatewayCatalog)>> {
        scan_records(self, root).await
    }
}

fn path_for_digest(root: &Path, digest: &str) -> UseResult<PathBuf> {
    let digest = validate_digest(digest)?;
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    Ok(root
        .join("sha256")
        .join(&hex[..2])
        .join(format!("{hex}.json")))
}

async fn scan_records(
    store: &CapabilityGatewayCatalogStore,
    root: &Path,
) -> UseResult<Vec<(String, CapabilityGatewayCatalog)>> {
    let sha_root = root.join("sha256");
    if !validate_existing_directory(&sha_root).await? {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(&sha_root)
        .await
        .map_err(|error| path_error("read catalog shards", &sha_root, error))?;
    let mut count = 0_usize;
    let mut records = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read catalog shard", &sha_root, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_DIRECTORY_ENTRIES {
            return Err(store_invalid(
                "The catalog store inventory exceeds its bound.",
            ));
        }
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| store_invalid("A catalog shard name is not UTF-8."))?
            .to_owned();
        if name.len() != 2
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(path_invalid());
        }
        let shard = entry.path();
        validate_directory(&shard).await?;
        let mut files = fs::read_dir(&shard)
            .await
            .map_err(|error| path_error("read catalog shard files", &shard, error))?;
        while let Some(file) = files
            .next_entry()
            .await
            .map_err(|error| path_error("read catalog record", &shard, error))?
        {
            count = count.saturating_add(1);
            if count > MAX_DIRECTORY_ENTRIES {
                return Err(store_invalid(
                    "The catalog store inventory exceeds its bound.",
                ));
            }
            let file_name = file
                .file_name()
                .to_str()
                .ok_or_else(|| store_invalid("A catalog filename is not UTF-8."))?
                .to_owned();
            let Some(hex) = file_name.strip_suffix(".json") else {
                return Err(path_invalid());
            };
            if hex.len() != 64
                || !hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                || !hex.starts_with(&name)
            {
                return Err(path_invalid());
            }
            let digest = format!("sha256:{hex}");
            let record_path = file.path();
            let Some((catalog, _bytes)) = read_catalog_at(&record_path, &digest).await? else {
                return Err(catalog_conflict());
            };
            store.validate_catalog(&catalog)?;
            records.push((digest, catalog));
            if records.len() > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS {
                return Err(store_invalid(
                    "The Capability Gateway catalog store exceeded its retained-record limit.",
                ));
            }
        }
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    if records.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(catalog_conflict());
    }
    Ok(records)
}

struct MutationGuard(StdFile);

impl Drop for MutationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn publication(
    catalog: &CapabilityGatewayCatalog,
    digest: String,
) -> CapabilityGatewayCatalogPublication {
    CapabilityGatewayCatalogPublication {
        digest,
        installation: catalog.installation().clone(),
        generation: catalog.generation(),
        revision: catalog.revision().to_owned(),
    }
}

fn canonical_catalog_bytes(catalog: &CapabilityGatewayCatalog) -> UseResult<Vec<u8>> {
    let bytes = catalog.canonical_bytes()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES {
        return Err(store_invalid(
            "The canonical Capability Gateway catalog exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

async fn read_catalog_at(
    path: &Path,
    expected_digest: &str,
) -> UseResult<Option<(CapabilityGatewayCatalog, Vec<u8>)>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect catalog record", path, error)),
    };
    if metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
    {
        return Err(catalog_conflict());
    }
    let before = file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("open catalog record", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| path_error("inspect opened catalog record", path, error))?;
    if metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
        || file_identity(&opened) != before
    {
        return Err(catalog_conflict());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_CAPABILITY_GATEWAY_CATALOG_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| path_error("read catalog record", path, error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("reinspect catalog record", path, error))?;
    if metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || bytes.len() as u64 != opened.len()
    {
        return Err(catalog_conflict());
    }
    let catalog = CapabilityGatewayCatalog::from_json(&bytes).map_err(|_| catalog_conflict())?;
    if catalog.canonical_bytes()? != bytes || catalog.descriptor_digest()? != expected_digest {
        return Err(catalog_conflict());
    }
    Ok(Some((catalog, bytes)))
}

async fn write_new_record(root: &Path, target: &Path, bytes: &[u8]) -> UseResult<()> {
    let parent = target.parent().ok_or_else(path_invalid)?;
    let staging_root = root.join(CATALOG_STAGING);
    ensure_owned_directory_chain(root, parent).await?;
    ensure_owned_directory_chain(root, &staging_root).await?;
    let digest = digest_for_bytes(bytes)?;
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    let temporary = staging_root.join(format!(".{hex}.tmp"));
    prepare_staging_file(&temporary, bytes).await?;
    // Persist the recovery name before linking it into the immutable shard.
    // This makes a crash between file creation and publication replayable.
    sync_directory(&staging_root).await?;
    match fs::hard_link(&temporary, target).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let Some((_catalog, current)) = read_catalog_at(target, &digest).await? else {
                return Err(catalog_conflict());
            };
            sync_directory(parent).await?;
            retire_staging(root, &digest).await?;
            if current != bytes {
                return Err(catalog_conflict());
            }
            return Ok(());
        }
        Err(error) => {
            return Err(path_error("publish catalog record", target, error));
        }
    }
    let Some((_catalog, published)) = read_catalog_at(target, &digest).await? else {
        return Err(catalog_conflict());
    };
    if published != bytes {
        return Err(catalog_conflict());
    }
    sync_directory(parent).await?;
    retire_staging(root, &digest).await
}

async fn prepare_staging_file(path: &Path, bytes: &[u8]) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file()
                || metadata.len() > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
            {
                return Err(catalog_conflict());
            }
            if read_raw_file(path).await? == bytes {
                return Ok(());
            }
            // The deterministic staging name identifies the requested
            // digest. A regular, bounded file with different bytes is an
            // incomplete/tampered replay artifact; remove only that owned
            // artifact while holding the mutation lock, then write the
            // requested canonical bytes again.
            retire_staging_path(path).await?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect catalog staging file", path, error)),
    }
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    configure_no_follow_async(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("create catalog staging file", path, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(path).await;
        return Err(path_error("write catalog staging file", path, error));
    }
    drop(file);
    validate_regular_file(path).await?;
    if read_raw_file(path).await? != bytes {
        return Err(catalog_conflict());
    }
    Ok(())
}

async fn retire_staging_path(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog staging file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(path_invalid());
    }
    fs::remove_file(path)
        .await
        .map_err(|error| path_error("retire catalog staging file", path, error))?;
    sync_directory(path.parent().ok_or_else(path_invalid)?).await
}

async fn retire_staging(root: &Path, digest: &str) -> UseResult<()> {
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    let path = root.join(CATALOG_STAGING).join(format!(".{hex}.tmp"));
    match fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(path_invalid());
            }
            retire_staging_path(&path).await
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error("inspect catalog staging file", &path, error)),
    }
}

async fn read_raw_file(path: &Path) -> UseResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog staging file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
    {
        return Err(catalog_conflict());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("open catalog staging file", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| path_error("inspect opened catalog staging file", path, error))?;
    let before = file_identity(&metadata);
    if metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
        || file_identity(&opened) != before
    {
        return Err(catalog_conflict());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_CAPABILITY_GATEWAY_CATALOG_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| path_error("read catalog staging file", path, error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("reinspect catalog staging file", path, error))?;
    if metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || bytes.len() as u64 != opened.len()
    {
        return Err(catalog_conflict());
    }
    Ok(bytes)
}

fn digest_for_bytes(bytes: &[u8]) -> UseResult<String> {
    use sha2::{Digest, Sha256};
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

async fn ensure_owned_directory_chain(root: &Path, target: &Path) -> UseResult<()> {
    if !target.starts_with(root) {
        return Err(path_invalid());
    }
    ensure_directory_exists(root).await?;
    validate_directory(root).await?;
    let relative = target.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(path_invalid());
        };
        let parent = current.clone();
        current.push(segment);
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {}
            Ok(_) => return Err(path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(path_error("create catalog directory", &current, error))
                    }
                }
                validate_directory(&current).await?;
                sync_directory(&parent).await?;
            }
            Err(error) => return Err(path_error("inspect catalog directory", &current, error)),
        }
    }
    Ok(())
}

/// Create a missing absolute directory path without traversing a symlinked
/// ancestor. `create_dir_all` is deliberately avoided because it follows an
/// intermediate link before this store can inspect it.
async fn ensure_directory_exists(path: &Path) -> UseResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut missing = Vec::new();
    let mut existing = false;
    // Inspect every ancestor, not only the final path component. Otherwise an
    // intermediate symlink could redirect a seemingly missing state root
    // before this store has a chance to reject it.
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link_or_reparse_point(&metadata) {
                    // A configured state root may be reached through an
                    // operating-system alias (for example macOS `/var`).
                    // The final state-root component is still required to be
                    // link-free; aliases outside that boundary are resolved
                    // later by `physical_paths` before no-follow I/O.
                    if ancestor == path {
                        return Err(path_invalid());
                    }
                    let followed = fs::metadata(ancestor).await.map_err(|error| {
                        path_error("resolve catalog state-root alias", ancestor, error)
                    })?;
                    if !followed.is_dir() {
                        return Err(path_invalid());
                    }
                } else if !metadata.is_dir() {
                    return Err(path_invalid());
                }
                existing = true;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.to_path_buf());
            }
            Err(error) => return Err(path_error("inspect catalog state root", ancestor, error)),
        }
    }
    if !existing {
        return Err(path_invalid());
    }
    while let Some(directory) = missing.pop() {
        let parent = directory.parent().ok_or_else(path_invalid)?;
        match fs::create_dir(&directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(path_error("create catalog state root", &directory, error)),
        }
        validate_directory(&directory).await?;
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn validate_existing_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {
            Ok(true)
        }
        Ok(_) => Err(path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error("inspect catalog directory", path, error)),
    }
}

async fn validate_existing_directory_chain(root: &Path, target: &Path) -> UseResult<bool> {
    if !target.starts_with(root) {
        return Err(path_invalid());
    }
    if !validate_existing_path_ancestors(root).await? {
        return Ok(false);
    }
    if !validate_existing_directory(root).await? {
        return Ok(false);
    }
    let relative = target.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(path_invalid());
        };
        current.push(segment);
        if !validate_existing_directory(&current).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn validate_existing_path_ancestors(path: &Path) -> UseResult<bool> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut complete = true;
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link_or_reparse_point(&metadata) {
                    if ancestor == path {
                        return Err(path_invalid());
                    }
                    let followed = fs::metadata(ancestor).await.map_err(|error| {
                        path_error("resolve catalog state-root alias", ancestor, error)
                    })?;
                    if !followed.is_dir() {
                        return Err(path_invalid());
                    }
                } else if !metadata.is_dir() {
                    return Err(path_invalid());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => complete = false,
            Err(error) => return Err(path_error("inspect catalog directory", ancestor, error)),
        }
    }
    Ok(complete)
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    if !validate_existing_directory(path).await? {
        return Err(path_invalid());
    }
    Ok(())
}

async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect catalog file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(path_invalid());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }
}

fn configure_no_follow_async(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

#[derive(Debug, Clone, Copy)]
enum MutationMode {
    Shared,
    Exclusive,
}

fn acquire_lock_blocking(path: &Path, mode: MutationMode) -> io::Result<StdFile> {
    let started = std::time::Instant::now();
    let file = loop {
        let mut options = StdOpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        configure_no_follow_blocking(&mut options);
        let file = options.open(path)?;
        let result = match mode {
            MutationMode::Shared => FileExt::try_lock_shared(&file),
            MutationMode::Exclusive => FileExt::try_lock_exclusive(&file),
        };
        match result {
            Ok(()) => break file,
            Err(error) if lock_is_contended(&error) && started.elapsed() < LOCK_WAIT => {
                drop(file);
                std::thread::sleep(LOCK_RETRY);
            }
            Err(error) => return Err(error),
        }
    };
    Ok(file)
}

fn configure_no_follow_blocking(options: &mut StdOpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| path_error("open catalog directory for sync", path, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync catalog directory", path, error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn validate_digest(value: &str) -> UseResult<String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(path_invalid());
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(path_invalid());
    }
    Ok(value.to_owned())
}

fn validate_revision(value: &str) -> UseResult<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(store_invalid("The catalog revision is invalid."));
    };
    if value.len() > MAX_REVISION_BYTES
        || hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(store_invalid("The catalog revision is invalid."));
    }
    Ok(())
}

fn store_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

fn path_invalid() -> UseError {
    UseError::new(
        ERROR_INVALID,
        "The Capability Gateway catalog store path is outside its owned layout.",
    )
}

fn catalog_conflict() -> UseError {
    UseError::new(
        ERROR_CONFLICT,
        "The immutable Capability Gateway catalog record differs from its addressed identity.",
    )
}

fn store_io(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_IO, message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_io(format!("Failed to {action} '{}': {error}", path.display()))
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityGatewayCatalogStore>();
};
