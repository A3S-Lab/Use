use a3s_use_core::{InstallationSnapshot, PlanQualifiedSurfaceRef, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::{
    input_error, ControlCapabilitySelection, ControlGrantSelection, ControlPackageLifecycle,
    ControlProviderSelection, ReviewedControlOperation,
};

const CONTROL_CAPABILITY_DESCRIPTOR_SCHEMA: &str = "a3s.use.control-capability-descriptor.v1";
const MAX_CONTROL_CAPABILITY_DESCRIPTOR_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlCapabilityDescriptor<'a> {
    schema: &'static str,
    installation: &'a a3s_use_core::InstallationId,
    installation_generation: u64,
    capability_generation: u64,
    snapshot_digest: String,
    package_lifecycles: &'a [ControlPackageLifecycle],
    grants: Vec<ControlCapabilityGrantRef<'a>>,
    provider_selections: Vec<ControlCapabilityProviderRef<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlCapabilityGrantRef<'a> {
    package_id: &'a str,
    grant_digest: &'a str,
    receipt_revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlCapabilityProviderRef<'a> {
    surface: &'a PlanQualifiedSurfaceRef,
    selection_digest: &'a str,
}

pub(super) fn project_capability(
    operation: &ReviewedControlOperation,
    snapshot: &InstallationSnapshot,
    package_lifecycles: &[ControlPackageLifecycle],
    grants: &[ControlGrantSelection],
    provider_selections: &[ControlProviderSelection],
) -> UseResult<ControlCapabilitySelection> {
    let capability_generation = operation.target_capability_generation()?;
    let descriptor = ControlCapabilityDescriptor {
        schema: CONTROL_CAPABILITY_DESCRIPTOR_SCHEMA,
        installation: &snapshot.installation,
        installation_generation: snapshot.generation,
        capability_generation,
        snapshot_digest: snapshot.descriptor_digest()?,
        package_lifecycles,
        grants: grants
            .iter()
            .map(|grant| ControlCapabilityGrantRef {
                package_id: grant.package_id(),
                grant_digest: &grant.grant_digest,
                receipt_revision: grant.receipt_revision,
            })
            .collect(),
        provider_selections: provider_selections
            .iter()
            .map(|provider| ControlCapabilityProviderRef {
                surface: provider.qualified_surface(),
                selection_digest: &provider.selection_digest,
            })
            .collect(),
    };
    let bytes = canonical_descriptor_bytes(&descriptor)?;
    Ok(ControlCapabilitySelection {
        generation: capability_generation,
        descriptor_digest: format!("sha256:{:x}", Sha256::digest(bytes)),
    })
}

fn canonical_descriptor_bytes(descriptor: &ControlCapabilityDescriptor<'_>) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    descriptor.serialize(&mut serializer).map_err(|error| {
        input_error(format!(
            "Failed to encode canonical Control Store capability descriptor: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_BYTES {
        return Err(input_error(
            "The canonical Control Store capability descriptor exceeds its size bound.",
        ));
    }
    Ok(bytes)
}
