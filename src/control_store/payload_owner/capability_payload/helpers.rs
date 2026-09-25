//! Shared digests and error constructors for Capability payload snapshot/restore.

use a3s_use_core::{InstallationId, UseError, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerLimits, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding,
};
use super::{
    ACTIVATION_SCHEMA, CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA, INVENTORY_DOMAIN,
    MAX_ACTIVATION_BYTES, ControlCapabilityPayloadEntry, ControlCapabilityPayloadSnapshot,
    StagedControlCapabilityPayloadRestore,
    VerifiedControlCapabilityPayloadSnapshot,
};

pub(super) fn inventory_digest(
    installation: &InstallationId,
    entries: &[ControlCapabilityPayloadEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ControlCapabilityPayloadEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| {
        capability_payload_error(format!(
            "Failed to encode the Capability payload inventory: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    digest.update(INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

pub(super) fn activation_bytes(snapshot: &ControlCapabilityPayloadSnapshot) -> UseResult<Vec<u8>> {
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
        capability_payload_error(format!(
            "Failed to encode the Capability payload activation marker: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ACTIVATION_BYTES {
        return Err(capability_payload_error(
            "The Capability payload activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

pub(super) fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn capability_payload_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::CapabilityPayload)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(capability_payload_error(
            "The Capability payload owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA {
        return Err(capability_payload_error(
            "The Capability payload owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

pub(super) fn wrap_capability_error(error: UseError) -> UseError {
    capability_payload_error(format!(
        "Capability payload store validation failed: {}",
        error.message
    ))
}

pub(super) fn capability_payload_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.capability_payload_snapshot_invalid",
        message,
    )
}

pub(super) fn capability_payload_io(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.capability_payload_snapshot_io", message)
}

pub(super) fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.capability_payload_restore_invalid",
        message,
    )
}

pub(super) fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.capability_payload_restore_target_not_empty",
        "The clean-target Capability payload restore refuses to merge or replace an existing root.",
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ControlCapabilityPayloadSnapshot>();
    assert_send_sync::<VerifiedControlCapabilityPayloadSnapshot>();
    assert_send_sync::<StagedControlCapabilityPayloadRestore>();
};
