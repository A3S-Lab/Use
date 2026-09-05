use std::fmt;
use std::sync::Arc;

use a3s_use_core::{CapabilityGatewayCatalog, PluginOperationAction, UseError, UseResult};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use async_trait::async_trait;
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::effect_port::{
    ControlCapabilityCatalogProjectionPort, ControlCapabilityCutoverApplication,
    ControlCapabilityCutoverRequest, ControlCapabilityIndexEffectPort, ControlEffectFailure,
    ControlEffectPortOutcome, ControlInvocationDrainRequest, ControlInvocationLeaseEffectPort,
    ControlReceiptApplication,
};
use super::super::model::{ControlCapabilityCatalogBinding, ControlPublishedCapabilityCursor};
use super::super::ControlStore;
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;
use crate::plugin_lifecycle::PluginLifecycleAction;

mod descriptor;
mod index;
mod lease;
mod model;
#[cfg(test)]
mod tests;

pub(in crate::control_store) use descriptor::{
    ControlCapabilityDescriptorProjection, ControlCapabilitySignerPolicy,
};
use index::ControlCapabilityIndexStore;
use lease::{ControlGenerationFileLease, ControlGenerationLeaseStore};
use model::ControlCapabilityIndexDocument;

const CATALOG_BINDING_ERROR: &str = "use.control.capability_catalog_binding_invalid";

#[derive(Clone)]
pub(in crate::control_store) struct ControlCapabilityPlaneEffectPort {
    control: ControlStore,
    index: ControlCapabilityIndexStore,
    leases: ControlGenerationLeaseStore,
    catalogs: CapabilityGatewayCatalogStore,
    catalog_projection: Arc<dyn ControlCapabilityCatalogProjectionPort>,
}

impl ControlCapabilityPlaneEffectPort {
    pub(in crate::control_store) fn new(
        control: ControlStore,
        catalogs: CapabilityGatewayCatalogStore,
        catalog_projection: Arc<dyn ControlCapabilityCatalogProjectionPort>,
    ) -> UseResult<Self> {
        if catalogs.installation() != &control.installation
            || catalogs.state_root() != control.state_root
        {
            return Err(UseError::new(
                CATALOG_BINDING_ERROR,
                "The Control Store and catalog payload owner do not share one installation root.",
            ));
        }
        let state_root = control.state_root.clone();
        Ok(Self {
            control,
            index: ControlCapabilityIndexStore::new(&state_root),
            leases: ControlGenerationLeaseStore::new(state_root),
            catalogs,
            catalog_projection,
        })
    }

    /// Compose the strict host projector for a fixed set of verified signed
    /// descriptions.  The proof set and signer policy are immutable for the
    /// lifetime of the port; callers must replace the port when a Registry
    /// publication or trust policy changes.
    pub(in crate::control_store) fn with_verified_descriptions(
        control: ControlStore,
        catalogs: CapabilityGatewayCatalogStore,
        proofs: Vec<a3s_use_core::CapabilityDescriptionProof>,
        signer_policy: ControlCapabilitySignerPolicy,
    ) -> UseResult<Self> {
        let projection = ControlCapabilityDescriptorProjection::new(proofs, signer_policy)?;
        Self::new(control, catalogs, Arc::new(projection))
    }

