use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::lease::acquire_locked;
use crate::spec::{normalize_sha256, parse_package_id};
use crate::store::{
    ensure_real_directory, ensure_real_file, existing_real_directory, write_receipt, AcquireLock,
};
use crate::{
    GenerationLease, Publication, PublicationCursor, ReconcileError, ReconcileStore,
    RetirementReport,
};

/// Machine-owned selection receipt. This is not a product manifest or a scope grant.
pub const SELECTION_SCHEMA: &str = "a3s.use.selection-snapshot.v1";
const MAX_PACKAGES: usize = 128;
const MAX_SURFACES: usize = 4096;
const MAX_RECEIPT: u64 = 64 * 1024;
const MAX_SELECTIONS: usize = 256;

/// Exact selected package identities, atomically published under the store writer.
/// Selection metadata does not retain package bytes; acquire a lease before use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionSnapshot {
    schema: String,
    selection_id: String,
    generation: u64,
    published: bool,
    packages: Vec<PublicationCursor>,
}

/// A comparison token, not authorization or proof of retained resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionCursor {
    pub selection_id: String,
    pub generation: u64,
    pub revision: String,
}

impl SelectionSnapshot {
    pub fn selection_id(&self) -> &str {
        &self.selection_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_published(&self) -> bool {
        self.published
    }

    pub fn packages(&self) -> &[PublicationCursor] {
        &self.packages
    }

    pub fn cursor(&self) -> SelectionCursor {
        let mut hash = Sha256::new();
        for value in [self.schema.as_str(), self.selection_id.as_str()] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        hash.update(self.generation.to_le_bytes());
        hash.update([u8::from(self.published)]);
        hash.update((self.packages.len() as u64).to_le_bytes());
        for package in &self.packages {
            hash.update((package.package_id.len() as u64).to_le_bytes());
            hash.update(package.package_id.as_bytes());
            hash.update(package.generation.to_le_bytes());
            hash.update(package.revision.as_bytes());
        }
        SelectionCursor {
            selection_id: self.selection_id.clone(),
            generation: self.generation,
            revision: format!("sha256:{:x}", hash.finalize()),
        }
    }
}

/// All selected generations retained by real cross-process package locks.
/// The host still owns scope admission and must settle effects before releasing.
#[derive(Debug)]
#[must_use = "retain the complete selection until its owning effects settle"]
pub struct SelectionLease {
    snapshot: SelectionSnapshot,
    leases: Vec<GenerationLease>,
}

/// One cleanup result. Failure for a package does not skip the other packages.
#[derive(Debug)]
pub struct SelectionRetirement {
    pub package_id: String,
    pub result: Result<RetirementReport, ReconcileError>,
}

impl SelectionLease {
    pub fn snapshot(&self) -> &SelectionSnapshot {
        &self.snapshot
    }

    pub fn publications(&self) -> impl ExactSizeIterator<Item = &Publication> {
        self.leases.iter().map(GenerationLease::publication)
    }

    pub fn release_sync(self) -> Vec<SelectionRetirement> {
        self.leases
            .into_iter()
            .rev()
            .map(|lease| SelectionRetirement {
                package_id: lease.publication().package_id.clone(),
                result: lease.release_sync(),
            })
            .collect()
    }

    pub async fn release(self) -> Result<Vec<SelectionRetirement>, ReconcileError> {
        tokio::task::spawn_blocking(move || self.release_sync())
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))
    }
}

impl ReconcileStore {
    /// Read metadata, including a withdrawn receipt needed for the next CAS publish.
    pub fn selection_sync(&self, id: &str) -> Result<Option<SelectionSnapshot>, ReconcileError> {
        let _writer = AcquireLock::acquire(&self.root)?;
        read_selection(&selection_directory(&self.root, id)?, id)
    }

