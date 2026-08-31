use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{InstallationId, UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::aggregate::validate_operation_record;
use super::model::{
    corruption_error, valid_machine_id, valid_sha256, ControlCapabilityStatus, ControlEffectRecord,
    ControlEffectStatus, ControlGeneration, ControlOperationRecord, ControlOperationStatus,
    ControlProjectionHistory, ControlStoreAuthority, ControlTransition,
};
use super::schema::{ControlStoreMetadata, CONTROL_STORE_SCHEMA_VERSION};

const CONTROL_STORE_EXPORT_SCHEMA: &str = "a3s.use.control-store-export.v9";
const MAX_CONTROL_STORE_EXPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPORTED_GENERATIONS: usize = 4096;
const MAX_EXPORTED_OPERATIONS: usize = 8192;
const MAX_EXPORTED_EFFECTS: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlStoreExport {
    schema: String,
    store_schema_version: u32,
    pub(super) installation: InstallationId,
    pub(super) current_generation: u64,
    pub(super) published_capability_generation: u64,
    pub(super) authority: ControlStoreAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifiedControlStoreExport {
    pub(super) export: ControlStoreExport,
    pub(super) descriptor_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeneratedControlStoreExport {
    bytes: Vec<u8>,
    current_generation: u64,
    descriptor_digest: String,
}

impl GeneratedControlStoreExport {
    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub(super) fn into_parts(self) -> (Vec<u8>, u64, String) {
        (self.bytes, self.current_generation, self.descriptor_digest)
    }
}

pub(super) fn encode(
    metadata: &ControlStoreMetadata,
    authority: ControlStoreAuthority,
) -> UseResult<GeneratedControlStoreExport> {
    let export = ControlStoreExport {
        schema: CONTROL_STORE_EXPORT_SCHEMA.to_string(),
        store_schema_version: metadata.schema_version,
        installation: metadata.installation.clone(),
        current_generation: metadata.current_generation,
        published_capability_generation: metadata.published_capability_generation,
        authority,
    };
    validate_export(&export)?;
    let bytes = canonical_json(&export)?;
    if bytes.len() > MAX_CONTROL_STORE_EXPORT_BYTES {
        return Err(export_error(
            "The canonical Control Store export exceeds its byte bound.",
        ));
    }
    Ok(GeneratedControlStoreExport {
        descriptor_digest: sha256_digest(&bytes),
        current_generation: metadata.current_generation,
        bytes,
    })
}

pub(super) fn verify(
    bytes: &[u8],
    expected_installation: &InstallationId,
) -> UseResult<VerifiedControlStoreExport> {
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_STORE_EXPORT_BYTES {
        return Err(export_error(
            "The Control Store export is empty or exceeds its byte bound.",
        ));
    }
    let export: ControlStoreExport = serde_json::from_slice(bytes)
        .map_err(|_| export_error("The Control Store export is not valid schema-v9 JSON."))?;
    validate_export(&export)?;
    let canonical = canonical_json(&export)?;
    if canonical != bytes {
        return Err(export_error(
            "The Control Store export is not in canonical JSON form.",
        ));
    }
    if export.installation != *expected_installation {
        return Err(identity_error());
    }
    Ok(VerifiedControlStoreExport {
        descriptor_digest: sha256_digest(&canonical),
        export,
    })
}

pub(super) fn validate_for_restore(
    export: &ControlStoreExport,
    expected_installation: &InstallationId,
) -> UseResult<()> {
    validate_export(export)?;
    if export.installation != *expected_installation {
        return Err(identity_error());
    }
    Ok(())
}

fn validate_export(export: &ControlStoreExport) -> UseResult<()> {
    if export.schema != CONTROL_STORE_EXPORT_SCHEMA
        || export.store_schema_version != CONTROL_STORE_SCHEMA_VERSION
        || export.installation.validate().is_err()
        || export.authority.generations.len() > MAX_EXPORTED_GENERATIONS
        || export.authority.operations.len() > MAX_EXPORTED_OPERATIONS
        || export.authority.effects.len() > MAX_EXPORTED_EFFECTS
    {
        return Err(export_error(
            "The Control Store export identity, schema, or aggregate bounds are invalid.",
        ));
    }
    validate_authority(export).map_err(|error| {
        export_error(format!(
            "The Control Store authority export is invalid: {}",
            error.message
        ))
    })
}

fn validate_authority(export: &ControlStoreExport) -> UseResult<()> {
    if export
        .authority
        .generations
        .windows(2)
        .any(|pair| pair[0].snapshot.generation >= pair[1].snapshot.generation)
        || export
            .authority
            .operations
            .windows(2)
            .any(|pair| pair[0].reviewed.operation_id() >= pair[1].reviewed.operation_id())
        || export.authority.effects.windows(2).any(|pair| {
            (pair[0].operation_id.as_str(), pair[0].intent.sequence)
                >= (pair[1].operation_id.as_str(), pair[1].intent.sequence)
        })
    {
        return Err(corruption_error(
            "Control Store export inventories are not sorted uniquely.",
        ));
    }

    let operations = export
        .authority
        .operations
        .iter()
        .map(|operation| {
            validate_operation_record(operation)?;
            operation
                .reviewed
                .validate_for_installation(&export.installation)?;
            Ok((operation.reviewed.operation_id(), operation))
        })
        .collect::<UseResult<BTreeMap<_, _>>>()?;
    let effects = group_effects(&export.authority.effects, &operations)?;
    let expected_generation_count = usize::try_from(export.current_generation)
        .map_err(|_| corruption_error("The Control Store generation count is invalid."))?;
    if export.authority.generations.len() != expected_generation_count {
        return Err(corruption_error(
            "Control Store generations are not a complete contiguous history.",
        ));
    }

    let mut committed_operations = BTreeSet::new();
    let mut published_cursor = 0_u64;
    let mut pending_count = 0_usize;
    let mut prior_generation: Option<&ControlGeneration> = None;
    let mut projection_history = ControlProjectionHistory::default();
    for (index, generation) in export.authority.generations.iter().enumerate() {
        let expected_generation = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| corruption_error("The Control Store generation is exhausted."))?;
        if generation.snapshot.generation != expected_generation
            || generation.snapshot.installation != export.installation
            || generation.snapshot.descriptor_digest()? != generation.snapshot_digest
            || !valid_sha256(&generation.capability.descriptor_digest)
        {
            return Err(corruption_error(
                "A Control Store generation does not match its identity or digest.",
            ));
        }
        let operation = operations
            .get(generation.operation_id.as_str())
            .copied()
            .ok_or_else(|| {
                corruption_error("A Control Store generation has no reviewed operation.")
            })?;
        if !committed_operations.insert(operation.reviewed.operation_id())
            || operation.reviewed.expected_generation + 1 != expected_generation
            || operation.reviewed.expected_capability_generation != published_cursor
            || generation.capability.generation
                != operation.reviewed.target_capability_generation()?
            || generation.committed_at_ms != operation.committed_at_ms.unwrap_or(0)
        {
            return Err(corruption_error(
                "A Control Store generation does not match its operation cursors.",
            ));
        }
        let operation_effects = effects
            .get(operation.reviewed.operation_id())
            .cloned()
            .unwrap_or_default();
        let transition = ControlTransition {
            operation_id: generation.operation_id.clone(),
            plan_digest: operation.reviewed.plan_digest().to_string(),
            snapshot: generation.snapshot.clone(),
            package_lifecycles: generation.package_lifecycles.clone(),
            grants: generation.grants.clone(),
            provider_selections: generation.provider_selections.clone(),
            capability: generation.capability.clone(),
            effects: operation_effects
                .iter()
                .map(|effect| effect.intent.clone())
                .collect(),
            committed_at_ms: generation.committed_at_ms,
        };
        transition.validate(&export.installation, &operation.reviewed)?;
        transition.validate_projection(
            &operation.reviewed,
            prior_generation,
            &projection_history,
        )?;
        transition.validate_effect_references(prior_generation)?;
        operation.reviewed.validate_snapshot_transition(
            prior_generation.map(|prior| &prior.snapshot),
            &generation.snapshot,
        )?;
        validate_effect_sequence(operation, &operation_effects)?;
        projection_history.observe(generation)?;

        let cutover = operation_effects
            .iter()
            .find(|effect| effect.intent.kind == super::model::ControlEffectKind::CapabilityCutover)
            .copied()
            .ok_or_else(|| {
                corruption_error("A committed Control Store operation has no capability cutover.")
            })?;
        let cutover_applied = cutover.status == ControlEffectStatus::Applied;
        if cutover_applied {
            published_cursor = published_cursor
                .checked_add(1)
                .ok_or_else(|| corruption_error("The capability generation is exhausted."))?;
            let expected_status = if published_cursor == export.published_capability_generation {
                ControlCapabilityStatus::Published
            } else {
                ControlCapabilityStatus::Retired
            };
            if generation.capability_status != expected_status
                || generation.capability_published_at_ms != cutover.observed_at_ms
                || generation
                    .capability_published_at_ms
                    .is_none_or(|time| time < generation.committed_at_ms)
            {
                return Err(corruption_error(
                    "An applied Control Store capability cutover has invalid publication state.",
                ));
            }
        } else if generation.capability_published_at_ms.is_some()
            || !matches!(
                generation.capability_status,
                ControlCapabilityStatus::Candidate | ControlCapabilityStatus::Abandoned
            )
        {
            return Err(corruption_error(
                "An unapplied Control Store capability cutover has publication evidence.",
            ));
        }

        match operation.status {
            ControlOperationStatus::Completed if cutover_applied => {}
            ControlOperationStatus::Rejected => {
                if cutover_applied
                    || generation.capability_status != ControlCapabilityStatus::Abandoned
                {
                    return Err(corruption_error(
                        "A rejected Control Store generation did not abandon an unpublished candidate.",
                    ));
                }
            }
            ControlOperationStatus::EffectsPending => {
                pending_count += 1;
                let expected_capability_status = if cutover_applied {
                    ControlCapabilityStatus::Published
                } else {
                    ControlCapabilityStatus::Candidate
                };
                if expected_generation != export.current_generation
                    || generation.capability_status != expected_capability_status
                {
                    return Err(corruption_error(
                        "An effect-pending Control Store generation is not the active candidate or published cutover.",
                    ));
                }
            }
            ControlOperationStatus::Completed => {
                return Err(corruption_error(
                    "A completed Control Store operation has no applied capability cutover.",
                ));
            }
            ControlOperationStatus::Reviewed | ControlOperationStatus::Cancelled => {
                return Err(corruption_error(
                    "An uncommitted Control Store operation owns a generation.",
                ))
            }
        }
        prior_generation = Some(generation);
    }
    if published_cursor != export.published_capability_generation || pending_count > 1 {
        return Err(corruption_error(
            "The exported Control Store cursors do not match capability history.",
        ));
    }

    for operation in &export.authority.operations {
        let committed = committed_operations.contains(operation.reviewed.operation_id());
        if committed
            != matches!(
                operation.status,
                ControlOperationStatus::EffectsPending
                    | ControlOperationStatus::Completed
                    | ControlOperationStatus::Rejected
            )
            || (!committed && effects.contains_key(operation.reviewed.operation_id()))
        {
            return Err(corruption_error(
                "A Control Store operation history has inconsistent generation ownership.",
            ));
        }
    }
    Ok(())
}

fn group_effects<'a>(
    records: &'a [ControlEffectRecord],
    operations: &BTreeMap<&str, &ControlOperationRecord>,
) -> UseResult<BTreeMap<&'a str, Vec<&'a ControlEffectRecord>>> {
    let mut grouped = BTreeMap::<&str, Vec<&ControlEffectRecord>>::new();
    for record in records {
        validate_effect_record(record)?;
        if !operations.contains_key(record.operation_id.as_str()) {
            return Err(corruption_error(
                "A Control Store effect has no reviewed operation.",
            ));
        }
        grouped
            .entry(record.operation_id.as_str())
            .or_default()
            .push(record);
    }
    Ok(grouped)
}

fn validate_effect_sequence(
    operation: &ControlOperationRecord,
    effects: &[&ControlEffectRecord],
) -> UseResult<()> {
    let committed_at_ms = operation.committed_at_ms.ok_or_else(|| {
        corruption_error("A Control Store effect sequence has no committed transition.")
    })?;
    let mut latest_event_at_ms = committed_at_ms;
    let mut required_rejected = false;
    let mut unfinished_seen = false;
    let cutover_sequence = effects
        .iter()
        .find(|effect| effect.intent.kind == super::model::ControlEffectKind::CapabilityCutover)
        .map(|effect| effect.intent.sequence);
    let cutover_applied = effects.iter().any(|effect| {
        effect.intent.kind == super::model::ControlEffectKind::CapabilityCutover
            && effect.status == ControlEffectStatus::Applied
    });
    let mut required_rejected_after_cutover = false;
    for (index, effect) in effects.iter().enumerate() {
        if usize::try_from(effect.intent.sequence).ok() != Some(index) {
            return Err(corruption_error(
                "A Control Store effect sequence is not contiguous.",
            ));
        }
        if let Some(observed_at_ms) = effect.observed_at_ms {
            if observed_at_ms < latest_event_at_ms {
                return Err(corruption_error(
                    "Control Store effect observations are not ordered after commit.",
                ));
            }
            latest_event_at_ms = observed_at_ms;
        }
        let finished = effect.status == ControlEffectStatus::Applied
            || (effect.status == ControlEffectStatus::Rejected && !effect.intent.required);
        if unfinished_seen && effect.status != ControlEffectStatus::Pending {
            return Err(corruption_error(
                "Control Store effects advanced outside canonical sequence order.",
            ));
        }
        unfinished_seen |= !finished;
        required_rejected |=
            effect.status == ControlEffectStatus::Rejected && effect.intent.required;
        required_rejected_after_cutover |= effect.status == ControlEffectStatus::Rejected
            && effect.intent.required
            && cutover_sequence.is_some_and(|cutover| effect.intent.sequence > cutover);
    }
    if operation
        .completed_at_ms
        .is_some_and(|completed_at_ms| completed_at_ms < latest_event_at_ms)
    {
        return Err(corruption_error(
            "A Control Store operation completed before its latest effect observation.",
        ));
    }
    match operation.status {
        ControlOperationStatus::Completed
            if effects.iter().all(|effect| {
                effect.status == ControlEffectStatus::Applied
                    || (effect.status == ControlEffectStatus::Rejected && !effect.intent.required)
            }) => {}
        ControlOperationStatus::Rejected if required_rejected && !cutover_applied => {}
        ControlOperationStatus::EffectsPending
            if !required_rejected || (cutover_applied && required_rejected_after_cutover) => {}
        _ => {
            return Err(corruption_error(
                "Control Store effect outcomes do not match operation status.",
            ))
        }
    }
    Ok(())
}

fn validate_effect_record(record: &ControlEffectRecord) -> UseResult<()> {
    if !valid_machine_id(&record.operation_id)
        || !valid_sha256(&record.intent.idempotency_key)
        || record.intent.installation.validate().is_err()
        || !valid_sha256(&record.intent.plan_digest)
        || !record.intent.subject.matches_kind(record.intent.kind)
        || !record.intent.subject.validate_identity()
        || !record
            .intent
            .owner
            .validate_shape(&record.intent.subject, record.intent.kind)
        || !valid_sha256(&record.payload_digest)
        || record.intent.installation_generation == 0
    {
        return Err(corruption_error(
            "A Control Store effect record has invalid identity evidence.",
        ));
    }
    let payload_digest = record.intent.descriptor_digest().map_err(|_| {
        corruption_error("A Control Store effect payload is not canonically encodable.")
    })?;
    if payload_digest != record.payload_digest {
        return Err(corruption_error(
            "A Control Store effect payload digest does not match its canonical command.",
        ));
    }
    let has_claim = record.attempt > 0
        && record.claim_owner.as_deref().is_some_and(valid_machine_id)
        && record.claim_token.as_deref().is_some_and(valid_machine_id)
        && record.lease_until_ms.is_some_and(|lease| lease > 0);
    let valid = match record.status {
        ControlEffectStatus::Pending => {
            record.attempt == 0
                && record.claim_owner.is_none()
                && record.claim_token.is_none()
                && record.lease_until_ms.is_none()
                && record.application.is_none()
                && record.evidence_digest.is_none()
                && record.error_code.is_none()
                && record.observed_at_ms.is_none()
        }
        ControlEffectStatus::Claimed => {
            has_claim
                && record.application.is_none()
                && record.evidence_digest.is_none()
                && record.error_code.is_none()
                && record.observed_at_ms.is_none()
        }
        ControlEffectStatus::Applied => {
            has_claim
                && record.application.as_ref().is_some_and(|application| {
                    application.validate_for(&record.intent).is_ok()
                        && application.descriptor_digest().is_ok_and(|digest| {
                            record.evidence_digest.as_deref() == Some(digest.as_str())
                        })
                })
                && record.evidence_digest.as_deref().is_some_and(valid_sha256)
                && record.error_code.is_none()
                && valid_observation_time(record)
        }
        ControlEffectStatus::Rejected | ControlEffectStatus::Unknown => {
            has_claim
                && record.application.is_none()
                && record.evidence_digest.as_deref().is_some_and(valid_sha256)
                && record
                    .error_code
                    .as_deref()
                    .is_some_and(|code| !code.is_empty() && code.len() <= 128)
                && valid_observation_time(record)
        }
    };
    if !valid {
        return Err(corruption_error(
            "A Control Store effect status does not match its claim and observation evidence.",
        ));
    }
    Ok(())
}

fn valid_observation_time(record: &ControlEffectRecord) -> bool {
    record
        .observed_at_ms
        .zip(record.lease_until_ms)
        .is_some_and(|(observed, lease)| observed > 0 && observed <= lease)
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|error| export_error(format!("Canonical export encoding failed: {error}")))?;
    Ok(bytes)
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn identity_error() -> UseError {
    UseError::new(
        "use.control_store.identity_mismatch",
        "The Control Store export belongs to a different installation.",
    )
}

fn export_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.export_invalid", message)
}
