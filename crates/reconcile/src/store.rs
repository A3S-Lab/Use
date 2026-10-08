use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use a3s_use_core::metadata_is_link_or_reparse_point;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ReconcileError;
use crate::spec::{
    files_sha256, normalize_sha256, parse_package_id, parse_surface_id, PackageSpec, SurfaceKind,
    SCHEMA,
};
use crate::unpack::{prepare, PreparedSurface};

/// One published surface inside the current generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSurface {
    pub kind: SurfaceKind,
    pub id: String,
    pub sha256: String,
    pub content_sha256: String,
    pub directory: PathBuf,
    pub entry: Option<PathBuf>,
    pub mcp: Option<serde_json::Value>,
}

/// The generation that is current for one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub package_id: String,
    pub generation: u64,
    pub surfaces: Vec<PublishedSurface>,
    revision: String,
}

/// An exact native publication identity. A serialized cursor is not authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationCursor {
    pub package_id: String,
    pub generation: u64,
    pub revision: String,
}

impl Publication {
    pub fn cursor(&self) -> PublicationCursor {
        PublicationCursor {
            package_id: self.package_id.clone(),
            generation: self.generation,
            revision: self.revision.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Snapshot {
    schema: String,
    pub(crate) package_id: String,
    pub(crate) generation: u64,
    pub(crate) published: bool,
    surfaces: Vec<SnapshotSurface>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotSurface {
    kind: String,
    id: String,
    sha256: String,
    content_sha256: String,
    entry: Option<String>,
    mcp: Option<serde_json::Value>,
}

struct Prepared {
    kind: SurfaceKind,
    id: String,
    entry: Option<String>,
    body: PreparedSurface,
}

/// Host-supplied root. The current generation is `snapshot.json`.
#[derive(Debug, Clone)]
pub struct ReconcileStore {
    pub(crate) root: PathBuf,
}

impl ReconcileStore {
    /// Create the root when it is missing. A symlink root is refused.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReconcileError> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|error| ReconcileError::io("create reconcile root", &root, error))?;
        ensure_real_directory(&root)?;
        let root = root
            .canonicalize()
            .map_err(|error| ReconcileError::io("canonicalize reconcile root", &root, error))?;
        Ok(Self { root })
    }

    pub async fn apply(&self, spec: &PackageSpec) -> Result<Publication, ReconcileError> {
        let spec = spec.clone();
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || apply(&root, &spec))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    /// Synchronous apply for a host that is already outside the async runtime.
    pub fn apply_sync(&self, spec: &PackageSpec) -> Result<Publication, ReconcileError> {
        apply(&self.root, spec)
    }

    pub async fn current(&self, package_id: &str) -> Result<Option<Publication>, ReconcileError> {
        let root = self.root.clone();
        let package_id = package_id.to_string();
        tokio::task::spawn_blocking(move || read_current(&root, &package_id))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    pub fn current_sync(&self, package_id: &str) -> Result<Option<Publication>, ReconcileError> {
        read_current(&self.root, package_id)
    }

    pub async fn withdraw(&self, package_id: &str) -> Result<(), ReconcileError> {
        let root = self.root.clone();
        let package_id = package_id.to_string();
        tokio::task::spawn_blocking(move || withdraw(&root, &package_id))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    /// Hide the package, then reclaim generations with no live leases.
    /// The hidden receipt retains its counter so reinstall cannot reuse an identity.
    pub fn withdraw_sync(&self, package_id: &str) -> Result<(), ReconcileError> {
        withdraw(&self.root, package_id)
    }
}

fn apply(root: &Path, spec: &PackageSpec) -> Result<Publication, ReconcileError> {
    let prepared = prepare_package(spec)?;
    let _lock = AcquireLock::acquire(root)?;
    let package_dir = package_dir(root, &spec.package_id)?;
    fs::create_dir_all(&package_dir)
        .map_err(|error| ReconcileError::io("create package directory", &package_dir, error))?;
    ensure_real_directory(&package_dir)?;
    let existing = read_snapshot(&package_dir)?;
    if let Some(snapshot) = existing.as_ref() {
        if snapshot.package_id != spec.package_id {
            return Err(ReconcileError::InvalidPackageId(
                snapshot.package_id.clone(),
            ));
        }
        if snapshot.published && same_generation(snapshot, &prepared) {
            let publication = publication_from(&package_dir, snapshot)?;
            if executable_entries_ready(&publication)? {
                return Ok(publication);
            }
        }
    }
    let generations = crate::lease::generation_directories(&package_dir)?;
    let generation = existing
        .as_ref()
        .map(|snapshot| snapshot.generation)
        .unwrap_or(0)
        .max(generations.last().copied().unwrap_or(0))
        .checked_add(1)
        .ok_or(ReconcileError::GenerationExhausted)?;
    let generation_dir = package_dir.join("g").join(generation.to_string());
    write_generation(&generation_dir, &prepared)?;
    let snapshot = Snapshot {
        schema: SCHEMA.to_string(),
        package_id: spec.package_id.clone(),
        generation,
        published: true,
        surfaces: prepared
            .iter()
            .map(|surface| {
                Ok(SnapshotSurface {
                    kind: surface.kind.as_str().to_string(),
                    id: surface.id.clone(),
                    sha256: surface.body.sha256.clone(),
                    content_sha256: files_sha256(&surface.body.files)?,
                    entry: surface.entry.clone(),
                    mcp: surface.body.mcp.clone(),
                })
            })
            .collect::<Result<Vec<_>, ReconcileError>>()?,
    };
    write_snapshot(&package_dir, &snapshot)?;
    publication_from(&package_dir, &snapshot)
}

fn prepare_package(spec: &PackageSpec) -> Result<Vec<Prepared>, ReconcileError> {
    parse_package_id(&spec.package_id)?;
    if spec.surfaces.is_empty() {
        return Err(ReconcileError::EmptyPackage);
    }
    let mut prepared = Vec::with_capacity(spec.surfaces.len());
    for surface in &spec.surfaces {
        parse_surface_id(&surface.id)?;
        if prepared
            .iter()
            .any(|existing: &Prepared| existing.kind == surface.kind && existing.id == surface.id)
        {
            return Err(ReconcileError::DuplicateSurface {
                kind: surface.kind.as_str().to_string(),
                id: surface.id.clone(),
            });
        }
        let expected = normalize_sha256(&surface.sha256)?;
        if matches!(
            surface.kind,
            SurfaceKind::Ui | SurfaceKind::Tool | SurfaceKind::Flow
        ) && surface.entry.is_none()
        {
            return Err(ReconcileError::EntryRequired {
                kind: surface.kind.as_str().to_string(),
                id: surface.id.clone(),
            });
        }
        let body = prepare(
            surface.kind,
            &surface.id,
            &expected,
            surface.entry.as_deref(),
            &surface.payload,
        )?;
        let entry = if surface.kind == SurfaceKind::Mcp {
            let owned = body
                .mcp
                .as_ref()
                .filter(|server| server["type"] == "stdio")
                .and_then(|server| server["command"].as_str())
                .filter(|command| body.files.iter().any(|file| file.path == *command))
                .map(str::to_owned);
            if surface.entry.is_some() && surface.entry != owned {
                return Err(ReconcileError::McpInvalid);
            }
            owned
        } else {
            surface.entry.clone()
        };
        prepared.push(Prepared {
            kind: surface.kind,
            id: surface.id.clone(),
            entry,
            body,
        });
    }
    prepared
        .sort_by(|left, right| (left.kind, left.id.as_str()).cmp(&(right.kind, right.id.as_str())));
    Ok(prepared)
}

fn executable_entries_ready(publication: &Publication) -> Result<bool, ReconcileError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for surface in &publication.surfaces {
            if !matches!(surface.kind, SurfaceKind::Tool | SurfaceKind::Mcp) {
                continue;
            }
            if let Some(entry) = &surface.entry {
                let metadata = fs::symlink_metadata(entry)
                    .map_err(|error| ReconcileError::io("inspect tool executable", entry, error))?;
                if metadata.permissions().mode() & 0o100 == 0 {
                    return Ok(false);
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = publication;
    Ok(true)
}

fn same_generation(snapshot: &Snapshot, prepared: &[Prepared]) -> bool {
    if snapshot.schema != SCHEMA || snapshot.surfaces.len() != prepared.len() {
        return false;
    }
    snapshot.surfaces.iter().zip(prepared).all(|(saved, next)| {
        saved.kind == next.kind.as_str()
            && saved.id == next.id
            && saved.sha256 == next.body.sha256
            && saved.entry == next.entry
            && saved.mcp == next.body.mcp
    })
}

fn write_generation(generation_dir: &Path, prepared: &[Prepared]) -> Result<(), ReconcileError> {
    for surface in prepared {
        let directory = generation_dir.join(surface.kind.as_str()).join(&surface.id);
        for file in &surface.body.files {
            let path = join_relative(&directory, &file.path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    ReconcileError::io("create surface directory", parent, error)
                })?;
                ensure_real_directory(parent)?;
            }
            fs::write(&path, &file.bytes)
                .map_err(|error| ReconcileError::io("write surface file", &path, error))?;
        }
        #[cfg(unix)]
        if matches!(surface.kind, SurfaceKind::Tool | SurfaceKind::Mcp) {
            use std::os::unix::fs::PermissionsExt;
            if let Some(entry) = surface.entry.as_deref() {
                let path = join_relative(&directory, entry)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                    .map_err(|error| ReconcileError::io("prepare tool executable", &path, error))?;
            }
        }
    }
    Ok(())
}

fn write_snapshot(package_dir: &Path, snapshot: &Snapshot) -> Result<(), ReconcileError> {
    let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| ReconcileError::Io {
        action: "encode snapshot",
        path: package_dir.join("snapshot.json"),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()),
    })?;
    write_receipt(package_dir, &bytes)
}

pub(crate) fn write_receipt(package_dir: &Path, bytes: &[u8]) -> Result<(), ReconcileError> {
    let temporary = package_dir.join("snapshot.json.new");
    match fs::symlink_metadata(&temporary) {
        Ok(_) => {
            ensure_real_file(&temporary)?;
            fs::remove_file(&temporary).map_err(|error| {
                ReconcileError::io("remove pending snapshot", &temporary, error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ReconcileError::io(
                "inspect pending snapshot",
                &temporary,
                error,
            ))
        }
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| ReconcileError::io("create pending snapshot", &temporary, error))?;
    file.write_all(bytes)
        .map_err(|error| ReconcileError::io("write snapshot", &temporary, error))?;
    drop(file);
    let destination = package_dir.join("snapshot.json");
    fs::rename(&temporary, &destination)
        .map_err(|error| ReconcileError::io("publish snapshot", &destination, error))?;
    Ok(())
}

fn withdraw(root: &Path, package_id: &str) -> Result<(), ReconcileError> {
    let _lock = AcquireLock::acquire(root)?;
    let package_dir = package_dir(root, package_id)?;
    let metadata = match fs::symlink_metadata(&package_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(ReconcileError::io("inspect package", &package_dir, error)),
    };
    if metadata_is_link_or_reparse_point(&metadata) {
        return Err(ReconcileError::LinkRefused { path: package_dir });
    }
    if !metadata.is_dir() {
        return Err(ReconcileError::Io {
            action: "inspect package",
            path: package_dir,
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a directory"),
        });
    }
    let Some(mut snapshot) = read_snapshot(&package_dir)? else {
        return Ok(());
    };
    if snapshot.package_id != package_id {
        return Err(ReconcileError::InvalidPackageId(snapshot.package_id));
    }
    if snapshot.published {
        snapshot.published = false;
        write_snapshot(&package_dir, &snapshot)?;
    }
    crate::lease::collect_locked(&package_dir, &snapshot).map(|_| ())
}

fn read_current(root: &Path, package_id: &str) -> Result<Option<Publication>, ReconcileError> {
    let _lock = AcquireLock::acquire(root)?;
    let package_dir = package_dir(root, package_id)?;
    let Some(snapshot) = read_snapshot(&package_dir)? else {
        return Ok(None);
    };
    if snapshot.package_id != package_id {
        return Err(ReconcileError::InvalidPackageId(snapshot.package_id));
    }
    if !snapshot.published {
        return Ok(None);
    }
    publication_from(&package_dir, &snapshot).map(Some)
}

pub(crate) fn read_snapshot(package_dir: &Path) -> Result<Option<Snapshot>, ReconcileError> {
    if !existing_real_directory(package_dir)? {
        return Ok(None);
    }
    let path = package_dir.join("snapshot.json");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata_is_link_or_reparse_point(&metadata) => {
            return Err(ReconcileError::LinkRefused { path });
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(ReconcileError::io(
                "inspect snapshot",
                &path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "not a regular file"),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReconcileError::io("inspect snapshot", &path, error)),
    }
    match fs::read(&path) {
        Ok(bytes) => {
            let snapshot: Snapshot =
                serde_json::from_slice(&bytes).map_err(|_| ReconcileError::InvalidSnapshot)?;
            if snapshot.schema != SCHEMA || snapshot.generation == 0 {
                return Err(ReconcileError::InvalidSnapshot);
            }
            Ok(Some(snapshot))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ReconcileError::io("read snapshot", &path, error)),
    }
}

pub(crate) fn publication_from(
    package_dir: &Path,
    snapshot: &Snapshot,
) -> Result<Publication, ReconcileError> {
    parse_package_id(&snapshot.package_id)?;
    if snapshot.surfaces.is_empty() {
        return Err(ReconcileError::EmptyPackage);
    }
    ensure_real_directory(&package_dir.join("g"))?;
    let generation_dir = package_dir.join("g").join(snapshot.generation.to_string());
    ensure_real_directory(&generation_dir)?;
    let mut surfaces = Vec::with_capacity(snapshot.surfaces.len());
    for surface in &snapshot.surfaces {
        let Some(kind) = SurfaceKind::parse(&surface.kind) else {
            return Err(ReconcileError::InvalidSurfaceKind(surface.kind.clone()));
        };
        parse_surface_id(&surface.id)?;
        let sha256 = normalize_sha256(&surface.sha256)?;
        let content_sha256 = normalize_sha256(&surface.content_sha256)?;
        if surfaces
            .iter()
            .any(|existing: &PublishedSurface| existing.kind == kind && existing.id == surface.id)
        {
            return Err(ReconcileError::DuplicateSurface {
                kind: kind.as_str().to_string(),
                id: surface.id.clone(),
            });
        }
        ensure_real_directory(&generation_dir.join(kind.as_str()))?;
        let directory = generation_dir.join(kind.as_str()).join(&surface.id);
        ensure_real_directory(&directory)?;
        if matches!(
            kind,
            SurfaceKind::Ui | SurfaceKind::Tool | SurfaceKind::Flow
        ) && surface.entry.is_none()
        {
            return Err(ReconcileError::EntryRequired {
                kind: kind.as_str().to_string(),
                id: surface.id.clone(),
            });
        }
        let entry = match &surface.entry {
            Some(relative) => {
                let entry = join_relative(&directory, relative)?;
                let mut parent = directory.clone();
                let mut parts = relative.split('/').peekable();
                while let Some(part) = parts.next() {
                    parent.push(part);
                    if parts.peek().is_some() {
                        ensure_real_directory(&parent)?;
                    }
                }
                ensure_real_file(&entry)?;
                Some(entry)
            }
            None => None,
        };
        surfaces.push(PublishedSurface {
            kind,
            id: surface.id.clone(),
            sha256,
            content_sha256,
            directory,
            entry,
            mcp: surface.mcp.clone(),
        });
    }
    Ok(Publication {
        package_id: snapshot.package_id.clone(),
        generation: snapshot.generation,
        surfaces,
        revision: format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(snapshot).map_err(|error| ReconcileError::io(
                    "encode publication cursor",
                    package_dir,
                    std::io::Error::other(error)
                ))?
            )
        ),
    })
}

pub(crate) fn package_dir(root: &Path, package_id: &str) -> Result<PathBuf, ReconcileError> {
    parse_package_id(package_id)?;
    ensure_real_directory(root)?;
    existing_real_directory(&root.join("packages"))?;
    Ok(root.join("packages").join(encode_id(package_id)))
}

pub(crate) fn existing_real_directory(path: &Path) -> Result<bool, ReconcileError> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_real_directory(path).map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ReconcileError::io("inspect directory", path, error)),
    }
}