    /// Acquire one exact published capability generation for the complete
    /// lifetime of an accepted call set.
    ///
    /// The Control cursor is read before and after all package locks. A
    /// concurrent cutover therefore either observes these leases during drain
    /// or makes this admission return stale without exposing a mixed graph.
    pub(in crate::control_store) async fn acquire_published(
        &self,
        expected: &ControlPublishedCapabilityCursor,
    ) -> a3s_use_core::UseResult<Option<ControlCapabilitySnapshotLease>> {
        expected.validate()?;
        if expected.installation != self.control.installation {
            return Err(a3s_use_core::UseError::new(
                "use.control.capability_scope_mismatch",
                "The requested capability cursor belongs to another installation.",
            ));
        }
        let maintenance = StateMaintenanceLock::new(&self.control.state_root)
            .acquire_shared()
            .await?;
        let before = self.control.published_capability().await?;
        if before.as_ref() != Some(expected) {
            return Ok(None);
        }
        let document = self.index.read(&expected.receipt_digest).await?;
        if !document.matches_cursor(expected)? {
            return Err(a3s_use_core::UseError::new(
                "use.control.capability_index_conflict",
                "The published Capability Index differs from its Control cursor.",
            ));
        }
        let catalog = self
            .catalogs
            .get_exact(
                &expected.catalog.digest,
                expected.catalog.generation,
                &expected.catalog.revision,
            )
            .await?
            .ok_or_else(catalog_payload_conflict)?;
        if !document.matches_catalog(&catalog)? {
            return Err(catalog_payload_conflict());
        }
        let Some(generation_leases) = self.leases.try_acquire_shared(&expected.packages).await?
        else {
            return Ok(None);
        };
        let confirmed = self.control.published_capability().await?;
        if confirmed.as_ref() != Some(expected) {
            return Ok(None);
        }
        Ok(Some(ControlCapabilitySnapshotLease {
            cursor: expected.clone(),
            document,
            catalog,
            _generation_leases: generation_leases,
            _maintenance: maintenance,
        }))
    }

