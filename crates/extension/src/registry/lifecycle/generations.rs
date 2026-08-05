use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use tokio::fs;

use super::{
    exact_receipt, lifecycle_identity_error, lifecycle_root, lifecycle_state_error,
    ExtensionLifecycleIdentity, ExtensionLifecycleResult, ExtensionLifecycleRollbackResult,
};
use crate::package::{io_error, sync_parent_directory, write_receipt};
use crate::registry::{
    verify_package_integrity, ExtensionReceipt, ExtensionRegistry, ExtensionRouteBinding,
    InstalledExtension,
};
use crate::ExtensionPaths;

const MAX_RETAINED_LIFECYCLE_GENERATIONS: usize = 32;
const MAX_RETAINED_RECEIPT_ENTRIES: usize = 64;
const MAX_RETAINED_RECEIPT_BYTES: u64 = 1024 * 1024;

impl ExtensionRegistry {
    /// Restore all exact prior receipts and discard every unpublished graph
    /// candidate through one Registry snapshot update. Replacements list a
    /// prior identity; additions intentionally do not.
    pub async fn rollback_lifecycle_package_graph(
        &self,
        candidates: &[ExtensionLifecycleIdentity],
        priors: &[ExtensionLifecycleIdentity],
    ) -> UseResult<Vec<ExtensionLifecycleRollbackResult>> {
        if candidates.is_empty() || candidates.len() > a3s_use_core::MAX_PLUGIN_PLAN_ITEMS {
            return Err(lifecycle_identity_error(
                "A lifecycle graph rollback requires a bounded non-empty candidate set.",
            ));
        }
        let candidate_ids = candidates
            .iter()
            .map(ExtensionLifecycleIdentity::package_id)
            .collect::<BTreeSet<_>>();
        let prior_by_package = priors
            .iter()
            .map(|prior| (prior.package_id(), prior))
            .collect::<BTreeMap<_, _>>();
        if candidate_ids.len() != candidates.len()
            || prior_by_package.len() != priors.len()
            || prior_by_package.iter().any(|(package_id, prior)| {
                !candidate_ids.contains(package_id)
                    || candidates.iter().any(|candidate| {
                        candidate.package_id() == *package_id
                            && candidate.generation() <= prior.generation()
                    })
            })
        {
            return Err(lifecycle_identity_error(
                "A lifecycle graph rollback contains duplicate or inconsistent generation identities.",
            ));
        }

        let _lock = crate::package::RegistryLock::acquire(&self.paths().registry_lock_path())?;
        let snapshot_before =
            crate::registry_io::read_registry_snapshot(&self.paths().registry_snapshot_path())
                .await?;
        let mut changed = candidates
            .iter()
            .map(|candidate| (candidate.package_id(), false))
            .collect::<BTreeMap<_, _>>();
        let mut mutations = Vec::new();

        for candidate in candidates {
            if snapshot_before.routes.iter().any(|binding| {
                binding.enabled && binding_matches_identity(self.paths(), binding, candidate)
            }) {
                return Err(lifecycle_state_error(
                    "A published lifecycle candidate cannot use pre-cutover graph rollback.",
                ));
            }
            let selected = self.get(candidate.package_id()).await?;
            match prior_by_package.get(candidate.package_id()).copied() {
                Some(prior) => {
                    if !snapshot_before.routes.iter().any(|binding| {
                        binding.enabled && binding_matches_identity(self.paths(), binding, prior)
                    }) {
                        return Err(lifecycle_state_error(
                            "The exact prior graph is no longer the Registry snapshot commit point.",
                        ));
                    }
                    match selected {
                        Some(extension) if exact_receipt(candidate, &extension.receipt).is_ok() => {
                            let prior_extension = self
                                .get_lifecycle_generation(prior)
                                .await?
                                .ok_or_else(|| {
                                    lifecycle_state_error(
                                        "A prior lifecycle receipt is missing during graph rollback.",
                                    )
                                })?;
                            mutations.push((
                                candidate,
                                extension.receipt,
                                Some(prior_extension.receipt),
                            ));
                        }
                        Some(extension) if exact_receipt(prior, &extension.receipt).is_ok() => {}
                        _ => {
                            return Err(lifecycle_state_error(
                                "A replacement receipt is neither its candidate nor exact prior generation.",
                            ))
                        }
                    }
                }
                None => {
                    match selected {
                        Some(extension) if exact_receipt(candidate, &extension.receipt).is_ok() => {
                            if extension.receipt.enabled {
                                return Err(lifecycle_state_error(
                                    "An added candidate became enabled before graph rollback.",
                                ));
                            }
                            mutations.push((candidate, extension.receipt, None));
                        }
                        None => {}
                        _ => return Err(lifecycle_state_error(
                            "An added graph candidate conflicts with another selected generation.",
                        )),
                    }
                }
            }
        }

        let mutation_result: UseResult<()> = async {
            for (candidate, _, replacement) in &mutations {
                let receipt_path = self.paths().receipt_path(candidate.package_id());
                if let Some(replacement) = replacement {
                    write_receipt(&receipt_path, replacement).await?;
                } else {
                    fs::remove_file(&receipt_path).await.map_err(|error| {
                        io_error("remove graph candidate receipt", &receipt_path, error)
                    })?;
                    sync_parent_directory(
                        receipt_path.parent().ok_or_else(path_identity_error)?,
                        "graph candidate receipt",
                    )
                    .await?;
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = mutation_result {
            for (_, original, _) in mutations.iter().rev() {
                write_receipt(&self.paths().receipt_path(&original.package_id), original).await?;
            }
            return Err(error);
        }
        for (candidate, _, _) in &mutations {
            changed.insert(candidate.package_id(), true);
        }

        let installed = match self.list().await {
            Ok(installed) => installed,
            Err(error) => {
                for (_, original, _) in mutations.iter().rev() {
                    write_receipt(&self.paths().receipt_path(&original.package_id), original)
                        .await?;
                }
                return Err(error);
            }
        };
        let snapshot = match self.publish_snapshot_locked(&installed).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                for (_, original, _) in mutations.iter().rev() {
                    write_receipt(&self.paths().receipt_path(&original.package_id), original)
                        .await?;
                }
                return Err(error);
            }
        };
        for candidate in candidates {
            if snapshot
                .routes
                .iter()
                .any(|binding| binding_matches_identity(self.paths(), binding, candidate))
            {
                return Err(lifecycle_state_error(
                    "A discarded graph candidate remains in the Registry snapshot.",
                ));
            }
            if let Some(prior) = prior_by_package.get(candidate.package_id()).copied() {
                if !snapshot.routes.iter().any(|binding| {
                    binding.enabled && binding_matches_identity(self.paths(), binding, prior)
                }) {
                    return Err(lifecycle_state_error(
                        "Graph rollback did not restore an exact prior snapshot route.",
                    ));
                }
                if self.remove_retained_receipt(prior).await? {
                    changed.insert(candidate.package_id(), true);
                }
            }
            if super::remove_exact_root(&self.lifecycle_package_root(candidate)).await? {
                changed.insert(candidate.package_id(), true);
            }
        }

        Ok(candidates
            .iter()
            .map(|candidate| ExtensionLifecycleRollbackResult {
                package_id: candidate.package_id().to_string(),
                changed: changed
                    .get(candidate.package_id())
                    .copied()
                    .unwrap_or(false),
                registry_generation: snapshot.generation,
            })
            .collect())
    }

    /// Load one exact active or retained package generation.
    ///
    /// The primary receipt is the current candidate/selected generation. A
    /// superseded generation is addressed only through its content-bound
    /// retained receipt, never by guessing from package directories.
    pub async fn get_lifecycle_generation(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<Option<InstalledExtension>> {
        if let Some(current) = self.get(identity.package_id()).await? {
            if exact_receipt(identity, &current.receipt).is_ok() {
                verify_package_integrity(&current).await?;
                return Ok(Some(current));
            }
        }
        Ok(self
            .retained_lifecycle_extensions(identity.package_id())
            .await?
            .into_iter()
            .find(|extension| exact_receipt(identity, &extension.receipt).is_ok()))
    }

    /// Restore the exact prior receipt when candidate preparation fails before
    /// capability cutover. Surface owners must remove candidate resources
    /// first; this checkpoint restores package selection and then removes only
    /// the unselected candidate root. Replays are idempotent.
    pub async fn rollback_lifecycle_package(
        &self,
        candidate: &ExtensionLifecycleIdentity,
        prior: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleResult> {
        if candidate.package_id() != prior.package_id()
            || candidate.generation() <= prior.generation()
        {
            return Err(lifecycle_identity_error(
                "A lifecycle rollback must bind newer candidate and exact prior generations of one package.",
            ));
        }
        let _lock = crate::package::RegistryLock::acquire(&self.paths().registry_lock_path())?;
        let published =
            crate::registry_io::read_registry_snapshot(&self.paths().registry_snapshot_path())
                .await?;
        if published
            .routes
            .iter()
            .any(|binding| binding_matches_identity(self.paths(), binding, candidate))
            || !published
                .routes
                .iter()
                .any(|binding| binding_matches_identity(self.paths(), binding, prior))
        {
            return Err(lifecycle_state_error(
                "A lifecycle candidate can roll back only while the exact prior generation remains the snapshot commit point.",
            ));
        }

        let selected = self.get(candidate.package_id()).await?;
        let mut changed = false;
        let prior_extension = match selected {
            Some(extension) if exact_receipt(candidate, &extension.receipt).is_ok() => {
                let prior_extension = self
                    .get_lifecycle_generation(prior)
                    .await?
                    .ok_or_else(|| {
                        lifecycle_state_error(
                            "The prior lifecycle receipt is missing during candidate rollback.",
                        )
                    })?;
                write_receipt(
                    &self.paths().receipt_path(prior.package_id()),
                    &prior_extension.receipt,
                )
                .await?;
                changed = true;
                prior_extension
            }
            Some(extension) if exact_receipt(prior, &extension.receipt).is_ok() => extension,
            _ => {
                return Err(lifecycle_state_error(
                    "The selected package receipt is neither the candidate nor its exact prior generation.",
                ))
            }
        };

        let installed = self.list().await?;
        let snapshot = self.publish_snapshot_locked(&installed).await?;
        if !snapshot
            .routes
            .iter()
            .any(|binding| binding_matches_identity(self.paths(), binding, prior))
        {
            return Err(lifecycle_state_error(
                "Candidate rollback did not restore the prior snapshot identity.",
            ));
        }
        if self.remove_retained_receipt(prior).await? {
            changed = true;
        }
        if super::remove_exact_root(&self.lifecycle_package_root(candidate)).await? {
            changed = true;
        }
        Ok(ExtensionLifecycleResult {
            changed,
            extension: prior_extension,
            registry_generation: snapshot.generation,
        })
    }

    pub(super) fn retained_receipt_path(&self, identity: &ExtensionLifecycleIdentity) -> PathBuf {
        self.paths().retained_lifecycle_receipt_path(
            identity.package_id(),
            identity.generation(),
            identity.package_sha256(),
        )
    }

    pub(super) async fn retain_lifecycle_receipt(
        &self,
        identity: &ExtensionLifecycleIdentity,
        receipt: &ExtensionReceipt,
    ) -> UseResult<bool> {
        exact_receipt(identity, receipt)?;
        let path = self.retained_receipt_path(identity);
        let directory = path.parent().ok_or_else(path_identity_error)?;
        ensure_owned_directory_chain(self.paths().state_root(), directory).await?;
        match fs::symlink_metadata(&path).await {
            Ok(metadata) => {
                validate_receipt_metadata(&metadata)?;
                let current = self.load_receipt(&path).await?;
                exact_receipt(identity, &current.receipt)?;
                verify_package_integrity(&current).await?;
                if current.receipt != *receipt {
                    return Err(lifecycle_state_error(
                        "A retained lifecycle receipt changed before candidate replay.",
                    ));
                }
                return Ok(false);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect retained lifecycle receipt", &path, error)),
        }
        if self
            .retained_lifecycle_extensions(identity.package_id())
            .await?
            .len()
            >= MAX_RETAINED_LIFECYCLE_GENERATIONS
        {
            return Err(retained_generation_limit_error());
        }
        write_receipt(&path, receipt).await?;
        Ok(true)
    }

    pub(super) async fn update_retained_lifecycle_receipt(
        &self,
        identity: &ExtensionLifecycleIdentity,
        expected: &ExtensionReceipt,
        replacement: &ExtensionReceipt,
    ) -> UseResult<()> {
        exact_receipt(identity, expected)?;
        exact_receipt(identity, replacement)?;
        let mut allowed = expected.clone();
        allowed.enabled = replacement.enabled;
        if allowed != *replacement {
            return Err(lifecycle_identity_error(
                "A retained lifecycle update may change only route visibility.",
            ));
        }
        let path = self.retained_receipt_path(identity);
        let directory = path.parent().ok_or_else(path_identity_error)?;
        let Some(()) =
            validate_existing_directory_chain(self.paths().state_root(), directory).await?
        else {
            return Err(lifecycle_state_error(
                "The retained lifecycle receipt disappeared before its visibility update.",
            ));
        };
        let metadata = fs::symlink_metadata(&path)
            .await
            .map_err(|error| io_error("inspect retained lifecycle receipt", &path, error))?;
        validate_receipt_metadata(&metadata)?;
        let current = self.load_receipt(&path).await?;
        exact_receipt(identity, &current.receipt)?;
        verify_package_integrity(&current).await?;
        if current.receipt != *expected {
            return Err(lifecycle_state_error(
                "The retained lifecycle receipt changed before its visibility update.",
            ));
        }
        write_receipt(&path, replacement).await
    }

    pub(super) async fn remove_retained_receipt(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<bool> {
        let path = self.retained_receipt_path(identity);
        let directory = path.parent().ok_or_else(path_identity_error)?;
        if validate_existing_directory_chain(self.paths().state_root(), directory)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_error("inspect retained lifecycle receipt", &path, error)),
        };
        validate_receipt_metadata(&metadata)?;
        let extension = self.load_receipt(&path).await?;
        exact_receipt(identity, &extension.receipt)?;
        fs::remove_file(&path)
            .await
            .map_err(|error| io_error("remove retained lifecycle receipt", &path, error))?;
        sync_parent_directory(directory, "retained lifecycle receipt").await?;
        Ok(true)
    }

    pub(super) async fn retained_lifecycle_extensions(
        &self,
        package_id: &str,
    ) -> UseResult<Vec<InstalledExtension>> {
        let directory = self
            .paths()
            .retained_lifecycle_receipt_directory(package_id);
        if validate_existing_directory_chain(self.paths().state_root(), &directory)
            .await?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let mut entries = fs::read_dir(&directory)
            .await
            .map_err(|error| io_error("read retained lifecycle receipts", &directory, error))?;
        let mut paths = Vec::new();
        let mut entries_seen = 0_usize;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("read retained lifecycle receipt entry", &directory, error))?
        {
            entries_seen = entries_seen.saturating_add(1);
            if entries_seen > MAX_RETAINED_RECEIPT_ENTRIES {
                return Err(UseError::new(
                    "use.extension.lifecycle_receipt_limit_exceeded",
                    "The retained lifecycle receipt directory exceeds its entry bound.",
                ));
            }
            let name = entry
                .file_name()
                .to_str()
                .ok_or_else(|| {
                    lifecycle_identity_error(
                        "A retained lifecycle receipt has a non-UTF-8 file name.",
                    )
                })?
                .to_string();
            let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
                io_error("inspect retained lifecycle receipt", &entry.path(), error)
            })?;
            if name.starts_with(".receipt-") && name.ends_with(".tmp") {
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.len() > MAX_RETAINED_RECEIPT_BYTES
                {
                    return Err(path_identity_error());
                }
                continue;
            }
            validate_receipt_metadata(&metadata)?;
            if !name.ends_with(".json") {
                return Err(path_identity_error());
            }
            paths.push(entry.path());
        }
        if paths.len() > MAX_RETAINED_LIFECYCLE_GENERATIONS {
            return Err(retained_generation_limit_error());
        }
        paths.sort();
        let mut retained = Vec::with_capacity(paths.len());
        for path in paths {
            let extension = self.load_receipt(&path).await?;
            if extension.receipt.package_id != package_id {
                return Err(lifecycle_identity_error(
                    "A retained lifecycle receipt belongs to another package.",
                ));
            }
            let identity = identity_from_receipt(&extension.receipt)?;
            if path != self.retained_receipt_path(&identity) {
                return Err(lifecycle_identity_error(
                    "A retained lifecycle receipt does not match its content-bound path.",
                ));
            }
            verify_package_integrity(&extension).await?;
            retained.push(extension);
        }
        Ok(retained)
    }
}

