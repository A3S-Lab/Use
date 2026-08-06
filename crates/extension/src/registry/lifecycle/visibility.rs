use a3s_use_core::{UseError, UseResult};

use super::cutover::{
    publication_from_record, recorded_cutover, registry_cutover_capacity,
    registry_cutover_conflict, ExtensionLifecycleCutoverRequest,
};
use super::generations::binding_matches_identity;
use super::model::{
    exact_receipt, lifecycle_state_error, ExtensionLifecycleGraphPublication,
    ExtensionLifecycleIdentity, ExtensionLifecycleResult,
};
use super::ExtensionRegistry;
use crate::package::{write_receipt, RegistryLock};
use crate::registry::{verify_package_integrity, MAX_PENDING_REGISTRY_CUTOVERS};
use crate::registry_io::read_registry_snapshot;

impl ExtensionRegistry {
    pub(super) async fn set_lifecycle_visibility(
        &self,
        identity: &ExtensionLifecycleIdentity,
        enabled: bool,
        host_version: &str,
    ) -> UseResult<ExtensionLifecycleResult> {
        let publication = self
            .set_lifecycle_visibility_with_evidence(identity, enabled, host_version, None)
            .await?;
        publication.packages.into_iter().next().ok_or_else(|| {
            lifecycle_state_error("A single-package visibility cutover omitted its package result.")
        })
    }

    pub(super) async fn set_lifecycle_visibility_with_evidence(
        &self,
        identity: &ExtensionLifecycleIdentity,
        enabled: bool,
        host_version: &str,
        cutover_request: Option<&ExtensionLifecycleCutoverRequest>,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
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
        let recorded_cutover = cutover_request
            .map(|request| recorded_cutover(&published, request))
            .transpose()?
            .flatten();
        if cutover_request.is_some()
            && recorded_cutover.is_none()
            && published.pending_cutovers.len() >= MAX_PENDING_REGISTRY_CUTOVERS
        {
            return Err(registry_cutover_capacity());
        }
        let published_binding = published
            .routes
            .iter()
            .find(|binding| binding_matches_identity(&self.paths, binding, identity));
        let published_exact = published_binding.is_some();
        let published_enabled = published_binding.is_some_and(|binding| binding.enabled);
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
        if let Some(record) = recorded_cutover {
            if !selected_is_exact
                || extension.receipt.enabled != enabled
                || published_enabled != enabled
            {
                return Err(registry_cutover_conflict(
                    "The durable single-package cutover no longer matches Registry visibility.",
                ));
            }
            verify_package_integrity(&extension).await?;
            return publication_from_record(
                vec![ExtensionLifecycleResult {
                    changed: false,
                    extension,
                    registry_generation: record.registry_generation_after,
                }],
                &record,
            );
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
            match cutover_request {
                Some(request) => {
                    self.publish_snapshot_with_cutover_locked(&installed, request)
                        .await?
                }
                None => self.publish_snapshot_locked(&installed).await?,
            }
        } else {
            if cutover_request.is_some() {
                return Err(registry_cutover_conflict(
                    "The durable single-package request did not produce a Registry cutover.",
                ));
            }
            published
        };
        let registry_generation = snapshot.generation;
        let registry_snapshot_digest = snapshot.descriptor_digest()?;
        Ok(ExtensionLifecycleGraphPublication {
            packages: vec![ExtensionLifecycleResult {
                changed,
                extension,
                registry_generation,
            }],
            registry_generation: snapshot.generation,
            registry_snapshot_digest,
        })
    }
}
