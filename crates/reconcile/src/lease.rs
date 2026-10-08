use std::fs::{self, File};
use std::path::Path;

use fs2::FileExt;

use crate::store::{
    existing_real_directory, lock_contended, open_lock_file, package_dir, publication_from,
    read_snapshot, remove_tree, AcquireLock, Snapshot,
};
use crate::{Publication, PublicationCursor, ReconcileError, ReconcileStore};

/// Resources retained or reclaimed by one package retirement pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RetirementReport {
    pub removed_generations: Vec<u64>,
    pub retained_generations: Vec<u64>,
}

/// A non-clone, cross-process pin of a verified, admitted package generation.
/// Publication replacement and withdrawal cannot delete its managed bytes.
/// This is package retention, not a scope grant or an atomic package-graph lease.
#[derive(Debug)]
#[must_use = "retain the lease until the owning instance and its effects settle"]
pub struct GenerationLease {
    store: ReconcileStore,
    publication: Publication,
    guard: Option<File>,
}

impl GenerationLease {
    pub fn publication(&self) -> &Publication {
        &self.publication
    }

    /// Release retention and report cleanup errors to the owning host.
    pub fn release_sync(mut self) -> Result<RetirementReport, ReconcileError> {
        self.guard.take();
        self.store
            .collect_retired_sync(&self.publication.package_id)
    }

    pub async fn release(self) -> Result<RetirementReport, ReconcileError> {
        tokio::task::spawn_blocking(move || self.release_sync())
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        if self.guard.take().is_none() {
            return;
        }
        // Destructors never wait for the writer. Residual data is recoverable by
        // the explicit retirement API if a writer or filesystem failure wins.
        let _ = self.store.try_collect_retired(&self.publication.package_id);
    }
}

impl ReconcileStore {
    /// Acquire the exact currently admitted publication, checking its content.
    pub fn acquire_sync(
        &self,
        cursor: &PublicationCursor,
    ) -> Result<GenerationLease, ReconcileError> {
        acquire(self, &cursor.package_id, Some(cursor))?.ok_or(ReconcileError::StalePublication)
    }