pub(super) fn identity_from_receipt(
    receipt: &ExtensionReceipt,
) -> UseResult<ExtensionLifecycleIdentity> {
    let generation = receipt.lifecycle_generation.ok_or_else(|| {
        lifecycle_identity_error("A lifecycle receipt omitted its exact generation.")
    })?;
    let package_sha256 = receipt.package_sha256.as_deref().ok_or_else(|| {
        lifecycle_identity_error("A lifecycle receipt omitted its package digest.")
    })?;
    ExtensionLifecycleIdentity::new(
        &receipt.package_id,
        format!("sha256:{package_sha256}"),
        format!("sha256:{}", receipt.manifest_sha256),
        generation,
    )
}

pub(super) fn binding_matches_identity(
    paths: &ExtensionPaths,
    binding: &ExtensionRouteBinding,
    identity: &ExtensionLifecycleIdentity,
) -> bool {
    binding.package_id == identity.package_id()
        && binding.lifecycle_generation == Some(identity.generation())
        && binding.package_sha256.as_deref() == Some(identity.package_sha256())
        && binding.manifest_sha256 == identity.manifest_sha256()
        && binding.package_root == lifecycle_root(paths, identity)
}

async fn ensure_owned_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(path_identity_error());
    }
    match fs::create_dir(root).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error("create lifecycle state root", root, error)),
    }
    validate_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| path_identity_error())?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(io_error(
                    "create retained lifecycle receipt directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(root: &Path, directory: &Path) -> UseResult<Option<()>> {
    if !directory.starts_with(root) {
        return Err(path_identity_error());
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| path_identity_error())?;
    let mut current = root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => return Err(path_identity_error()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(io_error(
                    "inspect retained lifecycle receipt directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(Some(()))
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect lifecycle receipt directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(path_identity_error());
    }
    Ok(())
}

fn validate_receipt_metadata(metadata: &std::fs::Metadata) -> UseResult<()> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RETAINED_RECEIPT_BYTES
    {
        return Err(path_identity_error());
    }
    Ok(())
}

fn path_identity_error() -> UseError {
    UseError::new(
        "use.extension.lifecycle_receipt_path_invalid",
        "A retained lifecycle receipt path is not an owned directory or bounded regular file.",
    )
}

fn retained_generation_limit_error() -> UseError {
    UseError::new(
        "use.extension.lifecycle_receipt_limit_exceeded",
        format!(
            "A cognitive package may retain at most {MAX_RETAINED_LIFECYCLE_GENERATIONS} lifecycle generations."
        ),
    )
}
