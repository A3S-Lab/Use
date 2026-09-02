use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginOperationAction, PluginSurfaceRef, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::control_store::effect_port::ControlCapabilityCutoverRequest;
use crate::control_store::model::{
    input_error, valid_error_code, valid_machine_id, valid_sha256, validate_grant_selections,
    validate_provider_selections, ControlCapabilityEffectAuthority, ControlCapabilityStatus,
    ControlCapabilitySurfaceState, ControlEffectIntent, ControlEffectKind, ControlEffectOwner,
    ControlEffectSubject, ControlPublishedCapabilityCursor, ControlPublishedCapabilityPackage,
};

const CONTROL_CAPABILITY_INDEX_SCHEMA: &str = "a3s.use.control-capability-index.v1";
const MAX_CONTROL_CAPABILITY_INDEX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityIndexDocument {
    schema: String,
    operation_id: String,
    plan_digest: String,
    operation_action: PluginOperationAction,
    sequence: u32,
    idempotency_key: String,
    expected_capability_generation: u64,
    pub(in crate::control_store) authority: ControlCapabilityEffectAuthority,
}

impl ControlCapabilityIndexDocument {
    pub(in crate::control_store) fn from_request(
        request: &ControlCapabilityCutoverRequest,
    ) -> UseResult<Self> {
        let document = Self {
            schema: CONTROL_CAPABILITY_INDEX_SCHEMA.to_owned(),
            operation_id: request.identity.operation_id.clone(),
            plan_digest: request.identity.plan_digest.clone(),
            operation_action: request.identity.operation_action,
            sequence: request.identity.sequence,
            idempotency_key: request.identity.idempotency_key.clone(),
            expected_capability_generation: request.expected_capability_generation,
            authority: request.authority.clone(),
        };
        document.validate()?;
        if request.identity.attempt == 0
            || request.identity.deadline_at_ms == 0
            || !request.identity.required
            || document.authority.generation.snapshot.installation != request.identity.installation
            || document.authority.generation.snapshot.generation
                != request.identity.installation_generation
            || document.authority.generation.capability.generation != request.capability_generation
            || document.authority.generation.capability.descriptor_digest
                != request.descriptor_digest
        {
            return Err(index_error(
                "The Capability Index document differs from its committed request.",
            ));
        }
        Ok(document)
    }

