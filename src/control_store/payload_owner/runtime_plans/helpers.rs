//! Shared digests and error constructors for Runtime plan payload snapshot/restore.

use a3s_use_core::{InstallationId, UseError, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerLimits, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding,
};
use super::{
    ACTIVATION_SCHEMA, CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA, INVENTORY_DOMAIN,
    MAX_ACTIVATION_BYTES, ControlRuntimePlanPayloadEntry, ControlRuntimePlanPayloadSnapshot,
    StagedControlRuntimePlanPayloadRestore, VerifiedControlRuntimePlanPayloadSnapshot,
};


pub(super) fn inventory_digest(
    installation: &InstallationId,
    entries: &[ControlRuntimePlanPayloadEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ControlRuntimePlanPayloadEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| {
        runtime_plan_error(format!(
            "Failed to encode the Runtime plan inventory: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    digest.update(INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

pub(super) fn activation_bytes(snapshot: &ControlRuntimePlanPayloadSnapshot) -> UseResult<Vec<u8>> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Activation<'a> {
        schema: &'static str,
        binding: &'a ControlPayloadSnapshotBinding,
        owner_manifest_digest: &'a str,
        inventory_digest: &'a str,
    }
    let bytes = canonical_json(&Activation {
        schema: ACTIVATION_SCHEMA,
        binding: &snapshot.manifest.binding,
        owner_manifest_digest: &snapshot.manifest.descriptor_digest,
        inventory_digest: &snapshot.manifest.inventory_digest,
    })
    .map_err(|error| {
        runtime_plan_error(format!(
            "Failed to encode the Runtime plan activation marker: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ACTIVATION_BYTES {
        return Err(runtime_plan_error(
            "The Runtime plan activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

pub(super) fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn runtime_plan_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::RuntimePlanPayload)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(runtime_plan_error(
            "The Runtime plan payload owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA {
        return Err(runtime_plan_error(
            "The Runtime plan payload owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

pub(super) fn wrap_plan_error(error: UseError) -> UseError {
    runtime_plan_error(format!(
        "Runtime plan store validation failed: {}",
        error.message
    ))
}

pub(super) fn runtime_plan_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_snapshot_invalid",
        message,
    )
}

pub(super) fn runtime_plan_io(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_snapshot_io",
        message,
    )
}

pub(super) fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_restore_invalid",
        message,
    )
}

pub(super) fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_restore_target_not_empty",
        "The clean-target Runtime plan restore refuses to merge or replace an existing root.",
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ControlRuntimePlanPayloadSnapshot>();
    assert_send_sync::<VerifiedControlRuntimePlanPayloadSnapshot>();
    assert_send_sync::<StagedControlRuntimePlanPayloadRestore>();
};
