use std::collections::BTreeMap;

use a3s_use_core::{
    OkfCapabilityProjection, OkfKnowledgeObservedState, PluginSurfaceKind, UseError, UseResult,
};

use crate::control_store::model::{
    ControlAppliedEffectEvidence, ControlEffectKind, ControlEffectOwner, ControlEffectRecord,
    ControlEffectStatus, ControlEffectSubject, ControlStoreAuthority,
    ControlSurfaceObservationState,
};
use crate::control_store::payload_owner::{
    ControlPayloadOwnerRegistry, ControlPayloadSnapshotBinding,
};
use crate::okf_knowledge::OkfKnowledgeBinding;

/// Semantically verified Control authority named by one payload binding.
/// Keeping this typed state separate ensures no owner bytes are interpreted
/// against an unchecked or merely digest-matching export.
pub(super) struct VerifiedControlKnowledgeHistory {
    authority: ControlStoreAuthority,
}

impl VerifiedControlKnowledgeHistory {
    pub(super) fn verify(
        registry: &ControlPayloadOwnerRegistry,
        snapshot_binding: &ControlPayloadSnapshotBinding,
        control_export: &[u8],
    ) -> UseResult<Self> {
        let verified = snapshot_binding
            .verify_control_export(registry, control_export)
            .map_err(wrap_reconciliation_error)?;
        Ok(Self {
            authority: verified.export.authority,
        })
    }

    /// Prove that owner payload is only retained evidence for effects present
    /// in the exact bound Control export. The payload never selects desired
    /// state.
    pub(super) fn reconcile(&self, bindings: &[OkfKnowledgeBinding]) -> UseResult<()> {
        let history = KnowledgeEffectHistory::new(&self.authority)?;
        let mut payload = BTreeMap::new();

        for binding in bindings {
            let key = KnowledgeIncarnation::from_binding(binding);
            if payload.insert(key.clone(), binding).is_some() {
                return Err(reconciliation_error(
                    "The Knowledge payload repeats one Control lifecycle incarnation.",
                ));
            }
            let preparations = history.preparations.get(&key).ok_or_else(|| {
                reconciliation_error(
                    "A Knowledge payload binding has no exact Control preparation intent.",
                )
            })?;
            let origin = preparations
                .iter()
                .copied()
                .find(|record| record.operation_id == binding.receipt.operation_id)
                .ok_or_else(|| {
                    reconciliation_error(
                        "A Knowledge payload binding does not name its exact Control preparation operation.",
                    )
                })?;
            validate_binding_origin(&self.authority, origin, binding)?;
        }

        for (key, preparations) in &history.preparations {
            for preparation in preparations {
                if preparation.status != ControlEffectStatus::Applied {
                    continue;
                }
                match payload.get(key).copied() {
                    Some(binding)
                        if binding.observation.state == OkfKnowledgeObservedState::Promoted =>
                    {
                        validate_applied_preparation(preparation, binding)?;
                    }
                    Some(binding)
                        if binding.observation.state == OkfKnowledgeObservedState::Removed =>
                    {
                        history.require_effective_removal(key, Some(binding))?;
                    }
                    None => history.require_effective_removal(key, None)?,
                    Some(_) => {
                        return Err(reconciliation_error(
                            "An applied Control Knowledge preparation is not retained as promoted or removed payload evidence.",
                        ));
                    }
                }
            }
        }

        for (key, removals) in &history.removals {
            for removal in removals {
                if removal.status != ControlEffectStatus::Applied {
                    continue;
                }
                if let Some(binding) = payload.get(key).copied() {
                    if binding.observation.state != OkfKnowledgeObservedState::Removed {
                        return Err(reconciliation_error(
                            "An applied Control Knowledge removal still has non-removed payload state.",
                        ));
                    }
                    validate_applied_removal(removal, binding)?;
                }
            }
        }
        Ok(())
    }
}

struct KnowledgeEffectHistory<'a> {
    preparations: BTreeMap<KnowledgeIncarnation, Vec<&'a ControlEffectRecord>>,
    removals: BTreeMap<KnowledgeIncarnation, Vec<&'a ControlEffectRecord>>,
}