    pub(in crate::control_store) fn from_bytes(bytes: &[u8]) -> UseResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_CAPABILITY_INDEX_BYTES {
            return Err(index_error(
                "The Capability Index document exceeds its byte bound.",
            ));
        }
        let document: Self = serde_json::from_slice(bytes)
            .map_err(|_| index_error("The Capability Index document is invalid JSON."))?;
        document.validate()?;
        if document.canonical_bytes()? != bytes {
            return Err(index_error(
                "The Capability Index document is not canonically encoded.",
            ));
        }
        Ok(document)
    }

    pub(in crate::control_store) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            index_error(format!(
                "Failed to encode the canonical Capability Index document: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_CAPABILITY_INDEX_BYTES {
            return Err(index_error(
                "The canonical Capability Index document exceeds its byte bound.",
            ));
        }
        Ok(bytes)
    }

    pub(in crate::control_store) fn receipt_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }

    pub(in crate::control_store) fn matches_cursor(
        &self,
        cursor: &ControlPublishedCapabilityCursor,
    ) -> UseResult<bool> {
        cursor.validate()?;
        let generation = &self.authority.generation;
        let lifecycles = generation
            .package_lifecycles
            .iter()
            .map(|lifecycle| {
                (
                    lifecycle.package_id.as_str(),
                    lifecycle.lifecycle_generation,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let packages = generation
            .snapshot
            .packages
            .iter()
            .filter(|package| package.enabled)
            .map(|package| {
                Ok(ControlPublishedCapabilityPackage {
                    package_id: package.package_id().to_owned(),
                    lifecycle_generation: lifecycles
                        .get(package.package_id())
                        .copied()
                        .ok_or_else(|| {
                            index_error("A Capability Index package has no lifecycle generation.")
                        })?,
                    package_digest: package
                        .package
                        .catalog
                        .record
                        .package
                        .sha256
                        .clone()
                        .ok_or_else(|| index_error("A Capability Index package has no digest."))?,
                    manifest_digest: package
                        .package
                        .catalog
                        .record
                        .package
                        .manifest_sha256
                        .clone()
                        .ok_or_else(|| {
                            index_error("A Capability Index package has no manifest digest.")
                        })?,
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        Ok(cursor.installation == generation.snapshot.installation
            && cursor.installation_generation == generation.snapshot.generation
            && cursor.capability_generation == generation.capability.generation
            && cursor.descriptor_digest == generation.capability.descriptor_digest
            && cursor.receipt_digest == self.receipt_digest()?
            && cursor.packages == packages)
    }

    fn validate(&self) -> UseResult<()> {
        let generation = &self.authority.generation;
        generation.snapshot.validate()?;
        let expected_intent = ControlEffectIntent::new(
            self.sequence,
            generation.snapshot.installation.clone(),
            self.plan_digest.clone(),
            self.operation_action,
            generation.snapshot.generation,
            ControlEffectSubject::Installation {
                expected_capability_generation: self.expected_capability_generation,
                capability_generation: generation.capability.generation,
                descriptor_digest: generation.capability.descriptor_digest.clone(),
            },
            ControlEffectOwner::CapabilityIndex,
            ControlEffectKind::CapabilityCutover,
            true,
        )?;
        if self.schema != CONTROL_CAPABILITY_INDEX_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.plan_digest)
            || self.operation_id != generation.operation_id
            || self.idempotency_key != expected_intent.idempotency_key
            || self.expected_capability_generation.checked_add(1)
                != Some(generation.capability.generation)
            || !valid_sha256(&generation.capability.descriptor_digest)
            || generation.capability_status != ControlCapabilityStatus::Candidate
            || generation.capability_published_at_ms.is_some()
            || generation.committed_at_ms == 0
            || generation.snapshot.descriptor_digest()? != generation.snapshot_digest
        {
            return Err(index_error(
                "The Capability Index document does not bind one candidate Control generation.",
            ));
        }
        validate_lifecycles(generation)?;
        validate_grant_selections(&generation.grants, &generation.snapshot)?;
        validate_provider_selections(&generation.provider_selections, &generation.snapshot)?;
        validate_materializations(&self.authority)?;
        Ok(())
    }
}

fn validate_lifecycles(
    generation: &crate::control_store::model::ControlGeneration,
) -> UseResult<()> {
    if generation.package_lifecycles.len() != generation.snapshot.packages.len()
        || generation
            .package_lifecycles
            .windows(2)
            .any(|pair| pair[0].package_id >= pair[1].package_id)
        || generation
            .package_lifecycles
            .iter()
            .zip(&generation.snapshot.packages)
            .any(|(lifecycle, package)| {
                lifecycle.package_id != package.package_id() || lifecycle.lifecycle_generation == 0
            })
    {
        return Err(index_error(
            "The Capability Index lifecycle inventory is incomplete or noncanonical.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SurfaceIncarnation {
    package_id: String,
    lifecycle_generation: u64,
    surface: PluginSurfaceRef,
}

fn validate_materializations(authority: &ControlCapabilityEffectAuthority) -> UseResult<()> {
    let generation = &authority.generation;
    let lifecycles = generation
        .package_lifecycles
        .iter()
        .map(|lifecycle| {
            (
                lifecycle.package_id.as_str(),
                lifecycle.lifecycle_generation,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected = generation
        .snapshot
        .packages
        .iter()
        .filter(|package| package.enabled)
        .flat_map(|package| {
            let lifecycle_generation = lifecycles.get(package.package_id()).copied();
            package.selected_surfaces.iter().filter_map(move |surface| {
                lifecycle_generation.map(|lifecycle_generation| SurfaceIncarnation {
                    package_id: package.package_id().to_owned(),
                    lifecycle_generation,
                    surface: surface.clone(),
                })
            })
        })
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for materialization in &authority.materializations {
        let intent = &materialization.intent;
        intent.validate_binding(
            &intent.installation,
            &intent.plan_digest,
            intent.operation_action,
        )?;
        let ControlEffectSubject::Surface {
            package_id,
            lifecycle_generation,
            package_digest,
            manifest_digest,
            surface,
            ..
        } = &intent.subject
        else {
            return Err(index_error(
                "A Capability Index materialization has no surface identity.",
            ));
        };
        let package = generation
            .snapshot
            .package_selection(package_id)
            .ok_or_else(|| index_error("A materialized package is absent from the generation."))?;
        let key = SurfaceIncarnation {
            package_id: package_id.clone(),
            lifecycle_generation: *lifecycle_generation,
            surface: surface.clone(),
        };
        if intent.kind != ControlEffectKind::SurfacePrepare
            || intent.installation != generation.snapshot.installation
            || !intent.owner.matches_generation(
                &intent.subject,
                intent.kind,
                &generation.provider_selections,
            )
            || package.package.catalog.record.package.sha256.as_deref()
                != Some(package_digest.as_str())
            || package
                .package
                .catalog
                .record
                .package
                .manifest_sha256
                .as_deref()
                != Some(manifest_digest.as_str())
            || !actual.insert(key)
        {
            return Err(index_error(
                "A Capability Index materialization differs from its selected surface.",
            ));
        }
        match &materialization.state {
            ControlCapabilitySurfaceState::Prepared {
                application,
                observed_at_ms,
            } => {
                application.validate_for(intent)?;
                if *observed_at_ms == 0 {
                    return Err(index_error(
                        "A prepared Capability Index surface has no observation time.",
                    ));
                }
            }
            ControlCapabilitySurfaceState::Degraded {
                evidence_digest,
                error_code,
                observed_at_ms,
            } => {
                if intent.required
                    || !valid_sha256(evidence_digest)
                    || !valid_error_code(error_code)
                    || *observed_at_ms == 0
                {
                    return Err(index_error(
                        "A degraded Capability Index surface has invalid evidence.",
                    ));
                }
            }
        }
    }
    if actual != expected {
        return Err(index_error(
            "The Capability Index materializations do not exactly cover callable surfaces.",
        ));
    }
    Ok(())
}

fn index_error(message: impl Into<String>) -> a3s_use_core::UseError {
    input_error(message)
}