    async fn cutover(
        &self,
        request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlCapabilityCutoverApplication> {
        if let Err(error) = ControlCapabilityIndexDocument::validate_request(request) {
            return rejected(request.identity.idempotency_key.as_str(), &error.code);
        }
        let current = match self.control.published_capability().await {
            Ok(current) => current,
            Err(error) => return deferred(request.identity.idempotency_key.as_str(), &error.code),
        };
        let generation_allowed = match current.as_ref() {
            None => request.expected_capability_generation == 0,
            Some(cursor) => {
                cursor.capability_generation == request.expected_capability_generation
                    || (cursor.capability_generation == request.capability_generation
                        && cursor.descriptor_digest == request.descriptor_digest)
            }
        };
        if !generation_allowed {
            return rejected(
                request.identity.idempotency_key.as_str(),
                "use.control.capability_publication_conflict",
            );
        }

        let catalog = match self.catalog_projection.project(&request.authority).await {
            ControlEffectPortOutcome::Applied(catalog) => catalog,
            ControlEffectPortOutcome::Deferred(failure) => {
                return ControlEffectPortOutcome::Deferred(failure)
            }
            ControlEffectPortOutcome::Rejected(failure) => {
                return ControlEffectPortOutcome::Rejected(failure)
            }
            ControlEffectPortOutcome::Unknown(failure) => {
                return ControlEffectPortOutcome::Unknown(failure)
            }
        };
        if let Err(error) =
            ControlCapabilityIndexDocument::validate_catalog_for_request(request, &catalog)
        {
            return rejected(request.identity.idempotency_key.as_str(), &error.code);
        }
        let publication = match self.catalogs.publish(&catalog).await {
            Ok(publication) => publication,
            Err(error) => {
                return catalog_publication_failure(
                    request.identity.idempotency_key.as_str(),
                    &error,
                )
            }
        };
        let catalog = match ControlCapabilityCatalogBinding::from_publication(&publication) {
            Ok(catalog) => catalog,
            // The payload owner has already accepted immutable bytes. Any
            // failure after this point is acceptance ambiguity, not a proven
            // no-effect rejection.
            Err(error) => return unknown(request.identity.idempotency_key.as_str(), &error.code),
        };
        let document = match ControlCapabilityIndexDocument::from_request(request, catalog.clone())
        {
            Ok(document) => document,
            Err(error) => return unknown(request.identity.idempotency_key.as_str(), &error.code),
        };
        let receipt_digest = match document.receipt_digest() {
            Ok(digest) => digest,
            Err(error) => return unknown(request.identity.idempotency_key.as_str(), &error.code),
        };
        let current = match self.control.published_capability().await {
            Ok(current) => current,
            Err(error) => return unknown(request.identity.idempotency_key.as_str(), &error.code),
        };
        let allowed = match current.as_ref() {
            None => request.expected_capability_generation == 0,
            Some(cursor)
                if cursor.capability_generation == request.expected_capability_generation =>
            {
                true
            }
            Some(cursor)
                if cursor.capability_generation == request.capability_generation
                    && cursor.descriptor_digest == request.descriptor_digest
                    && cursor.catalog == catalog
                    && cursor.receipt_digest == receipt_digest =>
            {
                true
            }
            Some(_) => false,
        };
        if !allowed {
            return unknown(
                request.identity.idempotency_key.as_str(),
                "use.control.capability_publication_conflict",
            );
        }
        match self.index.materialize(&document).await {
            Ok(receipt_digest) => {
                match ControlCapabilityCutoverApplication::new(request, receipt_digest, catalog) {
                    Ok(application) => ControlEffectPortOutcome::applied(application),
                    Err(error) => unknown(request.identity.idempotency_key.as_str(), &error.code),
                }
            }
            // Catalog publication precedes Index materialization. Even
            // contention or a path/conflict error here cannot prove that the
            // earlier immutable catalog publication was absent.
            Err(error) => unknown(request.identity.idempotency_key.as_str(), &error.code),
        }
    }

    async fn drain(
        &self,
        request: &ControlInvocationDrainRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        if let Err(error) = request.validate_for_owner() {
            return rejected(request.identity.idempotency_key.as_str(), &error.code);
        }
        let current = match self.control.published_capability().await {
            Ok(Some(current)) => current,
            Ok(None) => {
                return deferred(
                    request.identity.idempotency_key.as_str(),
                    "use.control.capability_not_published",
                )
            }
            Err(error) => return deferred(request.identity.idempotency_key.as_str(), &error.code),
        };
        if current.contains_incarnation(&request.package_id, request.lifecycle_generation) {
            return deferred(
                request.identity.idempotency_key.as_str(),
                "use.control.invocation_generation_still_published",
            );
        }
        let drain = match self
            .leases
            .try_acquire_exclusive(&request.package_id, request.lifecycle_generation)
            .await
        {
            Ok(Some(drain)) => drain,
            Ok(None) => {
                return deferred(
                    request.identity.idempotency_key.as_str(),
                    "use.control.invocation_generation_busy",
                )
            }
            Err(error) if error.code == "use.control.invocation_lease_path_invalid" => {
                return rejected(request.identity.idempotency_key.as_str(), &error.code)
            }
            Err(error) => return deferred(request.identity.idempotency_key.as_str(), &error.code),
        };
        let receipt_digest = match drain_receipt_digest(request) {
            Ok(digest) => digest,
            Err(error) => return rejected(request.identity.idempotency_key.as_str(), &error.code),
        };
        drop(drain);
        match ControlReceiptApplication::new(receipt_digest) {
            Ok(application) => ControlEffectPortOutcome::applied(application),
            Err(error) => rejected(request.identity.idempotency_key.as_str(), &error.code),
        }
    }
}

#[async_trait]
impl ControlCapabilityIndexEffectPort for ControlCapabilityPlaneEffectPort {
    async fn cutover(
        &self,
        request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlCapabilityCutoverApplication> {
        self.cutover(request).await
    }
}

#[async_trait]
impl ControlInvocationLeaseEffectPort for ControlCapabilityPlaneEffectPort {
    async fn drain(
        &self,
        request: &ControlInvocationDrainRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        self.drain(request).await
    }
}

pub(in crate::control_store) struct ControlCapabilitySnapshotLease {
    cursor: ControlPublishedCapabilityCursor,
    document: ControlCapabilityIndexDocument,
    catalog: CapabilityGatewayCatalog,
    _generation_leases: Vec<ControlGenerationFileLease>,
    _maintenance: StateMaintenanceGuard,
}

impl ControlCapabilitySnapshotLease {
    pub(in crate::control_store) fn cursor(&self) -> &ControlPublishedCapabilityCursor {
        &self.cursor
    }

    pub(in crate::control_store) fn package_count(&self) -> usize {
        self.cursor.packages.len()
    }

    pub(in crate::control_store) fn catalog(&self) -> &CapabilityGatewayCatalog {
        &self.catalog
    }

