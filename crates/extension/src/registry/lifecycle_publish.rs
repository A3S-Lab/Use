// Lifecycle publish/hide/retire helpers (included into lifecycle).

impl ExtensionRegistry {
    async fn publish_lifecycle_packages_for_host_version(
        &self,
        identities: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
        host_version: &str,
        package_lock: Option<&PluginPackageLock>,
        cutover_request: Option<&ExtensionLifecycleCutoverRequest>,
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

        let artifact_store = self.paths.artifact_store();
        let artifact_admission = artifact_store.acquire_reference_admission().await?;
        let _lock = RegistryLock::acquire_for_mutation(&self.paths).await?;
        let snapshot_before = read_registry_snapshot(&self.paths).await?;
        let recorded_cutover = cutover_request
            .map(|request| recorded_cutover(&snapshot_before, request))
            .transpose()?
            .flatten();
        if let Some(request) = cutover_request.filter(|_| recorded_cutover.is_none()) {
            request.require_current_generation(snapshot_before.generation)?;
        }
        if cutover_request.is_some()
            && recorded_cutover.is_none()
            && snapshot_before.pending_cutovers.len() >= MAX_PENDING_REGISTRY_CUTOVERS
        {
            return Err(registry_cutover_capacity());
        }
        if recorded_cutover.is_none() && !removed_package_ids.is_empty() {
            let surviving_extensions = self
                .list()
                .await?
                .into_iter()
                .filter(|extension| {
                    !removed_package_ids.contains(extension.receipt.package_id.as_str())
                })
                .collect::<Vec<_>>();
            for identity in removed {
                ensure_no_installed_dependents(&surviving_extensions, identity.package_id())?;
            }
        }
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
                    || !snapshot_before.packages.iter().any(|binding| {
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
                        && snapshot_before.packages.iter().any(|binding| {
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
            let package_bindings = snapshot_before
                .packages
                .iter()
                .filter(|binding| binding.package_id == identity.package_id())
                .collect::<Vec<_>>();
            match exact {
                Some(extension) => {
                    exact_receipt(identity, &extension.receipt)?;
                    verify_package_integrity(&extension).await?;
                    let exact_published = package_bindings.iter().any(|binding| {
                        binding.enabled
                            && binding_matches_identity(self.paths(), binding, identity)
                    });
                    let unpublished_uninstall_replay = package_lock.is_none()
                        && !selected_is_exact
                        && package_bindings.is_empty();
                    if extension.receipt.enabled {
                        if !exact_published
                            && !candidate_snapshot_complete
                            && !unpublished_uninstall_replay
                        {
                            return Err(lifecycle_graph_error(
                                "An enabled removed dependency is absent before candidate graph cutover.",
                            ));
                        }
                    } else if !package_bindings.is_empty() {
                        return Err(lifecycle_graph_error(
                            "A hidden removed dependency still has a Registry package binding.",
                        ));
                    }
                    removed_extensions.push(RemovedLifecyclePackage {
                        identity: identity.clone(),
                        extension,
                        selected: selected_is_exact,
                    });
                }
                None
                    if package_bindings.is_empty()
                        && (candidate_snapshot_complete || package_lock.is_none()) =>
                {
                    // A crash may occur after the exact removal journal deletes
                    // the retained receipt but before the parent graph record
                    // advances.
                }
                _ => {
                    return Err(lifecycle_graph_error(
                        "A removed dependency is neither its exact selected or retained generation nor a completed unpublished replay.",
                    ))
                }
            }
        }

        if let Some(record) = recorded_cutover {
            let candidates_match =
                identities
                    .iter()
                    .zip(&extensions)
                    .all(|(identity, extension)| {
                        extension.receipt.enabled
                            && snapshot_before.packages.iter().any(|binding| {
                                binding.enabled
                                    && binding_matches_identity(self.paths(), binding, identity)
                            })
                    });
            let removed_match = removed.iter().all(|identity| {
                !snapshot_before
                    .packages
                    .iter()
                    .any(|binding| binding_matches_identity(self.paths(), binding, identity))
            });
            if !candidates_match || !removed_match {
                return Err(registry_cutover_conflict(
                    "The durable package-graph cutover no longer matches Registry visibility.",
                ));
            }
            for extension in &extensions {
                verify_package_integrity(extension).await?;
            }
            let packages = extensions
                .into_iter()
                .map(|extension| ExtensionLifecycleResult {
                    changed: false,
                    extension,
                    registry_generation: record.registry_generation_after,
                })
                .collect();
            return publication_from_record(packages, &record);
        }

        let moved_removed = self
            .retain_removed_lifecycle_packages(
                &artifact_store,
                &artifact_admission,
                &removed_extensions,
            )
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
                &artifact_store,
                &artifact_admission,
                &self.paths.receipt_path(&extension.receipt.package_id),
                &extension.receipt,
            )
            .await
            {
                self.restore_lifecycle_receipts(
                    &artifact_store,
                    &artifact_admission,
                    &written_receipts,
                )
                .await?;
                self.restore_removed_lifecycle_packages(
                    &artifact_store,
                    &artifact_admission,
                    &moved_removed,
                )
                .await?;
                return Err(error);
            }
            written_receipts.push(original.clone());
        }

        let mut installed = match self.list().await {
            Ok(installed) => installed,
            Err(error) => {
                self.restore_lifecycle_receipts(
                    &artifact_store,
                    &artifact_admission,
                    &written_receipts,
                )
                .await?;
                self.restore_removed_lifecycle_packages(
                    &artifact_store,
                    &artifact_admission,
                    &moved_removed,
                )
                .await?;
                return Err(error);
            }
        };
        installed.retain(|extension| {
            !removed
                .iter()
                .any(|identity| exact_receipt(identity, &extension.receipt).is_ok())
        });
        let snapshot_result = match cutover_request {
            Some(request) => {
                self.publish_snapshot_with_cutover_locked(&installed, request)
                    .await
            }
            None => self.publish_snapshot_locked(&installed).await,
        };
        let snapshot = match snapshot_result {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.restore_lifecycle_receipts(&artifact_store, &artifact_admission, &originals)
                    .await?;
                self.restore_removed_lifecycle_packages(
                    &artifact_store,
                    &artifact_admission,
                    &moved_removed,
                )
                .await?;
                write_registry_snapshot(&self.paths, &snapshot_before).await?;
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
        artifact_store: &ArtifactStore,
        artifact_admission: &ArtifactReferenceAdmission,
        removed: &[RemovedLifecyclePackage],
    ) -> UseResult<Vec<RemovedLifecyclePackage>> {
        let mut moved = Vec::new();
        for package in removed.iter().filter(|package| package.selected) {
            let mut hidden = package.extension.receipt.clone();
            hidden.enabled = false;
            if let Err(error) = self
                .retain_lifecycle_receipt(
                    artifact_store,
                    artifact_admission,
                    &package.identity,
                    &hidden,
                )
                .await
            {
                self.restore_removed_lifecycle_packages(artifact_store, artifact_admission, &moved)
                    .await?;
                return Err(error);
            }
            moved.push(package.clone());
            let receipt_path = self.paths.receipt_path(package.identity.package_id());
            if let Err(error) = remove_file_with_windows_retry(
                receipt_path.clone(),
                "retain removed lifecycle package receipt",
            )
            .await
            {
                self.restore_removed_lifecycle_packages(artifact_store, artifact_admission, &moved)
                    .await?;
                return Err(error);
            }
            if let Err(error) = sync_parent_directory(
                receipt_path
                    .parent()
                    .ok_or_else(|| lifecycle_state_error("A lifecycle receipt has no parent."))?,
                "removed lifecycle package receipt",
            )
            .await
            {
                self.restore_removed_lifecycle_packages(artifact_store, artifact_admission, &moved)
                    .await?;
                return Err(error);
            }
        }
        Ok(moved)
    }

    async fn restore_removed_lifecycle_packages(
        &self,
        artifact_store: &ArtifactStore,
        artifact_admission: &ArtifactReferenceAdmission,
        moved: &[RemovedLifecyclePackage],
    ) -> UseResult<()> {
        for package in moved.iter().rev() {
            write_receipt(
                artifact_store,
                artifact_admission,
                &self.paths.receipt_path(package.identity.package_id()),
                &package.extension.receipt,
            )
            .await?;
            self.remove_retained_receipt(&package.identity).await?;
        }
        Ok(())
    }

    async fn restore_lifecycle_receipts(
        &self,
        artifact_store: &ArtifactStore,
        artifact_admission: &ArtifactReferenceAdmission,
        receipts: &[ExtensionReceipt],
    ) -> UseResult<()> {
        for receipt in receipts {
            write_receipt(
                artifact_store,
                artifact_admission,
                &self.paths.receipt_path(&receipt.package_id),
                receipt,
            )
            .await?;
        }
        Ok(())
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