pub(crate) fn ensure_real_file(path: &Path) -> Result<(), ReconcileError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ReconcileError::io("inspect file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) {
        return Err(ReconcileError::LinkRefused {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(ReconcileError::io(
            "inspect file",
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, "not a regular file"),
        ));
    }
    Ok(())
}

fn encode_id(package_id: &str) -> String {
    let mut encoded = String::new();
    for byte in package_id.bytes() {
        let character = byte as char;
        if character.is_ascii_alphanumeric()
            || character == '-'
            || character == '_'
            || character == '.'
        {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn join_relative(root: &Path, relative: &str) -> Result<PathBuf, ReconcileError> {
    crate::spec::validate_relative(relative)?;
    let mut path = root.to_path_buf();
    for component in relative.split('/') {
        path.push(component);
    }
    Ok(path)
}

pub(crate) fn ensure_real_directory(path: &Path) -> Result<(), ReconcileError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ReconcileError::io("inspect directory", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) {
        return Err(ReconcileError::LinkRefused {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(ReconcileError::Io {
            action: "inspect directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a directory"),
        })
    }
}

pub(crate) fn remove_tree(path: &Path) -> Result<(), ReconcileError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ReconcileError::io("inspect generation", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) {
        return Err(ReconcileError::LinkRefused {
            path: path.to_path_buf(),
        });
    }
    fs::remove_dir_all(path).map_err(|error| ReconcileError::io("remove generation", path, error))
}

pub(crate) struct AcquireLock {
    file: File,
}

impl AcquireLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, ReconcileError> {
        let path = root.join(".reconcile.lock");
        ensure_real_directory(root)?;
        let file = open_lock_file(&path)?;
        file.lock_exclusive()
            .map_err(|error| ReconcileError::io("lock reconcile", &path, error))?;
        Ok(Self { file })
    }

    pub(crate) fn try_acquire(root: &Path) -> Result<Option<Self>, ReconcileError> {
        ensure_real_directory(root)?;
        let path = root.join(".reconcile.lock");
        let file = open_lock_file(&path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if lock_contended(&error) => Ok(None),
            Err(error) => Err(ReconcileError::io("lock reconcile", &path, error)),
        }
    }
}

pub(crate) fn open_lock_file(path: &Path) -> Result<File, ReconcileError> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_real_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ReconcileError::io("inspect lock", path, error)),
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| ReconcileError::io("open lock", path, error))
}

pub(crate) fn lock_contended(error: &std::io::Error) -> bool {
    error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

impl Drop for AcquireLock {
    fn drop(&mut self) {
        // Call the fs2 trait method. Inherent File::unlock requires Rust 1.89.
        let _ = FileExt::unlock(&self.file);
    }
}
