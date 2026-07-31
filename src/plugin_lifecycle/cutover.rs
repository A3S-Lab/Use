use a3s_use_core::{PluginOperationPlan, UseResult};
use a3s_use_extension::{
    WorkspaceGrantCutoverEvidence, WorkspaceGrantOperationIntent, WORKSPACE_GRANT_CUTOVER_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_runtime::{
    RuntimeBindingCutoverEvidence, RuntimeBindingOperationIntent, RUNTIME_BINDING_CUTOVER_SCHEMA,
};

use super::binding::PluginLifecycleOperationBinding;
use super::validation::{lifecycle_error, valid_sha256};

/// Schema for one capability publication shared by every child sub-saga.
pub const PLUGIN_LIFECYCLE_CUTOVER_SCHEMA: &str = "a3s.use.plugin-lifecycle-cutover.v1";

/// Immutable parent evidence for one atomic capability publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleCutoverEvidence {
    schema: String,
    operation_id: String,
    plan_digest: String,
    lifecycle_binding_digest: String,
    state_revision_before: u64,
    state_revision_after: u64,
    capability_generation_before: u64,
    capability_generation_after: u64,
    capability_snapshot_digest: String,
    committed_at_ms: u64,
    cutover_digest: String,
}

impl PluginLifecycleCutoverEvidence {
    /// Bind one trusted capability snapshot publication to the parent operation.
    ///
    /// This is an evidence constructor, not publication authorization. The
    /// parent host must first validate recovered bindings against its durable
    /// reviewed plan and pass `verify_ready_for_cutover`.
    pub fn new(
        binding: &PluginLifecycleOperationBinding,
        capability_snapshot_digest: impl Into<String>,
        committed_at_ms: u64,
        now_ms: u64,
    ) -> UseResult<Self> {
        binding.validate()?;
        let mut evidence = Self {
            schema: PLUGIN_LIFECYCLE_CUTOVER_SCHEMA.to_string(),
            operation_id: binding.operation_id().to_string(),
            plan_digest: binding.plan_digest().to_string(),
            lifecycle_binding_digest: binding.binding_digest().to_string(),
            state_revision_before: binding.state_revision_before(),
            state_revision_after: binding.state_revision_after(),
            capability_generation_before: binding.capability_generation_before(),
            capability_generation_after: binding.capability_generation_after(),
            capability_snapshot_digest: capability_snapshot_digest.into(),
            committed_at_ms,
            cutover_digest: String::new(),
        };
        evidence.cutover_digest = evidence.calculate_digest()?;
        evidence.validate_against(binding, now_ms)?;
        Ok(evidence)
    }

    /// Validate schema, parent identity, generation, snapshot, and clock evidence.
    pub fn validate_against(
        &self,
        binding: &PluginLifecycleOperationBinding,
        now_ms: u64,
    ) -> UseResult<()> {
        binding.validate()?;
        PluginOperationPlan::validate_operation_id(&self.operation_id)?;
        if self.schema != PLUGIN_LIFECYCLE_CUTOVER_SCHEMA
            || self.operation_id != binding.operation_id()
            || self.plan_digest != binding.plan_digest()
            || self.lifecycle_binding_digest != binding.binding_digest()
            || self.state_revision_before != binding.state_revision_before()
            || self.state_revision_after != binding.state_revision_after()
            || self.capability_generation_before != binding.capability_generation_before()
            || self.capability_generation_after != binding.capability_generation_after()
            || !valid_sha256(&self.capability_snapshot_digest)
            || self.committed_at_ms < binding.transitioned_at_ms()
            || self.committed_at_ms > now_ms
            || !valid_sha256(&self.cutover_digest)
            || self.calculate_digest()? != self.cutover_digest
        {
            return Err(lifecycle_error(
                "The parent capability cutover does not match its lifecycle binding.",
            ));
        }
        Ok(())
    }

