use std::collections::BTreeMap;

use a3s_use_core::{
    PluginOperationPlan, PluginOperationPlanEnvelope, PluginWorkspaceGrant, UseResult,
    MAX_PLUGIN_PLAN_ITEMS,
};
use a3s_use_extension::WorkspaceGrantOperationIntent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_runtime::RuntimeBindingOperationIntent;

use super::validation::{
    expected_grant_operations, expected_runtime_operations, lifecycle_error, valid_sha256,
};

/// Schema for the host-owned cross-sub-saga operation binding.
pub const PLUGIN_LIFECYCLE_OPERATION_BINDING_SCHEMA: &str =
    "a3s.use.plugin-lifecycle-operation-binding.v1";

/// Exact scope-specific workspace-grant child intent retained by the parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleGrantIntentBinding {
    scope_id: String,
    change_set_digest: String,
    intent_digest: String,
}

/// Exact scope-specific Runtime-binding child intent retained by the parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleRuntimeIntentBinding {
    scope_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_change_set_digest: Option<String>,
    intent_digest: String,
}

/// Immutable parent binding between one reviewed plan and all grant/Runtime
/// child intents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleOperationBinding {
    schema: String,
    operation_id: String,
    plan_digest: String,
    state_revision_before: u64,
    state_revision_after: u64,
    capability_generation_before: u64,
    capability_generation_after: u64,
    transitioned_at_ms: u64,
    grant_operations: Vec<PluginLifecycleGrantIntentBinding>,
    runtime_operations: Vec<PluginLifecycleRuntimeIntentBinding>,
    binding_digest: String,
}

impl PluginLifecycleGrantIntentBinding {
    /// Explicit workspace or user scope.
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Canonical grant change-set digest reviewed in the operation plan.
    pub fn change_set_digest(&self) -> &str {
        &self.change_set_digest
    }

    /// Immutable child intent digest.
    pub fn intent_digest(&self) -> &str {
        &self.intent_digest
    }
}

impl PluginLifecycleRuntimeIntentBinding {
    /// Explicit workspace or user scope.
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Optional canonical grant change-set digest for this scope.
    pub fn grant_change_set_digest(&self) -> Option<&str> {
        self.grant_change_set_digest.as_deref()
    }

    /// Immutable child intent digest.
    pub fn intent_digest(&self) -> &str {
        &self.intent_digest
    }
}

