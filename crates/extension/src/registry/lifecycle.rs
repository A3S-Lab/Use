use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use a3s_use_core::{PluginPackageLock, UseError, UseResult};
use tokio::fs;

mod generations;
mod model;
mod package;

use generations::{binding_matches_identity, identity_from_receipt};
use model::{
    exact_receipt, lifecycle_graph_error, lifecycle_identity_error, lifecycle_root,
    lifecycle_state_error, remove_exact_root, validate_locked_extension, RemovedLifecyclePackage,
};
pub use model::{
    ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
    ExtensionLifecycleResult, ExtensionLifecycleRollbackResult,
};
use package::{commit_candidate_root, validate_candidate_source};

use super::{
    ensure_no_installed_dependents, published_binding_matches_extension, verify_package_integrity,
    ExtensionReceipt, ExtensionRegistry, InstalledExtension, UninstallResult,
    RECEIPT_SCHEMA_VERSION_V3,
};
use crate::package::{
    io_error, sync_parent_directory, unix_timestamp, write_receipt, RegistryLock,
};
use crate::registry_io::{read_registry_snapshot, write_registry_snapshot};

impl ExtensionRegistry {
    pub fn lifecycle_package_root(&self, identity: &ExtensionLifecycleIdentity) -> PathBuf {
        lifecycle_root(&self.paths, identity)
    }

