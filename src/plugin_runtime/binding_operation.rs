use a3s_runtime::contract::{RuntimeRemoval, RuntimeUnitClass};
use a3s_runtime::ProviderId;
use a3s_use_core::{
    PlanQualifiedSurfaceRef, PlannedProviderEvidence, PluginOperationPlan, UseError, UseResult,
    MAX_PLUGIN_PLAN_ITEMS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    RuntimeBindingReceipt, RuntimeMcpInitializeEvidence, RuntimePreparedTaskBinding,
    RuntimeProviderSelection, RuntimeServiceBindingReceipt, RuntimeServiceReadinessEvidence,
    RuntimeSurfaceContract, RUNTIME_SERVICE_BINDING_SCHEMA, RUNTIME_TASK_BINDING_SCHEMA,
};

pub const RUNTIME_BINDING_OPERATION_SCHEMA: &str = "a3s.use.plugin-runtime-binding-operation.v1";
pub const RUNTIME_BINDING_CUTOVER_SCHEMA: &str = "a3s.use.plugin-runtime-binding-cutover.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeBindingOperationPhase {
    IntentRecorded,
    Preparing,
    Prepared,
    Publishing,
    BindingsPublished,
    CutoverCommitted,
    Retiring,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBindingCandidatePlan {
    pub surface: PlanQualifiedSurfaceRef,
    pub package_digest: String,
    pub scope_id: String,
    pub descriptor_digest: String,
    pub provider: PlannedProviderEvidence,
    pub generation: u64,
    pub kind: RuntimeBindingCandidateKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "bindingKind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeBindingCandidateKind {
    Task {
        artifact_digest: String,
        artifact_media_type: String,
    },
    Service {
        unit_id: String,
        spec_digest: String,
        contract: RuntimeSurfaceContract,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBindingOperationIntent {
    pub operation_id: String,
    pub plan_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_change_set_digest: Option<String>,
    pub scope_id: String,
    pub state_revision_before: u64,
    pub state_revision_after: u64,
    pub capability_generation_before: u64,
    pub capability_generation_after: u64,
    pub transitioned_at_ms: u64,
    pub candidates: Vec<RuntimeBindingCandidatePlan>,
    pub retirements: Vec<RuntimeBindingReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBindingCutoverEvidence {
    pub schema: String,
    pub state_revision_before: u64,
    pub state_revision_after: u64,
    pub capability_generation_before: u64,
    pub capability_generation_after: u64,
    pub capability_snapshot_digest: String,
    pub committed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBindingRetirementEvidence {
    receipt: RuntimeBindingReceipt,
    retired_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    removal: Option<RuntimeRemoval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBindingOperationJournal {
    pub schema: String,
    pub intent_digest: String,
    pub intent: RuntimeBindingOperationIntent,
    pub phase: RuntimeBindingOperationPhase,
    pub prepared: Vec<RuntimeBindingReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover: Option<RuntimeBindingCutoverEvidence>,
    pub retired: Vec<RuntimeBindingRetirementEvidence>,
}

impl RuntimeBindingCandidatePlan {
    fn from_selection(selection: &RuntimeProviderSelection) -> UseResult<Vec<Self>> {
        let mut candidates = selection
            .surfaces()
            .iter()
            .map(|selected| {
                let plan = selected.plan();
                let spec = plan.spec();
                let kind = match spec.class {
                    RuntimeUnitClass::Task => RuntimeBindingCandidateKind::Task {
                        artifact_digest: spec.artifact.digest.clone(),
                        artifact_media_type: spec.artifact.media_type.clone(),
                    },
                    RuntimeUnitClass::Service => RuntimeBindingCandidateKind::Service {
                        unit_id: spec.unit_id.clone(),
                        spec_digest: spec
                            .digest()
                            .map_err(super::model::runtime_contract_error)?,
                        contract: plan.contract().clone(),
                    },
                };
                let candidate = Self {
                    surface: plan.surface(),
                    package_digest: plan.context().package_digest().to_string(),
                    scope_id: plan.context().scope_id().to_string(),
                    descriptor_digest: plan.descriptor_digest().to_string(),
                    provider: selected.provider().clone(),
                    generation: plan.context().generation(),
                    kind,
                };
                candidate.validate()?;
                Ok(candidate)
            })
            .collect::<UseResult<Vec<_>>>()?;
        candidates.sort_by(|left, right| left.surface.cmp(&right.surface));
        Ok(candidates)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.provider.surface != self.surface
            || self.generation == 0
            || !super::model::valid_sha256(&self.package_digest)
            || !super::model::valid_sha256(&self.descriptor_digest)
            || !super::model::valid_sha256(&self.provider.capability_digest)
            || !super::model::valid_sha256(&self.provider.semantics_profile_digest)
            || ProviderId::parse(&self.provider.provider_id).is_err()
            || !super::model::valid_machine_id(&self.provider.provider_build_id)
        {
            return Err(operation_error(
                "A Runtime binding candidate has invalid package or provider evidence.",
            ));
        }
        validate_candidate_shape(self)
    }

    pub fn matches_receipt(&self, receipt: &RuntimeBindingReceipt) -> UseResult<bool> {
        receipt.validate()?;
        let common = receipt.surface() == &self.surface
            && receipt.package_digest() == self.package_digest
            && receipt.scope_id() == self.scope_id
            && receipt.provider_id() == self.provider.provider_id
            && receipt.provider_build_id() == self.provider.provider_build_id
            && receipt.capability_digest() == self.provider.capability_digest
            && receipt.semantics_profile_digest() == self.provider.semantics_profile_digest
            && receipt.generation() == self.generation;
        if !common {
            return Ok(false);
        }
        Ok(match (&self.kind, receipt) {
            (
                RuntimeBindingCandidateKind::Task {
                    artifact_digest,
                    artifact_media_type,
                },
                RuntimeBindingReceipt::Task(receipt),
            ) => {
                receipt.descriptor_digest == self.descriptor_digest
                    && receipt.enforcement == self.provider.enforcement
                    && receipt.artifact_digest == *artifact_digest
                    && receipt.artifact_media_type == *artifact_media_type
            }
            (
                RuntimeBindingCandidateKind::Service {
                    unit_id,
                    spec_digest,
                    contract,
                },
                RuntimeBindingReceipt::Service(receipt),
            ) => {
                receipt.descriptor_digest == self.descriptor_digest
                    && receipt.enforcement == self.provider.enforcement
                    && receipt.unit_id == *unit_id
                    && receipt.spec_digest == *spec_digest
                    && receipt.contract == *contract
            }
            _ => false,
        })
    }
}

impl RuntimeBindingOperationIntent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation_id: impl Into<String>,
        plan_digest: impl Into<String>,
        grant_change_set_digest: Option<String>,
        scope_id: impl Into<String>,
        state_revision_before: u64,
        capability_generation_before: u64,
        transitioned_at_ms: u64,
        mut candidates: Vec<RuntimeBindingCandidatePlan>,
        mut retirements: Vec<RuntimeBindingReceipt>,
    ) -> UseResult<Self> {
        candidates.sort_by(|left, right| left.surface.cmp(&right.surface));
        retirements.sort_by(|left, right| left.surface().cmp(right.surface()));
        let intent = Self {
            operation_id: operation_id.into(),
            plan_digest: plan_digest.into(),
            grant_change_set_digest,
            scope_id: scope_id.into(),
            state_revision_before,
            state_revision_after: state_revision_before
                .checked_add(1)
                .ok_or_else(revision_exhausted)?,
            capability_generation_before,
            capability_generation_after: capability_generation_before
                .checked_add(1)
                .ok_or_else(generation_exhausted)?,
            transitioned_at_ms,
            candidates,
            retirements,
        };
        intent.validate()?;
        Ok(intent)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_selection(
        operation_id: impl Into<String>,
        plan_digest: impl Into<String>,
        grant_change_set_digest: Option<String>,
        scope_id: impl Into<String>,
        state_revision_before: u64,
        capability_generation_before: u64,
        transitioned_at_ms: u64,
        selection: &RuntimeProviderSelection,
        retirements: Vec<RuntimeBindingReceipt>,
    ) -> UseResult<Self> {
        Self::new(
            operation_id,
            plan_digest,
            grant_change_set_digest,
            scope_id,
            state_revision_before,
            capability_generation_before,
            transitioned_at_ms,
            RuntimeBindingCandidatePlan::from_selection(selection)?,
            retirements,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(&self.operation_id)?;
        if !super::model::valid_sha256(&self.plan_digest)
            || self
                .grant_change_set_digest
                .as_deref()
                .is_some_and(|digest| !super::model::valid_sha256(digest))
            || !super::model::valid_machine_id(&self.scope_id)
            || self.state_revision_before == 0
            || self.state_revision_before.checked_add(1) != Some(self.state_revision_after)
            || self.capability_generation_before == 0
            || self.capability_generation_before.checked_add(1)
                != Some(self.capability_generation_after)
            || self.transitioned_at_ms == 0
            || (self.candidates.is_empty() && self.retirements.is_empty())
            || self.candidates.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.retirements.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .candidates
                .windows(2)
                .any(|pair| pair[0].surface >= pair[1].surface)
            || self
                .retirements
                .windows(2)
                .any(|pair| pair[0].surface() >= pair[1].surface())
        {
            return Err(operation_error(
                "A Runtime binding operation intent has invalid identity, revision, or ordering.",
            ));
        }
        for candidate in &self.candidates {
            candidate.validate()?;
            if candidate.scope_id != self.scope_id
                || candidate.generation != self.capability_generation_after
            {
                return Err(operation_error(
                    "A Runtime binding candidate does not match the operation scope or next generation.",
                ));
            }
        }
        for retirement in &self.retirements {
            retirement.validate()?;
            if retirement.scope_id() != self.scope_id
                || retirement.generation() > self.capability_generation_before
            {
                return Err(operation_error(
                    "A Runtime binding retirement exceeds the operation's prior scope or generation.",
                ));
            }
        }
        for candidate in &self.candidates {
            if let Ok(index) = self
                .retirements
                .binary_search_by(|receipt| receipt.surface().cmp(&candidate.surface))
            {
                if candidate.generation <= self.retirements[index].generation() {
                    return Err(operation_error(
                        "A replacement Runtime binding must advance its exact surface generation.",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| {
            operation_error(format!(
                "Failed to encode Runtime binding operation intent: {error}"
            ))
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

impl RuntimeBindingCutoverEvidence {
    /// Derive the Runtime checkpoint from the same capability publication
    /// already accepted by the durable workspace-grant sub-saga.
    pub fn from_grant_cutover(
        intent: &RuntimeBindingOperationIntent,
        grant: &a3s_use_extension::WorkspaceGrantCutoverEvidence,
    ) -> UseResult<Self> {
        if grant.schema != a3s_use_extension::WORKSPACE_GRANT_CUTOVER_SCHEMA
            || grant.capability_generation_before != intent.capability_generation_before
            || grant.capability_generation_after != intent.capability_generation_after
            || !super::model::valid_sha256(&grant.capability_snapshot_digest)
            || grant.committed_at_ms < intent.transitioned_at_ms
        {
            return Err(operation_error(
                "Workspace-grant cutover evidence does not match the Runtime binding operation.",
            ));
        }
        let evidence = Self {
            schema: RUNTIME_BINDING_CUTOVER_SCHEMA.to_string(),
            state_revision_before: intent.state_revision_before,
            state_revision_after: intent.state_revision_after,
            capability_generation_before: grant.capability_generation_before,
            capability_generation_after: grant.capability_generation_after,
            capability_snapshot_digest: grant.capability_snapshot_digest.clone(),
            committed_at_ms: grant.committed_at_ms,
        };
        evidence.validate_against(intent)?;
        Ok(evidence)
    }

    pub fn validate_against(&self, intent: &RuntimeBindingOperationIntent) -> UseResult<()> {
        if self.schema != RUNTIME_BINDING_CUTOVER_SCHEMA
            || self.state_revision_before != intent.state_revision_before
            || self.state_revision_after != intent.state_revision_after
            || self.capability_generation_before != intent.capability_generation_before
            || self.capability_generation_after != intent.capability_generation_after
            || !super::model::valid_sha256(&self.capability_snapshot_digest)
            || self.committed_at_ms < intent.transitioned_at_ms
        {
            return Err(operation_error(
                "Runtime binding cutover evidence does not match the immutable operation intent.",
            ));
        }
        Ok(())
    }
}

impl RuntimeBindingRetirementEvidence {
    pub fn task(receipt: RuntimePreparedTaskBinding, retired_at_ms: u64) -> UseResult<Self> {
        let evidence = Self {
            receipt: RuntimeBindingReceipt::Task(receipt),
            retired_at_ms,
            removal: None,
        };
        evidence.receipt.validate()?;
        if retired_at_ms == 0 {
            return Err(operation_error(
                "Runtime Task binding retirement time must be positive.",
            ));
        }
        Ok(evidence)
    }

    pub fn service(
        receipt: RuntimeServiceBindingReceipt,
        removal: RuntimeRemoval,
    ) -> UseResult<Self> {
        let retired_at_ms = removal.removed_at_ms;
        let evidence = Self {
            receipt: RuntimeBindingReceipt::Service(receipt),
            retired_at_ms,
            removal: Some(removal),
        };
        evidence.validate_shape()?;
        Ok(evidence)
    }

    pub fn receipt(&self) -> &RuntimeBindingReceipt {
        &self.receipt
    }

    pub fn retired_at_ms(&self) -> u64 {
        self.retired_at_ms
    }

    pub fn removal(&self) -> Option<&RuntimeRemoval> {
        self.removal.as_ref()
    }

    pub(super) fn validate_against(
        &self,
        cutover: &RuntimeBindingCutoverEvidence,
        now_ms: u64,
    ) -> UseResult<()> {
        self.validate_shape()?;
        if self.retired_at_ms < cutover.committed_at_ms || self.retired_at_ms > now_ms {
            return Err(operation_error(
                "Runtime binding retirement evidence is before cutover or from the future.",
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> UseResult<()> {
        self.receipt.validate()?;
        match (&self.receipt, &self.removal) {
            (RuntimeBindingReceipt::Task(_), None) if self.retired_at_ms > 0 => Ok(()),
            (RuntimeBindingReceipt::Service(receipt), Some(removal)) => {
                removal
                    .validate()
                    .map_err(super::model::runtime_contract_error)?;
                if removal.unit_id != receipt.unit_id
                    || removal.generation != receipt.generation
                    || removal.removed_at_ms != self.retired_at_ms
                {
                    return Err(operation_error(
                        "Runtime Service retirement does not prove exact unit removal.",
                    ));
                }
                Ok(())
            }
            _ => Err(operation_error(
                "Runtime binding retirement evidence does not match its binding kind.",
            )),
        }
    }
}

impl RuntimeBindingOperationJournal {
    pub(super) fn new(intent: RuntimeBindingOperationIntent) -> UseResult<Self> {
        let intent_digest = intent.descriptor_digest()?;
        let journal = Self {
            schema: RUNTIME_BINDING_OPERATION_SCHEMA.to_string(),
            intent_digest,
            intent,
            phase: RuntimeBindingOperationPhase::IntentRecorded,
            prepared: Vec::new(),
            cutover: None,
            retired: Vec::new(),
        };
        journal.validate()?;
        Ok(journal)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.intent.validate()?;
        if self.schema != RUNTIME_BINDING_OPERATION_SCHEMA
            || !super::model::valid_sha256(&self.intent_digest)
            || self.intent.descriptor_digest()? != self.intent_digest
            || self
                .prepared
                .windows(2)
                .any(|pair| pair[0].surface() >= pair[1].surface())
            || self
                .retired
                .windows(2)
                .any(|pair| pair[0].receipt().surface() >= pair[1].receipt().surface())
        {
            return Err(operation_error(
                "A Runtime binding operation journal has invalid schema, digest, or ordering.",
            ));
        }
        for receipt in &self.prepared {
            let candidate = candidate_for(&self.intent, receipt.surface()).ok_or_else(|| {
                operation_error("A prepared Runtime binding is absent from operation intent.")
            })?;
            if !candidate.matches_receipt(receipt)? {
                return Err(operation_error(
                    "A prepared Runtime binding differs from operation intent.",
                ));
            }
        }
        let all_prepared = self.prepared.len() == self.intent.candidates.len();
        if let Some(cutover) = &self.cutover {
            cutover.validate_against(&self.intent)?;
            for evidence in &self.retired {
                evidence.validate_against(cutover, u64::MAX)?;
                let retirement = self
                    .intent
                    .retirements
                    .binary_search_by(|receipt| receipt.surface().cmp(evidence.receipt().surface()))
                    .ok()
                    .and_then(|index| self.intent.retirements.get(index));
                if retirement != Some(evidence.receipt()) {
                    return Err(operation_error(
                        "Retired Runtime binding evidence is absent from operation intent.",
                    ));
                }
            }
        }
        let all_retired = self.retired.len() == self.intent.retirements.len();
        let valid_phase = match self.phase {
            RuntimeBindingOperationPhase::IntentRecorded => {
                self.prepared.is_empty() && self.cutover.is_none() && self.retired.is_empty()
            }
            RuntimeBindingOperationPhase::Preparing => {
                !self.prepared.is_empty()
                    && !all_prepared
                    && self.cutover.is_none()
                    && self.retired.is_empty()
            }
            RuntimeBindingOperationPhase::Prepared
            | RuntimeBindingOperationPhase::Publishing
            | RuntimeBindingOperationPhase::BindingsPublished => {
                all_prepared && self.cutover.is_none() && self.retired.is_empty()
            }
            RuntimeBindingOperationPhase::CutoverCommitted => {
                all_prepared
                    && self.cutover.is_some()
                    && self.retired.is_empty()
                    && !self.intent.retirements.is_empty()
            }
            RuntimeBindingOperationPhase::Retiring => {
                all_prepared && self.cutover.is_some() && !all_retired
            }
            RuntimeBindingOperationPhase::Completed => {
                all_prepared && self.cutover.is_some() && all_retired
            }
        };
        if !valid_phase {
            return Err(operation_error(
                "A Runtime binding operation journal phase disagrees with its checkpoints.",
            ));
        }
        Ok(())
    }
}

pub(super) fn candidate_for<'a>(
    intent: &'a RuntimeBindingOperationIntent,
    surface: &PlanQualifiedSurfaceRef,
) -> Option<&'a RuntimeBindingCandidatePlan> {
    intent
        .candidates
        .binary_search_by(|candidate| candidate.surface.cmp(surface))
        .ok()
        .and_then(|index| intent.candidates.get(index))
}

fn validate_candidate_shape(candidate: &RuntimeBindingCandidatePlan) -> UseResult<()> {
    match &candidate.kind {
        RuntimeBindingCandidateKind::Task {
            artifact_digest,
            artifact_media_type,
        } => RuntimeBindingReceipt::Task(RuntimePreparedTaskBinding {
            schema: RUNTIME_TASK_BINDING_SCHEMA.to_string(),
            surface: candidate.surface.clone(),
            package_digest: candidate.package_digest.clone(),
            scope_id: candidate.scope_id.clone(),
            descriptor_digest: candidate.descriptor_digest.clone(),
            provider_id: candidate.provider.provider_id.clone(),
            provider_build_id: candidate.provider.provider_build_id.clone(),
            capability_digest: candidate.provider.capability_digest.clone(),
            enforcement: candidate.provider.enforcement,
            artifact_digest: artifact_digest.clone(),
            artifact_media_type: artifact_media_type.clone(),
            generation: candidate.generation,
            semantics_profile_digest: candidate.provider.semantics_profile_digest.clone(),
        })
        .validate(),
        RuntimeBindingCandidateKind::Service {
            unit_id,
            spec_digest,
            contract,
        } => {
            let readiness = match contract {
                RuntimeSurfaceContract::ToolService { .. } => {
                    RuntimeServiceReadinessEvidence::HttpHealthy
                }
                RuntimeSurfaceContract::McpService {
                    protocol_version, ..
                } => RuntimeServiceReadinessEvidence::McpInitialized {
                    initialize: RuntimeMcpInitializeEvidence::new(protocol_version, 1)?,
                },
                RuntimeSurfaceContract::ToolTask { .. } => {
                    return Err(operation_error(
                        "A Runtime Service candidate cannot carry a Task contract.",
                    ))
                }
            };
            RuntimeBindingReceipt::Service(RuntimeServiceBindingReceipt {
                schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
                surface: candidate.surface.clone(),
                package_digest: candidate.package_digest.clone(),
                scope_id: candidate.scope_id.clone(),
                descriptor_digest: candidate.descriptor_digest.clone(),
                provider_id: candidate.provider.provider_id.clone(),
                provider_build_id: candidate.provider.provider_build_id.clone(),
                capability_digest: candidate.provider.capability_digest.clone(),
                enforcement: candidate.provider.enforcement,
                unit_id: unit_id.clone(),
                generation: candidate.generation,
                spec_digest: spec_digest.clone(),
                semantics_profile_digest: candidate.provider.semantics_profile_digest.clone(),
                endpoint_ref: super::RuntimeEndpointRef::parse(
                    "gateway:binding-operation-validation",
                )?,
                runtime_started_at_ms: 1,
                observation_revision: 1,
                last_healthy_at_ms: 1,
                contract: contract.clone(),
                readiness,
            })
            .validate()
        }
    }
    .map_err(|_| operation_error("A Runtime binding candidate shape is invalid."))
}

fn revision_exhausted() -> UseError {
    operation_error("The Runtime binding operation state revision is exhausted.")
}

fn generation_exhausted() -> UseError {
    operation_error("The Runtime binding capability generation is exhausted.")
}

pub(super) fn operation_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.binding_operation_invalid", message)
}

pub(super) fn operation_state_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