impl PluginLifecycleOperationBinding {
    /// Bind one reviewed plan to every exact scope-specific child intent.
    pub fn from_intents(
        plan: &PluginOperationPlanEnvelope,
        transitioned_at_ms: u64,
        grant_intents: &[WorkspaceGrantOperationIntent],
        runtime_intents: &[RuntimeBindingOperationIntent],
    ) -> UseResult<Self> {
        plan.verify_apply(
            &plan.plan.operation_id,
            &plan.plan_digest,
            transitioned_at_ms,
        )?;
        let state_revision_after = plan
            .plan
            .state
            .state_revision
            .checked_add(1)
            .ok_or_else(|| lifecycle_error("The parent state revision is exhausted."))?;
        let capability_generation_after = plan
            .plan
            .state
            .capability_generation
            .checked_add(1)
            .ok_or_else(|| lifecycle_error("The parent capability generation is exhausted."))?;
        let mut grant_operations = grant_intents
            .iter()
            .map(|intent| {
                intent.validate()?;
                Ok(PluginLifecycleGrantIntentBinding {
                    scope_id: intent.scope_id.clone(),
                    change_set_digest: intent.change_set_digest.clone(),
                    intent_digest: intent.descriptor_digest()?,
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        let mut runtime_operations = runtime_intents
            .iter()
            .map(|intent| {
                intent.validate()?;
                Ok(PluginLifecycleRuntimeIntentBinding {
                    scope_id: intent.scope_id.clone(),
                    grant_change_set_digest: intent.grant_change_set_digest.clone(),
                    intent_digest: intent.descriptor_digest()?,
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        grant_operations.sort_by(|left, right| left.scope_id.cmp(&right.scope_id));
        runtime_operations.sort_by(|left, right| left.scope_id.cmp(&right.scope_id));
        let mut binding = Self {
            schema: PLUGIN_LIFECYCLE_OPERATION_BINDING_SCHEMA.to_string(),
            operation_id: plan.plan.operation_id.clone(),
            plan_digest: plan.plan_digest.clone(),
            state_revision_before: plan.plan.state.state_revision,
            state_revision_after,
            capability_generation_before: plan.plan.state.capability_generation,
            capability_generation_after,
            transitioned_at_ms,
            grant_operations,
            runtime_operations,
            binding_digest: String::new(),
        };
        binding.binding_digest = binding.calculate_digest()?;
        binding.validate_children(plan, grant_intents, runtime_intents)?;
        Ok(binding)
    }

    /// Validate intrinsic schema, ordering, revision, and digest evidence.
    pub fn validate(&self) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(&self.operation_id)?;
        if self.schema != PLUGIN_LIFECYCLE_OPERATION_BINDING_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || self.state_revision_before == 0
            || self.state_revision_before.checked_add(1) != Some(self.state_revision_after)
            || self.capability_generation_before == 0
            || self.capability_generation_before.checked_add(1)
                != Some(self.capability_generation_after)
            || self.transitioned_at_ms == 0
            || self.grant_operations.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.runtime_operations.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .grant_operations
                .windows(2)
                .any(|pair| pair[0].scope_id >= pair[1].scope_id)
            || self
                .runtime_operations
                .windows(2)
                .any(|pair| pair[0].scope_id >= pair[1].scope_id)
            || !valid_sha256(&self.binding_digest)
            || self.calculate_digest()? != self.binding_digest
        {
            return Err(lifecycle_error(
                "The plugin lifecycle binding has invalid schema, revision, ordering, or digest evidence.",
            ));
        }
        for operation in &self.grant_operations {
            PluginWorkspaceGrant::validate_scope_id(&operation.scope_id)?;
            if !valid_sha256(&operation.change_set_digest)
                || !valid_sha256(&operation.intent_digest)
            {
                return Err(lifecycle_error(
                    "A workspace-grant child binding has invalid digest evidence.",
                ));
            }
        }
        for operation in &self.runtime_operations {
            PluginWorkspaceGrant::validate_scope_id(&operation.scope_id)?;
            if operation
                .grant_change_set_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
                || !valid_sha256(&operation.intent_digest)
            {
                return Err(lifecycle_error(
                    "A Runtime child binding has invalid digest evidence.",
                ));
            }
        }
        Ok(())
    }

    /// Rebind the stored parent evidence to the exact reviewed plan.
    pub fn validate_against_plan(&self, plan: &PluginOperationPlanEnvelope) -> UseResult<()> {
        self.validate()?;
        plan.validate()?;
        if self.operation_id != plan.plan.operation_id
            || self.plan_digest != plan.plan_digest
            || self.state_revision_before != plan.plan.state.state_revision
            || self.capability_generation_before != plan.plan.state.capability_generation
            || self.transitioned_at_ms < plan.plan.created_at_ms
            || self.transitioned_at_ms >= plan.plan.expires_at_ms
        {
            return Err(lifecycle_error(
                "The parent lifecycle binding does not match the reviewed plan identity or apply window.",
            ));
        }

        let grants = expected_grant_operations(&plan.plan);
        let bound_grants = self
            .grant_operations
            .iter()
            .map(|operation| {
                (
                    operation.scope_id.clone(),
                    operation.change_set_digest.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if bound_grants != grants {
            return Err(lifecycle_error(
                "Workspace-grant child bindings do not exactly match the reviewed workspace impacts.",
            ));
        }

        let runtime = expected_runtime_operations(&plan.plan)?;
        let bound_runtime = self
            .runtime_operations
            .iter()
            .map(|operation| operation.scope_id.as_str())
            .collect::<Vec<_>>();
        if bound_runtime != runtime.keys().map(String::as_str).collect::<Vec<_>>() {
            return Err(lifecycle_error(
                "Runtime child bindings do not exactly cover the reviewed scope/surface changes.",
            ));
        }
        for operation in &self.runtime_operations {
            if operation.grant_change_set_digest != grants.get(&operation.scope_id).cloned() {
                return Err(lifecycle_error(
                    "A Runtime child binding does not match its scope's reviewed grant change set.",
                ));
            }
        }
        Ok(())
    }

    /// Parent operation identity.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Reviewed canonical plan digest.
    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    /// State revision before the mutation.
    pub fn state_revision_before(&self) -> u64 {
        self.state_revision_before
    }

    /// State revision selected by successful cutover.
    pub fn state_revision_after(&self) -> u64 {
        self.state_revision_after
    }

    /// Capability generation before publication.
    pub fn capability_generation_before(&self) -> u64 {
        self.capability_generation_before
    }

    /// Capability generation selected by successful publication.
    pub fn capability_generation_after(&self) -> u64 {
        self.capability_generation_after
    }

    /// Trusted apply transition time.
    pub fn transitioned_at_ms(&self) -> u64 {
        self.transitioned_at_ms
    }

    /// Sorted workspace-grant child bindings.
    pub fn grant_operations(&self) -> &[PluginLifecycleGrantIntentBinding] {
        &self.grant_operations
    }

    /// Sorted Runtime child bindings.
    pub fn runtime_operations(&self) -> &[PluginLifecycleRuntimeIntentBinding] {
        &self.runtime_operations
    }

    /// SHA-256 over the complete parent binding.
    pub fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    pub(crate) fn grant_operation(
        &self,
        scope_id: &str,
    ) -> Option<&PluginLifecycleGrantIntentBinding> {
        self.grant_operations
            .binary_search_by(|operation| operation.scope_id.as_str().cmp(scope_id))
            .ok()
            .and_then(|index| self.grant_operations.get(index))
    }

    pub(crate) fn runtime_operation(
        &self,
        scope_id: &str,
    ) -> Option<&PluginLifecycleRuntimeIntentBinding> {
        self.runtime_operations
            .binary_search_by(|operation| operation.scope_id.as_str().cmp(scope_id))
            .ok()
            .and_then(|index| self.runtime_operations.get(index))
    }

    fn calculate_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestInput<'a> {
            schema: &'a str,
            operation_id: &'a str,
            plan_digest: &'a str,
            state_revision_before: u64,
            state_revision_after: u64,
            capability_generation_before: u64,
            capability_generation_after: u64,
            transitioned_at_ms: u64,
            grant_operations: &'a [PluginLifecycleGrantIntentBinding],
            runtime_operations: &'a [PluginLifecycleRuntimeIntentBinding],
        }
        let bytes = serde_json::to_vec(&DigestInput {
            schema: &self.schema,
            operation_id: &self.operation_id,
            plan_digest: &self.plan_digest,
            state_revision_before: self.state_revision_before,
            state_revision_after: self.state_revision_after,
            capability_generation_before: self.capability_generation_before,
            capability_generation_after: self.capability_generation_after,
            transitioned_at_ms: self.transitioned_at_ms,
            grant_operations: &self.grant_operations,
            runtime_operations: &self.runtime_operations,
        })
        .map_err(|error| {
            lifecycle_error(format!(
                "Failed to encode the plugin lifecycle binding: {error}"
            ))
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

pub(super) fn validate_grant_children(
    binding: &PluginLifecycleOperationBinding,
    intents: &[WorkspaceGrantOperationIntent],
) -> UseResult<()> {
    if intents.len() != binding.grant_operations.len() {
        return Err(lifecycle_error(
            "Workspace-grant child intent coverage changed after parent binding.",
        ));
    }
    let mut seen = BTreeMap::new();
    for intent in intents {
        intent.validate()?;
        let operation = binding
            .grant_operation(&intent.scope_id)
            .ok_or_else(|| lifecycle_error("An unrelated workspace-grant intent was supplied."))?;
        if seen.insert(intent.scope_id.as_str(), ()).is_some()
            || intent.operation_id != binding.operation_id
            || intent.plan_digest != binding.plan_digest
            || intent.change_set_digest != operation.change_set_digest
            || intent.state_revision_before != binding.state_revision_before
            || intent.revision != binding.state_revision_after
            || intent.capability_generation_before != binding.capability_generation_before
            || intent.capability_generation_after != binding.capability_generation_after
            || intent.transitioned_at_ms != binding.transitioned_at_ms
            || intent.descriptor_digest()? != operation.intent_digest
        {
            return Err(lifecycle_error(
                "A workspace-grant child intent differs from the immutable parent binding.",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_runtime_identity(
    binding: &PluginLifecycleOperationBinding,
    intents: &[RuntimeBindingOperationIntent],
) -> UseResult<()> {
    if intents.len() != binding.runtime_operations.len() {
        return Err(lifecycle_error(
            "Runtime child intent coverage changed after parent binding.",
        ));
    }
    let mut seen = BTreeMap::new();
    for intent in intents {
        intent.validate()?;
        let operation = binding
            .runtime_operation(&intent.scope_id)
            .ok_or_else(|| lifecycle_error("An unrelated Runtime intent was supplied."))?;
        if seen.insert(intent.scope_id.as_str(), ()).is_some()
            || intent.operation_id != binding.operation_id
            || intent.plan_digest != binding.plan_digest
            || intent.grant_change_set_digest != operation.grant_change_set_digest
            || intent.state_revision_before != binding.state_revision_before
            || intent.state_revision_after != binding.state_revision_after
            || intent.capability_generation_before != binding.capability_generation_before
            || intent.capability_generation_after != binding.capability_generation_after
            || intent.transitioned_at_ms != binding.transitioned_at_ms
            || intent.descriptor_digest()? != operation.intent_digest
        {
            return Err(lifecycle_error(
                "A Runtime child intent differs from the immutable parent binding.",
            ));
        }
    }
    Ok(())
}
