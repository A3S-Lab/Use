use a3s_use_core::{PluginPackageLock, UseError, UseResult};
use sha2::{Digest, Sha256};

use super::model::{
    canonical_sha256, ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity,
    ExtensionLifecycleResult,
};
use super::ExtensionRegistry;
use crate::package::RegistryLock;
use crate::registry::{
    route_bindings, ExtensionRegistryCutoverRecord, ExtensionRegistrySnapshot, InstalledExtension,
    MAX_PENDING_REGISTRY_CUTOVERS, REGISTRY_SCHEMA_VERSION,
};
use crate::registry_io::{read_registry_snapshot, write_registry_snapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExtensionLifecycleCutoverRequest {
    pub(super) idempotency_key: String,
    pub(super) request_digest: String,
}

impl ExtensionLifecycleCutoverRequest {
    fn new(
        idempotency_key: &str,
        operation: &str,
        package_lock: Option<&PluginPackageLock>,
        candidates: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
    ) -> UseResult<Self> {
        let idempotency_key =
            canonical_sha256(idempotency_key.to_string(), "cutover idempotency key")?;
        if operation.is_empty() || candidates.is_empty() && removed.is_empty() {
            return Err(registry_cutover_conflict(
                "A durable Registry cutover request must identify one bounded mutation.",
            ));
        }
        let mut identity = format!("{operation}\n{idempotency_key}");
        if let Some(package_lock) = package_lock {
            identity.push('\n');
            identity.push_str(&package_lock.descriptor_digest()?);
        }
        identity.push_str("\ncandidates");
        for candidate in candidates {
            identity.push('\n');
            identity.push_str(&candidate.descriptor_digest()?);
        }
        identity.push_str("\nremoved");
        for prior in removed {
            identity.push('\n');
            identity.push_str(&prior.descriptor_digest()?);
        }
        Ok(Self {
            idempotency_key,
            request_digest: format!("sha256:{:x}", Sha256::digest(identity.as_bytes())),
        })
    }
}

impl ExtensionRegistry {
    /// Publish one exact generation and atomically retain its original
    /// cutover evidence until the owning operation acknowledges it.
    pub async fn publish_lifecycle_package_with_durable_cutover(
        &self,
        identity: &ExtensionLifecycleIdentity,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "single-package-publish",
            None,
            std::slice::from_ref(identity),
            &[],
        )?;
        self.set_lifecycle_visibility_with_evidence(
            identity,
            true,
            env!("CARGO_PKG_VERSION"),
            Some(&request),
        )
        .await
    }

    pub async fn publish_lifecycle_package_graph_with_durable_cutover(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "package-graph-publish",
            Some(package_lock),
            identities,
            &[],
        )?;
        self.publish_lifecycle_packages_for_host_version(
            identities,
            &[],
            env!("CARGO_PKG_VERSION"),
            Some(package_lock),
            Some(&request),
        )
        .await
    }

    pub async fn publish_lifecycle_package_graph_transition_with_durable_cutover(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "package-graph-transition",
            Some(package_lock),
            identities,
            removed,
        )?;
        self.publish_lifecycle_packages_for_host_version(
            identities,
            removed,
            env!("CARGO_PKG_VERSION"),
            Some(package_lock),
            Some(&request),
        )
        .await
    }

    pub async fn hide_lifecycle_package_graph_with_durable_cutover(
        &self,
        package_lock: &PluginPackageLock,
        identities: &[ExtensionLifecycleIdentity],
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "package-graph-hide",
            Some(package_lock),
            &[],
            identities,
        )?;
        self.publish_lifecycle_packages_for_host_version(
            &[],
            identities,
            env!("CARGO_PKG_VERSION"),
            None,
            Some(&request),
        )
        .await
    }

    pub async fn hide_lifecycle_package_with_durable_cutover(
        &self,
        identity: &ExtensionLifecycleIdentity,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "single-package-hide",
            None,
            &[],
            std::slice::from_ref(identity),
        )?;
        self.set_lifecycle_visibility_with_evidence(
            identity,
            false,
            env!("CARGO_PKG_VERSION"),
            Some(&request),
        )
        .await
    }

    /// Remove a durable cutover replay record after the Grant journal owns
    /// the same evidence. This metadata-only update preserves Registry
    /// generation and capability digest.
    pub async fn complete_lifecycle_cutover(&self, idempotency_key: &str) -> UseResult<()> {
        canonical_sha256(idempotency_key.to_string(), "cutover idempotency key")?;
        let _lock = RegistryLock::acquire_for_mutation(&self.paths).await?;
        let path = self.paths.registry_snapshot_path();
        let mut snapshot = read_registry_snapshot(&path).await?;
        let before = snapshot.pending_cutovers.len();
        snapshot
            .pending_cutovers
            .retain(|record| record.idempotency_key != idempotency_key);
        if snapshot.pending_cutovers.len() != before {
            write_registry_snapshot(&path, &snapshot).await?;
        }
        Ok(())
    }

    pub(super) async fn publish_snapshot_with_cutover_locked(
        &self,
        installed: &[InstalledExtension],
        request: &ExtensionLifecycleCutoverRequest,
    ) -> UseResult<ExtensionRegistrySnapshot> {
        let path = self.paths.registry_snapshot_path();
        let current = read_registry_snapshot(&path).await?;
        if recorded_cutover(&current, request)?.is_some() {
            return Err(registry_cutover_conflict(
                "The Registry cutover record already exists outside replay handling.",
            ));
        }
        if current.pending_cutovers.len() >= MAX_PENDING_REGISTRY_CUTOVERS {
            return Err(registry_cutover_capacity());
        }
        let routes = route_bindings(installed);
        if routes == current.routes {
            return Err(registry_cutover_conflict(
                "A new durable cutover request did not change Registry visibility.",
            ));
        }
        let generation = current.generation.checked_add(1).ok_or_else(|| {
            UseError::new(
                "use.extension.generation_exhausted",
                "The extension registry generation is exhausted.",
            )
        })?;
        let mut snapshot = ExtensionRegistrySnapshot {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation,
            routes,
            pending_cutovers: current.pending_cutovers,
        };
        let snapshot_digest = snapshot.descriptor_digest()?;
        snapshot
            .pending_cutovers
            .push(ExtensionRegistryCutoverRecord::new(
                &request.idempotency_key,
                &request.request_digest,
                current.generation,
                generation,
                snapshot_digest,
            )?);
        snapshot.validate()?;
        write_registry_snapshot(&path, &snapshot).await?;
        Ok(snapshot)
    }
}

