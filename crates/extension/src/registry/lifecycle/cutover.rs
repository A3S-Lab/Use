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
    legacy_request_digest: Option<String>,
    pub(super) expected_generation: Option<u64>,
}

impl ExtensionLifecycleCutoverRequest {
    fn new(
        idempotency_key: &str,
        operation: &str,
        package_lock: Option<&PluginPackageLock>,
        candidates: &[ExtensionLifecycleIdentity],
        removed: &[ExtensionLifecycleIdentity],
        expected_generation: Option<u64>,
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
        let legacy_request_digest = expected_generation
            .map(|_| format!("sha256:{:x}", Sha256::digest(identity.as_bytes())));
        if let Some(generation) = expected_generation {
            identity.push_str("\nexpected-generation\n");
            identity.push_str(&generation.to_string());
        }
        Ok(Self {
            idempotency_key,
            request_digest: format!("sha256:{:x}", Sha256::digest(identity.as_bytes())),
            legacy_request_digest,
            expected_generation,
        })
    }

    pub(super) fn require_current_generation(&self, current_generation: u64) -> UseResult<()> {
        if self
            .expected_generation
            .is_some_and(|expected| expected != current_generation)
        {
            return Err(registry_cutover_conflict(
                "The reviewed capability generation changed before Registry cutover.",
            )
            .with_detail("expectedGeneration", self.expected_generation)
            .with_detail("actualGeneration", current_generation));
        }
        Ok(())
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
        self.publish_lifecycle_package_with_generation_durable_cutover(
            identity,
            None,
            idempotency_key,
        )
        .await
    }

    /// Publish one exact lifecycle generation only if the immutable Registry
    /// snapshot still has the generation reviewed by the owning plan.
    pub async fn publish_lifecycle_package_at_generation_with_durable_cutover(
        &self,
        identity: &ExtensionLifecycleIdentity,
        expected_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.publish_lifecycle_package_with_generation_durable_cutover(
            identity,
            Some(expected_generation),
            idempotency_key,
        )
        .await
    }

    async fn publish_lifecycle_package_with_generation_durable_cutover(
        &self,
        identity: &ExtensionLifecycleIdentity,
        expected_generation: Option<u64>,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "single-package-publish",
            None,
            std::slice::from_ref(identity),
            &[],
            expected_generation,
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
        expected_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "package-graph-publish",
            Some(package_lock),
            identities,
            &[],
            Some(expected_generation),
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
        expected_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "package-graph-transition",
            Some(package_lock),
            identities,
            removed,
            Some(expected_generation),
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
        expected_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "package-graph-hide",
            Some(package_lock),
            &[],
            identities,
            Some(expected_generation),
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
        self.hide_lifecycle_package_with_generation_durable_cutover(identity, None, idempotency_key)
            .await
    }

    /// Hide one exact lifecycle generation only if the immutable Registry
    /// snapshot still has the generation reviewed by the owning plan.
    pub async fn hide_lifecycle_package_at_generation_with_durable_cutover(
        &self,
        identity: &ExtensionLifecycleIdentity,
        expected_generation: u64,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        self.hide_lifecycle_package_with_generation_durable_cutover(
            identity,
            Some(expected_generation),
            idempotency_key,
        )
        .await
    }

    async fn hide_lifecycle_package_with_generation_durable_cutover(
        &self,
        identity: &ExtensionLifecycleIdentity,
        expected_generation: Option<u64>,
        idempotency_key: &str,
    ) -> UseResult<ExtensionLifecycleGraphPublication> {
        let request = ExtensionLifecycleCutoverRequest::new(
            idempotency_key,
            "single-package-hide",
            None,
            &[],
            std::slice::from_ref(identity),
            expected_generation,
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
        let mut snapshot = read_registry_snapshot(&self.paths).await?;
        let before = snapshot.pending_cutovers.len();
        snapshot
            .pending_cutovers
            .retain(|record| record.idempotency_key != idempotency_key);
        if snapshot.pending_cutovers.len() != before {
            write_registry_snapshot(&self.paths, &snapshot).await?;
        }
        Ok(())
    }

    pub(super) async fn publish_snapshot_with_cutover_locked(
        &self,
        installed: &[InstalledExtension],
        request: &ExtensionLifecycleCutoverRequest,
    ) -> UseResult<ExtensionRegistrySnapshot> {
        let current = read_registry_snapshot(&self.paths).await?;
        if recorded_cutover(&current, request)?.is_some() {
            return Err(registry_cutover_conflict(
                "The Registry cutover record already exists outside replay handling.",
            ));
        }
        if current.pending_cutovers.len() >= MAX_PENDING_REGISTRY_CUTOVERS {
            return Err(registry_cutover_capacity());
        }
        request.require_current_generation(current.generation)?;
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
            installation: self.installation().clone(),
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
        write_registry_snapshot(&self.paths, &snapshot).await?;
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
    if record.request_digest != request.request_digest
        && request.legacy_request_digest.as_deref() != Some(record.request_digest.as_str())
    {
        return Err(registry_cutover_conflict(
            "A Registry cutover idempotency key was reused for a different lifecycle mutation.",
        ));
    }
    if request.expected_generation.is_some_and(|expected| {
        record.registry_generation_before != expected
            || expected.checked_add(1) != Some(record.registry_generation_after)
    }) {
        return Err(registry_cutover_conflict(
            "The Registry cutover record does not match the reviewed capability generation.",
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