    /// Commit one exact immutable cognitive-package generation as
    /// installed-disabled. This is the only schema-v3 package commit path.
    pub async fn commit_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        candidate: &ExtensionLifecyclePackage,
    ) -> UseResult<ExtensionLifecycleResult> {
        candidate.validate_identity(identity)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let mut retained_created = None;
        let mut retained_candidate = None;
        if let Some(current) = self.get(identity.package_id()).await? {
            if current.receipt.schema_version == RECEIPT_SCHEMA_VERSION_V3 {
                if exact_receipt(identity, &current.receipt).is_ok()
                    && candidate.matches_provenance(&current.receipt)
                {
                    if current.receipt.enabled {
                        return Err(lifecycle_state_error(
                            "The exact lifecycle generation is already published while package commit is being replayed.",
                        ));
                    }
                    verify_package_integrity(&current).await?;
                    let retained = self
                        .retained_lifecycle_extensions(identity.package_id())
                        .await?;
                    let snapshot =
                        read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
                    if retained.is_empty() {
                        if snapshot.routes.iter().any(|binding| {
                            binding.enabled
                                && binding_matches_identity(&self.paths, binding, identity)
                        }) {
                            return Err(lifecycle_state_error(
                                "A replayed install candidate is already published.",
                            ));
                        }
                    } else if retained.len() == 1 {
                        let package_routes = snapshot
                            .routes
                            .iter()
                            .filter(|binding| binding.package_id == identity.package_id())
                            .collect::<Vec<_>>();
                        if package_routes.len() != 1
                            || !published_binding_matches_extension(package_routes[0], &retained[0])
                        {
                            return Err(lifecycle_state_error(
                                "A replayed upgrade candidate must preserve the exact retained generation as the Registry snapshot commit point.",
                            ));
                        }
                    } else {
                        return Err(lifecycle_state_error(
                            "A replayed upgrade candidate has ambiguous retained package generations.",
                        ));
                    }
                    return Ok(ExtensionLifecycleResult {
                        changed: false,
                        extension: current,
                        registry_generation: snapshot.generation,
                    });
                }
                let current_generation = current.receipt.lifecycle_generation.ok_or_else(|| {
                    lifecycle_state_error(
                        "The current cognitive-package receipt omitted its lifecycle generation.",
                    )
                })?;
                if identity.generation() <= current_generation {
                    return Err(UseError::new(
                        "use.extension.lifecycle_generation_stale",
                        "A candidate cognitive-package generation must be newer than the selected generation.",
                    ));
                }
                verify_package_integrity(&current).await?;
                let published =
                    read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
                if !published
                    .routes
                    .iter()
                    .any(|binding| published_binding_matches_extension(binding, &current))
                {
                    return Err(UseError::new(
                        "use.extension.lifecycle_generation_unpublished",
                        "The selected cognitive-package generation must reach its exact snapshot commit before an upgrade candidate is staged.",
                    ));
                }
                let retained = self
                    .retained_lifecycle_extensions(identity.package_id())
                    .await?;
                if retained.iter().any(|generation| generation != &current) {
                    return Err(UseError::new(
                        "use.extension.lifecycle_generation_retirement_required",
                        "A prior cognitive-package generation is still retained and must finish retirement before another candidate is staged.",
                    ));
                }
                let current_identity = identity_from_receipt(&current.receipt)?;
                retained_candidate = Some((current_identity, current.receipt));
            } else {
                return Err(UseError::new(
                    "use.extension.lifecycle_legacy_conflict",
                    "A legacy extension receipt already owns this cognitive package ID.",
                ));
            }
        }

        let installed = self.list().await?;
        if let Some(conflict) = installed.iter().find(|extension| {
            extension.receipt.package_id != identity.package_id
                && extension.receipt.route == candidate.manifest.route
        }) {
            return Err(UseError::new(
                "use.extension.route_conflict",
                format!(
                    "Route '{}' is already owned by extension '{}'.",
                    candidate.manifest.route, conflict.receipt.package_id
                ),
            ));
        }

        validate_candidate_source(candidate).await?;
        let target = self.lifecycle_package_root(identity);
        let target_created = commit_candidate_root(candidate, &target).await?;
        if let Some((retained_identity, receipt)) = retained_candidate {
            let retained = self
                .retain_lifecycle_receipt(&retained_identity, &receipt)
                .await;
            let created = match retained {
                Ok(retained) => retained,
                Err(error) => {
                    if target_created {
                        let _ = remove_exact_root(&target).await;
                    }
                    return Err(error);
                }
            };
            if created {
                retained_created = Some(retained_identity);
            }
        }
        let receipt = ExtensionReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION_V3,
            package_id: identity.package_id.clone(),
            component_id: format!("use/{}", identity.package_id),
            route: candidate.manifest.route.clone(),
            version: candidate.manifest.version.clone(),
            package_root: target.clone(),
            manifest_sha256: identity.manifest_sha256().to_string(),
            package_sha256: Some(identity.package_sha256().to_string()),
            trust: candidate.trust,
            registry: candidate.registry.clone(),
            verified_catalog: candidate.verified_catalog.clone(),
            installed_at_unix: unix_timestamp(),
            enabled: false,
            lifecycle_generation: Some(identity.generation),
        };
        let receipt_path = self.paths.receipt_path(identity.package_id());
        if let Err(error) = write_receipt(&receipt_path, &receipt).await {
            let committed = self
                .get(identity.package_id())
                .await
                .ok()
                .flatten()
                .is_some_and(|extension| extension.receipt == receipt);
            if !committed {
                if target_created {
                    let _ = remove_exact_root(&target).await;
                }
                if let Some(identity) = retained_created {
                    let _ = self.remove_retained_receipt(&identity).await;
                }
                return Err(error);
            }
        }

        // Candidate commit is staging, not publication. Keeping every new
        // generation out of the immutable route snapshot prevents a later
        // graph node from replacing the prior closure before the one atomic
        // dependency-graph cutover.
        let snapshot = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        Ok(ExtensionLifecycleResult {
            changed: true,
            extension: InstalledExtension {
                receipt,
                manifest: candidate.manifest.clone(),
            },
            registry_generation: snapshot.generation,
        })
    }

    pub async fn publish_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, true, env!("CARGO_PKG_VERSION"))
            .await
    }

    /// Publish a fully prepared dependency closure through one Registry
    /// snapshot cutover. Receipt updates remain invisible to route admission
    /// until the complete enabled set is durably projected.
    pub async fn publish_lifecycle_packages(
        &self,
        identities: &[ExtensionLifecycleIdentity],
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_packages_for_host_version(
                identities,
                &[],
                env!("CARGO_PKG_VERSION"),
                None,
            )
            .await?
            .packages)
    }

    /// Publish changed nodes from one reviewed dependency graph while proving
    /// that every omitted lock node is the exact already-published generation
    /// selected as retained by the operation plan.
    pub async fn publish_lifecycle_package_graph(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_package_graph_with_evidence(package_lock, identities)
            .await?
            .packages)
    }

    pub async fn publish_lifecycle_package_graph_with_evidence(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.publish_lifecycle_packages_for_host_version(
            identities,
            &[],
            env!("CARGO_PKG_VERSION"),
            Some(package_lock),
        )
        .await
    }

    /// Publish changed candidate nodes and hide prior-only dependency nodes in
    /// one Registry snapshot. Removed identities must be absent from the
    /// candidate lock and remain bound to their exact reviewed generation.
    pub async fn publish_lifecycle_package_graph_transition(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_package_graph_transition_with_evidence(
                package_lock,
                identities,
                removed,
            )
            .await?
            .packages)
    }

    pub async fn publish_lifecycle_package_graph_transition_with_evidence(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.publish_lifecycle_packages_for_host_version(
            identities,
            removed,
            env!("CARGO_PKG_VERSION"),
            Some(package_lock),
        )
        .await
    }

    /// Hide an exact dependency closure through one Registry snapshot. A
    /// replay after later package cleanup returns the same route-free
    /// snapshot evidence without incrementing the generation.
    pub async fn hide_lifecycle_package_graph_with_evidence(
        &self,
        identities: &[ExtensionLifecycleIdentity],
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.publish_lifecycle_packages_for_host_version(
            &[],
            identities,
            env!("CARGO_PKG_VERSION"),
            None,
        )
        .await
    }

    pub async fn hide_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, false, env!("CARGO_PKG_VERSION"))
            .await
    }

    pub async fn drain_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        timeout: Duration,
    ) -> UseResult<ExtensionLifecycleResult> {
        crate::route_lock::deadline_after(timeout)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let extension = self.exact_lifecycle_extension(identity).await?;
        if extension.receipt.enabled {
            return Err(lifecycle_state_error(
                "The cognitive package must be hidden before accepted calls can drain.",
            ));
        }
        let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        let snapshot = if published
            .routes
            .iter()
            .any(|binding| binding_matches_identity(&self.paths, binding, identity))
        {
            let installed = self.list().await?;
            self.publish_snapshot_locked(&installed).await?
        } else {
            published
        };
        let _drain = crate::route_lock::acquire_drain_lock(
            &self
                .paths
                .lifecycle_package_lock_path(identity.package_id(), identity.generation()),
            timeout,
        )
        .await?;
        Ok(ExtensionLifecycleResult {
            changed: false,
            extension,
            registry_generation: snapshot.generation,
        })
    }

    pub async fn remove_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        timeout: Duration,
    ) -> UseResult<UninstallResult> {
        crate::route_lock::deadline_after(timeout)?;
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let target = self.lifecycle_package_root(identity);
        let selected = self.get(identity.package_id()).await?;
        let selected_is_exact = selected
            .as_ref()
            .is_some_and(|extension| exact_receipt(identity, &extension.receipt).is_ok());
        if !selected_is_exact {
            let retained = self.get_lifecycle_generation(identity).await?;
            let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
            let published_binding = published
                .routes
                .iter()
                .find(|binding| binding_matches_identity(&self.paths, binding, identity));
            if published_binding.is_some_and(|binding| binding.enabled) {
                return Err(lifecycle_state_error(format!(
                    "Published cognitive-package generation '{}#{}' cannot be retired without an exact selected receipt.",
                    identity.package_id(),
                    identity.generation()
                )));
            }
            let repair_missing_selected_snapshot =
                published_binding.is_some() && selected.is_none() && retained.is_none();
            if published_binding.is_some() && !repair_missing_selected_snapshot {
                return Err(lifecycle_state_error(
                    "A retained cognitive-package generation is still present in the Registry snapshot.",
                ));
            }
            if retained
                .as_ref()
                .is_some_and(|extension| extension.receipt.enabled)
            {
                return Err(lifecycle_state_error(
                    "The retained cognitive-package generation must be hidden before removal.",
                ));
            }
            let _drain = crate::route_lock::acquire_drain_lock(
                &self
                    .paths
                    .lifecycle_package_lock_path(identity.package_id(), identity.generation()),
                timeout,
            )
            .await?;
            let mut changed = false;
            if repair_missing_selected_snapshot {
                let installed = self.list().await?;
                let repaired = self.publish_snapshot_locked(&installed).await?;
                if repaired
                    .routes
                    .iter()
                    .any(|binding| binding_matches_identity(&self.paths, binding, identity))
                {
                    return Err(lifecycle_state_error(
                        "The missing lifecycle receipt could not be removed from the Registry snapshot.",
                    ));
                }
                changed = true;
            }
            if retained.is_some() {
                self.remove_retained_receipt(identity).await?;
                changed = true;
            }
            if remove_exact_root(&target).await? {
                changed = true;
            }
            return Ok(UninstallResult {
                package_id: identity.package_id.clone(),
                changed,
            });
        }
        let extension = selected.ok_or_else(|| {
            lifecycle_state_error("The exact selected lifecycle receipt disappeared.")
        })?;
        exact_receipt(identity, &extension.receipt)?;
        verify_package_integrity(&extension).await?;
        let installed = self.list().await?;
        ensure_no_installed_dependents(&installed, identity.package_id())?;
        if !self
            .retained_lifecycle_extensions(identity.package_id())
            .await?
            .is_empty()
        {
            return Err(UseError::new(
                "use.extension.lifecycle_generation_retirement_required",
                "Retained prior generations must finish exact retirement before the selected package is removed.",
            ));
        }
        if extension.receipt.enabled {
            return Err(lifecycle_state_error(
                "The cognitive package must be hidden before its immutable generation is removed.",
            ));
        }
        let _drain = crate::route_lock::acquire_drain_lock(
            &self
                .paths
                .lifecycle_package_lock_path(identity.package_id(), identity.generation()),
            timeout,
        )
        .await?;
        let receipt_path = self.paths.receipt_path(identity.package_id());
        fs::remove_file(&receipt_path)
            .await
            .map_err(|error| io_error("remove lifecycle package receipt", &receipt_path, error))?;
        let installed = self.list().await?;
        self.publish_snapshot_locked(&installed).await?;
        remove_exact_root(&target).await?;
        Ok(UninstallResult {
            package_id: identity.package_id.clone(),
            changed: true,
        })
    }

    async fn set_lifecycle_visibility(
        &self,
        identity: &ExtensionLifecycleIdentity,
        enabled: bool,
        host_version: &str,
    ) -> UseResult<ExtensionLifecycleResult> {
        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let selected = self.get(identity.package_id()).await?;
        let selected_is_exact = selected
            .as_ref()
            .is_some_and(|extension| exact_receipt(identity, &extension.receipt).is_ok());
        let mut extension = if selected_is_exact {
            selected.ok_or_else(|| {
                lifecycle_state_error("The exact selected lifecycle receipt disappeared.")
            })?
        } else {
            self.get_lifecycle_generation(identity)
                .await?
                .ok_or_else(|| {
                    UseError::new(
                        "use.extension.not_installed",
                        format!(
                            "Cognitive package generation '{}#{}' is not installed.",
                            identity.package_id(),
                            identity.generation()
                        ),
                    )
                })?
        };
        if enabled && !extension.supports_use_version(host_version) {
            return Err(UseError::new(
                "use.extension.host_incompatible",
                format!(
                    "Cognitive package '{}' is not compatible with this A3S Use host.",
                    identity.package_id
                ),
            ));
        }
        let published = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        let published_exact = published
            .routes
            .iter()
            .any(|binding| binding_matches_identity(&self.paths, binding, identity));
        if !selected_is_exact && (enabled || published_exact) {
            return Err(lifecycle_state_error(
                "A retained generation can be hidden only after atomic capability cutover selected its replacement.",
            ));
        }
        if selected_is_exact
            && !enabled
            && !published_exact
            && published
                .routes
                .iter()
                .any(|binding| binding.package_id == identity.package_id())
        {
            return Err(lifecycle_state_error(
                "An unpublished upgrade candidate cannot hide or replace the still-published prior generation.",
            ));
        }
        let changed = extension.receipt.enabled != enabled;
        if changed {
            let previous = extension.receipt.clone();
            extension.receipt.enabled = enabled;
            if selected_is_exact {
                write_receipt(
                    &self.paths.receipt_path(identity.package_id()),
                    &extension.receipt,
                )
                .await?;
            } else {
                self.update_retained_lifecycle_receipt(identity, &previous, &extension.receipt)
                    .await?;
            }
        }
        let snapshot = if selected_is_exact && (changed || published_exact) {
            let installed = self.list().await?;
            self.publish_snapshot_locked(&installed).await?
        } else {
            published
        };
        Ok(ExtensionLifecycleResult {
            changed,
            extension,
            registry_generation: snapshot.generation,
        })
    }

    async fn publish_lifecycle_packages_for_host_version(
        &self,
        identities: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
        host_version: &str,
        package_lock: Option<&PluginPackageLock>,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        if identities.is_empty() && removed.is_empty()
            || identities.len().saturating_add(removed.len()) > a3s_use_core::MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(lifecycle_state_error(
                "A package-graph publication must contain a bounded non-empty closure.",
            ));
        }
        let mut package_ids = BTreeSet::new();
        for identity in identities {
            if !package_ids.insert(identity.package_id()) {
                return Err(lifecycle_state_error(
                    "A package-graph publication contains a duplicate package identity.",
                ));
            }
        }
        let mut removed_package_ids = BTreeSet::new();
        for identity in removed {
            if package_ids.contains(identity.package_id())
                || !removed_package_ids.insert(identity.package_id())
            {
                return Err(lifecycle_state_error(
                    "A package-graph transition contains a duplicate or overlapping removed package identity.",
                ));
            }
        }

        let _lock = RegistryLock::acquire(&self.paths.registry_lock_path())?;
        let snapshot_before = read_registry_snapshot(&self.paths.registry_snapshot_path()).await?;
        if let Some(package_lock) = package_lock {
            package_lock.validate()?;
            if package_lock.host.use_version != host_version {
                return Err(lifecycle_graph_error(
                    "The reviewed package lock belongs to a different A3S Use host version.",
                ));
            }
            for identity in identities {
                if package_lock.package(identity.package_id()).is_none() {
                    return Err(lifecycle_graph_error(
                        "A changed lifecycle package is absent from the reviewed package lock.",
                    ));
                }
            }
            for identity in removed {
                if package_lock.package(identity.package_id()).is_some() {
                    return Err(lifecycle_graph_error(
                        "A removed lifecycle package is still present in the candidate package lock.",
                    ));
                }
            }
            for locked in &package_lock.packages {
                if package_ids.contains(locked.package_id()) {
                    continue;
                }
                let retained = self.get(locked.package_id()).await?.ok_or_else(|| {
                    lifecycle_graph_error(
                        "A retained cognitive-package dependency is not installed.",
                    )
                })?;
                validate_locked_extension(locked, &retained, host_version)?;
                if !retained.receipt.enabled
                    || !snapshot_before.routes.iter().any(|binding| {
                        binding.enabled && published_binding_matches_extension(binding, &retained)
                    })
                {
                    return Err(lifecycle_graph_error(
                        "A retained cognitive-package dependency is not in the published capability generation.",
                    ));
                }
            }
        }
        let mut extensions = Vec::with_capacity(identities.len());
        for identity in identities {
            let extension = self.exact_lifecycle_extension(identity).await?;
            if !extension.supports_use_version(host_version) {
                return Err(UseError::new(
                    "use.extension.host_incompatible",
                    format!(
                        "Cognitive package '{}' is not compatible with this A3S Use host.",
                        identity.package_id()
                    ),
                ));
            }
            if let Some(package_lock) = package_lock {
                let locked = package_lock.package(identity.package_id()).ok_or_else(|| {
                    lifecycle_graph_error(
                        "A changed lifecycle package disappeared from its reviewed lock.",
                    )
                })?;
                validate_locked_extension(locked, &extension, host_version)?;
            }
            extensions.push(extension);
        }

        let mut candidate_snapshot_complete = package_lock.is_some();
        if let Some(package_lock) = package_lock {
            for locked in &package_lock.packages {
                let extension = if let Some(index) = identities
                    .iter()
                    .position(|identity| identity.package_id() == locked.package_id())
                {
                    extensions.get(index).cloned()
                } else {
                    self.get(locked.package_id()).await?
                };
                candidate_snapshot_complete &= extension.as_ref().is_some_and(|extension| {
                    extension.receipt.enabled
                        && snapshot_before.routes.iter().any(|binding| {
                            binding.enabled
                                && published_binding_matches_extension(binding, extension)
                        })
                });
            }
        }

        let mut removed_extensions = Vec::with_capacity(removed.len());
        for identity in removed {
            let selected = self.get(identity.package_id()).await?;
            let selected_is_exact = selected
                .as_ref()
                .is_some_and(|extension| exact_receipt(identity, &extension.receipt).is_ok());
            if selected.is_some() && !selected_is_exact {
                return Err(lifecycle_graph_error(
                    "A removed dependency has a different selected lifecycle generation.",
                ));
            }
            let exact = if selected_is_exact {
                selected
            } else {
                self.get_lifecycle_generation(identity).await?
            };
            let package_routes = snapshot_before
                .routes
                .iter()
                .filter(|binding| binding.package_id == identity.package_id())
                .collect::<Vec<_>>();
            match exact {
                Some(extension) => {
                    exact_receipt(identity, &extension.receipt)?;
                    verify_package_integrity(&extension).await?;
                    let exact_published = package_routes.iter().any(|binding| {
                        binding.enabled
                            && binding_matches_identity(self.paths(), binding, identity)
                    });
                    let route_free_uninstall_replay = package_lock.is_none()
                        && !selected_is_exact
                        && package_routes.is_empty();
                    if extension.receipt.enabled {
                        if !exact_published
                            && !candidate_snapshot_complete
                            && !route_free_uninstall_replay
                        {
                            return Err(lifecycle_graph_error(
                                "An enabled removed dependency is absent before candidate graph cutover.",
                            ));
                        }
                    } else if !package_routes.is_empty() {
                        return Err(lifecycle_graph_error(
                            "A hidden removed dependency still has a Registry snapshot route.",
                        ));
                    }
                    removed_extensions.push(RemovedLifecyclePackage {
                        identity: identity.clone(),
                        extension,
                        selected: selected_is_exact,
                    });
                }
                None
                    if package_routes.is_empty()
                        && (candidate_snapshot_complete || package_lock.is_none()) =>
                {
                    // A crash may occur after the exact removal journal deletes
                    // the retained receipt but before the parent graph record
                    // advances.
                }
                _ => {
                    return Err(lifecycle_graph_error(
                        "A removed dependency is neither its exact selected or retained generation nor a completed route-free replay.",
                    ))
                }
            }
        }

        let moved_removed = self
            .retain_removed_lifecycle_packages(&removed_extensions)
            .await?;
        let originals = extensions
            .iter()
            .map(|extension| extension.receipt.clone())
            .collect::<Vec<_>>();
        let changed = extensions
            .iter()
            .map(|extension| !extension.receipt.enabled)
            .collect::<Vec<_>>();
        let mut written_receipts = Vec::new();
        for (extension, original) in extensions.iter_mut().zip(&originals) {
            if extension.receipt.enabled {
                continue;
            }
            extension.receipt.enabled = true;
            if let Err(error) = write_receipt(
                &self.paths.receipt_path(&extension.receipt.package_id),
                &extension.receipt,
            )
            .await
            {
                self.restore_lifecycle_receipts(&written_receipts).await?;
                self.restore_removed_lifecycle_packages(&moved_removed)
                    .await?;
                return Err(error);
            }
            written_receipts.push(original.clone());
        }

        let mut installed = match self.list().await {
            Ok(installed) => installed,
            Err(error) => {
                self.restore_lifecycle_receipts(&written_receipts).await?;
                self.restore_removed_lifecycle_packages(&moved_removed)
                    .await?;
                return Err(error);
            }
        };
        installed.retain(|extension| {
            !removed
                .iter()
                .any(|identity| exact_receipt(identity, &extension.receipt).is_ok())
        });
        let snapshot = match self.publish_snapshot_locked(&installed).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.restore_lifecycle_receipts(&originals).await?;
                self.restore_removed_lifecycle_packages(&moved_removed)
                    .await?;
                write_registry_snapshot(&self.paths.registry_snapshot_path(), &snapshot_before)
                    .await?;
                return Err(error);
            }
        };
        let packages = extensions
            .into_iter()
            .zip(changed)
            .map(|(extension, changed)| ExtensionLifecycleResult {
                changed,
                extension,
                registry_generation: snapshot.generation,
            })
            .collect();
        Ok(ExtensionLifecycleGraphPublication {
            packages,
            registry_generation: snapshot.generation,
            registry_snapshot_digest: snapshot.descriptor_digest()?,
        })
    }

    async fn retain_removed_lifecycle_packages(
        &self,
        removed: &[RemovedLifecyclePackage],
    ) -> UseResult<Vec<RemovedLifecyclePackage>> {
        let mut moved = Vec::new();
        for package in removed.iter().filter(|package| package.selected) {
            let mut hidden = package.extension.receipt.clone();
            hidden.enabled = false;
            if let Err(error) = self
                .retain_lifecycle_receipt(&package.identity, &hidden)
                .await
            {
                self.restore_removed_lifecycle_packages(&moved).await?;
                return Err(error);
            }
            moved.push(package.clone());
            let receipt_path = self.paths.receipt_path(package.identity.package_id());
            if let Err(error) = fs::remove_file(&receipt_path).await {
                self.restore_removed_lifecycle_packages(&moved).await?;
                return Err(io_error(
                    "retain removed lifecycle package receipt",
                    &receipt_path,
                    error,
                ));
            }
            if let Err(error) = sync_parent_directory(
                receipt_path
                    .parent()
                    .ok_or_else(|| lifecycle_state_error("A lifecycle receipt has no parent."))?,
                "removed lifecycle package receipt",
            )
            .await
            {
                self.restore_removed_lifecycle_packages(&moved).await?;
                return Err(error);
            }
        }
        Ok(moved)
    }

    async fn restore_removed_lifecycle_packages(
        &self,
        moved: &[RemovedLifecyclePackage],
    ) -> UseResult<()> {
        for package in moved.iter().rev() {
            write_receipt(
                &self.paths.receipt_path(package.identity.package_id()),
                &package.extension.receipt,
            )
            .await?;
            self.remove_retained_receipt(&package.identity).await?;
        }
        Ok(())
    }

    async fn restore_lifecycle_receipts(&self, receipts: &[ExtensionReceipt]) -> UseResult<()> {
        for receipt in receipts {
            write_receipt(&self.paths.receipt_path(&receipt.package_id), receipt).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn publish_lifecycle_package_for_host_version(
        &self,
        identity: &ExtensionLifecycleIdentity,
        host_version: &str,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.set_lifecycle_visibility(identity, true, host_version)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn publish_lifecycle_packages_for_test_host_version(
        &self,
        identities: &[ExtensionLifecycleIdentity],
        host_version: &str,
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_packages_for_host_version(identities, &[], host_version, None)
            .await?
            .packages)
    }

    #[cfg(test)]
    pub(crate) async fn publish_lifecycle_package_graph_for_test_host_version(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        host_version: &str,
    ) -> UseResult<Vec<ExtensionLifecycleResult>> {
        Ok(self
            .publish_lifecycle_packages_for_host_version(
                identities,
                &[],
                host_version,
                Some(package_lock),
            )
            .await?
            .packages)
    }

    #[cfg(test)]
    pub(crate) async fn acquire_lifecycle_route_for_host_version(
        &self,
        route: &str,
        host_version: &str,
    ) -> UseResult<Option<super::ExtensionRouteLease>> {
        let Some(candidate) = self
            .find_route_for_host_version(route, host_version)
            .await?
        else {
            return Ok(None);
        };
        self.acquire_extension_lease_for_host_version(candidate, Some(route), host_version)
            .await
    }

    async fn exact_lifecycle_extension(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<InstalledExtension> {
        self.get_lifecycle_generation(identity)
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.extension.not_installed",
                    format!(
                        "Cognitive package generation '{}#{}' is not installed.",
                        identity.package_id(),
                        identity.generation()
                    ),
                )
            })
    }
}
