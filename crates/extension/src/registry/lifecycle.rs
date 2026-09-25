use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use a3s_use_core::{PluginPackageLock, PluginSurfaceRef, UseError, UseResult};

mod cutover;
mod generations;
mod model;
mod package;
mod staging;
#[cfg(test)]
mod testing;
mod visibility;

use cutover::{
    publication_from_record, recorded_cutover, registry_cutover_capacity,
    registry_cutover_conflict, ExtensionLifecycleCutoverRequest,
};
use generations::{binding_matches_identity, identity_from_receipt};
use model::{
    exact_receipt, lifecycle_graph_error, lifecycle_identity_error, lifecycle_root,
    lifecycle_state_error, validate_locked_extension, RemovedLifecyclePackage,
};
pub use model::{
    ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
    ExtensionLifecycleResult, ExtensionLifecycleRollbackResult,
};
#[cfg(all(test, windows))]
pub(crate) use package::install_before_candidate_commit_hook;

use super::{
    ensure_no_installed_dependents, published_binding_matches_extension, verify_package_integrity,
    ExtensionReceipt, ExtensionRegistry, InstalledExtension, UninstallResult,
    EXTENSION_RECEIPT_SCHEMA_VERSION, MAX_PENDING_REGISTRY_CUTOVERS,
};
use crate::package::{
    remove_file_with_windows_retry, sync_parent_directory, unix_timestamp, write_receipt,
    RegistryLock,
};
use crate::registry_io::{read_registry_snapshot, write_registry_snapshot};
use crate::{ArtifactReferenceAdmission, ArtifactStore};

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
        let mut selected_surfaces = candidate
            .manifest
            .plugin_surfaces()?
            .into_iter()
            .map(|surface| surface.surface)
            .collect::<Vec<_>>();
        selected_surfaces.sort();
        self.commit_lifecycle_package_selection(identity, candidate, &selected_surfaces)
            .await
    }

    /// Commit one exact immutable generation with the surface selection bound
    /// by its reviewed lifecycle intent.
    pub async fn commit_lifecycle_package_selection(
        &self,
        identity: &ExtensionLifecycleIdentity,
        candidate: &ExtensionLifecyclePackage,
        selected_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<ExtensionLifecycleResult> {
        candidate.validate_identity(identity)?;
        let mut selected_surfaces = selected_surfaces.to_vec();
        selected_surfaces.sort();
        if selected_surfaces.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(lifecycle_state_error(
                "The lifecycle package surface selection contains duplicates.",
            ));
        }
        super::validate_surface_selection(
            &candidate.manifest,
            candidate.verified_catalog.as_ref(),
            &selected_surfaces,
        )?;
        let artifact_store = self.paths.artifact_store();
        let artifact_admission = artifact_store.acquire_reference_admission().await?;
        let _lock = RegistryLock::acquire_for_mutation(&self.paths).await?;
        let mut retained_created = None;
        let mut retained_candidate = None;
        if let Some(current) = self.get(identity.package_id()).await? {
            if current.receipt.schema_version == EXTENSION_RECEIPT_SCHEMA_VERSION {
                if exact_receipt(identity, &current.receipt).is_ok()
                    && candidate.matches_provenance(&current.receipt)
                {
                    if current.receipt.enabled {
                        return Err(lifecycle_state_error(
                            "The exact lifecycle generation is already published while package commit is being replayed.",
                        ));
                    }
                    if current.selected_surfaces()? != selected_surfaces {
                        return Err(lifecycle_state_error(
                            "The replayed lifecycle package selection changed after commit.",
                        ));
                    }
                    verify_package_integrity(&current).await?;
                    let retained = self
                        .retained_lifecycle_extensions(identity.package_id())
                        .await?;
                    let snapshot = read_registry_snapshot(&self.paths).await?;
                    if retained.is_empty() {
                        if snapshot.packages.iter().any(|binding| {
                            binding.enabled
                                && binding_matches_identity(&self.paths, binding, identity)
                        }) {
                            return Err(lifecycle_state_error(
                                "A replayed install candidate is already published.",
                            ));
                        }
                    } else if retained.len() == 1 {
                        let package_bindings = snapshot
                            .packages
                            .iter()
                            .filter(|binding| binding.package_id == identity.package_id())
                            .collect::<Vec<_>>();
                        if package_bindings.len() != 1
                            || !published_binding_matches_extension(
                                package_bindings[0],
                                &retained[0],
                            )
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
                let published = read_registry_snapshot(&self.paths).await?;
                if !published
                    .packages
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
                    "use.extension.lifecycle_receipt_incompatible",
                    "An obsolete pre-release receipt owns this cognitive package ID; remove the state and reinstall the package.",
                )
                .with_suggestion("Remove the pre-release A3S Use state, then install the package again."));
            }
        }

        let target = self.lifecycle_package_root(identity);
        artifact_store
            .admit_prepared_package(&artifact_admission, candidate)
            .await?;
        if let Some((retained_identity, receipt)) = retained_candidate {
            let retained = self
                .retain_lifecycle_receipt(
                    &artifact_store,
                    &artifact_admission,
                    &retained_identity,
                    &receipt,
                )
                .await;
            let created = match retained {
                Ok(retained) => retained,
                Err(error) => return Err(error),
            };
            if created {
                retained_created = Some(retained_identity);
            }
        }
        let receipt = ExtensionReceipt {
            schema_version: EXTENSION_RECEIPT_SCHEMA_VERSION,
            installation: self.installation().clone(),
            package_id: identity.package_id.clone(),
            component_id: format!("use/{}", identity.package_id),
            route_alias: candidate.manifest.route_alias.clone(),
            version: candidate.manifest.version.clone(),
            package_root: target.clone(),
            manifest_sha256: identity.manifest_sha256().to_string(),
            package_sha256: Some(identity.package_sha256().to_string()),
            trust: candidate.trust,
            registry: candidate.registry.clone(),
            verified_catalog: candidate.verified_catalog.clone(),
            planning_bundle: candidate.planning_bundle.clone(),
            selected_surfaces,
            installed_at_unix: unix_timestamp(),
            enabled: false,
            lifecycle_generation: Some(identity.generation),
        };
        let receipt_path = self.paths.receipt_path(identity.package_id());
        if let Err(error) = write_receipt(
            &artifact_store,
            &artifact_admission,
            &receipt_path,
            &receipt,
        )
        .await
        {
            let committed = self
                .get(identity.package_id())
                .await
                .ok()
                .flatten()
                .is_some_and(|extension| extension.receipt == receipt);
            if !committed {
                if let Some(identity) = retained_created {
                    let _ = self.remove_retained_receipt(&identity).await;
                }
                return Err(error);
            }
        }
        drop(artifact_admission);

        // Candidate commit is staging, not publication. Keeping every new
        // generation out of the immutable package snapshot prevents a later
        // graph node from replacing the prior closure before the one atomic
        // dependency-graph cutover.
        let snapshot = read_registry_snapshot(&self.paths).await?;
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

    /// Publish one exact installed generation and return the immutable
    /// Registry snapshot selected by the same atomic cutover.
    pub async fn publish_lifecycle_package_with_evidence(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.set_lifecycle_visibility_with_evidence(identity, true, env!("CARGO_PKG_VERSION"), None)
            .await
    }

    /// Publish a fully prepared dependency closure through one Registry
    /// snapshot cutover. Receipt updates remain invisible to generation admission
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
            None,
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
            None,
        )
        .await
    }

    /// Hide an exact dependency closure through one Registry snapshot. A
    /// replay after later package cleanup returns the same unpublished
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

    /// Mark one prior generation hidden after an atomic graph cutover has
    /// already removed its exact Registry package binding. This operation never
    /// owns a visibility cutover and fails before mutation if the binding is present.
    pub async fn retire_hidden_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleResult> {
        self.retire_lifecycle_visibility(identity, env!("CARGO_PKG_VERSION"))
            .await
    }

    /// Hide one exact installed generation and return the immutable Registry
    /// snapshot selected by the same atomic cutover.
    pub async fn hide_lifecycle_package_with_evidence(
        &self,
        identity: &ExtensionLifecycleIdentity,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.set_lifecycle_visibility_with_evidence(
            identity,
            false,
            env!("CARGO_PKG_VERSION"),
            None,
        )
        .await
    }

    pub async fn drain_lifecycle_package(
        &self,
        identity: &ExtensionLifecycleIdentity,
        timeout: Duration,
    ) -> UseResult<ExtensionLifecycleResult> {
        crate::generation_lease::deadline_after(timeout)?;
        let _lock = RegistryLock::acquire_for_mutation(&self.paths).await?;
        let extension = self.exact_lifecycle_extension(identity).await?;
        if extension.receipt.enabled {
            return Err(lifecycle_state_error(
                "The cognitive package must be hidden before accepted calls can drain.",
            ));
        }
        let published = read_registry_snapshot(&self.paths).await?;
        let snapshot = if published
            .packages
            .iter()
            .any(|binding| binding_matches_identity(&self.paths, binding, identity))
        {
            let installed = self.list().await?;
            self.publish_snapshot_locked(&installed).await?
        } else {
            published
        };
        let _drain = crate::generation_lease::acquire_drain_lock(
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
        crate::generation_lease::deadline_after(timeout)?;
        let _lock = RegistryLock::acquire_for_mutation(&self.paths).await?;
        let selected = self.get(identity.package_id()).await?;
        let selected_is_exact = selected
            .as_ref()
            .is_some_and(|extension| exact_receipt(identity, &extension.receipt).is_ok());
        if !selected_is_exact {
            if selected.is_none() {
                let installed = self.list().await?;
                ensure_no_installed_dependents(&installed, identity.package_id())?;
            }
            let retained = self.get_lifecycle_generation(identity).await?;
            let published = read_registry_snapshot(&self.paths).await?;
            let published_binding = published
                .packages
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
            let _drain = crate::generation_lease::acquire_drain_lock(
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
                    .packages
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
        let _drain = crate::generation_lease::acquire_drain_lock(
            &self
                .paths
                .lifecycle_package_lock_path(identity.package_id(), identity.generation()),
            timeout,
        )
        .await?;
        let receipt_path = self.paths.receipt_path(identity.package_id());
        remove_file_with_windows_retry(receipt_path.clone(), "remove lifecycle package receipt")
            .await?;
        let installed = self.list().await?;
        self.publish_snapshot_locked(&installed).await?;
        Ok(UninstallResult {
            package_id: identity.package_id.clone(),
            changed: true,
        })
    }
}

include!("lifecycle_publish.rs");
