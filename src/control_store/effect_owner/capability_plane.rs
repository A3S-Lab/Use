use std::fmt;

use a3s_use_core::PluginOperationAction;
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use async_trait::async_trait;
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::effect_port::{
    ControlCapabilityCutoverRequest, ControlCapabilityIndexEffectPort, ControlEffectFailure,
    ControlEffectPortOutcome, ControlInvocationDrainRequest, ControlInvocationLeaseEffectPort,
    ControlReceiptApplication,
};
use super::super::model::ControlPublishedCapabilityCursor;
use super::super::ControlStore;
use crate::plugin_lifecycle::PluginLifecycleAction;

mod index;
mod lease;
mod model;
#[cfg(test)]
mod tests;

use index::ControlCapabilityIndexStore;
use lease::{ControlGenerationFileLease, ControlGenerationLeaseStore};
use model::ControlCapabilityIndexDocument;

#[derive(Debug, Clone)]
pub(in crate::control_store) struct ControlCapabilityPlaneEffectPort {
    control: ControlStore,
    index: ControlCapabilityIndexStore,
    leases: ControlGenerationLeaseStore,
}

impl ControlCapabilityPlaneEffectPort {
    pub(in crate::control_store) fn new(control: ControlStore) -> Self {
        let state_root = control.state_root.clone();
        Self {
            control,
            index: ControlCapabilityIndexStore::new(&state_root),
            leases: ControlGenerationLeaseStore::new(state_root),
        }
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
            _generation_leases: generation_leases,
            _maintenance: maintenance,
        }))
    }

    async fn cutover(
        &self,
        request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
        let document = match ControlCapabilityIndexDocument::from_request(request) {
            Ok(document) => document,
            Err(error) => return rejected(request.identity.idempotency_key.as_str(), &error.code),
        };
        let receipt_digest = match document.receipt_digest() {
            Ok(digest) => digest,
            Err(error) => return rejected(request.identity.idempotency_key.as_str(), &error.code),
        };
        let current = match self.control.published_capability().await {
            Ok(current) => current,
            Err(error) => return deferred(request.identity.idempotency_key.as_str(), &error.code),
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
                    && cursor.receipt_digest == receipt_digest =>
            {
                true
            }
            Some(_) => false,
        };
        if !allowed {
            return rejected(
                request.identity.idempotency_key.as_str(),
                "use.control.capability_publication_conflict",
            );
        }
        match self.index.materialize(&document).await {
            Ok(receipt_digest) => match ControlReceiptApplication::new(receipt_digest) {
                Ok(application) => ControlEffectPortOutcome::applied(application),
                Err(error) => rejected(request.identity.idempotency_key.as_str(), &error.code),
            },
            Err(error) if error.code == "use.control.capability_index_contended" => {
                deferred(request.identity.idempotency_key.as_str(), &error.code)
            }
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "use.control.capability_index_path_invalid"
                        | "use.control.capability_index_conflict"
                ) =>
            {
                rejected(request.identity.idempotency_key.as_str(), &error.code)
            }
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
    ) -> ControlEffectPortOutcome<ControlReceiptApplication> {
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
            .finish()
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
