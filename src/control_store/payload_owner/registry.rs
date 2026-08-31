use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use a3s_use_core::UseResult;

use super::{
    canonical_json, registry_error, ControlPayloadOwnerId, ControlPayloadOwnerRegistration,
};

const CONTROL_PAYLOAD_OWNER_REGISTRY_SCHEMA: &str = "a3s.use.control-payload-owner-registry.v1";
const MAX_CONTROL_PAYLOAD_REGISTRY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPayloadOwnerRegistry {
    schema: String,
    registrations: Vec<ControlPayloadOwnerRegistration>,
    descriptor_digest: String,
}

impl ControlPayloadOwnerRegistry {
    pub(in crate::control_store) fn new(
        mut registrations: Vec<ControlPayloadOwnerRegistration>,
    ) -> UseResult<Self> {
        registrations.sort_by_key(ControlPayloadOwnerRegistration::owner);
        validate_registration_set(&registrations)?;
        let descriptor_digest = registry_digest(&registrations)?;
        let registry = Self {
            schema: CONTROL_PAYLOAD_OWNER_REGISTRY_SCHEMA.to_string(),
            registrations,
            descriptor_digest,
        };
        registry.validate()?;
        Ok(registry)
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        validate_registration_set(&self.registrations)?;
        if self.schema != CONTROL_PAYLOAD_OWNER_REGISTRY_SCHEMA
            || !super::super::model::valid_sha256(&self.descriptor_digest)
            || registry_digest(&self.registrations)? != self.descriptor_digest
        {
            return Err(registry_error(
                "The Control payload owner registry is noncanonical or has drifted.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn registrations(&self) -> &[ControlPayloadOwnerRegistration] {
        &self.registrations
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }

    pub(super) fn registration(
        &self,
        owner: ControlPayloadOwnerId,
    ) -> Option<&ControlPayloadOwnerRegistration> {
        self.registrations
            .binary_search_by_key(&owner, ControlPayloadOwnerRegistration::owner)
            .ok()
            .map(|index| &self.registrations[index])
    }
}

fn validate_registration_set(registrations: &[ControlPayloadOwnerRegistration]) -> UseResult<()> {
    if registrations.len() != ControlPayloadOwnerId::ALL.len()
        || registrations
            .iter()
            .map(ControlPayloadOwnerRegistration::owner)
            .ne(ControlPayloadOwnerId::ALL)
        || registrations
            .iter()
            .any(|registration| registration.validate().is_err())
    {
        return Err(registry_error(
            "The Control payload registry must contain each frozen external owner exactly once.",
        ));
    }
    Ok(())
}

fn registry_digest(registrations: &[ControlPayloadOwnerRegistration]) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Descriptor<'a> {
        schema: &'static str,
        registrations: &'a [ControlPayloadOwnerRegistration],
    }

    let bytes = canonical_json(&Descriptor {
        schema: CONTROL_PAYLOAD_OWNER_REGISTRY_SCHEMA,
        registrations,
    })
    .map_err(|error| {
        registry_error(format!(
            "Failed to encode the canonical Control payload registry: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_PAYLOAD_REGISTRY_BYTES {
        return Err(registry_error(
            "The Control payload owner registry exceeds its canonical byte bound.",
        ));
    }
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
