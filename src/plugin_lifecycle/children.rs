use std::collections::BTreeMap;

use a3s_use_core::{PluginOperationPlan, PluginOperationPlanEnvelope, UseResult};
use a3s_use_extension::WorkspaceGrantOperationIntent;

use crate::plugin_runtime::{
    RuntimeBindingCandidateKind, RuntimeBindingOperationIntent, RuntimeBindingReceipt,
};

use super::binding::{
    validate_grant_children, validate_runtime_identity, PluginLifecycleOperationBinding,
};
use super::validation::{
    expected_runtime_operations, lifecycle_error, planned_providers, ExpectedRuntimeKind,
};

impl PluginLifecycleOperationBinding {
    /// Recheck exact child intent content against the immutable reviewed plan.
    pub fn validate_children(
        &self,
        plan: &PluginOperationPlanEnvelope,
        grant_intents: &[WorkspaceGrantOperationIntent],
        runtime_intents: &[RuntimeBindingOperationIntent],
    ) -> UseResult<()> {
        self.validate_against_plan(plan)?;
        self.validate_child_identity(grant_intents, runtime_intents)?;
        validate_runtime_children(&plan.plan, runtime_intents)
    }

    pub(crate) fn validate_child_identity(
        &self,
        grant_intents: &[WorkspaceGrantOperationIntent],
        runtime_intents: &[RuntimeBindingOperationIntent],
    ) -> UseResult<()> {
        validate_grant_children(self, grant_intents)?;
        validate_runtime_identity(self, runtime_intents)
    }
}

fn validate_runtime_children(
    plan: &PluginOperationPlan,
    intents: &[RuntimeBindingOperationIntent],
) -> UseResult<()> {
    let expected = expected_runtime_operations(plan)?;
    let planned_providers = planned_providers(plan)?;
    let mut used_providers = BTreeMap::new();
    for intent in intents {
        let scope = expected.get(&intent.scope_id).ok_or_else(|| {
            lifecycle_error("A Runtime child scope is absent from the reviewed plan.")
        })?;
        if intent.candidates.len() != scope.candidates.len()
            || intent.retirements.len() != scope.retirements.len()
        {
            return Err(lifecycle_error(
                "A Runtime child intent does not exactly cover reviewed surface changes.",
            ));
        }
        for candidate in &intent.candidates {
            let expected = scope.candidates.get(&candidate.surface).ok_or_else(|| {
                lifecycle_error("A Runtime candidate is absent from the reviewed plan.")
            })?;
            let kind_matches = matches!(
                (&candidate.kind, expected.kind),
                (
                    RuntimeBindingCandidateKind::Task { .. },
                    ExpectedRuntimeKind::Task
                ) | (
                    RuntimeBindingCandidateKind::Service { .. },
                    ExpectedRuntimeKind::Service
                )
            );
            let provider = planned_providers.get(&candidate.surface).ok_or_else(|| {
                lifecycle_error("A Runtime candidate has no reviewed provider evidence.")
            })?;
            if candidate.package_digest != expected.package_digest
                || !kind_matches
                || candidate.provider != *provider
            {
                return Err(lifecycle_error(
                    "A Runtime candidate changed package, kind, or provider evidence.",
                ));
            }
            match used_providers.insert(candidate.surface.clone(), candidate.provider.clone()) {
                Some(existing) if existing != candidate.provider => {
                    return Err(lifecycle_error(
                        "Different scopes selected conflicting Runtime provider evidence.",
                    ))
                }
                Some(_) | None => {}
            }
        }
        for retirement in &intent.retirements {
            let expected = scope.retirements.get(retirement.surface()).ok_or_else(|| {
                lifecycle_error("A Runtime retirement is absent from the reviewed plan.")
            })?;
            let kind_matches = matches!(
                (retirement, expected.kind),
                (RuntimeBindingReceipt::Task(_), ExpectedRuntimeKind::Task)
                    | (
                        RuntimeBindingReceipt::Service(_),
                        ExpectedRuntimeKind::Service
                    )
            );
            if retirement.package_digest() != expected.package_digest || !kind_matches {
                return Err(lifecycle_error(
                    "A Runtime retirement changed reviewed package or workload ownership.",
                ));
            }
        }
    }
    if used_providers != planned_providers {
        return Err(lifecycle_error(
            "Runtime child intents do not exactly consume reviewed provider evidence.",
        ));
    }
    Ok(())
}