pub(super) fn recorded_cutover(
    snapshot: &ExtensionRegistrySnapshot,
    request: &ExtensionLifecycleCutoverRequest,
) -> UseResult<Option<ExtensionRegistryCutoverRecord>> {
    let Some(record) = snapshot
        .pending_cutovers
        .iter()
        .find(|record| record.idempotency_key == request.idempotency_key)
    else {
        return Ok(None);
    };
    if record.request_digest != request.request_digest {
        return Err(registry_cutover_conflict(
            "A Registry cutover idempotency key was reused for a different lifecycle mutation.",
        ));
    }
    Ok(Some(record.clone()))
}

pub(super) fn publication_from_record(
    packages: Vec<ExtensionLifecycleResult>,
    record: &ExtensionRegistryCutoverRecord,
) -> UseResult<ExtensionLifecycleGraphPublication> {
    record.validate()?;
    Ok(ExtensionLifecycleGraphPublication {
        packages,
        registry_generation: record.registry_generation_after,
        registry_snapshot_digest: record.registry_snapshot_digest.clone(),
    })
}

pub(super) fn registry_cutover_conflict(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_cutover_conflict", message)
}

pub(super) fn registry_cutover_capacity() -> UseError {
    UseError::new(
        "use.extension.registry_cutover_capacity",
        "The Registry has too many unfinished cutovers to admit another mutation.",
    )
}