    /// Derive exact workspace-grant child cutover evidence for one scope.
    pub fn grant_cutover(
        &self,
        binding: &PluginLifecycleOperationBinding,
        intent: &WorkspaceGrantOperationIntent,
        now_ms: u64,
    ) -> UseResult<WorkspaceGrantCutoverEvidence> {
        self.validate_against(binding, now_ms)?;
        intent.validate()?;
        let child = binding
            .grant_operation(&intent.scope_id)
            .ok_or_else(|| lifecycle_error("The grant intent is absent from parent cutover."))?;
        if intent.operation_id != self.operation_id
            || intent.plan_digest != self.plan_digest
            || intent.change_set_digest != child.change_set_digest()
            || intent.descriptor_digest()? != child.intent_digest()
            || intent.state_revision_before != self.state_revision_before
            || intent.revision != self.state_revision_after
            || intent.capability_generation_before != self.capability_generation_before
            || intent.capability_generation_after != self.capability_generation_after
            || intent.transitioned_at_ms != binding.transitioned_at_ms()
        {
            return Err(lifecycle_error(
                "The grant intent changed before parent cutover derivation.",
            ));
        }
        let evidence = WorkspaceGrantCutoverEvidence {
            schema: WORKSPACE_GRANT_CUTOVER_SCHEMA.to_string(),
            capability_generation_before: self.capability_generation_before,
            capability_generation_after: self.capability_generation_after,
            capability_snapshot_digest: self.capability_snapshot_digest.clone(),
            committed_at_ms: self.committed_at_ms,
        };
        evidence.validate_against(intent)?;
        Ok(evidence)
    }

    /// Derive exact Runtime-binding child cutover evidence for one scope.
    pub fn runtime_cutover(
        &self,
        binding: &PluginLifecycleOperationBinding,
        intent: &RuntimeBindingOperationIntent,
        now_ms: u64,
    ) -> UseResult<RuntimeBindingCutoverEvidence> {
        self.validate_against(binding, now_ms)?;
        intent.validate()?;
        let child = binding
            .runtime_operation(&intent.scope_id)
            .ok_or_else(|| lifecycle_error("The Runtime intent is absent from parent cutover."))?;
        if intent.operation_id != self.operation_id
            || intent.plan_digest != self.plan_digest
            || intent.grant_change_set_digest.as_deref() != child.grant_change_set_digest()
            || intent.descriptor_digest()? != child.intent_digest()
            || intent.state_revision_before != self.state_revision_before
            || intent.state_revision_after != self.state_revision_after
            || intent.capability_generation_before != self.capability_generation_before
            || intent.capability_generation_after != self.capability_generation_after
            || intent.transitioned_at_ms != binding.transitioned_at_ms()
        {
            return Err(lifecycle_error(
                "The Runtime intent changed before parent cutover derivation.",
            ));
        }
        let evidence = RuntimeBindingCutoverEvidence {
            schema: RUNTIME_BINDING_CUTOVER_SCHEMA.to_string(),
            state_revision_before: self.state_revision_before,
            state_revision_after: self.state_revision_after,
            capability_generation_before: self.capability_generation_before,
            capability_generation_after: self.capability_generation_after,
            capability_snapshot_digest: self.capability_snapshot_digest.clone(),
            committed_at_ms: self.committed_at_ms,
        };
        evidence.validate_against(intent)?;
        Ok(evidence)
    }

    /// Exact capability snapshot digest published by the parent.
    pub fn capability_snapshot_digest(&self) -> &str {
        &self.capability_snapshot_digest
    }

    /// Trusted capability publication time.
    pub fn committed_at_ms(&self) -> u64 {
        self.committed_at_ms
    }

    /// SHA-256 over the complete parent cutover evidence.
    pub fn cutover_digest(&self) -> &str {
        &self.cutover_digest
    }

    fn calculate_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestInput<'a> {
            schema: &'a str,
            operation_id: &'a str,
            plan_digest: &'a str,
            lifecycle_binding_digest: &'a str,
            state_revision_before: u64,
            state_revision_after: u64,
            capability_generation_before: u64,
            capability_generation_after: u64,
            capability_snapshot_digest: &'a str,
            committed_at_ms: u64,
        }
        let bytes = serde_json::to_vec(&DigestInput {
            schema: &self.schema,
            operation_id: &self.operation_id,
            plan_digest: &self.plan_digest,
            lifecycle_binding_digest: &self.lifecycle_binding_digest,
            state_revision_before: self.state_revision_before,
            state_revision_after: self.state_revision_after,
            capability_generation_before: self.capability_generation_before,
            capability_generation_after: self.capability_generation_after,
            capability_snapshot_digest: &self.capability_snapshot_digest,
            committed_at_ms: self.committed_at_ms,
        })
        .map_err(|error| {
            lifecycle_error(format!(
                "Failed to encode parent capability cutover evidence: {error}"
            ))
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}