    pub async fn selection(&self, id: &str) -> Result<Option<SelectionSnapshot>, ReconcileError> {
        let store = self.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || store.selection_sync(&id))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    /// Compare-and-publish the complete set after verifying all exact current packages.
    /// A stale writer or failed package leaves the previous selection unchanged.
    pub fn publish_selection_sync(
        &self,
        id: &str,
        expected: Option<&SelectionCursor>,
        packages: &[PublicationCursor],
    ) -> Result<SelectionSnapshot, ReconcileError> {
        let mut packages = packages.to_vec();
        packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
        validate_packages(&packages)?;
        let writer = AcquireLock::acquire(&self.root)?;
        let directory = selection_directory(&self.root, id)?;
        let current = read_selection(&directory, id)?;
        compare_current(current.as_ref(), expected)?;
        let _leases = retain_packages(self, &packages, &writer)?;
        if let Some(current) = current.as_ref() {
            if current.published && current.packages == packages {
                return Ok(current.clone());
            }
        }
        let snapshot = SelectionSnapshot {
            schema: SELECTION_SCHEMA.into(),
            selection_id: id.into(),
            generation: current
                .as_ref()
                .map_or(0, |snapshot| snapshot.generation)
                .checked_add(1)
                .ok_or(ReconcileError::GenerationExhausted)?,
            published: true,
            packages,
        };
        create_selection_directory(&directory)?;
        write_selection(&directory, &snapshot)?;
        Ok(snapshot)
    }

    pub async fn publish_selection(
        &self,
        id: &str,
        expected: Option<&SelectionCursor>,
        packages: &[PublicationCursor],
    ) -> Result<SelectionSnapshot, ReconcileError> {
        let store = self.clone();
        let id = id.to_owned();
        let expected = expected.cloned();
        let packages = packages.to_vec();
        tokio::task::spawn_blocking(move || {
            store.publish_selection_sync(&id, expected.as_ref(), &packages)
        })
        .await
        .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    /// Acquire all exact current packages under the same fence as the selection check.
    pub fn acquire_selection_sync(
        &self,
        cursor: &SelectionCursor,
    ) -> Result<SelectionLease, ReconcileError> {
        let writer = AcquireLock::acquire(&self.root)?;
        let directory = selection_directory(&self.root, &cursor.selection_id)?;
        let snapshot = read_selection(&directory, &cursor.selection_id)?
            .ok_or(ReconcileError::StaleSelection)?;
        compare_current(Some(&snapshot), Some(cursor))?;
        if !snapshot.published {
            return Err(ReconcileError::StaleSelection);
        }
        let leases = retain_packages(self, &snapshot.packages, &writer)?;
        Ok(SelectionLease { snapshot, leases })
    }

    pub async fn acquire_selection(
        &self,
        cursor: &SelectionCursor,
    ) -> Result<SelectionLease, ReconcileError> {
        let store = self.clone();
        let cursor = cursor.clone();
        tokio::task::spawn_blocking(move || store.acquire_selection_sync(&cursor))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }

    /// Withdraw selection admission without withdrawing packages used by other selections.
    /// Hidden receipts keep their counter, preventing ABA on a later publication.
    pub fn withdraw_selection_sync(
        &self,
        cursor: &SelectionCursor,
    ) -> Result<SelectionSnapshot, ReconcileError> {
        let _writer = AcquireLock::acquire(&self.root)?;
        let directory = selection_directory(&self.root, &cursor.selection_id)?;
        let mut snapshot = read_selection(&directory, &cursor.selection_id)?
            .ok_or(ReconcileError::StaleSelection)?;
        compare_current(Some(&snapshot), Some(cursor))?;
        if snapshot.published {
            snapshot.published = false;
            write_selection(&directory, &snapshot)?;
        }
        Ok(snapshot)
    }

    pub async fn withdraw_selection(
        &self,
        cursor: &SelectionCursor,
    ) -> Result<SelectionSnapshot, ReconcileError> {
        let store = self.clone();
        let cursor = cursor.clone();
        tokio::task::spawn_blocking(move || store.withdraw_selection_sync(&cursor))
            .await
            .map_err(|error| ReconcileError::Task(error.to_string()))?
    }
}

fn compare_current(
    current: Option<&SelectionSnapshot>,
    expected: Option<&SelectionCursor>,
) -> Result<(), ReconcileError> {
    if current.map(SelectionSnapshot::cursor).as_ref() != expected {
        return Err(ReconcileError::StaleSelection);
    }
    Ok(())
}

fn retain_packages(
    store: &ReconcileStore,
    packages: &[PublicationCursor],
    writer: &AcquireLock,
) -> Result<Vec<GenerationLease>, ReconcileError> {
    let mut leases = Vec::with_capacity(packages.len());
    let mut surfaces = 0usize;
    for cursor in packages {
        let lease = acquire_locked(store, &cursor.package_id, Some(cursor), writer)?
            .ok_or(ReconcileError::StalePublication)?;
        surfaces = surfaces
            .checked_add(lease.publication().surfaces.len())
            .filter(|count| *count <= MAX_SURFACES)
            .ok_or(ReconcileError::SelectionTooLarge)?;
        leases.push(lease);
    }
    Ok(leases)
}

fn validate_packages(packages: &[PublicationCursor]) -> Result<(), ReconcileError> {
    if packages.len() > MAX_PACKAGES {
        return Err(ReconcileError::SelectionTooLarge);
    }
    for (index, cursor) in packages.iter().enumerate() {
        parse_package_id(&cursor.package_id)?;
        if cursor.generation == 0 || normalize_sha256(&cursor.revision)? != cursor.revision {
            return Err(ReconcileError::InvalidSelectionSnapshot);
        }
        if index > 0 && packages[index - 1].package_id >= cursor.package_id {
            return Err(ReconcileError::DuplicateSelectionPackage(
                cursor.package_id.clone(),
            ));
        }
    }
    Ok(())
}

fn selection_directory(root: &Path, id: &str) -> Result<PathBuf, ReconcileError> {
    let mut chars = id.chars();
    if id.len() > 128
        || !chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ReconcileError::InvalidSelectionId(id.into()));
    }
    ensure_real_directory(root)?;
    let parent = root.join("selections");
    existing_real_directory(&parent)?;
    let directory = parent.join(id);
    existing_real_directory(&directory)?;
    Ok(directory)
}