    pub(in crate::control_store) fn document_receipt_digest(
        &self,
    ) -> a3s_use_core::UseResult<String> {
        self.document.receipt_digest()
    }
}

impl fmt::Debug for ControlCapabilitySnapshotLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlCapabilitySnapshotLease")
            .field("cursor", &self.cursor)
            .field("package_count", &self.package_count())
            .field("catalog_revision", &self.catalog.revision())
            .finish()
    }
}

impl fmt::Debug for ControlCapabilityPlaneEffectPort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlCapabilityPlaneEffectPort")
            .field("installation", &self.control.installation)
            .field("state_root", &self.control.state_root)
            .field("catalog_root", &self.catalogs.root())
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DrainReceipt<'a> {
    schema: &'static str,
    operation_id: &'a str,
    installation: &'a a3s_use_core::InstallationId,
    plan_digest: &'a str,
    operation_action: PluginOperationAction,
    sequence: u32,
    idempotency_key: &'a str,
    package_id: &'a str,
    lifecycle_generation: u64,
    package_digest: &'a str,
    manifest_digest: &'a str,
    lifecycle_action: PluginLifecycleAction,
}

fn drain_receipt_digest(
    request: &ControlInvocationDrainRequest,
) -> a3s_use_core::UseResult<String> {
    let receipt = DrainReceipt {
        schema: "a3s.use.control-invocation-drain-receipt.v1",
        operation_id: &request.identity.operation_id,
        installation: &request.identity.installation,
        plan_digest: &request.identity.plan_digest,
        operation_action: request.identity.operation_action,
        sequence: request.identity.sequence,
        idempotency_key: &request.identity.idempotency_key,
        package_id: &request.package_id,
        lifecycle_generation: request.lifecycle_generation,
        package_digest: &request.package_digest,
        manifest_digest: &request.manifest_digest,
        lifecycle_action: request.lifecycle_action,
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    receipt.serialize(&mut serializer).map_err(|error| {
        a3s_use_core::UseError::new(
            "use.control.invocation_receipt_invalid",
            format!("Failed to encode invocation drain receipt: {error}"),
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn catalog_publication_failure<T>(key: &str, error: &UseError) -> ControlEffectPortOutcome<T> {
    match error.code.as_str() {
        "use.plugin.capability_gateway_catalog_store_invalid"
        | "use.plugin.capability_gateway_catalog_store_conflict"
        | "use.state.maintenance_path_invalid" => rejected(key, &error.code),
        // The active marker is checked before the catalog mutation starts, so
        // this classification proves that no payload was accepted.
        "use.state.maintenance_restore_active" => deferred(key, &error.code),
        // I/O may fail after an immutable link reached durable storage. The
        // same content-addressed publication is replayable, but automatic
        // retry would violate the generic owner contract without an explicit
        // reconciliation decision.
        _ => unknown(key, &error.code),
    }
}

fn catalog_payload_conflict() -> UseError {
    UseError::new(
        CATALOG_BINDING_ERROR,
        "The published Control cursor does not resolve to its exact immutable catalog payload.",
    )
}

fn failure(key: &str, code: &str) -> ControlEffectFailure {
    let mut digest = Sha256::new();
    digest.update(b"a3s.use.control-capability-plane-failure.v1\0");
    digest.update(key.as_bytes());
    digest.update([0]);
    digest.update(code.as_bytes());
    ControlEffectFailure {
        evidence_digest: format!("sha256:{:x}", digest.finalize()),
        error_code: bounded_code(code),
    }
}

fn bounded_code(code: &str) -> String {
    if !code.is_empty()
        && code.len() <= 128
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        code.to_owned()
    } else {
        "use.control.capability_plane_failed".to_owned()
    }
}

fn rejected<T>(key: &str, code: &str) -> ControlEffectPortOutcome<T> {
    ControlEffectPortOutcome::rejected(failure(key, code))
}

fn deferred<T>(key: &str, code: &str) -> ControlEffectPortOutcome<T> {
    ControlEffectPortOutcome::deferred(failure(key, code))
}

fn unknown<T>(key: &str, code: &str) -> ControlEffectPortOutcome<T> {
    ControlEffectPortOutcome::unknown(failure(key, code))
}