impl<'a> KnowledgeEffectHistory<'a> {
    fn new(authority: &'a ControlStoreAuthority) -> UseResult<Self> {
        let mut history = Self {
            preparations: BTreeMap::new(),
            removals: BTreeMap::new(),
        };
        for record in &authority.effects {
            if record.intent.owner != ControlEffectOwner::KnowledgeHost {
                continue;
            }
            let key = KnowledgeIncarnation::from_effect(record)?;
            match record.intent.kind {
                ControlEffectKind::SurfacePrepare => {
                    history.preparations.entry(key).or_default().push(record);
                }
                ControlEffectKind::SurfaceRemove => {
                    history.removals.entry(key).or_default().push(record);
                }
                ControlEffectKind::SurfaceStop => {}
                ControlEffectKind::CapabilityCutover | ControlEffectKind::CallsDrain => {
                    return Err(reconciliation_error(
                        "The Control export assigned an unsupported effect to the Knowledge owner.",
                    ));
                }
            }
        }
        Ok(history)
    }

    fn require_effective_removal(
        &self,
        key: &KnowledgeIncarnation,
        retained: Option<&OkfKnowledgeBinding>,
    ) -> UseResult<()> {
        let removal = self
            .removals
            .get(key)
            .and_then(|records| {
                records.iter().copied().find(|record| {
                    matches!(
                        record.status,
                        ControlEffectStatus::Applied
                            | ControlEffectStatus::Claimed
                            | ControlEffectStatus::Unknown
                    )
                })
            })
            .ok_or_else(|| {
                reconciliation_error(
                    "Applied Control Knowledge payload disappeared without an exact removal effect.",
                )
            })?;
        if removal.status == ControlEffectStatus::Applied {
            if let Some(binding) = retained {
                validate_applied_removal(removal, binding)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct KnowledgeIncarnation {
    package_id: String,
    lifecycle_generation: u64,
    package_digest: String,
    manifest_digest: String,
    surface_id: String,
}

impl KnowledgeIncarnation {
    fn from_effect(record: &ControlEffectRecord) -> UseResult<Self> {
        let ControlEffectSubject::Surface {
            package_id,
            lifecycle_generation,
            package_digest,
            manifest_digest,
            surface,
            ..
        } = &record.intent.subject
        else {
            return Err(reconciliation_error(
                "A Control Knowledge effect does not target one surface incarnation.",
            ));
        };
        if surface.kind != PluginSurfaceKind::Okf {
            return Err(reconciliation_error(
                "A Control Knowledge effect targets a non-OKF surface.",
            ));
        }
        Ok(Self {
            package_id: package_id.clone(),
            lifecycle_generation: *lifecycle_generation,
            package_digest: package_digest.clone(),
            manifest_digest: manifest_digest.clone(),
            surface_id: surface.id.clone(),
        })
    }

    fn from_binding(binding: &OkfKnowledgeBinding) -> Self {
        Self {
            package_id: binding.receipt.surface.package_id.clone(),
            lifecycle_generation: binding.receipt.generation,
            package_digest: binding.receipt.package_digest.clone(),
            manifest_digest: binding.receipt.manifest_digest.clone(),
            surface_id: binding.receipt.surface.surface.id.clone(),
        }
    }
}

fn validate_binding_origin(
    authority: &ControlStoreAuthority,
    origin: &ControlEffectRecord,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    let generation_index = origin
        .intent
        .installation_generation
        .checked_sub(1)
        .and_then(|generation| usize::try_from(generation).ok())
        .ok_or_else(|| reconciliation_error("The Knowledge effect generation is invalid."))?;
    let generation = authority
        .generations
        .get(generation_index)
        .filter(|generation| {
            generation.snapshot.generation == origin.intent.installation_generation
        })
        .ok_or_else(|| {
            reconciliation_error(
                "The Knowledge binding origin has no exact Control installation generation.",
            )
        })?;
    let expected_bundle = generation
        .snapshot
        .package_selection(&binding.receipt.surface.package_id)
        .and_then(|package| {
            package
                .package
                .catalog
                .record
                .surfaces
                .iter()
                .find(|surface| {
                    surface.kind == PluginSurfaceKind::Okf
                        && surface.id == binding.receipt.surface.surface.id
                })
        })
        .and_then(|surface| surface.okf_bundle.as_ref())
        .ok_or_else(|| {
            reconciliation_error(
                "The Knowledge binding origin has no exact Control OKF bundle contract.",
            )
        })?;
    if expected_bundle != &binding.receipt.bundle
        || binding.receipt.staged_at_ms < generation.committed_at_ms
    {
        return Err(reconciliation_error(
            "The Knowledge binding differs from its committed Control generation.",
        ));
    }

    match binding.observation.state {
        OkfKnowledgeObservedState::Promoted => match origin.status {
            ControlEffectStatus::Applied => validate_applied_preparation(origin, binding),
            ControlEffectStatus::Claimed | ControlEffectStatus::Unknown => Ok(()),
            ControlEffectStatus::Pending | ControlEffectStatus::Rejected => {
                Err(reconciliation_error(
                    "Promoted Knowledge payload contradicts its Control preparation outcome.",
                ))
            }
        },
        OkfKnowledgeObservedState::Staged | OkfKnowledgeObservedState::Failed => {
            if matches!(
                origin.status,
                ControlEffectStatus::Applied | ControlEffectStatus::Pending
            ) {
                Err(reconciliation_error(
                    "Nonterminal Knowledge payload contradicts its Control preparation outcome.",
                ))
            } else {
                Ok(())
            }
        }
        OkfKnowledgeObservedState::Removed => {
            if origin.status == ControlEffectStatus::Pending {
                Err(reconciliation_error(
                    "Removed Knowledge payload has a Control preparation that never started.",
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn validate_applied_preparation(
    record: &ControlEffectRecord,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    let projection = OkfCapabilityProjection::from_promoted(&binding.receipt, &binding.observation)
        .map_err(wrap_reconciliation_error)?;
    let observation_digest = binding
        .observation
        .descriptor_digest()
        .map_err(wrap_reconciliation_error)?;
    let projection_digest = projection
        .descriptor_digest()
        .map_err(wrap_reconciliation_error)?;
    let Some(application) = &record.application else {
        return Err(reconciliation_error(
            "An applied Control Knowledge preparation omitted typed evidence.",
        ));
    };
    let ControlAppliedEffectEvidence::KnowledgeHost {
        state,
        receipt_digest,
        projection_digest: retained_projection,
    } = &application.evidence
    else {
        return Err(reconciliation_error(
            "An applied Control Knowledge preparation carries another owner's evidence.",
        ));
    };
    if *state != ControlSurfaceObservationState::Prepared
        || receipt_digest != &observation_digest
        || retained_projection.as_deref() != Some(projection_digest.as_str())
        || record
            .observed_at_ms
            .is_none_or(|observed| binding.observation.observed_at_ms > observed)
    {
        return Err(reconciliation_error(
            "The promoted Knowledge payload differs from its applied Control evidence.",
        ));
    }
    Ok(())
}

fn validate_applied_removal(
    record: &ControlEffectRecord,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    let observation_digest = binding
        .observation
        .descriptor_digest()
        .map_err(wrap_reconciliation_error)?;
    let Some(application) = &record.application else {
        return Err(reconciliation_error(
            "An applied Control Knowledge removal omitted typed evidence.",
        ));
    };
    let ControlAppliedEffectEvidence::KnowledgeHost {
        state,
        receipt_digest,
        projection_digest,
    } = &application.evidence
    else {
        return Err(reconciliation_error(
            "An applied Control Knowledge removal carries another owner's evidence.",
        ));
    };
    if *state != ControlSurfaceObservationState::Removed
        || receipt_digest != &observation_digest
        || projection_digest.is_some()
        || record
            .observed_at_ms
            .is_none_or(|observed| binding.observation.observed_at_ms > observed)
    {
        return Err(reconciliation_error(
            "The removed Knowledge payload differs from its applied Control evidence.",
        ));
    }
    Ok(())
}

fn wrap_reconciliation_error(error: UseError) -> UseError {
    reconciliation_error(format!(
        "Control Knowledge history reconciliation failed: {}",
        error.message
    ))
}

fn reconciliation_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.knowledge_payload_snapshot_invalid",
        message,
    )
}