fn create_selection_directory(directory: &Path) -> Result<(), ReconcileError> {
    if existing_real_directory(directory)? {
        return Ok(());
    }
    let parent = directory
        .parent()
        .ok_or(ReconcileError::InvalidSelectionSnapshot)?;
    fs::create_dir_all(parent)
        .map_err(|error| ReconcileError::io("create selection root", parent, error))?;
    ensure_real_directory(parent)?;
    let mut count = 0;
    for entry in fs::read_dir(parent)
        .map_err(|error| ReconcileError::io("read selection root", parent, error))?
    {
        let entry =
            entry.map_err(|error| ReconcileError::io("read selection entry", parent, error))?;
        ensure_real_directory(&entry.path())?;
        count += 1;
        if count >= MAX_SELECTIONS {
            return Err(ReconcileError::SelectionTooLarge);
        }
    }
    fs::create_dir(directory)
        .map_err(|error| ReconcileError::io("create selection", directory, error))?;
    ensure_real_directory(directory)
}

fn read_selection(directory: &Path, id: &str) -> Result<Option<SelectionSnapshot>, ReconcileError> {
    if !existing_real_directory(directory)? {
        return Ok(None);
    }
    let path = directory.join("snapshot.json");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReconcileError::io("inspect selection", &path, error)),
        Ok(_) => ensure_real_file(&path)?,
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .map_err(|error| ReconcileError::io("open selection", &path, error))?
        .take(MAX_RECEIPT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ReconcileError::io("read selection", &path, error))?;
    if bytes.len() as u64 > MAX_RECEIPT {
        return Err(ReconcileError::SelectionTooLarge);
    }
    let snapshot: SelectionSnapshot =
        serde_json::from_slice(&bytes).map_err(|_| ReconcileError::InvalidSelectionSnapshot)?;
    if snapshot.schema != SELECTION_SCHEMA
        || snapshot.selection_id != id
        || snapshot.generation == 0
    {
        return Err(ReconcileError::InvalidSelectionSnapshot);
    }
    validate_packages(&snapshot.packages)?;
    Ok(Some(snapshot))
}

fn write_selection(directory: &Path, snapshot: &SelectionSnapshot) -> Result<(), ReconcileError> {
    let bytes =
        serde_json::to_vec(snapshot).map_err(|_| ReconcileError::InvalidSelectionSnapshot)?;
    if bytes.len() as u64 > MAX_RECEIPT {
        return Err(ReconcileError::SelectionTooLarge);
    }
    write_receipt(directory, &bytes)
}