    pub async fn acquire(
        &self,
        cursor: &PublicationCursor,
    ) -> Result<GenerationLease, ReconcileError> {
        let store = self.clone();
        let cursor = cursor.clone();
        tokio::task::spawn_blocking(move || store.acquire_sync(&cursor))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    pub fn acquire_current_sync(
        &self,
        package_id: &str,
    ) -> Result<Option<GenerationLease>, ReconcileError> {
        acquire(self, package_id, None)
    }

    pub async fn acquire_current(
        &self,
        package_id: &str,
    ) -> Result<Option<GenerationLease>, ReconcileError> {
        let store = self.clone();
        let package_id = package_id.to_owned();
        tokio::task::spawn_blocking(move || store.acquire_current_sync(&package_id))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    pub fn collect_retired_sync(
        &self,
        package_id: &str,
    ) -> Result<RetirementReport, ReconcileError> {
        let _writer = AcquireLock::acquire(&self.root)?;
        collect_package(&self.root, package_id)
    }

    pub async fn collect_retired(
        &self,
        package_id: &str,
    ) -> Result<RetirementReport, ReconcileError> {
        let store = self.clone();
        let package_id = package_id.to_owned();
        tokio::task::spawn_blocking(move || store.collect_retired_sync(&package_id))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    fn try_collect_retired(
        &self,
        package_id: &str,
    ) -> Result<Option<RetirementReport>, ReconcileError> {
        let Some(_writer) = AcquireLock::try_acquire(&self.root)? else {
            return Ok(None);
        };
        collect_package(&self.root, package_id).map(Some)
    }
}

fn acquire(
    store: &ReconcileStore,
    package_id: &str,
    expected: Option<&PublicationCursor>,
) -> Result<Option<GenerationLease>, ReconcileError> {
    let _writer = AcquireLock::acquire(&store.root)?;
    acquire_locked(store, package_id, expected, &_writer)
}

pub(crate) fn acquire_locked(
    store: &ReconcileStore,
    package_id: &str,
    expected: Option<&PublicationCursor>,
    _writer: &AcquireLock,
) -> Result<Option<GenerationLease>, ReconcileError> {
    let directory = package_dir(&store.root, package_id)?;
    let Some(snapshot) = read_snapshot(&directory)? else {
        return Ok(None);
    };
    if snapshot.package_id != package_id {
        return Err(ReconcileError::InvalidPackageId(snapshot.package_id));
    }
    let publication = if snapshot.published {
        publication_from(&directory, &snapshot)?
    } else {
        return Ok(None);
    };
    if expected.is_some_and(|cursor| publication.cursor() != *cursor) {
        return Err(ReconcileError::StalePublication);
    }
    for surface in &publication.surfaces {
        surface.verify_content()?;
    }
    let guards = directory.join("leases");
    fs::create_dir_all(&guards)
        .map_err(|error| ReconcileError::io("create generation leases", &guards, error))?;
    crate::store::ensure_real_directory(&guards)?;
    let path = guards.join(format!("{}.lock", publication.generation));
    let guard = open_lock_file(&path)?;
    FileExt::try_lock_shared(&guard)
        .map_err(|error| ReconcileError::io("retain generation", &path, error))?;
    Ok(Some(GenerationLease {
        store: store.clone(),
        publication,
        guard: Some(guard),
    }))
}

fn collect_package(root: &Path, package_id: &str) -> Result<RetirementReport, ReconcileError> {
    let directory = package_dir(root, package_id)?;
    let Some(snapshot) = read_snapshot(&directory)? else {
        return Ok(RetirementReport::default());
    };
    if snapshot.package_id != package_id {
        return Err(ReconcileError::InvalidPackageId(snapshot.package_id));
    }
    if snapshot.published {
        publication_from(&directory, &snapshot)?;
    }
    collect_locked(&directory, &snapshot)
}

pub(crate) fn collect_locked(
    directory: &Path,
    snapshot: &Snapshot,
) -> Result<RetirementReport, ReconcileError> {
    let generations = generation_directories(directory)?;
    let guards = directory.join("leases");
    if !existing_real_directory(&guards)? {
        fs::create_dir(&guards)
            .map_err(|error| ReconcileError::io("create generation leases", &guards, error))?;
    }
    let mut report = RetirementReport::default();
    for generation in generations {
        if snapshot.published && generation == snapshot.generation {
            continue;
        }
        let path = guards.join(format!("{generation}.lock"));
        let guard = open_lock_file(&path)?;
        match guard.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if lock_contended(&error) => {
                report.retained_generations.push(generation);
                continue;
            }
            Err(error) => return Err(ReconcileError::io("retire generation", &path, error)),
        }
        remove_tree(&directory.join("g").join(generation.to_string()))?;
        drop(guard);
        fs::remove_file(&path)
            .map_err(|error| ReconcileError::io("remove generation lease", &path, error))?;
        report.removed_generations.push(generation);
    }
    // A crash after data removal may leave an unlocked guard file behind.
    for entry in fs::read_dir(&guards)
        .map_err(|error| ReconcileError::io("read generation leases", &guards, error))?
    {
        let entry =
            entry.map_err(|error| ReconcileError::io("read generation lease", &guards, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let raw = name
            .strip_suffix(".lock")
            .ok_or_else(|| ReconcileError::InvalidPath(name.clone()))?;
        let generation = parse_generation(raw)?;
        if directory.join("g").join(generation.to_string()).exists() {
            continue;
        }
        let path = entry.path();
        let guard = open_lock_file(&path)?;
        match guard.try_lock_exclusive() {
            Ok(()) => {
                drop(guard);
                fs::remove_file(&path).map_err(|error| {
                    ReconcileError::io("remove orphan generation lease", &path, error)
                })?;
            }
            Err(error) if lock_contended(&error) => {}
            Err(error) => {
                return Err(ReconcileError::io(
                    "retire orphan generation lease",
                    &path,
                    error,
                ))
            }
        }
    }
    Ok(report)
}

pub(crate) fn generation_directories(directory: &Path) -> Result<Vec<u64>, ReconcileError> {
    let root = directory.join("g");
    if !existing_real_directory(&root)? {
        return Ok(Vec::new());
    }
    let mut generations = Vec::new();
    for entry in
        fs::read_dir(&root).map_err(|error| ReconcileError::io("read generations", &root, error))?
    {
        let entry = entry.map_err(|error| ReconcileError::io("read generation", &root, error))?;
        crate::store::ensure_real_directory(&entry.path())?;
        generations.push(parse_generation(&entry.file_name().to_string_lossy())?);
    }
    generations.sort_unstable();
    Ok(generations)
}

fn parse_generation(raw: &str) -> Result<u64, ReconcileError> {
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value != 0 && value.to_string() == raw)
        .ok_or_else(|| ReconcileError::InvalidPath(raw.to_owned()))
}
